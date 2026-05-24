use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::types::{Lang, OutlineEntry, OutlineKind};

const MAX_NAME_LEN: usize = 120;

pub(crate) fn is_stylesheet_lang(lang: Lang) -> bool {
    matches!(lang, Lang::Css | Lang::Scss | Lang::Less)
}

pub(crate) fn walk_top_level(
    root: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    let mut current_section: Option<OutlineEntry> = None;
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if let Some(name) = comment_heading(child, lines) {
            push_section(&mut entries, current_section.take());
            current_section = Some(section_entry(child, name));
            continue;
        }

        let Some(entry) = node_to_entry(child, lines, lang, 0) else {
            continue;
        };
        if entry.kind == OutlineKind::Import {
            entries.push(entry);
        } else if let Some(section) = current_section.as_mut() {
            section.end_line = entry.end_line;
            section.children.push(entry);
        } else {
            entries.push(entry);
        }
    }

    push_section(&mut entries, current_section);
    entries
}

pub(crate) fn dependency_sources(content: &str, lang: Lang) -> Vec<String> {
    let Some(tree) = parse_stylesheet(content, lang) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    collect_dependency_sources(tree.root_node(), &lines, lang, &mut sources, &mut seen);
    for source in url_sources(content) {
        push_source(source, &mut sources, &mut seen);
    }
    sources
}

pub(crate) fn import_source(text: &str, lang: Lang) -> Option<String> {
    import_sources(text, lang).into_iter().next()
}

fn import_sources(text: &str, lang: Lang) -> Vec<String> {
    let trimmed = text.trim();
    if !is_stylesheet_import_statement(trimmed, lang) {
        return Vec::new();
    }

    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    for source in url_sources(trimmed)
        .into_iter()
        .chain(quoted_sources(trimmed))
    {
        push_source(source, &mut sources, &mut seen);
    }
    sources
}

fn is_stylesheet_import_statement(trimmed: &str, lang: Lang) -> bool {
    if trimmed.starts_with("@import") || trimmed.starts_with("@namespace") {
        return true;
    }
    lang == Lang::Scss && (trimmed.starts_with("@use") || trimmed.starts_with("@forward"))
}

pub(crate) fn is_external_source(source: &str) -> bool {
    let source = source.trim();
    let lower = source.to_ascii_lowercase();
    if source.is_empty()
        || source.starts_with('#')
        || lower.starts_with("data:")
        || is_windows_drive_path(source)
    {
        return false;
    }
    source.starts_with('@')
        || source.starts_with("//")
        || source.starts_with('/')
        || has_url_scheme(source)
}

pub(crate) fn resolve_source(dir: &Path, source: &str, lang: Lang) -> Option<PathBuf> {
    let path_part = source.split(['?', '#']).next().unwrap_or(source).trim();
    if path_part.is_empty() {
        return None;
    }

    let candidate = dir.join(path_part);
    if candidate.is_file() {
        return Some(candidate);
    }

    stylesheet_resolution_candidates(&candidate, lang)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

fn stylesheet_resolution_candidates(candidate: &Path, lang: Lang) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let exts: &[&str] = match lang {
        Lang::Css => &["css"],
        Lang::Scss => &["scss", "css"],
        Lang::Less => &["less", "css"],
        _ => &[],
    };

    if candidate.extension().is_none() {
        for ext in exts {
            out.push(candidate.with_extension(ext));
            push_partial_candidate(candidate, ext, &mut out);
            out.push(candidate.join(format!("index.{ext}")));
            out.push(candidate.join(format!("_index.{ext}")));
        }
    } else if lang == Lang::Scss {
        if let Some(ext) = candidate.extension().and_then(|ext| ext.to_str()) {
            push_partial_candidate(candidate, ext, &mut out);
        }
    }

    out
}

fn push_partial_candidate(candidate: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let stem = if candidate.extension().is_some() {
        candidate.file_stem()
    } else {
        candidate.file_name()
    };
    let Some(stem) = stem.and_then(|name| name.to_str()) else {
        return;
    };
    if stem.starts_with('_') {
        return;
    }
    let mut partial_name = String::with_capacity(stem.len() + ext.len() + 2);
    partial_name.push('_');
    partial_name.push_str(stem);
    partial_name.push('.');
    partial_name.push_str(ext);
    out.push(candidate.with_file_name(partial_name));
}

