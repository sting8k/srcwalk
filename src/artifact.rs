use std::path::Path;

use crate::types::{FileType, Lang};
use crate::ArtifactMode;

const MAX_ARTIFACT_ANCHORS: usize = 20;

pub(crate) fn is_artifact_js_ts_file(path: &Path) -> bool {
    matches!(
        crate::lang::detect_file_type(path),
        FileType::Code(Lang::JavaScript | Lang::TypeScript | Lang::Tsx)
    )
}

pub(crate) fn is_artifact_search_file(path: &Path) -> bool {
    matches!(
        crate::lang::detect_file_type(path),
        FileType::Code(Lang::JavaScript | Lang::TypeScript | Lang::Tsx)
    )
}

pub(crate) fn add_anchors(output: String, path: &Path, artifact: ArtifactMode) -> String {
    if !artifact.enabled() {
        return output;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return output;
    };
    let (anchors, omitted) = extract_artifact_anchors(&content);
    if anchors.is_empty() {
        return output;
    }

    let mut block = String::from("Artifact anchors:\n");
    for anchor in anchors {
        use std::fmt::Write as _;
        let _ = writeln!(
            block,
            "[{}]          {} {}",
            anchor.line, anchor.kind, anchor.name
        );
    }
    if omitted > 0 {
        use std::fmt::Write as _;
        let _ = writeln!(block, "... {omitted} more artifact anchors omitted");
    }
    let block = block.trim_end();

    if let Some(header_end) = output.find("\n\n") {
        let (header, rest) = output.split_at(header_end);
        format!("{header}\n\n{block}{rest}")
    } else {
        format!("{output}\n\n{block}")
    }
}

#[derive(Clone)]
pub(crate) struct ArtifactAnchor {
    pub(crate) line: u32,
    pub(crate) kind: &'static str,
    pub(crate) name: String,
}

fn extract_artifact_anchors(content: &str) -> (Vec<ArtifactAnchor>, usize) {
    let (anchors, total) = extract_all_artifact_anchors(content);
    let omitted = total.saturating_sub(anchors.len().min(MAX_ARTIFACT_ANCHORS));
    (
        anchors.into_iter().take(MAX_ARTIFACT_ANCHORS).collect(),
        omitted,
    )
}

pub(crate) fn capped_anchors(content: &str, limit: usize) -> (Vec<ArtifactAnchor>, usize) {
    let (anchors, total) = extract_all_artifact_anchors(content);
    let omitted = total.saturating_sub(anchors.len().min(limit));
    (anchors.into_iter().take(limit).collect(), omitted)
}

pub(crate) fn search_anchor_matches(content: &str, query: &str) -> Vec<ArtifactAnchor> {
    if query.is_empty() {
        return Vec::new();
    }
    let (anchors, _) = extract_all_artifact_anchors(content);
    anchors
        .into_iter()
        .filter(|anchor| {
            anchor.name.contains(query)
                || format!("{} {}", anchor.kind, anchor.name).contains(query)
        })
        .collect()
}

fn extract_all_artifact_anchors(content: &str) -> (Vec<ArtifactAnchor>, usize) {
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0usize;

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx as u32 + 1;
        for (kind, name) in extract_export_names(line)
            .into_iter()
            .map(|name| ("export", name))
            .chain(
                extract_named_amd_modules(line)
                    .into_iter()
                    .map(|name| ("mod", name)),
            )
        {
            total += 1;
            if seen.insert((kind, name.clone())) {
                anchors.push(ArtifactAnchor {
                    line: line_no,
                    kind,
                    name,
                });
            }
        }
    }

    (anchors, total)
}

fn extract_export_names(line: &str) -> Vec<String> {
    extract_es_export_names(line)
        .into_iter()
        .chain(extract_named_commonjs_exports(line))
        .chain(extract_umd_global_exports(line))
        .collect()
}

fn extract_named_amd_modules(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    for marker in ["define(\"", "define('"] {
        let mut rest = line;
        while let Some(pos) = rest.find(marker) {
            let after = &rest[pos + marker.len()..];
            let quote = marker.as_bytes()[marker.len() - 1] as char;
            if let Some(end) = after.find(quote) {
                let name = &after[..end];
                if is_safe_amd_module_name(name) {
                    names.push(name.to_string());
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    names
}

fn is_safe_amd_module_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && !name.chars().any(char::is_whitespace)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '@'))
}

fn extract_es_export_names(line: &str) -> Vec<String> {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("export ") else {
        return Vec::new();
    };
    if let Some(names) = rest.strip_prefix('{') {
        let names = names.split('}').next().unwrap_or(names);
        return names
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                let name = part.split(" as ").nth(1).unwrap_or(part);
                clean_export_name(name)
            })
            .collect();
    }
    for prefix in [
        "default function ",
        "function ",
        "class ",
        "const ",
        "let ",
        "var ",
    ] {
        if let Some(name) = rest.strip_prefix(prefix) {
            return clean_export_name(name).into_iter().collect();
        }
    }
    Vec::new()
}

fn extract_named_commonjs_exports(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    for marker in ["module.exports.", "exports."] {
        let mut offset = 0;
        while let Some(pos) = line[offset..].find(marker) {
            let absolute_pos = offset + pos;
            if absolute_pos > 0 {
                let prev = line.as_bytes()[absolute_pos - 1];
                if prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b'$' | b'.') {
                    offset = absolute_pos + marker.len();
                    continue;
                }
            }
            let after = &line[absolute_pos + marker.len()..];
            if let Some(name) = clean_export_name(after) {
                names.push(name);
            }
            offset = absolute_pos + marker.len();
        }
    }
    names
}

fn extract_umd_global_exports(line: &str) -> Vec<String> {
    let prefix = &line[..line.len().min(600)];
    if !(prefix.contains("module.exports") && prefix.contains("define")) {
        return Vec::new();
    }
    let wrapper_end = ["}(this", "}(", "})("]
        .into_iter()
        .filter_map(|needle| prefix.find(needle))
        .min()
        .unwrap_or(prefix.len());
    let wrapper = &prefix[..wrapper_end];
    let mut names = Vec::new();
    for eq_pos in wrapper.match_indices('=').map(|(pos, _)| pos) {
        let before = &wrapper[..eq_pos];
        let Some(dot_pos) = before.rfind('.') else {
            continue;
        };
        let name = &before[dot_pos + 1..];
        if let Some(name) = clean_export_name(name) {
            if !matches!(name.as_str(), "amd" | "exports" | "module" | "moduleName") {
                names.push(name);
            }
        }
    }
    names
}

fn clean_export_name(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    if name.is_empty() {
        return None;
    }
    Some(name)
}
