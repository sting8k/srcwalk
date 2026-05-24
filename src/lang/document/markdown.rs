use std::collections::HashSet;

use crate::types::{OutlineEntry, OutlineKind};

use super::{clipped, decode_basic_entities, push_import_entry};

#[derive(Debug, Clone)]
struct MarkdownHeading {
    level: usize,
    start_line: u32,
    text: String,
    marker: String,
}

#[derive(Debug, Clone)]
struct FenceState {
    marker: char,
    len: usize,
    start_line: u32,
    info: String,
}

pub(super) fn outline_entries(content: &str) -> Vec<OutlineEntry> {
    let mut entries = markdown_link_entries(content);
    let mut body = Vec::new();
    let mut headings = Vec::new();
    let mut code_blocks = Vec::new();
    let total = markdown_total_lines(content);
    let mut fence: Option<FenceState> = None;

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx as u32 + 1;
        if let Some(state) = fence.as_ref() {
            if closes_fence(line, state.marker, state.len) {
                let state = fence.take().expect("checked above");
                code_blocks.push(code_block_entry(state.start_line, line_no, &state.info));
            }
            continue;
        }

        if let Some((marker, len, info)) = parse_fence_open(line) {
            fence = Some(FenceState {
                marker,
                len,
                start_line: line_no,
                info,
            });
            continue;
        }

        if let Some((level, text)) = parse_atx_heading(line) {
            headings.push(MarkdownHeading {
                level,
                start_line: line_no,
                marker: format!("{} {}", "#".repeat(level), text),
                text,
            });
        }
    }

    if let Some(state) = fence {
        code_blocks.push(code_block_entry(
            state.start_line,
            total.max(state.start_line),
            &state.info,
        ));
    }

    for (idx, heading) in headings.iter().enumerate() {
        let mut end_line = total.max(heading.start_line);
        for next in headings.iter().skip(idx + 1) {
            if next.level <= heading.level {
                end_line = next.start_line.saturating_sub(1).max(heading.start_line);
                break;
            }
        }
        body.push(OutlineEntry {
            kind: OutlineKind::Section,
            name: clipped(&heading.text),
            start_line: heading.start_line,
            end_line,
            signature: Some(clipped(&heading.marker)),
            children: Vec::new(),
            doc: None,
        });
    }

    body.extend(code_blocks);
    body.sort_by_key(|entry| (entry.start_line, entry.end_line));
    entries.extend(body);
    entries
}

fn markdown_link_entries(content: &str) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut fence: Option<FenceState> = None;

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx as u32 + 1;
        if let Some(state) = fence.as_ref() {
            if closes_fence(line, state.marker, state.len) {
                fence = None;
            }
            continue;
        }
        if let Some((marker, len, info)) = parse_fence_open(line) {
            fence = Some(FenceState {
                marker,
                len,
                start_line: line_no,
                info,
            });
            continue;
        }

        for source in markdown_sources_from_line(line) {
            push_import_entry(source, line_no, &mut entries, &mut seen);
        }
    }

    entries
}

pub(super) fn dependency_sources(content: &str) -> Vec<String> {
    markdown_link_entries(content)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    // CommonMark: 4+ leading spaces are an indented code block, not a heading.
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let level = trimmed.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &trimmed[level..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let text = clean_heading_text(after.trim());
    (!text.is_empty()).then_some((level, text))
}

fn clean_heading_text(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    while text.ends_with('#') {
        text.pop();
        text = text.trim_end().to_string();
    }

    let mut anchor = None;
    if text.ends_with('}') {
        if let Some(start) = text.rfind("{#") {
            let attr = &text[start + 2..text.len() - 1];
            let id = attr
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_start_matches('#')
                .trim();
            if !id.is_empty() {
                anchor = Some(id.to_string());
            }
            text.truncate(start);
            text = text.trim_end().to_string();
        }
    }

    match anchor {
        Some(anchor) => format!("{text} #{anchor}"),
        None => text,
    }
}

fn parse_fence_open(line: &str) -> Option<(char, usize, String)> {
    // CommonMark: 4+ leading spaces are an indented code block, not a fence.
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|&c| c == marker).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim().to_string();
    Some((marker, len, info))
}

fn closes_fence(line: &str, marker: char, len: usize) -> bool {
    // CommonMark: a closing fence may be preceded by at most 3 spaces.
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return false;
    }
    let trimmed = &line[indent..];
    let marker_count = trimmed.chars().take_while(|&c| c == marker).count();
    if marker_count < len {
        return false;
    }
    // CommonMark: a closing fence may only be followed by spaces or tabs.
    trimmed[marker_count..]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

fn code_block_entry(start_line: u32, end_line: u32, info: &str) -> OutlineEntry {
    let name = info
        .split_whitespace()
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("code");
    let fence = if info.is_empty() {
        "```".to_string()
    } else {
        format!("```{info}")
    };
    OutlineEntry {
        kind: OutlineKind::CodeBlock,
        name: clipped(name),
        start_line,
        end_line,
        signature: Some(clipped(fence)),
        children: Vec::new(),
        doc: None,
    }
}

fn markdown_sources_from_line(line: &str) -> Vec<String> {
    let mut sources = Vec::new();
    if let Some(source) = reference_definition_source(line) {
        sources.push(source);
    }

    let mut offset = 0;
    while let Some(found) = line[offset..].find("](") {
        let start = offset + found + 2;
        if let Some((raw, next)) = parse_markdown_paren_source(line, start) {
            if let Some(source) = clean_dependency_source(&raw) {
                sources.push(source);
            }
            offset = next;
        } else {
            break;
        }
    }

    sources
}

fn reference_definition_source(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find("]:")?;
    if end == 1 {
        return None;
    }
    let rest = trimmed[end + 2..].trim_start();
    first_markdown_destination(rest).and_then(|source| clean_dependency_source(&source))
}

fn parse_markdown_paren_source(line: &str, mut index: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut escaped = false;
    while index < line.len() {
        let c = line[index..].chars().next()?;
        index += c.len_utf8();
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == ')' {
            return Some((out, index));
        }
        out.push(c);
    }
    None
}

fn first_markdown_destination(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    if let Some(after) = rest.strip_prefix('<') {
        let end = after.find('>')?;
        return Some(after[..end].to_string());
    }
    let mut end = rest.len();
    for (idx, c) in rest.char_indices() {
        if c.is_whitespace() {
            end = idx;
            break;
        }
    }
    Some(rest[..end].to_string())
}

fn clean_dependency_source(raw: &str) -> Option<String> {
    let source = first_markdown_destination(raw)?.trim().to_string();
    if source.is_empty() || source.starts_with('#') {
        return None;
    }
    Some(decode_basic_entities(&source))
}

fn markdown_total_lines(content: &str) -> u32 {
    content.lines().count() as u32
}
