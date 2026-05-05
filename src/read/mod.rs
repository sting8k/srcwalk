pub mod imports;
pub mod outline;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use memmap2::Mmap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format;
use crate::lang::detect_file_type;
use crate::lang::outline::get_outline_entries as lang_get_outline_entries;
use crate::types::{estimate_tokens, FileType, OutlineEntry, ViewMode};

pub(crate) const RAW_TOKEN_CAP: u64 = 5_000;
const RAW_LINE_CAP: u32 = 200;
const FILE_SIZE_CAP: u64 = 500_000; // 500KB

/// Sections exceeding this token count are degraded to an outline of the range.
/// Explicit `--budget` overrides this limit; otherwise `SRCWALK_SECTION_SOFT_LIMIT`
/// can override the default raw token cap.
fn section_token_limit(budget: Option<u64>) -> u64 {
    budget.unwrap_or_else(|| {
        std::env::var("SRCWALK_SECTION_SOFT_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(RAW_TOKEN_CAP)
    })
}

fn raw_body_over_cap(tokens: u64, lines: u32) -> bool {
    tokens > RAW_TOKEN_CAP || lines > RAW_LINE_CAP
}

fn capped_line_end(buf: &[u8], start_byte: usize, max_lines: u32, max_tokens: u64) -> (usize, u32) {
    let max_bytes = (max_tokens * 4) as usize;
    let mut end = start_byte;
    let mut lines = 0u32;

    while end < buf.len() && lines < max_lines && end.saturating_sub(start_byte) < max_bytes {
        if let Some(rel) = memchr::memchr(b'\n', &buf[end..]) {
            let next = end + rel + 1;
            if next.saturating_sub(start_byte) > max_bytes {
                break;
            }
            end = next;
            lines += 1;
        } else {
            let next = buf.len().min(start_byte + max_bytes);
            if next > end {
                end = next;
                lines += 1;
            }
            break;
        }
    }

    if end == start_byte && start_byte < buf.len() {
        end = buf.len().min(start_byte + max_bytes.max(1));
        lines = 1;
    }

    (end, lines)
}

/// Main entry point for read mode. Routes through the decision tree.
pub fn read_file(
    path: &Path,
    section: Option<&str>,
    full: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SrcwalkError::NotFound {
                path: path.to_path_buf(),
                suggestion: suggest_similar(path),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(SrcwalkError::PermissionDenied {
                path: path.to_path_buf(),
            });
        }
        Err(e) => {
            return Err(SrcwalkError::IoError {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };

    // Directory → list contents
    if meta.is_dir() {
        return list_directory(path);
    }

    let byte_len = meta.len();

    // Empty check before mmap — mmap on 0-byte file may fail on some platforms
    if byte_len == 0 {
        return Ok(format::file_header(path, 0, 0, ViewMode::Empty));
    }

    // Section param → return those lines verbatim when within the section token limit.
    if let Some(range) = section {
        return read_section(path, range, None, cache);
    }

    // Binary detection
    let file = fs::File::open(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let buf = &mmap[..];

    if crate::lang::detection::is_binary(buf) {
        let mime = mime_from_ext(path);
        return Ok(format::binary_header(path, byte_len, mime));
    }

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Generated
    if crate::lang::detection::is_generated_by_name(name)
        || crate::lang::detection::is_generated_by_content(buf)
    {
        let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;
        return Ok(format::file_header(
            path,
            byte_len,
            line_count,
            ViewMode::Generated,
        ));
    }

    let tokens = estimate_tokens(byte_len);
    let content = String::from_utf8_lossy(buf);
    let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;

    let file_type = detect_file_type(path);
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    // Raw body output requires explicit `--full` and is capped by both tokens
    // and lines. Default path reads always return a structural/smart view.
    if full {
        if !raw_body_over_cap(tokens, line_count) {
            let header = format::file_header(path, byte_len, line_count, ViewMode::Full);
            let numbered = format::number_lines(&content, 1);
            return Ok(format!("{header}\n\n{numbered}"));
        }

        let (head_end, shown) = capped_line_end(buf, 0, RAW_LINE_CAP, RAW_TOKEN_CAP);
        let head = String::from_utf8_lossy(&buf[..head_end]);
        let numbered_head = format::number_lines(&head, 1);
        let outline = cache.get_or_compute(path, mtime, || {
            outline::generate(path, file_type, &content, buf, true)
        });

        let header = format::file_header(path, byte_len, line_count, ViewMode::Full);
        let next_start = shown + 1;
        return Ok(format!(
            "{header}\n\n> Caveat: full=true capped; raw body exceeds {RAW_TOKEN_CAP} tokens or {RAW_LINE_CAP} lines. Showing first {shown} of {line_count} lines.\n\n{numbered_head}\n\n## Outline\n\n{outline}\n\n> Next: continue with --section {next_start}-<end> or use a narrower --section range."
        ));
    }

    let capped = byte_len > FILE_SIZE_CAP;

    let outline = cache.get_or_compute(path, mtime, || {
        outline::generate(path, file_type, &content, buf, capped)
    });

    let mode = match file_type {
        FileType::StructuredData => ViewMode::Keys,
        _ => ViewMode::Outline,
    };
    let header = format::file_header(path, byte_len, line_count, mode);
    Ok(format!(
        "{header}\n\n{outline}\n\n> Next: drill into a symbol with --section <name> or a line range\n> Next: need raw file text? retry with --full, or use --section <range> for a smaller slice."
    ))
}

/// Would this file produce an outline (rather than full content) in default read mode?
/// Used by the MCP layer to decide whether to append related-file hints.
pub fn would_outline(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.is_dir() && m.len() > 0)
}

/// Wrapper around `read_file` that, for `--full` requests with `--budget`,
/// degrades gracefully instead of letting the post-hoc `budget::apply`
/// truncate body bytes mid-function and leave a misleading `[full]` header.
///
/// Cascade (when `full=true` and rendered output exceeds `budget`):
///   1. full file        → if fits, return as-is.
///   2. outline           → labelled `[outline (full requested, over budget)]` + note.
///   3. signatures only   → labelled `[signatures (...)]` + note (outline still overflows).
///   4. header + advice   → file too large at any granularity for this budget.
///
/// For `section`, non-`full`, or no-budget paths, behaves identically to `read_file`
/// (caller still applies the top-level budget cap if needed).
pub fn read_file_with_budget(
    path: &Path,
    section: Option<&str>,
    full: bool,
    budget: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    // Fast path: not a full-file budgeted request → defer to read_file.
    let Some(b) = budget else {
        return read_file(path, section, full, cache);
    };
    if let Some(range) = section {
        return read_section(path, range, Some(b), cache);
    }
    if !full {
        return read_file(path, section, full, cache);
    }

    let full_out = read_file(path, section, full, cache)?;
    if estimate_tokens(full_out.len() as u64) <= b {
        return Ok(full_out);
    }

    // Step 2: outline cascade.
    let outline_out = render_outline_view(path, cache, ViewMode::OutlineCascade)?;
    let with_note = append_cascade_note(&outline_out, "full body", full_out.len(), b);
    if estimate_tokens(with_note.len() as u64) <= b {
        return Ok(with_note);
    }

    // Step 3: signatures only.
    let sig_out = render_signatures_view(path, cache)?;
    let sig_with_note = append_cascade_note(&sig_out, "outline", outline_out.len(), b);
    if estimate_tokens(sig_with_note.len() as u64) <= b {
        return Ok(sig_with_note);
    }

    // Step 4: terminal — header + advice only.
    let meta = std::fs::metadata(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let line_count =
        std::fs::read(path).map_or(0, |buf| memchr::memchr_iter(b'\n', &buf).count() as u32 + 1);
    let header = format::file_header(path, meta.len(), line_count, ViewMode::Signatures);
    Ok(format!(
        "{header}\n\n> Caveat: file too large for budget {b} tokens at any granularity.\
         > Next: use --section <fn-name> or raise --budget."
    ))
}

fn render_outline_view(
    path: &Path,
    cache: &OutlineCache,
    mode: ViewMode,
) -> Result<String, SrcwalkError> {
    let meta = std::fs::metadata(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let buf = std::fs::read(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let content = String::from_utf8_lossy(&buf);
    let line_count = memchr::memchr_iter(b'\n', &buf).count() as u32 + 1;
    let file_type = detect_file_type(path);
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let outline = cache.get_or_compute(path, mtime, || {
        outline::generate(path, file_type, &content, &buf, true)
    });
    let header = format::file_header(path, meta.len(), line_count, mode);
    Ok(format!("{header}\n\n{outline}"))
}

/// Signatures-only view: keep top-level outline lines (no nested children body).
/// Heuristic: drop indented continuation lines from the outline, preserving
/// only the first non-indented entry per block.
fn render_signatures_view(path: &Path, cache: &OutlineCache) -> Result<String, SrcwalkError> {
    let outline_full = render_outline_view(path, cache, ViewMode::Signatures)?;
    let mut lines = outline_full.lines();
    let header = lines.next().unwrap_or("");
    let mut kept: Vec<&str> = vec![header];
    for line in lines {
        // Keep blank separators and lines starting at column 0 or with one level of indent.
        if line.is_empty() {
            kept.push(line);
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ').count();
        if indent <= 2 {
            kept.push(line);
        }
    }
    Ok(kept.join("\n"))
}

fn append_cascade_note(body: &str, prev_kind: &str, prev_bytes: usize, budget: u64) -> String {
    let prev_tokens = estimate_tokens(prev_bytes as u64);
    format!(
        "{body}\n\n> Note: {prev_kind} ({prev_tokens} tokens) exceeded budget ({budget}).\n> Next: use --section <fn-name> for a specific symbol, or raise --budget."
    )
}

/// Resolve a heading address to a line range in a markdown file.
/// Returns `(start_line, end_line)` as 1-indexed inclusive range.
/// Returns `None` if heading not found.
fn resolve_heading(buf: &[u8], heading: &str) -> Option<(usize, usize)> {
    let heading_trimmed = heading.trim_end();
    let heading_level = heading_trimmed.chars().take_while(|&c| c == '#').count();

    if heading_level == 0 {
        return None;
    }

    // Build line offsets
    let mut line_offsets: Vec<usize> = vec![0];
    for pos in memchr::memchr_iter(b'\n', buf) {
        line_offsets.push(pos + 1);
    }
    // Exclude phantom empty line after trailing newline (match outline's count)
    let total_lines = if buf.last() == Some(&b'\n') {
        line_offsets.len() - 1
    } else {
        line_offsets.len()
    };

    let mut in_code_block = false;
    let mut found_line: Option<usize> = None;

    // Scan for the target heading
    for (line_idx, &offset) in line_offsets.iter().enumerate() {
        let line_end = if line_idx + 1 < line_offsets.len() {
            line_offsets[line_idx + 1] - 1 // exclude newline
        } else {
            buf.len()
        };

        if let Ok(line_str) = std::str::from_utf8(&buf[offset..line_end]) {
            let trimmed = line_str.trim_end();

            // Track code blocks
            if trimmed.starts_with("```") {
                in_code_block = !in_code_block;
                continue;
            }

            // Skip headings inside code blocks
            if in_code_block {
                continue;
            }

            // Check if this line matches the heading (exact or with anchor/attribute/ATX-close suffix)
            // Accept: "## Foo", "## Foo {#anchor}", "## Foo {:.class}", "## Foo ##", "## Foo\t"
            let matches = trimmed == heading_trimmed
                || (trimmed.starts_with(heading_trimmed)
                    && trimmed[heading_trimmed.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| matches!(c, ' ' | '\t' | '{' | '#')));
            if matches {
                found_line = Some(line_idx + 1); // 1-indexed
                break;
            }
        }
    }

    let start_line = found_line?;

    // Find the next heading of same or higher level
    in_code_block = false;
    let start_idx = start_line - 1; // convert back to 0-indexed for iteration

    for (line_idx, &offset) in line_offsets.iter().enumerate().skip(start_idx + 1) {
        let line_end = if line_idx + 1 < line_offsets.len() {
            line_offsets[line_idx + 1] - 1
        } else {
            buf.len()
        };

        if let Ok(line_str) = std::str::from_utf8(&buf[offset..line_end]) {
            let trimmed = line_str.trim_end();

            if trimmed.starts_with("```") {
                in_code_block = !in_code_block;
                continue;
            }

            if in_code_block {
                continue;
            }

            // Check if this is a heading
            if trimmed.starts_with('#') {
                let level = trimmed.chars().take_while(|&c| c == '#').count();
                if level <= heading_level {
                    // 0-based line_idx of next heading = 1-indexed line before it
                    return Some((start_line, line_idx));
                }
            }
        }
    }

    // No next heading found — section goes to end of file
    Some((start_line, total_lines))
}

/// Collect up to `top_n` headings whose text is closest (by edit distance)
/// to the queried heading. Returns headings as they appear in the file
/// (e.g. "## Foo Bar"), excluding ones inside fenced code blocks.
fn suggest_headings(buf: &[u8], query: &str, top_n: usize) -> Vec<String> {
    let q = query.trim_end();
    let q_text = q.trim_start_matches('#').trim();
    if q_text.is_empty() {
        return Vec::new();
    }

    let mut in_code_block = false;
    let mut scored: Vec<(usize, String)> = Vec::new();
    for line in buf.split(|&b| b == b'\n') {
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let trimmed = s.trim_end();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || !trimmed.starts_with('#') {
            continue;
        }
        let h_text = trimmed.trim_start_matches('#').trim();
        if h_text.is_empty() {
            continue;
        }
        // Strip kramdown attr / ATX-close trailing markers from comparison text.
        let h_clean = h_text
            .split('{')
            .next()
            .unwrap_or(h_text)
            .trim_end_matches('#')
            .trim();
        let dist = edit_distance(&q_text.to_ascii_lowercase(), &h_clean.to_ascii_lowercase());
        scored.push((dist, trimmed.to_string()));
    }

    scored.sort_by_key(|(d, _)| *d);
    scored.into_iter().take(top_n).map(|(_, h)| h).collect()
}

/// Read a specific line range from a file.
/// Uses memchr to find the Nth newline offset and slice the mmap buffer directly
/// instead of collecting all lines into a Vec.
fn read_section(
    path: &Path,
    range: &str,
    budget: Option<u64>,
    _cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let file = fs::File::open(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let buf = &mmap[..];

    // Resolve section address: line range, focused line, heading, symbol name,
    // or a comma-separated list of those addresses.
    let mut focus_line = None;
    let (start, end) = if range.starts_with('#') {
        // Markdown heading. Try the full heading first so headings containing
        // commas still work; if that fails, fall through to comma-list parsing.
        match resolve_heading(buf, range) {
            Some(r) => r,
            None if range.contains(',') => return read_multi_section(path, buf, range, budget),
            None => {
                let suggestions = suggest_headings(buf, range, 5);
                let reason = if suggestions.is_empty() {
                    "heading not found in file".to_string()
                } else {
                    format!(
                        "heading not found in file. Closest matches:\n  {}",
                        suggestions.join("\n  ")
                    )
                };
                return Err(SrcwalkError::InvalidQuery {
                    query: range.to_string(),
                    reason,
                });
            }
        }
    } else if range.contains(',') {
        return read_multi_section(path, buf, range, budget);
    } else if let Some((start, end, focus)) = parse_range(range) {
        // Line range like "45-89" or focused line like "45"
        focus_line = focus;
        (start, end)
    } else if let Some(r) = resolve_symbol(buf, path, range) {
        // Symbol name like "isCustomization" or "handleRequest"
        r
    } else {
        let suggestions = suggest_symbols(buf, path, range, 3);
        let reason = if suggestions.is_empty() {
            "not a valid line number (e.g. \"45\"), line range (e.g. \"45-89\"), heading (e.g. \"## Foo\"), or symbol name in this file"
                .to_string()
        } else {
            format!("symbol not found. Closest:\n  {}", suggestions.join("\n  "))
        };
        return Err(SrcwalkError::InvalidQuery {
            query: range.to_string(),
            reason,
        });
    };

    // Find line offsets using memchr — no full-file Vec<&str> allocation
    let mut line_offsets: Vec<usize> = vec![0];
    for pos in memchr::memchr_iter(b'\n', buf) {
        line_offsets.push(pos + 1);
    }
    let total = line_offsets.len();

    let s = (start.saturating_sub(1)).min(total);
    let e = end.min(total);

    if s >= e {
        return Err(SrcwalkError::InvalidQuery {
            query: range.to_string(),
            reason: format!("range out of bounds (file has {total} lines)"),
        });
    }

    let start_byte = line_offsets[s];
    let end_byte = if e < line_offsets.len() {
        line_offsets[e]
    } else {
        buf.len()
    };

    let selected = String::from_utf8_lossy(&buf[start_byte..end_byte]);
    let byte_len = selected.len() as u64;
    let line_count = (e - s) as u32;
    let tok_est = estimate_tokens(byte_len);
    let limit = section_token_limit(budget);

    if tok_est > limit {
        // Degrade: render outline entries within the section range
        let file_type = detect_file_type(path);
        let content = String::from_utf8_lossy(buf);
        let header = format::file_header(path, byte_len, line_count, ViewMode::SectionOutline);

        let start32 = start as u32;
        let end32 = end as u32;

        if let crate::types::FileType::Code(lang) = file_type {
            let entries = lang_get_outline_entries(&content, lang);
            let filtered = filter_entries_in_range(&entries, start32, end32);
            if !filtered.is_empty() {
                let body = format_section_outline(&filtered);
                return Ok(format!(
                    "{header}\n\n{body}\n\n\
                     > Caveat: section spans ~{tok_est} tokens / {line_count} lines (limit: {limit} tokens). Showing outline of {start}-{end}.\n\
                     > Next: retry with --budget <N>, or use a narrower --section range."
                ));
            }
        }

        // Fallback: no structured outline available — return header + advice only
        return Ok(format!(
            "{header}\n\n\
             > Caveat: section spans ~{tok_est} tokens / {line_count} lines (limit: {limit} tokens).\n\
             > Next: retry with --budget <N>, or use a narrower --section range."
        ));
    }

    let header = format::file_header(path, byte_len, line_count, ViewMode::Section);
    let formatted = if let Some(focus) = focus_line {
        format_focused_lines(&selected, start as u32, focus)
    } else {
        format::number_lines(&selected, start as u32)
    };
    Ok(format!("{header}\n\n{formatted}"))
}

fn format_focused_lines(content: &str, start: u32, focus_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let last = (start as usize + lines.len()).max(1);
    let width = (last.ilog10() + 1).max(4) as usize;
    let mut out = String::with_capacity(content.len() + lines.len() * (width + 5));
    for (i, line) in lines.iter().enumerate() {
        let num = start as usize + i;
        let prefix = if num == focus_line { "► " } else { "  " };
        let _ = writeln!(out, "{prefix}{num:>width$} │ {line}");
    }
    out
}

/// Resolve multiple comma-separated section addresses and return their bodies concatenated.
fn read_multi_section(
    path: &Path,
    buf: &[u8],
    range: &str,
    budget: Option<u64>,
) -> Result<String, SrcwalkError> {
    let requested: Vec<&str> = range
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if requested.is_empty() {
        return Err(SrcwalkError::InvalidQuery {
            query: range.to_string(),
            reason: "empty section list".to_string(),
        });
    }

    let all_symbol_names = requested
        .iter()
        .all(|section| parse_range(section).is_none() && !section.starts_with('#'));
    let mut blocks: Vec<(usize, usize, Option<usize>, String)> = Vec::new(); // start, end, focus, label
    let mut errors: Vec<String> = Vec::new();

    for section in &requested {
        if let Some((start, end, focus)) = parse_range(section) {
            blocks.push((start, end, focus, (*section).to_string()));
        } else if section.starts_with('#') {
            if let Some((start, end)) = resolve_heading(buf, section) {
                blocks.push((start, end, None, (*section).to_string()));
            } else {
                let suggestions = suggest_headings(buf, section, 3);
                if suggestions.is_empty() {
                    errors.push(format!("{section}: not found"));
                } else {
                    errors.push(format!(
                        "{section}: not found. Closest:\n    {}",
                        suggestions.join("\n    ")
                    ));
                }
            }
        } else if let Some((start, end)) = resolve_symbol(buf, path, section) {
            blocks.push((start, end, None, (*section).to_string()));
        } else {
            let suggestions = suggest_symbols(buf, path, section, 3);
            if suggestions.is_empty() {
                errors.push(format!("{section}: not found"));
            } else {
                errors.push(format!(
                    "{section}: not found. Closest:\n    {}",
                    suggestions.join("\n    ")
                ));
            }
        }
    }

    if !errors.is_empty() && blocks.is_empty() {
        let noun = if all_symbol_names {
            "symbols"
        } else {
            "sections"
        };
        return Err(SrcwalkError::InvalidQuery {
            query: range.to_string(),
            reason: format!("{noun} not found:\n  {}", errors.join("\n  ")),
        });
    }

    // Sort blocks by start line for natural reading order.
    blocks.sort_by_key(|(start, _, _, _)| *start);

    // Build line offsets.
    let mut line_offsets: Vec<usize> = vec![0];
    for pos in memchr::memchr_iter(b'\n', buf) {
        line_offsets.push(pos + 1);
    }
    let total = line_offsets.len();

    let mut parts: Vec<String> = Vec::new();
    let mut rendered_labels: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_lines: u32 = 0;

    for (start, end, focus, label) in &blocks {
        let s = (start.saturating_sub(1)).min(total);
        let e = (*end).min(total);
        if s >= e {
            errors.push(format!(
                "{label}: range out of bounds (file has {total} lines)"
            ));
            continue;
        }
        let start_byte = line_offsets[s];
        let end_byte = if e < line_offsets.len() {
            line_offsets[e]
        } else {
            buf.len()
        };
        let selected = String::from_utf8_lossy(&buf[start_byte..end_byte]);
        total_bytes += selected.len() as u64;
        total_lines += (e - s) as u32;
        rendered_labels.push(label.clone());
        let formatted = if let Some(focus) = focus {
            format_focused_lines(&selected, *start as u32, *focus)
        } else {
            format::number_lines(&selected, *start as u32)
        };
        parts.push(formatted);
    }

    if parts.is_empty() {
        let noun = if all_symbol_names {
            "symbols"
        } else {
            "sections"
        };
        return Err(SrcwalkError::InvalidQuery {
            query: range.to_string(),
            reason: format!("{noun} not found:\n  {}", errors.join("\n  ")),
        });
    }

    let tok_est = estimate_tokens(total_bytes);
    let limit = section_token_limit(budget);

    if tok_est > limit {
        let header = format::file_header(path, total_bytes, total_lines, ViewMode::SectionOutline);
        let noun = if all_symbol_names {
            "symbols"
        } else {
            "sections"
        };
        return Ok(format!(
            "{header}\n\n\
             > Caveat: {count} {noun} ({names}) span ~{tok_est} tokens (limit {limit}).\n\
             > Next: use a narrower --section list, or retry with --budget <N>.",
            count = rendered_labels.len(),
            names = rendered_labels.join(", "),
        ));
    }

    let section_count = parts.len();
    let noun = if all_symbol_names {
        "symbol"
    } else {
        "section"
    };
    let plural = if section_count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    let header = format::file_header(path, total_bytes, total_lines, ViewMode::Section);
    let header = header.replace("[section]", &format!("[{section_count} {plural}, section]"));
    let body = parts.join("\n\n---\n\n");

    if errors.is_empty() {
        Ok(format!("{header}\n\n{body}"))
    } else {
        let missing = errors.join("\n  ");
        let missing_label = if all_symbol_names {
            "Missing symbols"
        } else {
            "Missing sections"
        };
        Ok(format!(
            "{header}\n\n{body}\n\n> {missing_label}:\n>   {missing}"
        ))
    }
}

/// Filter outline entries (and children) to those overlapping [`range_start`, `range_end`].
fn filter_entries_in_range(
    entries: &[OutlineEntry],
    range_start: u32,
    range_end: u32,
) -> Vec<&OutlineEntry> {
    let mut out = Vec::new();
    for e in entries {
        // For container entries (class/struct) that span beyond the range,
        // skip the parent — we'll include matching children directly.
        if !e.children.is_empty() && (e.start_line < range_start || e.end_line > range_end) {
            // Recurse into children
            for c in &e.children {
                if c.start_line <= range_end && c.end_line >= range_start {
                    out.push(c);
                }
            }
        } else if e.start_line <= range_end && e.end_line >= range_start {
            out.push(e);
        }
    }
    out
}

/// Format filtered outline entries for section degrade output.
fn format_section_outline(entries: &[&OutlineEntry]) -> String {
    const MAX_SECTION_OUTLINE_LINES: usize = 100;
    let mut lines = Vec::new();
    for e in entries {
        if lines.len() >= MAX_SECTION_OUTLINE_LINES {
            break;
        }
        let range = if e.start_line == e.end_line {
            format!("[{}]", e.start_line)
        } else {
            format!("[{}-{}]", e.start_line, e.end_line)
        };
        let sig = e.signature.as_deref().unwrap_or(&e.name);
        lines.push(format!("  {range:>14}    {sig}"));
        // Show children in range
        for c in &e.children {
            if lines.len() >= MAX_SECTION_OUTLINE_LINES {
                break;
            }
            let cr = if c.start_line == c.end_line {
                format!("[{}]", c.start_line)
            } else {
                format!("[{}-{}]", c.start_line, c.end_line)
            };
            let csig = c.signature.as_deref().unwrap_or(&c.name);
            lines.push(format!("    {cr:>12}    {csig}"));
        }
    }
    if entries.len() > MAX_SECTION_OUTLINE_LINES {
        lines.push(format!(
            "  ... section outline capped at {MAX_SECTION_OUTLINE_LINES} entries; use a narrower --section range"
        ));
    }
    lines.join("\n")
}

/// Parse "45-89" or focused line "45". 1-indexed.
fn parse_range(s: &str) -> Option<(usize, usize, Option<usize>)> {
    if !s.contains('-') {
        let line: usize = s.trim().parse().ok()?;
        if line == 0 {
            return None;
        }
        return Some((line.saturating_sub(2).max(1), line + 2, Some(line)));
    }

    let (a, b) = s.split_once('-')?;
    let start: usize = a.trim().parse().ok()?;
    let end: usize = b.trim().parse().ok()?;
    if start == 0 || end < start {
        return None;
    }
    Some((start, end, None))
}

/// Resolve a symbol name to its line range using AST outline.
/// Returns (`start_line`, `end_line`) if found.
fn resolve_symbol(buf: &[u8], path: &Path, symbol: &str) -> Option<(usize, usize)> {
    let content = std::str::from_utf8(buf).ok()?;
    let FileType::Code(lang) = detect_file_type(path) else {
        return None;
    };
    let entries = lang_get_outline_entries(content, lang);
    find_symbol_in_entries(&entries, symbol)
}

/// Collect symbol names from outline entries (recursively) with their line ranges,
/// then rank by prefix match + edit distance, returning top `top_n` suggestions.
fn suggest_symbols(buf: &[u8], path: &Path, query: &str, top_n: usize) -> Vec<String> {
    let Ok(content) = std::str::from_utf8(buf) else {
        return Vec::new();
    };
    let FileType::Code(lang) = detect_file_type(path) else {
        return Vec::new();
    };
    let entries = lang_get_outline_entries(content, lang);
    let mut flat: Vec<(&str, usize, usize)> = Vec::new();
    collect_symbol_names(&entries, &mut flat);

    let q = query.to_ascii_lowercase();
    let mut scored: Vec<(usize, &str, usize, usize)> = flat
        .iter()
        .map(|&(name, start, end)| {
            let nl = name.to_ascii_lowercase();
            // Prefix match gets a big bonus (distance 0 override)
            let dist = if nl.starts_with(&q) {
                0
            } else {
                edit_distance(&q, &nl)
            };
            (dist, name, start, end)
        })
        .collect();
    scored.sort_by_key(|(d, _, _, _)| *d);
    scored
        .into_iter()
        .take(top_n)
        .map(|(_, name, start, end)| format!("{name} [{start}-{end}]"))
        .collect()
}

/// Flatten outline entries into (name, `start_line`, `end_line`) tuples.
fn collect_symbol_names<'a>(entries: &'a [OutlineEntry], out: &mut Vec<(&'a str, usize, usize)>) {
    for entry in entries {
        out.push((
            &entry.name,
            entry.start_line as usize,
            entry.end_line as usize,
        ));
        collect_symbol_names(&entry.children, out);
    }
}

/// Recursively search for a symbol in outline entries.
fn find_symbol_in_entries(entries: &[OutlineEntry], symbol: &str) -> Option<(usize, usize)> {
    for entry in entries {
        if entry.name == symbol {
            return Some((entry.start_line as usize, entry.end_line as usize));
        }
        // Search children (methods inside class, etc.)
        if let Some(range) = find_symbol_in_entries(&entry.children, symbol) {
            return Some(range);
        }
    }
    None
}

/// List directory contents — treat as glob on dir/*.
fn list_directory(path: &Path) -> Result<String, SrcwalkError> {
    let mut entries: Vec<String> = Vec::new();
    let read_dir = fs::read_dir(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let mut items: Vec<_> = read_dir.filter_map(std::result::Result::ok).collect();
    items.sort_by_key(std::fs::DirEntry::file_name);

    for entry in &items {
        let ft = entry.file_type().ok();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let meta = entry.metadata().ok();

        let suffix = match ft {
            Some(t) if t.is_dir() => "/".to_string(),
            Some(t) if t.is_symlink() => " →".to_string(),
            _ => match meta {
                Some(m) => {
                    let tokens = estimate_tokens(m.len());
                    format!("  ({tokens} tokens)")
                }
                None => String::new(),
            },
        };
        entries.push(format!("  {name}{suffix}"));
    }

    let header = format!("# {} ({} items)", path.display(), items.len());
    Ok(format!("{header}\n\n{}", entries.join("\n")))
}

/// Public entry point for did-you-mean on path-like fallthrough queries.
/// Resolves the query relative to scope and checks the parent directory.
pub fn suggest_similar_file(scope: &Path, query: &str) -> Option<String> {
    let resolved = scope.join(query);
    suggest_similar(&resolved)
}

/// Suggest a similar file name from the parent directory (edit distance).
fn suggest_similar(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let entries = fs::read_dir(parent).ok()?;

    let mut best: Option<(usize, String)> = None;
    for entry in entries.flatten() {
        let candidate = entry.file_name();
        let candidate = candidate.to_string_lossy();
        let dist = edit_distance(name, &candidate);
        if dist <= 3 {
            match &best {
                Some((d, _)) if dist < *d => best = Some((dist, candidate.into_owned())),
                None => best = Some((dist, candidate.into_owned())),
                _ => {}
            }
        }
    }
    best.map(|(_, name)| name)
}

/// Simple Levenshtein distance — only used on short file names.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Guess MIME type from extension for binary file headers.
fn mime_from_ext(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz" | "tgz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("wasm") => "application/wasm",
        Some("woff" | "woff2") => "font/woff2",
        Some("ttf" | "otf") => "font/ttf",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_found() {
        let input = b"# Title\nSome content\n## Section\nSection content\n";
        let result = resolve_heading(input, "## Section");

        assert_eq!(result, Some((3, 4)));
    }

    #[test]
    fn heading_not_found() {
        let input = b"# Title\nContent\n";
        let result = resolve_heading(input, "## Missing");

        assert_eq!(result, None);
    }

    #[test]
    fn heading_in_code_block() {
        let input = b"# Real\n```\n## Fake\n```\n";
        let result = resolve_heading(input, "## Fake");

        // Heading inside code block should be skipped
        assert_eq!(result, None);
    }

    #[test]
    fn duplicate_headings() {
        let input = b"## First\ntext\n## First\ntext\n";
        let result = resolve_heading(input, "## First");

        // Should return the first occurrence
        assert_eq!(result, Some((1, 2)));
    }

    #[test]
    fn last_heading_to_eof() {
        let input = b"# Start\ntext\n## End\nfinal line\n";
        let result = resolve_heading(input, "## End");

        // Last heading should extend to total_lines (4)
        assert_eq!(result, Some((3, 4)));
    }

    #[test]
    fn nested_sections() {
        let input = b"## A\ncontent\n### B\nmore\n## C\ntext\n";
        let result = resolve_heading(input, "## A");

        // ## A should include ### B, ending when ## C starts (line 5)
        // So range is [1, 4]
        assert_eq!(result, Some((1, 4)));
    }

    #[test]
    fn no_hashes() {
        let input = b"# Heading\ntext\n";

        // Empty string
        assert_eq!(resolve_heading(input, ""), None);

        // String without hashes
        assert_eq!(resolve_heading(input, "hello"), None);
    }

    #[test]
    fn default_path_read_returns_outline_not_full() {
        let path = std::env::temp_dir().join("srcwalk_default_outline.rs");
        std::fs::write(&path, b"fn alpha() {}\nfn beta() {}\n").unwrap();

        let cache = OutlineCache::new();
        let out = read_file(&path, None, false, &cache).unwrap();

        assert!(out.contains("[outline]"), "expected outline header: {out}");
        assert!(
            !out.contains("[full]"),
            "default read must not be full: {out}"
        );
        assert!(
            out.contains("alpha"),
            "outline should include symbols: {out}"
        );
        assert!(
            out.contains("retry with --full"),
            "outline footer should mention --full for raw text: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_full_fits_raw_caps() {
        let path = std::env::temp_dir().join("srcwalk_full_fits.rs");
        std::fs::write(&path, b"fn alpha() {}\nfn beta() {}\n").unwrap();

        let cache = OutlineCache::new();
        let out = read_file(&path, None, true, &cache).unwrap();

        assert!(
            out.contains("[full]"),
            "explicit full should be full: {out}"
        );
        assert!(
            out.contains("1  fn alpha()"),
            "full body should be numbered: {out}"
        );
        assert!(
            !out.contains("full=true capped"),
            "small full should not cap: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_full_caps_after_raw_line_limit() {
        use std::io::Write;

        let path = std::env::temp_dir().join("srcwalk_full_line_cap.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..250 {
            writeln!(f, "fn func_{i}() {{}}").unwrap();
        }
        drop(f);

        let cache = OutlineCache::new();
        let out = read_file(&path, None, true, &cache).unwrap();

        assert!(
            out.contains("full=true capped"),
            "expected cap warning: {out}"
        );
        assert!(
            out.contains("Showing first 200 of 251 lines"),
            "expected 200-line page: {out}"
        );
        assert!(
            out.contains("--section 201-<end>"),
            "expected next-page hint: {out}"
        );
        assert!(
            out.contains("func_0"),
            "expected first page body/outline: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn long_section_over_200_lines_returns_source_when_within_token_limit() {
        use std::io::Write;

        let path = std::env::temp_dir().join("srcwalk_section_long_fn.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn long_fn() {{").unwrap();
        for i in 0..220 {
            writeln!(f, "    let value_{i} = {i};").unwrap();
        }
        writeln!(f, "}}").unwrap();
        drop(f);

        let cache = OutlineCache::new();
        let out = read_file(&path, Some("long_fn"), false, &cache).unwrap();

        assert!(out.contains("[section]"), "expected raw section: {out}");
        assert!(
            out.contains("let value_219 = 219;"),
            "expected full long function source: {out}"
        );
        assert!(
            !out.contains("[section, outline (over limit)]"),
            "line count alone should not force an outline: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn section_budget_controls_token_degradation() {
        let path = std::env::temp_dir().join("srcwalk_section_budget.rs");
        let mut body = String::from("fn noisy() {\n");
        for i in 0..80 {
            body.push_str(&format!(
                "    let value_{i} = \"padding padding padding padding padding padding padding padding\";\n"
            ));
        }
        body.push_str("}\n");
        std::fs::write(&path, body).unwrap();

        let cache = OutlineCache::new();
        let low_budget =
            read_file_with_budget(&path, Some("noisy"), false, Some(100), &cache).unwrap();
        assert!(
            low_budget.contains("[section, outline (over limit)]"),
            "expected low budget to outline: {low_budget}"
        );
        assert!(
            low_budget.contains("limit: 100 tokens"),
            "expected budget limit in footer: {low_budget}"
        );

        let high_budget =
            read_file_with_budget(&path, Some("noisy"), false, Some(5_000), &cache).unwrap();
        assert!(
            high_budget.contains("[section]") && high_budget.contains("padding padding"),
            "expected high budget to return source: {high_budget}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn budget_cascade_full_to_outline() {
        // Build a file large enough that --full would emit ~5k tokens.
        let mut body = String::from("<?php\nclass Big {\n");
        for i in 0..120 {
            body.push_str(&format!(
                "    public function method_{i}() {{\n        $x = {i}; // padding line {i}\n        return $x * 2;\n    }}\n"
            ));
        }
        body.push_str("}\n");
        let path = std::env::temp_dir().join("srcwalk_p11_cascade.php");
        std::fs::write(&path, body.as_bytes()).unwrap();

        let cache = OutlineCache::new();
        let out = read_file_with_budget(&path, None, true, Some(800), &cache).unwrap();

        // Budget honored.
        let tokens = estimate_tokens(out.len() as u64);
        assert!(tokens <= 800, "cascade overshot budget: {tokens} tokens");
        // Header relabelled, not [full].
        assert!(
            out.contains("[outline (full requested, over budget)]") || out.contains("[signatures"),
            "expected cascade header label, got: {}",
            &out[..out.len().min(200)]
        );
        // Cascade note present.
        assert!(out.contains("exceeded budget"), "missing cascade note");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn budget_cascade_passthrough_when_fits() {
        // Tiny file fits in budget → unchanged behavior (full content).
        let path = std::env::temp_dir().join("srcwalk_p11_tiny.php");
        std::fs::write(&path, b"<?php\nclass Tiny { public function f() {} }\n").unwrap();

        let cache = OutlineCache::new();
        let out = read_file_with_budget(&path, None, true, Some(2000), &cache).unwrap();

        assert!(
            out.contains("[full]"),
            "expected [full] label, got header in: {out}"
        );
        assert!(
            !out.contains("exceeded budget"),
            "no cascade note for fitting file"
        );

        let _ = std::fs::remove_file(&path);
    }

    // --- suggest_symbols tests ---

    #[test]
    fn suggest_symbols_prefix_match() {
        let code = b"fn collect_ranges() {}\nfn collect_names() {}\nfn parse_input() {}\n";
        let path = std::env::temp_dir().join("srcwalk_suggest_prefix.rs");
        std::fs::write(&path, code).unwrap();

        let suggestions = suggest_symbols(code, &path, "collect", 3);
        assert!(
            suggestions.len() >= 2,
            "expected at least 2 prefix matches: {suggestions:?}"
        );
        // Prefix matches should come first (distance 0)
        assert!(
            suggestions[0].starts_with("collect_"),
            "first should be prefix match: {}",
            suggestions[0]
        );
        assert!(
            suggestions[1].starts_with("collect_"),
            "second should be prefix match: {}",
            suggestions[1]
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn suggest_symbols_edit_distance_fallback() {
        let code = b"fn tag_comment_matches() {}\nfn find_symbol() {}\n";
        let path = std::env::temp_dir().join("srcwalk_suggest_edit.rs");
        std::fs::write(&path, code).unwrap();

        let suggestions = suggest_symbols(code, &path, "tag_comment", 3);
        assert!(!suggestions.is_empty(), "should have suggestions");
        assert!(
            suggestions[0].contains("tag_comment_matches"),
            "closest should be tag_comment_matches: {}",
            suggestions[0]
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn suggest_symbols_includes_line_ranges() {
        let code = b"fn alpha() {}\nfn beta() {}\n";
        let path = std::env::temp_dir().join("srcwalk_suggest_ranges.rs");
        std::fs::write(&path, code).unwrap();

        let suggestions = suggest_symbols(code, &path, "alph", 3);
        assert!(!suggestions.is_empty());
        // Format should be "name [start-end]"
        assert!(
            suggestions[0].contains('[') && suggestions[0].contains(']'),
            "should include line range: {}",
            suggestions[0]
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn suggest_symbols_empty_for_non_code() {
        let md = b"# Heading\nSome text\n";
        let path = std::env::temp_dir().join("srcwalk_suggest_md.md");
        std::fs::write(&path, md).unwrap();

        let suggestions = suggest_symbols(md, &path, "foo", 3);
        assert!(
            suggestions.is_empty(),
            "non-code file should return empty suggestions"
        );

        let _ = std::fs::remove_file(&path);
    }

    // --- symbol suggest on miss integration ---

    #[test]
    fn section_symbol_miss_shows_suggestions() {
        let code = "fn resolve_heading() {}\nfn resolve_symbol() {}\nfn resolve_range() {}\n";
        let path = std::env::temp_dir().join("srcwalk_section_miss.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let err = read_section(&path, "resolve_sym", None, &cache).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symbol not found. Closest:"),
            "should show suggestions: {msg}"
        );
        assert!(
            msg.contains("resolve_symbol"),
            "should suggest resolve_symbol: {msg}"
        );

        let _ = std::fs::remove_file(&path);
    }

    // --- multi-symbol section tests ---

    #[test]
    fn multi_symbol_section_returns_all_bodies() {
        let code = "fn aaa() {\n    1\n}\nfn bbb() {\n    2\n}\nfn ccc() {\n    3\n}\n";
        let path = std::env::temp_dir().join("srcwalk_multi_sym.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let out = read_section(&path, "aaa,ccc", None, &cache).unwrap();
        assert!(
            out.contains("2 symbols, section"),
            "header should show symbol count: {out}"
        );
        assert!(out.contains("aaa()"), "should contain aaa body");
        assert!(out.contains("ccc()"), "should contain ccc body");
        assert!(!out.contains("bbb()"), "should NOT contain bbb body");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_symbol_section_sorted_by_line_order() {
        let code = "fn first() {\n    1\n}\nfn second() {\n    2\n}\n";
        let path = std::env::temp_dir().join("srcwalk_multi_order.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        // Request in reverse order
        let out = read_section(&path, "second,first", None, &cache).unwrap();
        let pos_first = out.find("first()").unwrap();
        let pos_second = out.find("second()").unwrap();
        assert!(
            pos_first < pos_second,
            "should be sorted by line order, not request order"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_symbol_section_partial_miss_returns_found() {
        let code = "fn real_fn() {}\nfn other_fn() {}\n";
        let path = std::env::temp_dir().join("srcwalk_multi_miss.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let out = read_section(&path, "real_fn,nope_fn", None, &cache).unwrap();
        assert!(
            out.contains("real_fn()"),
            "should contain found symbol: {out}"
        );
        assert!(
            out.contains("Missing symbols"),
            "should note missing: {out}"
        );
        assert!(out.contains("nope_fn"), "should name missing symbol: {out}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_symbol_section_all_miss_errors() {
        let code = "fn real_fn() {}\n";
        let path = std::env::temp_dir().join("srcwalk_multi_all_miss.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let err = read_section(&path, "zzz_fake,yyy_fake", None, &cache).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symbols not found"),
            "all-miss should error: {msg}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_section_line_ranges_return_all_blocks() {
        let code = "l1\nl2\nl3\nl4\nl5\nl6\n";
        let path = std::env::temp_dir().join("srcwalk_multi_ranges.txt");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let out = read_section(&path, "5-6,2-3", None, &cache).unwrap();
        assert!(
            out.contains("2 sections, section"),
            "header should show section count: {out}"
        );
        let pos_l2 = out.find("l2").unwrap();
        let pos_l5 = out.find("l5").unwrap();
        assert!(
            pos_l2 < pos_l5,
            "blocks should be sorted by line order: {out}"
        );
        assert!(out.contains("---"), "blocks should be separated: {out}");
        assert!(
            !out.contains("l4"),
            "unrequested line should be omitted: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_section_mixes_symbol_and_line_range() {
        let code = "fn first() {\n    1\n}\nlet outside = 9;\nfn second() {\n    2\n}\n";
        let path = std::env::temp_dir().join("srcwalk_multi_mixed.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let out = read_section(&path, "second,4-4", None, &cache).unwrap();
        assert!(
            out.contains("2 sections, section"),
            "mixed list should use sections wording: {out}"
        );
        assert!(out.contains("outside = 9"), "should contain range: {out}");
        assert!(
            out.contains("second()"),
            "should contain symbol body: {out}"
        );
        assert!(
            !out.contains("first()"),
            "unrequested symbol should be omitted: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_section_partial_miss_returns_found() {
        let code = "fn real_fn() {}\nlet kept = true;\n";
        let path = std::env::temp_dir().join("srcwalk_multi_section_miss.rs");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let out = read_section(&path, "real_fn,nope_fn,2-2", None, &cache).unwrap();
        assert!(
            out.contains("real_fn()"),
            "should contain found symbol: {out}"
        );
        assert!(
            out.contains("kept = true"),
            "should contain found range: {out}"
        );
        assert!(
            out.contains("Missing sections"),
            "should note missing: {out}"
        );
        assert!(
            out.contains("nope_fn"),
            "should name missing section: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn multi_section_all_invalid_ranges_error() {
        let code = "one\ntwo\n";
        let path = std::env::temp_dir().join("srcwalk_multi_section_oob.txt");
        std::fs::write(&path, code).unwrap();

        let cache = OutlineCache::new();
        let err = read_section(&path, "10-12,20-21", None, &cache).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("sections not found"),
            "all-invalid should error: {msg}"
        );
        assert!(
            msg.contains("range out of bounds"),
            "should explain bounds: {msg}"
        );

        let _ = std::fs::remove_file(&path);
    }
}
