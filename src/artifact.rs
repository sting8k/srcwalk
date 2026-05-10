use std::fmt::Write as _;
use std::path::Path;

use crate::error::SrcwalkError;
use crate::lang::outline::outline_language;
use crate::lang::treesitter::is_iife_function;
use crate::types::{estimate_tokens, FileType, Lang, ViewMode};
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

pub(crate) fn read_js_ts_symbol_section(
    path: &Path,
    symbol: &str,
    budget: Option<u64>,
) -> Option<Result<String, SrcwalkError>> {
    if symbol.starts_with('#') || symbol.contains(',') || parse_line_range(symbol).is_some() {
        return None;
    }

    let FileType::Code(lang @ (Lang::JavaScript | Lang::TypeScript | Lang::Tsx)) =
        crate::lang::detect_file_type(path)
    else {
        return None;
    };

    let result = (|| {
        let content = std::fs::read_to_string(path).map_err(|source| SrcwalkError::IoError {
            path: path.to_path_buf(),
            source,
        })?;
        if let Some((start_byte, end_byte)) = parse_byte_range_section(symbol) {
            return Ok(Some(render_artifact_byte_section(
                path, &content, start_byte, end_byte, budget,
            )));
        }

        let Some(span) = find_js_ts_symbol_span(&content, lang, symbol) else {
            return Ok(None);
        };

        Ok(Some(render_artifact_symbol_section(
            path, &content, symbol, span, budget,
        )))
    })();

    match result {
        Ok(Some(output)) => Some(Ok(output)),
        Ok(None) => None,
        Err(err) => Some(Err(err)),
    }
}

#[derive(Clone, Copy)]
struct ArtifactSymbolSpan {
    start_byte: usize,
    end_byte: usize,
    start_line: u32,
    end_line: u32,
    start_col: u32,
    end_col: u32,
}

fn find_js_ts_symbol_span(content: &str, lang: Lang, symbol: &str) -> Option<ArtifactSymbolSpan> {
    let ts_lang = outline_language(lang)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_lang).ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if let Some(line) = parse_iife_symbol(symbol) {
        return find_iife_span_in_node(root, line);
    }
    find_js_ts_symbol_span_in_node(root, content.as_bytes(), symbol)
}

fn find_js_ts_symbol_span_in_node(
    node: tree_sitter::Node,
    content: &[u8],
    symbol: &str,
) -> Option<ArtifactSymbolSpan> {
    if is_named_js_ts_symbol(node, content, symbol) {
        let span_node = js_section_span_node(node);
        return Some(span_from_node(span_node));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(span) = find_js_ts_symbol_span_in_node(child, content, symbol) {
            return Some(span);
        }
    }
    None
}

fn find_iife_span_in_node(node: tree_sitter::Node, line: u32) -> Option<ArtifactSymbolSpan> {
    if is_iife_function(node) && node.start_position().row as u32 + 1 == line {
        return Some(span_from_node(node));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(span) = find_iife_span_in_node(child, line) {
            return Some(span);
        }
    }
    None
}

fn js_section_span_node(node: tree_sitter::Node) -> tree_sitter::Node {
    if node.kind() == "variable_declarator" && node.child_by_field_name("value").is_none() {
        if let Some(parent) = node.parent() {
            if matches!(
                parent.kind(),
                "variable_declaration" | "lexical_declaration"
            ) {
                return parent;
            }
        }
    }
    node
}

fn span_from_node(node: tree_sitter::Node) -> ArtifactSymbolSpan {
    let start = node.start_position();
    let end = node.end_position();
    ArtifactSymbolSpan {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: start.row as u32 + 1,
        end_line: end.row as u32 + 1,
        start_col: start.column as u32 + 1,
        end_col: end.column as u32 + 1,
    }
}

fn is_named_js_ts_symbol(node: tree_sitter::Node, content: &[u8], symbol: &str) -> bool {
    match node.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "method_definition"
        | "variable_declarator" => node
            .child_by_field_name("name")
            .and_then(|name| name.utf8_text(content).ok())
            .is_some_and(|name| name == symbol),
        _ => false,
    }
}

