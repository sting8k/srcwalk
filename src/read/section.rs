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

use super::{edit_distance, RAW_TOKEN_CAP};

fn section_token_limit(budget: Option<u64>) -> u64 {
    budget.unwrap_or_else(|| {
        std::env::var("SRCWALK_SECTION_SOFT_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(RAW_TOKEN_CAP)
    })
}

/// Resolve a heading address to a line range in a markdown file.
/// Returns `(start_line, end_line)` as 1-indexed inclusive range.
/// Returns `None` if heading not found.
pub(super) fn resolve_heading(buf: &[u8], heading: &str) -> Option<(usize, usize)> {
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
pub(super) fn read_section(
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
                let next = section_over_limit_next_step(path, range, line_count, file_type);
                return Ok(format!(
                    "{header}\n\n{body}\n\n\
                     > Caveat: section cap ~{tok_est}/{limit} tokens; lines {line_count}; outline {start}-{end}.\n\
                     > Next: {next}"
                ));
            }
        }

        // Fallback: no structured outline available — return header + advice only
        let next = section_over_limit_next_step(path, range, line_count, file_type);
        return Ok(format!(
            "{header}\n\n\
             > Caveat: section cap ~{tok_est}/{limit} tokens; lines {line_count}.\n\
             > Next: {next}"
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

fn section_over_limit_next_step(
    path: &Path,
    section: &str,
    line_count: u32,
    file_type: FileType,
) -> String {
    if line_count <= 1 && is_js_ts_file_type(file_type) {
        return format!(
            "minified artifact? retry `srcwalk {} --artifact --section {}` or `--artifact --section bytes:<start>-<end>`.",
            crate::format::display_path(path),
            section
);
    }
    "use narrower --section or --budget <N>.".to_string()
}

fn is_js_ts_file_type(file_type: FileType) -> bool {
    matches!(
        file_type,
        FileType::Code(
            crate::types::Lang::JavaScript
                | crate::types::Lang::TypeScript
                | crate::types::Lang::Tsx
        )
    )
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

    let mut valid_blocks: Vec<(usize, usize, Option<usize>, String)> = Vec::new();
    for (start, end, focus, label) in blocks {
        let s = start.saturating_sub(1);
        if s >= total {
            errors.push(format!(
                "{label}: range out of bounds (file has {total} lines)"
            ));
            continue;
        }
        valid_blocks.push((start, end.min(total), focus, label));
    }

    let mut merged_blocks: Vec<(usize, usize, Option<usize>, String)> = Vec::new();
    for (start, end, focus, label) in valid_blocks {
        if let Some((_, last_end, last_focus, last_label)) = merged_blocks.last_mut() {
            if start <= *last_end {
                *last_end = (*last_end).max(end);
                if *last_focus != focus {
                    *last_focus = None;
                }
                *last_label = format!("{last_label}, {label}");
                continue;
            }
        }
        merged_blocks.push((start, end, focus, label));
    }

    let limit = section_token_limit(budget);
    let compact_line_cap = compact_section_line_cap(limit, merged_blocks.len());
    let mut parts: Vec<String> = Vec::new();
    let mut compact_parts: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_lines: u32 = 0;

    for (start, end, focus, label) in &merged_blocks {
        let s = start.saturating_sub(1);
        let e = *end;
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
        let formatted = if let Some(focus) = focus {
            format_focused_lines(&selected, *start as u32, *focus)
        } else {
            format::number_lines(&selected, *start as u32)
        };
        parts.push(format!(
            "## section: {label} [{start}-{end}]\n\n{formatted}"
        ));
        compact_parts.push(format_compact_section(
            &selected,
            *start,
            *end,
            *focus,
            label,
            compact_line_cap,
        ));
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

    if tok_est > limit {
        let section_count = compact_parts.len();
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
        let header = format::file_header(path, total_bytes, total_lines, ViewMode::SectionOutline)
            .replace(
                "[section, outline (over limit)]",
                &format!("[{section_count} {plural}, compact (over limit)]"),
            );
        let body = compact_parts.join("\n\n---\n\n");
        let mut output = format!(
            "{header}\n\n{body}\n\n\
             > Caveat: compacted ~{tok_est}/{limit} tokens; shown {section_count} {plural}.\n\
             > Next: narrow --section or raise --budget."
        );
        if !errors.is_empty() {
            let missing = errors.join("\n  ");
            let missing_label = if all_symbol_names {
                "Missing symbols"
            } else {
                "Missing sections"
            };
            let _ = write!(output, "\n> {missing_label}:\n>   {missing}");
        }
        return Ok(output);
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

fn compact_section_line_cap(limit: u64, section_count: usize) -> usize {
    let usable = limit.saturating_sub(160);
    let per_section = usable / section_count.max(1) as u64;
    ((per_section / 12) as usize).clamp(3, 12)
}

fn format_compact_section(
    selected: &str,
    start: usize,
    end: usize,
    focus: Option<usize>,
    label: &str,
    line_cap: usize,
) -> String {
    let lines: Vec<&str> = selected.lines().collect();
    let total = lines.len();
    let anchor = focus
        .filter(|line| (*line >= start) && (*line <= end))
        .or_else(|| first_range_start_in_label(label, start, end));
    let shown = compact_line_indices(total, start, anchor, line_cap);
    let width = (start + total.saturating_sub(1)).max(1).ilog10() as usize + 1;
    let mut formatted = String::new();
    let mut previous_idx = None;
    for idx in &shown {
        if let Some(prev) = previous_idx {
            if *idx > prev + 1 {
                let _ = writeln!(formatted, "  ...");
            }
        }
        let num = start + idx;
        let prefix = if anchor == Some(num) { "► " } else { "  " };
        let _ = writeln!(formatted, "{prefix}{num:>width$} │ {}", lines[*idx]);
        previous_idx = Some(*idx);
    }
    if total > shown.len() {
        let omitted = total - shown.len();
        let _ = writeln!(
            formatted,
            "  ... {omitted} lines omitted; narrow --section or raise --budget."
        );
    }

    format!(
        "## section: {label} [{start}-{end}] (compact)\n\n{}",
        formatted.trim_end()
    )
}

fn first_range_start_in_label(label: &str, start: usize, end: usize) -> Option<usize> {
    label
        .split(',')
        .filter_map(|part| parse_range(part.trim()))
        .map(|(range_start, _, focus)| focus.unwrap_or(range_start))
        .find(|line| (*line >= start) && (*line <= end))
}

fn compact_line_indices(
    total: usize,
    section_start: usize,
    anchor: Option<usize>,
    line_cap: usize,
) -> Vec<usize> {
    if total <= line_cap {
        return (0..total).collect();
    }
    let Some(anchor_line) = anchor else {
        return (0..line_cap).collect();
    };
    let anchor_idx = anchor_line.saturating_sub(section_start).min(total - 1);
    if anchor_idx < line_cap {
        return (0..line_cap).collect();
    }

    let head_count = (line_cap / 3).clamp(1, 3);
    let anchor_count = line_cap.saturating_sub(head_count).max(1);
    let before = anchor_count / 2;
    let anchor_start = anchor_idx
        .saturating_sub(before)
        .min(total.saturating_sub(anchor_count));

    let mut indices: Vec<usize> = (0..head_count).collect();
    indices.extend(anchor_start..anchor_start + anchor_count);
    indices.sort_unstable();
    indices.dedup();
    indices.truncate(line_cap);
    indices
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
pub(super) fn suggest_symbols(buf: &[u8], path: &Path, query: &str, top_n: usize) -> Vec<String> {
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
        if entry.name == symbol || entry.signature.as_deref() == Some(symbol) {
            return Some((entry.start_line as usize, entry.end_line as usize));
        }
        // Search children (methods inside class, etc.)
        if let Some(range) = find_symbol_in_entries(&entry.children, symbol) {
            return Some(range);
        }
    }
    None
}