pub(crate) fn outline_name_matches(kind: OutlineKind, name: &str, query: &str) -> bool {
    match kind {
        OutlineKind::Selector => selector_matches_query(name, query),
        OutlineKind::AtRule => at_rule_matches_query(name, query),
        OutlineKind::Variable => variable_matches_query(name, query),
        _ => false,
    }
}

fn parse_stylesheet(content: &str, lang: Lang) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    let language = stylesheet_language(lang)?;
    parser.set_language(&language).ok()?;
    parser.parse(content, None)
}

fn stylesheet_language(lang: Lang) -> Option<tree_sitter::Language> {
    match lang {
        Lang::Css => Some(tree_sitter_css::LANGUAGE.into()),
        Lang::Scss => Some(tree_sitter_scss::language()),
        Lang::Less => Some(tree_sitter_less::language()),
        _ => None,
    }
}

fn push_section(out: &mut Vec<OutlineEntry>, section: Option<OutlineEntry>) {
    let Some(section) = section else {
        return;
    };
    if !section.children.is_empty() {
        out.push(section);
    }
}

fn section_entry(node: tree_sitter::Node, name: String) -> OutlineEntry {
    OutlineEntry {
        kind: OutlineKind::Section,
        name,
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children: Vec::new(),
        doc: None,
    }
}

fn comment_heading(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    if node.kind() != "comment" {
        return None;
    }
    clean_comment_heading(&node_text(node, lines))
}

fn clean_comment_heading(raw: &str) -> Option<String> {
    let text = raw.trim();
    let text = text.strip_prefix("/*").unwrap_or(text).trim();
    let text = text.strip_suffix("*/").unwrap_or(text).trim();

    let mut content_lines = text.lines().filter_map(clean_comment_heading_line);
    let heading = content_lines.next()?;
    if content_lines.next().is_some() || !is_heading_like_comment(&heading) {
        return None;
    }
    Some(clipped(heading))
}

fn clean_comment_heading_line(line: &str) -> Option<String> {
    let line = line.trim().trim_start_matches('*').trim();
    let line = line
        .trim_matches(|c: char| {
            c.is_whitespace() || matches!(c, '─' | '-' | '—' | '=' | '#' | '•' | '·' | '_' | '*')
        })
        .trim();
    (!line.is_empty()).then(|| line.to_string())
}

fn is_heading_like_comment(line: &str) -> bool {
    let len = line.chars().count();
    if len == 0 || len > 72 || line.ends_with(['.', '!', '?', ';']) || line.contains(':') {
        return false;
    }

    let word_count = line
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .filter(|word| !word.is_empty())
        .count();
    (1..=8).contains(&word_count)
}

fn collect_entries(
    parent: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    depth: usize,
    out: &mut Vec<OutlineEntry>,
) {
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if let Some(entry) = node_to_entry(child, lines, lang, depth) {
            out.push(entry);
        }
    }
}

fn node_to_entry(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    depth: usize,
) -> Option<OutlineEntry> {
    match node.kind() {
        "rule_set" => selector_entry(node, lines, lang, depth),
        "declaration" | "variable" => variable_entry(node, lines),
        "function_statement" if lang == Lang::Scss => Some(function_entry(node, lines)),
        "mixin_statement" if lang == Lang::Scss => Some(mixin_entry(node, lines, lang)),
        "mixin_definition" if lang == Lang::Less => Some(mixin_entry(node, lines, lang)),
        "import_statement" | "use_statement" | "forward_statement" => {
            Some(import_entry(node, lines))
        }
        "keyframes_statement" => Some(keyframes_entry(node, lines)),
        "media_statement"
        | "supports_statement"
        | "scope_statement"
        | "namespace_statement"
        | "charset_statement"
        | "postcss_statement"
        | "at_rule" => Some(at_rule_entry(node, lines, lang, depth)),
        _ => None,
    }
}

fn import_entry(node: tree_sitter::Node, lines: &[&str]) -> OutlineEntry {
    OutlineEntry {
        kind: OutlineKind::Import,
        name: clipped(normalized(node_text(node, lines))),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children: Vec::new(),
        doc: None,
    }
}

fn selector_entry(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    depth: usize,
) -> Option<OutlineEntry> {
    let selectors = first_named_child(node, "selectors")?;
    let name = clipped(normalized(node_text(selectors, lines)));
    if name.is_empty() {
        return None;
    }

    let mut children = Vec::new();
    if depth < 2 {
        if let Some(block) = first_named_child(node, "block") {
            collect_custom_properties(block, lines, &mut children);
            collect_entries(block, lines, lang, depth + 1, &mut children);
        }
    }

    Some(OutlineEntry {
        kind: OutlineKind::Selector,
        name,
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children,
        doc: None,
    })
}