fn render_artifact_byte_section(
    path: &Path,
    content: &str,
    start_byte: usize,
    end_byte: usize,
    budget: Option<u64>,
) -> String {
    let start_byte = floor_char_boundary(content, start_byte.min(content.len()));
    let end_byte = ceil_char_boundary(content, end_byte.min(content.len()));
    let (start_byte, end_byte) = if start_byte <= end_byte {
        (start_byte, end_byte)
    } else {
        (end_byte, start_byte)
    };
    let selected = &content[start_byte..end_byte];
    let byte_len = selected.len() as u64;
    let line_count = selected.lines().count().max(1) as u32;
    let header = crate::format::file_header(path, byte_len, line_count, ViewMode::Section);
    let max_chars = artifact_section_char_limit(budget);
    let (snippet, truncated) = compact_artifact_text(selected, max_chars);
    let fence = artifact_code_fence(path);
    let tok_est = estimate_tokens(byte_len);

    let mut out = String::new();
    let _ = writeln!(out, "{header}\n");
    let _ = writeln!(out, "## artifact bytes: {start_byte}-{end_byte}\n");
    let _ = writeln!(out, "```{fence}");
    out.push_str(snippet.trim());
    let _ = writeln!(out, "\n```");
    if truncated {
        let _ = writeln!(
            out,
            "\n> Caveat: byte section truncated ~{tok_est}/{} tokens; byte span preserved.",
            budget.unwrap_or(1_500)
        );
        out.push_str("> Next: raise --budget <N> or narrow `bytes:start-end`.");
    } else {
        out.push_str("> Next: use callers/callees --artifact for relations, or --full for raw artifact text.");
    }
    out
}

fn render_artifact_symbol_section(
    path: &Path,
    content: &str,
    symbol: &str,
    span: ArtifactSymbolSpan,
    budget: Option<u64>,
) -> String {
    let selected = &content[span.start_byte..span.end_byte];
    let byte_len = selected.len() as u64;
    let line_count = span.end_line.saturating_sub(span.start_line) + 1;
    let header = crate::format::file_header(path, byte_len, line_count, ViewMode::Section);
    let max_chars = artifact_section_char_limit(budget);
    let (snippet, truncated) = compact_artifact_text(selected, max_chars);
    let fence = artifact_code_fence(path);
    let tok_est = estimate_tokens(byte_len);

    let mut out = String::new();
    let _ = writeln!(out, "{header}\n");
    let _ = writeln!(
        out,
        "## artifact section: {symbol} [line {}:{}-{}:{}, bytes {}-{}]\n",
        span.start_line,
        span.start_col,
        span.end_line,
        span.end_col,
        span.start_byte,
        span.end_byte
    );
    let _ = writeln!(out, "```{fence}");
    out.push_str(snippet.trim());
    let _ = writeln!(out, "\n```");
    if truncated {
        let _ = writeln!(
            out,
            "\n> Caveat: artifact section truncated ~{tok_est}/{} tokens; AST byte span preserved.",
            budget.unwrap_or(1_500)
        );
        out.push_str("> Next: use callers/callees --artifact or raise --budget <N>.");
    } else {
        out.push_str("> Next: use callers/callees --artifact for relations, or --full for raw artifact text.");
    }
    out
}

fn artifact_section_char_limit(budget: Option<u64>) -> usize {
    let token_limit = budget.unwrap_or(1_500).clamp(200, 2_000);
    (token_limit as usize * 4).clamp(800, 6_000)
}

fn compact_artifact_text(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let end_byte = text
        .char_indices()
        .nth(max_chars)
        .map_or(text.len(), |(idx, _)| idx);
    (format!("{}…", text[..end_byte].trim_end()), true)
}

fn artifact_code_fence(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("ts" | "tsx") => "ts",
        _ => "js",
    }
}

fn parse_byte_range_section(symbol: &str) -> Option<(usize, usize)> {
    let range = symbol.strip_prefix("bytes:")?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn parse_iife_symbol(symbol: &str) -> Option<u32> {
    symbol
        .strip_prefix("<iife@")?
        .strip_suffix('>')?
        .parse()
        .ok()
}

fn parse_line_range(s: &str) -> Option<()> {
    let mut parts = s.split('-');
    let start = parts.next()?;
    if start.parse::<usize>().is_err() {
        return None;
    }
    match parts.next() {
        None => Some(()),
        Some(end) if parts.next().is_none() && end.parse::<usize>().is_ok() => Some(()),
        _ => None,
    }
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
