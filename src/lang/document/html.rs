use std::collections::HashSet;

use crate::types::{OutlineEntry, OutlineKind};

use super::{
    clipped, decode_basic_entities, end_line, first_direct_named_child, node_text, normalized,
    push_import_entry, start_line,
};

pub(super) fn outline_entries(content: &str) -> Vec<OutlineEntry> {
    let Some(tree) = parse_html(content) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut entries = html_dependency_entries(tree.root_node(), &lines);
    let mut body = Vec::new();
    collect_html_entries(tree.root_node(), &lines, 0, &mut body);
    entries.extend(body);
    entries
}

pub(super) fn dependency_sources(content: &str) -> Vec<String> {
    let Some(tree) = parse_html(content) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    html_dependency_entries(tree.root_node(), &lines)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

fn parse_html(content: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_html::LANGUAGE.into();
    parser.set_language(&language).ok()?;
    parser.parse(content, None)
}

fn collect_html_entries(
    node: tree_sitter::Node,
    lines: &[&str],
    depth: usize,
    out: &mut Vec<OutlineEntry>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if let Some(entry) = html_element_entry(child, lines, depth) {
            out.push(entry);
        } else {
            collect_html_entries(child, lines, depth, out);
        }
    }
}

fn html_element_entry(
    node: tree_sitter::Node,
    lines: &[&str],
    depth: usize,
) -> Option<OutlineEntry> {
    if !matches!(node.kind(), "element" | "script_element" | "style_element") {
        return None;
    }
    let tag_node = first_direct_named_child(node, &["start_tag", "self_closing_tag"])?;
    let tag = tag_name(tag_node, lines)?;
    let tag_lower = tag.to_ascii_lowercase();
    let header = clipped(node_text(tag_node, lines));
    let id = attr_value(&header, "id");
    let name_attr = attr_value(&header, "name");

    let (kind, name) = if tag_lower == "title" {
        let title = element_text(node, lines);
        if title.is_empty() {
            return None;
        }
        (OutlineKind::Section, format!("title: {title}"))
    } else if is_heading_tag(&tag_lower) {
        let text = element_text(node, lines);
        let display = if text.is_empty() { tag.clone() } else { text };
        let display = match id.as_deref() {
            Some(id) if !id.is_empty() => format!("{display} #{id}"),
            _ => display,
        };
        (OutlineKind::Section, display)
    } else if is_structural_html_element(&tag_lower, id.as_deref(), name_attr.as_deref()) {
        (
            OutlineKind::Element,
            element_display_name(&tag, id.as_deref(), name_attr.as_deref()),
        )
    } else {
        return None;
    };

    let mut children = Vec::new();
    if depth < 2 {
        collect_html_entries(node, lines, depth + 1, &mut children);
    }

    Some(OutlineEntry {
        kind,
        name: clipped(name),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: Some(header),
        children,
        doc: None,
    })
}

fn is_heading_tag(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn is_structural_html_element(tag: &str, id: Option<&str>, name_attr: Option<&str>) -> bool {
    id.is_some_and(|id| !id.is_empty())
        || name_attr.is_some_and(|name| !name.is_empty() && matches!(tag, "a" | "form" | "iframe"))
        || tag.contains('-')
        || matches!(
            tag,
            "main"
                | "section"
                | "article"
                | "nav"
                | "form"
                | "template"
                | "header"
                | "footer"
                | "aside"
                | "dialog"
        )
}

fn element_display_name(tag: &str, id: Option<&str>, name_attr: Option<&str>) -> String {
    let mut out = tag.to_string();
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        out.push('#');
        out.push_str(id);
    }
    if let Some(name) = name_attr.filter(|name| !name.is_empty()) {
        out.push_str("[name=");
        out.push_str(name);
        out.push(']');
    }
    out
}

fn html_dependency_entries(root: tree_sitter::Node, lines: &[&str]) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    collect_html_dependency_entries(root, lines, &mut entries, &mut seen);
    entries
}

