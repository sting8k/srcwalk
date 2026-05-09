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
        if m.is_definition || m.text.len() <= 220 {
            continue;
        }
        if let Some(snippet) = centered_snippet(&m.text, needle, 180) {
            m.text = snippet;
        }
    }
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