fn variable_entry(node: tree_sitter::Node, lines: &[&str]) -> Option<OutlineEntry> {
    let header = header_text(node, lines);
    let trimmed = header.trim_start();
    if !trimmed.starts_with(['$', '@']) {
        return None;
    }
    let name = trimmed
        .split(|c: char| c == ':' || c.is_whitespace())
        .next()
        .unwrap_or("");
    if name.len() <= 1 {
        return None;
    }
    let name = clipped(name);

    Some(OutlineEntry {
        kind: OutlineKind::Variable,
        name,
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children: Vec::new(),
        doc: None,
    })
}

fn function_entry(node: tree_sitter::Node, lines: &[&str]) -> OutlineEntry {
    let name = node
        .child_by_field_name("name")
        .map(|child| normalized(node_text(child, lines)))
        .or_else(|| {
            first_named_child(node, "function_name")
                .map(|child| normalized(node_text(child, lines)))
        })
        .or_else(|| {
            first_named_child(node, "identifier").map(|child| normalized(node_text(child, lines)))
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| header_text(node, lines));
    let mut children = Vec::new();
    if let Some(block) = first_named_child(node, "block") {
        collect_entries(block, lines, Lang::Scss, 1, &mut children);
    }

    OutlineEntry {
        kind: OutlineKind::Function,
        name: clipped(name),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: Some(clipped(header_text(node, lines))),
        children,
        doc: None,
    }
}

fn mixin_entry(node: tree_sitter::Node, lines: &[&str], lang: Lang) -> OutlineEntry {
    let name = if lang == Lang::Less {
        less_mixin_name(node, lines)
    } else {
        node.child_by_field_name("name")
            .map(|child| normalized(node_text(child, lines)))
            .or_else(|| {
                first_direct_named_child(
                    node,
                    &["class_name", "id_name", "function_name", "identifier"],
                )
                .map(|child| normalized(node_text(child, lines)))
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| header_text(node, lines))
    };
    let mut children = Vec::new();
    if let Some(block) = first_named_child(node, "block") {
        collect_entries(block, lines, lang, 1, &mut children);
    }

    OutlineEntry {
        kind: OutlineKind::Mixin,
        name: clipped(name),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: Some(clipped(header_text(node, lines))),
        children,
        doc: None,
    }
}

fn less_mixin_name(node: tree_sitter::Node, lines: &[&str]) -> String {
    let header = header_text(node, lines);
    header
        .split(['(', '{'])
        .next()
        .unwrap_or(header.as_str())
        .trim()
        .to_string()
}

fn keyframes_entry(node: tree_sitter::Node, lines: &[&str]) -> OutlineEntry {
    let keyword = first_named_child(node, "at_keyword")
        .map(|child| normalized(node_text(child, lines)))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "@keyframes".to_string());
    let name = first_named_child(node, "keyframes_name")
        .map(|child| normalized(node_text(child, lines)))
        .filter(|text| !text.is_empty());
    let display = match name {
        Some(name) => format!("{keyword} {name}"),
        None => header_text(node, lines),
    };

    OutlineEntry {
        kind: OutlineKind::AtRule,
        name: clipped(display),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children: Vec::new(),
        doc: None,
    }
}

fn at_rule_entry(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    depth: usize,
) -> OutlineEntry {
    let mut children = Vec::new();
    if depth < 2 {
        if let Some(block) = first_named_child(node, "block") {
            collect_entries(block, lines, lang, depth + 1, &mut children);
        }
    }

    OutlineEntry {
        kind: OutlineKind::AtRule,
        name: clipped(header_text(node, lines)),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: None,
        children,
        doc: None,
    }
}

fn collect_custom_properties(
    block: tree_sitter::Node,
    lines: &[&str],
    out: &mut Vec<OutlineEntry>,
) {
    let mut cursor = block.walk();
    for child in block.children(&mut cursor) {
        if child.kind() != "declaration" {
            continue;
        }
        let Some(property) = first_named_child(child, "property_name") else {
            continue;
        };
        let name = normalized(node_text(property, lines));
        if !name.starts_with("--") {
            continue;
        }
        out.push(OutlineEntry {
            kind: OutlineKind::Property,
            name,
            start_line: start_line(child),
            end_line: end_line(child),
            signature: None,
            children: Vec::new(),
            doc: None,
        });
    }
}

fn collect_dependency_sources(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    sources: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match node.kind() {
        "import_statement" | "namespace_statement" | "use_statement" | "forward_statement" => {
            for source in import_sources(&node_text(node, lines), lang) {
                push_source(source, sources, seen);
            }
        }
        "declaration" => {
            for source in url_sources(&node_text(node, lines)) {
                push_source(source, sources, seen);
            }
        }
        "call_expression" if is_url_call(node, lines) => {
            if let Some(source) = first_url_source(&node_text(node, lines)) {
                push_source(source, sources, seen);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_dependency_sources(child, lines, lang, sources, seen);
        }
    }
}

fn push_source(source: String, sources: &mut Vec<String>, seen: &mut HashSet<String>) {
    if source.is_empty() || !seen.insert(source.clone()) {
        return;
    }
    sources.push(source);
}

fn is_url_call(node: tree_sitter::Node, lines: &[&str]) -> bool {
    let Some(function) = first_named_child(node, "function_name") else {
        return false;
    };
    node_text(function, lines).eq_ignore_ascii_case("url")
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

fn first_named_child<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    child
}

fn header_text(node: tree_sitter::Node, lines: &[&str]) -> String {
    let end = first_named_child(node, "block")
        .map_or_else(|| node.end_position(), |block| block.start_position());
    clipped(
        normalized(text_between(lines, node.start_position(), end))
            .trim_end_matches('{')
            .trim(),
    )
}

fn node_text(node: tree_sitter::Node, lines: &[&str]) -> String {
    text_between(lines, node.start_position(), node.end_position())
}

fn text_between(lines: &[&str], start: tree_sitter::Point, end: tree_sitter::Point) -> String {
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

fn slice_line(line: &str, start: usize, end: usize) -> &str {
    let start = start.min(line.len());
    let end = end.min(line.len()).max(start);
    &line[start..end]
}

fn slice_line_from(line: &str, start: usize) -> &str {
    let start = start.min(line.len());
    &line[start..]
}

fn slice_line_to(line: &str, end: usize) -> &str {
    let end = end.min(line.len());
    &line[..end]
}

fn normalized(text: impl AsRef<str>) -> String {
    text.as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clipped(text: impl AsRef<str>) -> String {
    let text = text.as_ref().trim();
    if text.len() > MAX_NAME_LEN {
        format!("{}...", crate::types::truncate_str(text, MAX_NAME_LEN - 3))
    } else {
        text.to_string()
    }
}

fn start_line(node: tree_sitter::Node) -> u32 {
    node.start_position().row as u32 + 1
}

fn end_line(node: tree_sitter::Node) -> u32 {
    node.end_position().row as u32 + 1
}

fn quoted_sources(text: &str) -> Vec<String> {
    let mut index = 0;
    let mut out = Vec::new();

    while index < text.len() {
        let Some(c) = text[index..].chars().next() else {
            break;
        };
        if c != '\'' && c != '"' {
            index += c.len_utf8();
            continue;
        }
        if let Some((source, next_index)) = parse_quoted_source(text, index, c) {
            out.push(source);
            index = next_index;
        } else {
            break;
        }
    }

    out
}

fn parse_quoted_source(text: &str, mut index: usize, quote: char) -> Option<(String, usize)> {
    index += quote.len_utf8();
    let mut out = String::new();
    let mut escaped = false;

    while index < text.len() {
        let c = text[index..].chars().next()?;
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
        if c == quote {
            return Some((out.trim().to_string(), index));
        }
        out.push(c);
    }
    None
}

fn first_url_source(text: &str) -> Option<String> {
    url_sources(text).into_iter().next()
}

fn url_sources(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut index = 0;
    let mut out = Vec::new();

    while index < text.len() {
        if text[index..].starts_with("/*") {
            index = skip_css_comment(text, index + 2);
            continue;
        }

        let Some(c) = text[index..].chars().next() else {
            break;
        };
        if c == '\'' || c == '"' {
            index = skip_css_string(text, index, c);
            continue;
        }

        if lower[index..].starts_with("url") && starts_url_function(text, index) {
            let open = skip_css_whitespace(text, index + 3);
            if let Some((source, next_index)) = parse_url_argument(text, open + 1) {
                out.push(source);
                index = next_index;
                continue;
            }
        }
        index += c.len_utf8();
    }

    out
}

fn starts_url_function(text: &str, index: usize) -> bool {
    let before_ok = text[..index]
        .chars()
        .next_back()
        .is_none_or(|c| !is_css_ident_char(c));
    let open = skip_css_whitespace(text, index + 3);
    before_ok && text[open..].starts_with('(')
}

fn skip_css_comment(text: &str, index: usize) -> usize {
    text[index..]
        .find("*/")
        .map_or(text.len(), |offset| index + offset + 2)
}

fn skip_css_string(text: &str, mut index: usize, quote: char) -> usize {
    index += quote.len_utf8();
    let mut escaped = false;

    while index < text.len() {
        let Some(c) = text[index..].chars().next() else {
            break;
        };
        index += c.len_utf8();

        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == quote {
            break;
        }
    }
    index
}

fn skip_css_whitespace(text: &str, mut index: usize) -> usize {
    while index < text.len() {
        let Some(c) = text[index..].chars().next() else {
            break;
        };
        if !c.is_whitespace() {
            break;
        }
        index += c.len_utf8();
    }
    index
}

fn parse_url_argument(text: &str, mut index: usize) -> Option<(String, usize)> {
    index = skip_css_whitespace(text, index);
    let first = text[index..].chars().next()?;
    if first == '\'' || first == '"' {
        parse_quoted_url_argument(text, index, first)
    } else {
        parse_unquoted_url_argument(text, index)
    }
}

fn parse_quoted_url_argument(text: &str, mut index: usize, quote: char) -> Option<(String, usize)> {
    index += quote.len_utf8();
    let mut out = String::new();
    let mut escaped = false;

    while index < text.len() {
        let c = text[index..].chars().next()?;
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
        if c == quote {
            index = skip_css_whitespace(text, index);
            return text[index..]
                .starts_with(')')
                .then(|| (out.trim().to_string(), index + 1));
        }
        out.push(c);
    }
    None
}

fn parse_unquoted_url_argument(text: &str, mut index: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut escaped = false;

    while index < text.len() {
        let c = text[index..].chars().next()?;
        index += c.len_utf8();

        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            ')' => return Some((out.trim().to_string(), index)),
            '(' | '\'' | '"' => return None,
            _ => out.push(c),
        }
    }
    None
}

fn is_windows_drive_path(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn has_url_scheme(source: &str) -> bool {
    let Some(colon) = source.find(':') else {
        return false;
    };
    let scheme = &source[..colon];
    !scheme.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_letter_paths_are_not_external_sources() {
        assert!(!is_external_source(r"C:\project\styles\theme.scss"));
        assert!(!is_external_source("D:/project/styles/theme.less"));
    }

    #[test]
    fn url_schemes_and_sass_builtins_stay_external_sources() {
        assert!(is_external_source("https://cdn.example.com/theme.css"));
        assert!(is_external_source("sass:math"));
        assert!(is_external_source("@fontsource/inter/400.css"));
    }
}

fn selector_matches_query(selector: &str, query: &str) -> bool {
    if selector == query {
        return true;
    }
    let query = query.trim();
    if query.is_empty() {
        return false;
    }

    for component in selector_components(selector) {
        if component == query {
            return true;
        }
        if let Some(stripped) = component.strip_prefix(['.', '#']) {
            if stripped == query {
                return true;
            }
        }
    }
    false
}

fn selector_components(selector: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = selector.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (byte_idx, c) = chars[i];
        if c == '.' || c == '#' {
            let start = byte_idx;
            let mut end = selector.len();
            let mut j = i + 1;
            while j < chars.len() {
                let (next_idx, next) = chars[j];
                if !is_css_ident_char(next) {
                    end = next_idx;
                    break;
                }
                j += 1;
            }
            if end > start + 1 {
                out.push(selector[start..end].to_string());
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

fn variable_matches_query(name: &str, query: &str) -> bool {
    let query = query.trim();
    name == query || name.trim_start_matches(['$', '@']) == query
}

fn at_rule_matches_query(name: &str, query: &str) -> bool {
    if name == query {
        return true;
    }
    let Some(rest) = name.strip_prefix("@keyframes ") else {
        return false;
    };
    rest.split_whitespace().next() == Some(query)
}

fn is_css_ident_char(c: char) -> bool {
    c == '_' || c == '-' || c.is_ascii_alphanumeric() || !c.is_ascii()
}
