use std::collections::HashSet;
use std::path::{Path, PathBuf};

mod html;
mod markdown;

use crate::types::{Lang, OutlineEntry, OutlineKind};

const MAX_NAME_LEN: usize = 120;

pub(crate) fn is_document_lang(lang: Lang) -> bool {
    matches!(lang, Lang::Html | Lang::Markdown)
}

pub(crate) fn outline_entries(content: &str, lang: Lang) -> Vec<OutlineEntry> {
    match lang {
        Lang::Html => html::outline_entries(content),
        Lang::Markdown => markdown::outline_entries(content),
        _ => Vec::new(),
    }
}

pub(crate) fn dependency_sources(content: &str, lang: Lang) -> Vec<String> {
    match lang {
        Lang::Html => html::dependency_sources(content),
        Lang::Markdown => markdown::dependency_sources(content),
        _ => Vec::new(),
    }
}

pub(crate) fn is_external_source(source: &str) -> bool {
    let source = source.trim();
    if source.is_empty() || source.starts_with('#') || is_windows_drive_path(source) {
        return false;
    }

    let lower = source.to_ascii_lowercase();
    if lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
    {
        return false;
    }

    source.starts_with("//") || source.starts_with('/') || has_url_scheme(source)
}

pub(crate) fn resolve_source(dir: &Path, source: &str, lang: Lang) -> Option<PathBuf> {
    let path_part = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .trim()
        .trim_matches('<')
        .trim_matches('>');
    if path_part.is_empty() || is_external_source(path_part) {
        return None;
    }

    let candidate = dir.join(path_part);
    if candidate.is_file() {
        return Some(candidate);
    }
    if candidate.is_dir() {
        for name in ["README.md", "index.md", "index.html", "index.htm"] {
            let nested = candidate.join(name);
            if nested.is_file() {
                return Some(nested);
            }
        }
    }

    if candidate.extension().is_none() {
        let exts: &[&str] = match lang {
            Lang::Html => &["html", "htm", "md"],
            Lang::Markdown => &["md", "mdx", "rst", "html", "htm"],
            _ => &[],
        };
        for ext in exts {
            let with_ext = candidate.with_extension(ext);
            if with_ext.is_file() {
                return Some(with_ext);
            }
        }
    }

    None
}

pub(crate) fn outline_name_matches(kind: OutlineKind, name: &str, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }

    let name_norm = normalized(name).to_ascii_lowercase();
    let query_norm = normalized(query).to_ascii_lowercase();
    if name_norm == query_norm {
        return true;
    }

    match kind {
        OutlineKind::Section => section_name_matches(name, query),
        OutlineKind::Element => element_name_matches(name, query),
        OutlineKind::CodeBlock => code_block_name_matches(name, query),
        _ => false,
    }
}

fn section_name_matches(name: &str, query: &str) -> bool {
    let query_norm = normalized(query).to_ascii_lowercase();
    let query_anchor = query_norm.trim_start_matches('#');
    let text = normalized(name);
    let (text, anchor) = if let Some((before, found)) = split_trailing_anchor(&text) {
        (before.to_string(), Some(found.to_string()))
    } else {
        (text, None)
    };
    let text_norm = text.to_ascii_lowercase();
    let text_without_title = text_norm.strip_prefix("title: ").unwrap_or(&text_norm);

    text_norm == query_norm
        || text_without_title == query_norm
        || slugify(&text) == query_anchor
        || anchor.is_some_and(|id| id.eq_ignore_ascii_case(query_anchor))
}

fn element_name_matches(name: &str, query: &str) -> bool {
    let query_norm = query.trim().to_ascii_lowercase();
    let query_anchor = query_norm.trim_start_matches('#');
    let name_norm = name.to_ascii_lowercase();

    if name_norm == query_norm {
        return true;
    }
    if let Some(id) = fragment_id_in_name(name) {
        if id.eq_ignore_ascii_case(query_anchor) {
            return true;
        }
    }
    if let Some(tag) = name.split(['#', '[', ' ']).next() {
        if tag.eq_ignore_ascii_case(&query_norm) {
            return true;
        }
    }
    false
}

fn code_block_name_matches(name: &str, query: &str) -> bool {
    let query = query.trim();
    name.eq_ignore_ascii_case(query)
        || (name == "code" && query.eq_ignore_ascii_case("code-block"))
        || query.eq_ignore_ascii_case(&format!("{name} code"))
}

pub(super) fn push_import_entry(
    source: String,
    line: u32,
    entries: &mut Vec<OutlineEntry>,
    seen: &mut HashSet<String>,
) {
    if source.is_empty() || !seen.insert(source.clone()) {
        return;
    }
    entries.push(OutlineEntry {
        kind: OutlineKind::Import,
        name: clipped(source),
        start_line: line,
        end_line: line,
        signature: None,
        children: Vec::new(),
        doc: None,
    });
}

fn first_direct_named_child<'tree>(
    node: tree_sitter::Node<'tree>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.is_named() && kinds.contains(&child.kind()));
    found
}

pub(super) fn node_text(node: tree_sitter::Node, lines: &[&str]) -> String {
    text_between(lines, node.start_position(), node.end_position())
}