fn collect_html_dependency_entries(
    node: tree_sitter::Node,
    lines: &[&str],
    entries: &mut Vec<OutlineEntry>,
    seen: &mut HashSet<String>,
) {
    if matches!(node.kind(), "start_tag" | "self_closing_tag") {
        if let Some(tag) = tag_name(node, lines) {
            let header = node_text(node, lines);
            for source in html_tag_dependency_sources(&tag, &header) {
                push_import_entry(source, start_line(node), entries, seen);
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_html_dependency_entries(child, lines, entries, seen);
        }
    }
}

fn html_tag_dependency_sources(tag: &str, header: &str) -> Vec<String> {
    let tag = tag.to_ascii_lowercase();
    let mut out = Vec::new();

    match tag.as_str() {
        "script" => push_attr_source(header, "src", &mut out),
        "link" | "a" | "area" => push_attr_source(header, "href", &mut out),
        "img" | "source" => {
            push_attr_source(header, "src", &mut out);
            push_srcset_sources(header, &mut out);
        }
        "video" => {
            push_attr_source(header, "src", &mut out);
            push_attr_source(header, "poster", &mut out);
        }
        "audio" | "iframe" | "track" | "embed" | "input" => {
            push_attr_source(header, "src", &mut out);
        }
        "object" => push_attr_source(header, "data", &mut out),
        _ => {}
    }

    out
}

fn push_attr_source(header: &str, attr: &str, out: &mut Vec<String>) {
    if let Some(source) = attr_value(header, attr).and_then(|value| clean_html_source(&value)) {
        out.push(source);
    }
}

fn push_srcset_sources(header: &str, out: &mut Vec<String>) {
    let Some(value) = attr_value(header, "srcset") else {
        return;
    };
    for candidate in value.split(',') {
        let source = candidate.split_whitespace().next().unwrap_or("");
        if let Some(source) = clean_html_source(source) {
            out.push(source);
        }
    }
}

fn clean_html_source(raw: &str) -> Option<String> {
    let source = decode_basic_entities(raw.trim());
    if source.is_empty() || source.starts_with('#') {
        return None;
    }
    Some(source)
}

fn tag_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    first_direct_named_child(node, &["tag_name"])
        .map(|child| normalized(node_text(child, lines)))
        .filter(|name| !name.is_empty())
}

fn attr_value(header: &str, attr: &str) -> Option<String> {
    let bytes = header.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && !is_attr_name_start(bytes[index]) {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && is_attr_name_char(bytes[index]) {
            index += 1;
        }
        if start == index {
            break;
        }
        let name = &header[start..index];
        let mut value_start = index;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        if !name.eq_ignore_ascii_case(attr) {
            index = skip_html_attr_value(header, value_start).max(index + 1);
            continue;
        }
        if value_start >= bytes.len() || bytes[value_start] != b'=' {
            return Some(String::new());
        }
        value_start += 1;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        return parse_html_attr_value(header, value_start);
    }
    None
}

fn skip_html_attr_value(header: &str, mut index: usize) -> usize {
    let bytes = header.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'=' {
        return index;
    }
    index += 1;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() {
        return index;
    }
    if bytes[index] == b'\'' || bytes[index] == b'"' {
        let quote = bytes[index];
        index += 1;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        if index < bytes.len() {
            index += 1;
        }
        return index;
    }
    while index < bytes.len()
        && !bytes[index].is_ascii_whitespace()
        && bytes[index] != b'>'
        && bytes[index] != b'/'
    {
        index += 1;
    }
    index
}

fn parse_html_attr_value(header: &str, mut index: usize) -> Option<String> {
    let bytes = header.as_bytes();
    if index >= bytes.len() {
        return Some(String::new());
    }
    if bytes[index] == b'\'' || bytes[index] == b'"' {
        let quote = bytes[index];
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        return Some(header[start..index].to_string());
    }
    let start = index;
    while index < bytes.len()
        && !bytes[index].is_ascii_whitespace()
        && bytes[index] != b'>'
        && bytes[index] != b'/'
    {
        index += 1;
    }
    Some(header[start..index].to_string())
}

fn is_attr_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b':'
}

fn is_attr_name_char(byte: u8) -> bool {
    is_attr_name_start(byte) || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
}

fn element_text(node: tree_sitter::Node, lines: &[&str]) -> String {
    let mut parts = Vec::new();
    collect_text_nodes(node, lines, &mut parts);
    clipped(normalized(parts.join(" ")))
}

fn collect_text_nodes(node: tree_sitter::Node, lines: &[&str], out: &mut Vec<String>) {
    if node.kind() == "text" {
        let text = normalized(node_text(node, lines));
        if !text.is_empty() {
            out.push(text);
        }
        return;
    }
    if matches!(node.kind(), "script_element" | "style_element") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_text_nodes(child, lines, out);
        }
    }
}
