use std::fs;

use crate::types::SearchResult;
use crate::ArtifactMode;

pub fn compact_artifact_snippets(result: &mut SearchResult, artifact: ArtifactMode) {
    if !artifact.enabled() {
        return;
    }
    let needle = result.query.trim();
    if needle.is_empty() || needle.len() > 120 {
        return;
    }
    for m in &mut result.matches {
        if !m.is_definition && crate::artifact::is_artifact_js_ts_file(&m.path) {
            if let Some(snippet) = artifact_usage_byte_snippet(m.path.as_path(), m.line, needle) {
                m.text = snippet;
                continue;
            }
        }
        if m.text.len() <= 360 {
            continue;
        }
        if let Some(snippet) = centered_snippet(&m.text, needle, 360) {
            m.text = snippet;
        }
    }
}

fn artifact_usage_byte_snippet(path: &std::path::Path, line: u32, needle: &str) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let line_start = line_start_byte(&content, line)?;
    let line_text = content[line_start..]
        .split_once('\n')
        .map_or(&content[line_start..], |(line, _)| line);
    let line_hit = find_case_insensitive(line_text, needle)?;
    let hit_start = line_start + line_hit;
    let hit_end = hit_start + needle.len();
    let (window_start, window_end) = centered_byte_window(&content, hit_start, hit_end, 360);
    let prefix = if window_start > 0 { "…" } else { "" };
    let suffix = if window_end < content.len() {
        "…"
    } else {
        ""
    };
    let snippet = content[window_start..window_end].trim();
    if snippet.is_empty() {
        return None;
    }
    Some(format!(
        "{prefix}{snippet}{suffix}  → --section bytes:{hit_start}-{hit_end}"
    ))
}

fn line_start_byte(content: &str, line: u32) -> Option<usize> {
    if line <= 1 {
        return Some(0);
    }
    let mut current = 1_u32;
    for (idx, ch) in content.char_indices() {
        if ch == '\n' {
            current += 1;
            if current == line {
                return Some(idx + 1);
            }
        }
    }
    None
}

fn find_case_insensitive(text: &str, needle: &str) -> Option<usize> {
    text.to_lowercase().find(&needle.to_lowercase())
}

fn centered_byte_window(
    content: &str,
    hit_start: usize,
    hit_end: usize,
    max_chars: usize,
) -> (usize, usize) {
    let chars: Vec<(usize, char)> = content.char_indices().collect();
    let hit_char = chars.partition_point(|(idx, _)| *idx < hit_start);
    let half = max_chars / 2;
    let start_char = hit_char.saturating_sub(half);
    let end_char = (start_char + max_chars).min(chars.len());
    let start_byte = chars.get(start_char).map_or(0, |(idx, _)| *idx);
    let end_byte = chars
        .get(end_char)
        .map_or_else(|| content.len(), |(idx, _)| *idx);
    (start_byte, end_byte.max(hit_end))
}

fn centered_snippet(text: &str, needle: &str, max_chars: usize) -> Option<String> {
    let text_lower = text.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let hit = text_lower.find(&needle_lower)?;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let hit_char = chars.partition_point(|(idx, _)| *idx < hit);
    let half = max_chars / 2;
    let start_char = hit_char.saturating_sub(half);
    let end_char = (start_char + max_chars).min(chars.len());
    let start_byte = chars.get(start_char).map_or(0, |(idx, _)| *idx);
    let end_byte = chars
        .get(end_char)
        .map_or_else(|| text.len(), |(idx, _)| *idx);
    let prefix = if start_byte > 0 { "…" } else { "" };
    let suffix = if end_byte < text.len() { "…" } else { "" };
    Some(format!(
        "{prefix}{}{suffix}",
        text[start_byte..end_byte].trim()
    ))
}