pub(super) fn text_between(
    lines: &[&str],
    start: tree_sitter::Point,
    end: tree_sitter::Point,
) -> String {
    if start.row >= lines.len() || end.row >= lines.len() || start.row > end.row {
        return String::new();
    }

    if start.row == end.row {
        return slice_line(lines[start.row], start.column, end.column).to_string();
    }

    let mut out = String::new();
    out.push_str(slice_line_from(lines[start.row], start.column));
    for line in lines.iter().take(end.row).skip(start.row + 1) {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.push_str(slice_line_to(lines[end.row], end.column));
    out
}

pub(super) fn slice_line(line: &str, start: usize, end: usize) -> &str {
    let start = line.floor_char_boundary(start.min(line.len()));
    let end = line.floor_char_boundary(end.min(line.len())).max(start);
    &line[start..end]
}

pub(super) fn slice_line_from(line: &str, start: usize) -> &str {
    let start = line.floor_char_boundary(start.min(line.len()));
    &line[start..]
}

pub(super) fn slice_line_to(line: &str, end: usize) -> &str {
    let end = line.floor_char_boundary(end.min(line.len()));
    &line[..end]
}

pub(super) fn start_line(node: tree_sitter::Node) -> u32 {
    node.start_position().row as u32 + 1
}

pub(super) fn end_line(node: tree_sitter::Node) -> u32 {
    node.end_position().row as u32 + 1
}

pub(super) fn normalized(text: impl AsRef<str>) -> String {
    text.as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn clipped(text: impl AsRef<str>) -> String {
    let text = text.as_ref().trim();
    if text.len() > MAX_NAME_LEN {
        format!("{}...", crate::types::truncate_str(text, MAX_NAME_LEN - 3))
    } else {
        text.to_string()
    }
}

pub(super) fn decode_basic_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn has_url_scheme(source: &str) -> bool {
    let Some(pos) = source.find(':') else {
        return false;
    };
    let scheme = &source[..pos];
    if scheme.len() < 2 || !scheme.as_bytes()[0].is_ascii_alphabetic() {
        return false;
    }
    scheme
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
}

fn is_windows_drive_path(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn split_trailing_anchor(text: &str) -> Option<(&str, &str)> {
    let hash = text.rfind(" #")?;
    let anchor = text[hash + 2..].trim();
    if anchor.is_empty() || anchor.contains(char::is_whitespace) {
        return None;
    }
    Some((text[..hash].trim_end(), anchor.trim_start_matches('#')))
}

fn fragment_id_in_name(name: &str) -> Option<&str> {
    let hash = name.find('#')?;
    let rest = &name[hash + 1..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')))
        .unwrap_or(rest.len());
    let id = &rest[..end];
    (!id.is_empty()).then_some(id)
}

fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{html, markdown};
    use crate::types::OutlineKind;

    #[test]
    fn markdown_links_ignore_fenced_code() {
        let content = "# Guide\n[local](docs/setup.md)\n```md\n[noise](secret.md)\n# Fake\n```\n";
        let sources = markdown::dependency_sources(content);
        assert_eq!(sources, vec!["docs/setup.md"]);
        let entries = markdown::outline_entries(content);
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section && entry.name == "Guide"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::CodeBlock && entry.name == "md"));
        assert!(!entries.iter().any(|entry| entry.name == "Fake"));
    }
    #[test]
    fn markdown_fences_require_clean_closing_and_headings_allow_three_space_indent() {
        let content =
            "# Top\n```md\n``` trailing text\n# Hidden\n```\n   ## Visible\n    ## Indented code\n";
        let entries = markdown::outline_entries(content);
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section && entry.name == "Top"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section && entry.name == "Visible"));
        assert!(!entries.iter().any(|entry| entry.name == "Hidden"));
        assert!(!entries.iter().any(|entry| entry.name == "Indented code"));
    }

    #[test]
    fn markdown_four_space_fences_are_content_not_fence_markers() {
        let content = "# Top\n    ```md\n# Visible after indented opener\n```md\n    ```\n# Hidden\n```\n# Visible after real close\n";
        let entries = markdown::outline_entries(content);
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section && entry.name == "Top"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section
                && entry.name == "Visible after indented opener"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section
                && entry.name == "Visible after real close"));
        assert!(!entries.iter().any(|entry| entry.name == "Hidden"));
    }

    #[test]
    fn html_sources_and_outline_use_parser_ranges() {
        let content = r#"<!doctype html>
<html>
<head><title>Home</title><link rel="stylesheet" href="./style.css"></head>
<body>
<main id="app"><h1 id="hero">Welcome</h1><my-card src="ignored"></my-card></main>
<script src="./app.js"></script>
<img srcset="small.png 1x, large.png 2x" src="fallback.png">
<img data-caption="Thumbnail src image" src="real.png">
</body>
</html>"#;
        let sources = html::dependency_sources(content);
        assert_eq!(
            sources,
            vec![
                "./style.css",
                "./app.js",
                "fallback.png",
                "small.png",
                "large.png",
                "real.png"
            ]
        );
        let entries = html::outline_entries(content);
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Section && entry.name == "title: Home"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == OutlineKind::Element && entry.name == "main#app"));
    }
}
