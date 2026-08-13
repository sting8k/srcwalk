use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{render_next_actions, NextAction};
use crate::format;
use crate::lang::detect_file_type;
use crate::lang::outline::get_outline_entries as lang_get_outline_entries;
use crate::precision;

use crate::types::{estimate_tokens, FileType, OutlineEntry, ViewMode};

use super::{edit_distance, RAW_TOKEN_CAP};

const MAX_CONTEXT_LINES: usize = 10;

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
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    read_section_with_context(path, range, budget, cache, None)
}

pub(super) fn read_section_with_context(
    path: &Path,
    range: &str,
    budget: Option<u64>,
    _cache: &OutlineCache,
    context_lines: Option<usize>,
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
    let requested_range = parse_requested_range(range);
    let (start, end) = if range.starts_with('#') {
        // Markdown heading. Try the full heading first so headings containing
        // commas still work; if that fails, fall through to comma-list parsing.
        match resolve_heading(buf, range) {
            Some((start, end)) => {
                if let Some(context) = context_lines {
                    expand_range(start, end, context)
                } else {
                    (start, end)
                }
            }
            None if range.contains(',') => {
                return read_multi_section(path, buf, range, budget, context_lines)
            }
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
        return read_multi_section(path, buf, range, budget, context_lines);
    } else if let Some(line) = parse_focused_line(range).filter(|_| context_lines.is_some()) {
        let context = context_lines.expect("checked context_lines above");
        focus_line = Some(line);
        expand_range(line, line, context)
    } else if let Some((start, end, focus)) = parse_range(range) {
        // Line range like "45-89" or focused line like "45"
        focus_line = focus;
        if let Some(context) = context_lines {
            expand_range(start, end, context)
        } else {
            (start, end)
        }
    } else if let Some((start, end)) = resolve_symbol(buf, path, range) {
        // Symbol name like "isCustomization" or "handleRequest"
        if let Some(context) = context_lines {
            expand_range(start, end, context)
        } else {
            (start, end)
        }
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

    let file_type = detect_file_type(path);

    if tok_est > limit {
        // Degrade: render outline entries within the section range
        let content = String::from_utf8_lossy(buf);
        let header = format::file_header(path, byte_len, line_count, ViewMode::SectionOutline);

        let start32 = start as u32;
        let end32 = end as u32;

        if let Some(lang) = file_type.structural_lang() {
            let entries = lang_get_outline_entries(&content, lang);
            let filtered = filter_entries_in_range(&entries, start32, end32);
            if !filtered.is_empty() {
                let body = format_section_outline(&filtered);
                let next = render_next_actions(&[NextAction::guidance(
                    section_over_limit_next_step(
                        path,
                        range,
                        (start32, end32),
                        line_count,
                        tok_est,
                        file_type,
                    ),
                    "section over-limit drilldown",
                    20,
                )]);
                let body = match super::document_packet_for_file_type(file_type, "section") {
                    Some(packet) => format!("{packet}\n\n{body}"),
                    None => body,
                };
                return Ok(format!(
                    "{header}\n\n{body}\n\n\
                     > Caveat: section cap ~{tok_est}/{limit} tokens; lines {line_count}; outline {start}-{end}.\n\
                     {next}"
                ));
            }
        }

        // Fallback: no structured outline available — return header + advice only
        let next = render_next_actions(&[NextAction::guidance(
            section_over_limit_next_step(
                path,
                range,
                (start32, end32),
                line_count,
                tok_est,
                file_type,
            ),
            "section over-limit fallback",
            20,
        )]);
        let packet = super::document_packet_for_file_type(file_type, "section");
        let body = match packet {
            Some(packet) => format!("{header}\n\n{packet}"),
            None => header,
        };
        return Ok(format!(
            "{body}\n\n\
             > Caveat: section cap ~{tok_est}/{limit} tokens; lines {line_count}.\n\
             {next}"
        ));
    }

    let header = format::file_header(path, byte_len, line_count, ViewMode::Section);
    let packet = super::document_packet_for_file_type(file_type, "section");
    let formatted = if let Some(focus) = focus_line {
        format_focused_lines(&selected, start as u32, focus)
    } else {
        format::number_lines(&selected, start as u32)
    };
    let frame = requested_range.and_then(|(requested_start, requested_end)| {
        let content = String::from_utf8_lossy(buf);
        super::completion::structural_read_frame(
            &content,
            file_type,
            requested_start as u32,
            requested_end as u32,
            s as u32 + 1,
            e as u32,
        )
    });
    let framed = match frame {
        Some(frame) => format!("{frame}\n\n{formatted}"),
        None => formatted,
    };
    let mut output = match packet {
        Some(packet) => format!("{header}\n\n{packet}\n\n{framed}"),
        None => format!("{header}\n\n{framed}"),
    };
    if parse_range(range).is_some() {
        let content = String::from_utf8_lossy(buf);
        if let Some(completion) = super::completion::partial_function_completion(
            path,
            &content,
            file_type,
            s as u32 + 1,
            e as u32,
        ) {
            output.push_str("\n\n");
            output.push_str(&completion);
        }
    }
    Ok(output)
}

fn section_over_limit_next_step(
    path: &Path,
    section: &str,
    resolved_range: (u32, u32),
    line_count: u32,
    tok_est: u64,
    file_type: FileType,
) -> String {
    if line_count <= 1 && is_js_ts_file_type(file_type) {
        return format!(
            "minified artifact? retry `srcwalk {} --artifact --section {}` or `--artifact --section bytes:<start>-<end>`.",
            crate::format::display_path(path),
            section
        );
    }

    let selector = format!("{}-{}", resolved_range.0, resolved_range.1);
    section_budget_next_step(&selector, tok_est)
}

fn section_budget_next_step(selector: &str, tok_est: u64) -> String {
    if selector.len() > 160 {
        return format!("raise --budget {tok_est} to read selected range(s), or narrow --section.");
    }
    format!(
        "read exact selected range(s) with --section {selector} --budget {tok_est}, or narrow --section."
    )
}

fn merged_section_selector(blocks: &[(usize, usize, Option<usize>, String)]) -> String {
    blocks
        .iter()
        .map(|(start, end, _, _)| format!("{start}-{end}"))
        .collect::<Vec<_>>()
        .join(",")
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

fn capped_context_lines(context_lines: Option<usize>) -> Option<usize> {
    context_lines.map(|count| count.min(MAX_CONTEXT_LINES))
}

fn expand_range(start: usize, end: usize, context: usize) -> (usize, usize) {
    (
        start.saturating_sub(context).max(1),
        end.saturating_add(context),
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
    context_lines: Option<usize>,
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
    let context_lines = capped_context_lines(context_lines);
    let mut blocks: Vec<(usize, usize, Option<usize>, String)> = Vec::new(); // start, end, focus, label
    let mut errors: Vec<String> = Vec::new();

    for section in &requested {
        if let Some(line) = parse_focused_line(section).filter(|_| context_lines.is_some()) {
            let context = context_lines.expect("checked context_lines above");
            let (start, end) = expand_range(line, line, context);
            blocks.push((start, end, Some(line), (*section).to_string()));
        } else if let Some((start, end, focus)) = parse_range(section) {
            let (start, end) = if let Some(context) = context_lines {
                expand_range(start, end, context)
            } else {
                (start, end)
            };
            blocks.push((start, end, focus, (*section).to_string()));
        } else if section.starts_with('#') {
            if let Some((start, end)) = resolve_heading(buf, section) {
                let (start, end) = if let Some(context) = context_lines {
                    expand_range(start, end, context)
                } else {
                    (start, end)
                };
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
            let (start, end) = if let Some(context) = context_lines {
                expand_range(start, end, context)
            } else {
                (start, end)
            };
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
    let file_type = detect_file_type(path);
    let mut rendered_blocks: Vec<(usize, usize, String, String)> = Vec::new();
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
        rendered_blocks.push((*start, *end, label.clone(), formatted));
        compact_parts.push(format_compact_section(
            path,
            &selected,
            *start,
            *end,
            *focus,
            label,
            compact_line_cap,
        ));
    }

    if rendered_blocks.is_empty() {
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
        let body = join_compact_parts(&compact_parts, path);
        let selector = merged_section_selector(&merged_blocks);
        let next = render_next_actions(&[NextAction::guidance(
            section_budget_next_step(&selector, tok_est),
            "compact section drilldown",
            20,
        )]);
        let packet = super::document_packet_for_file_type(file_type, "section");
        let body_with_packet = match packet {
            Some(packet) => format!("{header}\n\n{packet}\n\n{body}"),
            None => format!("{header}\n\n{body}"),
        };
        let mut output = format!(
            "{body_with_packet}\n\n\
             > Caveat: compacted ~{tok_est}/{limit} tokens; shown {section_count} {plural}.\n\
             {next}"
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

    let structural_entries = file_type.is_code().then_some(()).and_then(|()| {
        let lang = file_type.structural_lang()?;
        crate::lang::outline::outline_language(lang)?;
        let content = String::from_utf8_lossy(buf);
        Some(lang_get_outline_entries(&content, lang))
    });
    let parts = rendered_blocks
        .into_iter()
        .map(|(start, end, label, formatted)| {
            let frame = structural_entries.as_deref().and_then(|entries| {
                super::completion::structural_read_frame_from_entries(
                    entries,
                    start as u32,
                    end as u32,
                    start as u32,
                    end as u32,
                )
            });
            let framed = match frame {
                Some(frame) => format!("{frame}\n\n{formatted}"),
                None => formatted,
            };
            format!("## section: {label} [{start}-{end}]\n\n{framed}")
        })
        .collect::<Vec<_>>();
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

    let body_with_packet = match super::document_packet_for_file_type(file_type, "section") {
        Some(packet) => format!("{header}\n\n{packet}\n\n{body}"),
        None => format!("{header}\n\n{body}"),
    };

    if errors.is_empty() {
        Ok(body_with_packet)
    } else {
        let missing = errors.join("\n  ");
        let missing_label = if all_symbol_names {
            "Missing symbols"
        } else {
            "Missing sections"
        };
        Ok(format!(
            "{body_with_packet}\n\n> {missing_label}:\n>   {missing}"
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

/// Join compact sections, capping total offered lines at `precision::CAP`.
/// (US-060) Every collapsed section stays reachable via its own `show` command.
fn join_compact_parts(parts: &[String], path: &Path) -> String {
    let mut body = String::new();
    let mut offered = 0usize;

    for (i, part) in parts.iter().enumerate() {
        if precision::exceeded_line_cap(offered) {
            let _ = writeln!(
                body,
                "> ... {} sections collapsed ({} lines offered); run `srcwalk show {}:<range>` per section.",
                parts.len() - i,
                offered,
                crate::format::display_path(path),
            );
            break;
        }
        if i > 0 {
            body.push_str("\n\n---\n\n");
        }
        offered += part.lines().count();
        body.push_str(part);
    }
    body
}

fn format_compact_section(
    path: &Path,
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

    let mut header = format!("## section: {label} [{start}-{end}] (compact)");
    // US-060: a >W-line range is anchored to an `expand:` command, never printed in full.
    if precision::should_anchor_range(start.abs_diff(end)) {
        let _ = write!(
            header,
            "\n> {}\n>",
            precision::anchor_range(path, start, end, label)
        );
    }
    format!("{header}\n\n{}", formatted.trim_end())
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

fn parse_focused_line(s: &str) -> Option<usize> {
    if s.contains('-') {
        return None;
    }
    let line: usize = s.trim().parse().ok()?;
    (line > 0).then_some(line)
}

fn parse_requested_range(s: &str) -> Option<(usize, usize)> {
    if !s.contains('-') {
        let line = parse_focused_line(s)?;
        return Some((line, line));
    }

    let (start, end, _) = parse_range(s)?;
    Some((start, end))
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

/// Resolved state of a `<path>:<symbol>` target, distinguished by the shared
/// grammar parser and the AST-outline resolver. Every exact-body command
/// (show/context/callees) and callers share one result shape so ambiguity and
/// unresolved states are handled identically everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathSymbolResolution {
    /// Not a `<path>:<symbol>` form: no single separating colon, the right side
    /// is a line/range (`path:1-3`), or the drive colon of a naked `C:\` path.
    NotForm,
    /// The right side is a colon-bearing selector (`A::target`). Raw `::` input
    /// is unsupported as `path:symbol` — the canonical dotted `.` form is what
    /// commands accept. Explicit, consistent error contract.
    UnsupportedColonSymbol { symbol: String },
    /// The named file does not exist / could not be resolved to a real file.
    /// Carries the raw path and symbol so callers can report the missing path
    /// honestly without pretending an existing named file.
    NamedPathMissing { path: PathBuf, symbol: String },
    /// The named file exists and is readable, and the selector resolves uniquely
    /// to one definition.
    Unique {
        path: PathBuf,
        symbol: String,
        start_line: usize,
        end_line: usize,
    },
    /// The named file exists and is readable, and the selector has no exact
    /// definition match (zero-cardinality against the AST outline).
    NamedFileUnresolved { path: PathBuf, symbol: String },
    /// The named file exists but could not be read; resolution never ran.
    NamedFileUnreadable { path: PathBuf, symbol: String },
    /// The named file exists and was read but has no parseable structural
    /// outline (or non-UTF-8 content), so exact resolution was not attempted.
    NamedFileUnresolvable { path: PathBuf, symbol: String },
    /// The named file exists and the selector matches N>1 distinct definitions.
    Ambiguous {
        path: PathBuf,
        symbol: String,
        ranges: Vec<(usize, usize)>,
    },
}

/// Split a `<path>:<symbol>` target on its SINGLE separating colon, preserving a
/// leading Windows drive colon (`C:\\repo` / `C:/repo`) and treating a `::`
/// colon-bearing selector as its own state. Returns `(path_part, symbol)` only
/// for a clean, well-formed split; `None` otherwise.
fn split_path_symbol(target: &str) -> Option<(&str, &str)> {
    // Last colon that is not part of a "::" run (so `A::target` keeps its colons
    // in the symbol side, while `C:\\...` / `C:/...` keep the drive colon in
    // the path side).
    let mut split_at = None;
    for (i, b) in target.bytes().enumerate() {
        if b == b':' {
            let prev_double = i >= 1 && target.as_bytes()[i - 1] == b':';
            let next_double = target.as_bytes().get(i + 1) == Some(&b':');
            if !prev_double && !next_double {
                split_at = Some(i);
            }
        }
    }
    let at = split_at?;
    let path_part = &target[..at];
    let symbol = &target[at + 1..];
    if path_part.is_empty() || symbol.is_empty() {
        return None;
    }
    Some((path_part, symbol))
}

pub(crate) fn resolve_path_symbol_resolution(target: &str, scope: &Path) -> PathSymbolResolution {
    let Some((path_part, symbol)) = split_path_symbol(target) else {
        return PathSymbolResolution::NotForm;
    };

    // A naked Windows drive path (`C:\\repo` / `C:/repo`) with no symbol after
    // the drive colon: if the symbol side begins with a root slash, the "colon"
    // was actually the drive colon, not a path-symbol separator.
    if path_part.len() == 1
        && path_part.as_bytes()[0].is_ascii_alphabetic()
        && (symbol.starts_with('/') || symbol.starts_with('\\'))
    {
        return PathSymbolResolution::NotForm;
    }

    // Colon-bearing selector (`A::target`) is not the canonical dotted form.
    if symbol.contains(':') {
        return PathSymbolResolution::UnsupportedColonSymbol {
            symbol: symbol.to_string(),
        };
    }

    if parse_range(symbol).is_some() {
        return PathSymbolResolution::NotForm;
    }

    let Some(path) = resolve_existing_file(path_part, scope) else {
        return PathSymbolResolution::NamedPathMissing {
            path: PathBuf::from(path_part),
            symbol: symbol.to_string(),
        };
    };
    let Some(buf) = fs::read(&path).ok() else {
        return PathSymbolResolution::NamedFileUnreadable {
            path,
            symbol: symbol.to_string(),
        };
    };
    match resolve_symbol_ranges(&buf, &path, symbol) {
        Some(ranges) if ranges.len() == 1 => {
            let (start_line, end_line) = ranges[0];
            PathSymbolResolution::Unique {
                path,
                symbol: symbol.to_string(),
                start_line,
                end_line,
            }
        }
        Some(ranges) if ranges.is_empty() => PathSymbolResolution::NamedFileUnresolved {
            path,
            symbol: symbol.to_string(),
        },
        Some(ranges) => PathSymbolResolution::Ambiguous {
            path,
            symbol: symbol.to_string(),
            ranges,
        },
        None => PathSymbolResolution::NamedFileUnresolvable {
            path,
            symbol: symbol.to_string(),
        },
    }
}

/// A path-qualified symbol resolved from the same AST outline used by `--section`.
/// Retains only the resolved exact target (for the reader/plumbing); ambiguity
/// and unresolved states are surfaced via `resolve_path_symbol_resolution`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathSymbolTarget {
    pub(crate) path: PathBuf,
    pub(crate) symbol: String,
    pub(crate) range: Option<(usize, usize)>,
}

/// Resolve a `<path>:<symbol>` target, returning `Some` only for well-formed,
/// uniquely-resolved targets (used by the reader and best-effort plumbing).
/// Non-canonical, unresolved, and ambiguous cases return `None`; callers that
/// need to distinguish those states use `resolve_path_symbol_resolution`.
pub(crate) fn resolve_path_symbol_target(target: &str, scope: &Path) -> Option<PathSymbolTarget> {
    match resolve_path_symbol_resolution(target, scope) {
        PathSymbolResolution::Unique {
            path,
            symbol,
            start_line,
            end_line,
        } => Some(PathSymbolTarget {
            path,
            symbol,
            range: Some((start_line, end_line)),
        }),
        // The historical signature returned Some with range=None for a resolvable
        // file whose symbol didn't match, so callers (e.g. `show` suggestion)
        // treat it as a known path with no exact body. Preserve that behavior for
        // the named-file-unresolved state.
        PathSymbolResolution::NamedFileUnresolved { path, symbol, .. } => Some(PathSymbolTarget {
            path,
            symbol,
            range: None,
        }),
        // A readable real file that has no structural outline (e.g. plain text)
        // is still a recognized `path:symbol` on a real file: the prior wrapper
        // returned Some(range=None) whenever resolve_existing_file+read
        // succeeded. Preserve that for the resolvable-but-no-outline state.
        PathSymbolResolution::NamedFileUnresolvable { path, symbol, .. } => {
            Some(PathSymbolTarget {
                path,
                symbol,
                range: None,
            })
        }
        // Missing path, unreadable file, ambiguity, and colon-bearing selectors
        // return None (the prior `resolve_existing_file(...)?` /
        // `fs::read(...).ok()?` early returns produced None for a missing or
        // unreadable path). They surface through resolve_path_symbol_resolution.
        _ => None,
    }
}

fn resolve_existing_file(raw: &str, scope: &Path) -> Option<PathBuf> {
    let path = Path::new(raw);
    let mut candidates = Vec::new();
    if path.is_absolute() {
        candidates.push(path.to_path_buf());
    } else {
        candidates.push(scope.join(path));
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_path = cwd.join(path);
            if candidates.first() != Some(&cwd_path) {
                candidates.push(cwd_path);
            }
        }
    }
    candidates.into_iter().find(|candidate| {
        std::fs::metadata(candidate)
            .ok()
            .is_some_and(|meta| meta.is_file())
    })
}

/// Resolve a symbol name to its first matching `(start_line, end_line)` against
/// the AST outline. Retained for the `--section` reader, which intentionally
/// preserves first-match navigation for pre-existing exact-section behavior.
/// New exact-body plumbing uses `resolve_symbol_ranges` for cardinality.
fn resolve_symbol(buf: &[u8], path: &Path, symbol: &str) -> Option<(usize, usize)> {
    let content = std::str::from_utf8(buf).ok()?;
    let lang = detect_file_type(path).structural_lang()?;
    let entries = lang_get_outline_entries(content, lang);
    // Shared US-064 resolver: exact dotted-name precedence first, then `Q.N`
    // qualifier + plain-name interpretation (container / Go receiver).
    crate::lang::qualified::resolve_selector_first(&entries, Some(lang), symbol)
        .map(|(start, end)| (start as usize, end as usize))
}

/// Resolve a symbol name against the AST outline, returning every distinct
/// `(start_line, end_line)` match in deterministic order. Uses the shared
/// cardinality primitive `resolve_selector_matches` so N>1 same-file definitions
/// (e.g. overloads) are surfaced explicitly rather than silently selecting the
/// first. Returns `Some(vec![...])` for a resolvable file; `None` when the file
/// has no structural language or the outline cannot be read.
fn resolve_symbol_ranges(buf: &[u8], path: &Path, symbol: &str) -> Option<Vec<(usize, usize)>> {
    let content = std::str::from_utf8(buf).ok()?;
    let lang = detect_file_type(path).structural_lang()?;
    let entries = lang_get_outline_entries(content, lang);
    Some(
        crate::lang::qualified::resolve_selector_matches(&entries, Some(lang), symbol)
            .into_iter()
            .map(|(start, end)| (start as usize, end as usize))
            .collect::<Vec<_>>(),
    )
}

/// Collect symbol names from outline entries (recursively) with their line ranges,
/// then rank by prefix match + edit distance, returning top `top_n` suggestions.
pub(super) fn suggest_symbols(buf: &[u8], path: &Path, query: &str, top_n: usize) -> Vec<String> {
    let Ok(content) = std::str::from_utf8(buf) else {
        return Vec::new();
    };
    let Some(lang) = detect_file_type(path).structural_lang() else {
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

#[cfg(test)]
mod precision_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wide_compact_section_anchors_with_expand_command() {
        let selected = (0..80)
            .map(|i| format!("line {i} padding"))
            .collect::<Vec<_>>()
            .join("\n");
        // 81-letter string, wide range [1-81], focus at 40.
        let out = format_compact_section(
            Path::new("src/wide.txt"),
            &selected,
            1,
            81,
            Some(40),
            "1-81",
            12,
        );
        assert!(
            out.contains("expand: srcwalk show src/wide.txt:1-81"),
            "wide range should anchor with an expand command:\n{out}"
        );
    }

    #[test]
    fn narrow_compact_section_has_no_expand_anchor() {
        let selected = (0..10)
            .map(|i| format!("line {i} padding"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = format_compact_section(
            Path::new("src/narrow.txt"),
            &selected,
            1,
            10,
            None,
            "1-10",
            12,
        );
        assert!(
            !out.contains("expand:"),
            "ranges ≤ W must not anchor:\n{out}"
        );
    }
}
