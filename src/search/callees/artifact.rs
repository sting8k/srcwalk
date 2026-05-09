use std::path::Path;

use streaming_iterator::StreamingIterator;

use crate::lang::outline::outline_language;
use crate::types::Lang;

use super::{
    callee_query_str, extract_call_sites, extract_call_sites_in_byte_range, extract_callee_names,
    with_callee_query, CallSite, ResolvedCallee,
};

pub fn resolve_same_file(
    target: &str,
    source_path: &Path,
    source_content: &str,
    lang: Lang,
    callee_names: &[String],
) -> Option<Vec<ResolvedCallee>> {
    if !matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) {
        return None;
    }
    let defs = collect_nested_js_function_defs(source_content, lang)?;
    if !defs.iter().any(|def| def.name == target) {
        return None;
    }
    let mut remaining: std::collections::HashSet<&str> =
        callee_names.iter().map(String::as_str).collect();
    let mut resolved = Vec::new();
    for def in defs {
        if remaining.remove(def.name.as_str()) {
            resolved.push(ResolvedCallee {
                name: def.name,
                file: source_path.to_path_buf(),
                start_line: def.start_line,
                end_line: def.end_line,
                signature: def.signature,
            });
        }
        if remaining.is_empty() {
            break;
        }
    }
    Some(resolved)
}

pub fn extract_call_sites_for_target(
    content: &str,
    lang: Lang,
    target: &str,
    fallback_range: Option<(u32, u32)>,
) -> Vec<CallSite> {
    let Some((start_byte, end_byte)) = find_nested_js_function_byte_range(content, lang, target)
    else {
        return extract_call_sites(content, lang, fallback_range);
    };
    extract_call_sites_in_byte_range(content, lang, start_byte, end_byte)
}

pub fn extract_callee_names_for_target(
    content: &str,
    lang: Lang,
    target: &str,
    fallback_range: Option<(u32, u32)>,
) -> Vec<String> {
    let Some((start_byte, end_byte)) = find_nested_js_function_byte_range(content, lang, target)
    else {
        return extract_callee_names(content, lang, fallback_range);
    };
    extract_callee_names_in_byte_range(content, lang, start_byte, end_byte)
}

struct NestedJsDef {
    name: String,
    start_line: u32,
    end_line: u32,
    signature: Option<String>,
    start_byte: usize,
    end_byte: usize,
}

fn collect_nested_js_function_defs(content: &str, lang: Lang) -> Option<Vec<NestedJsDef>> {
    let tree = parse_lang_tree(content, lang)?;
    let mut defs = Vec::new();
    collect_nested_js_function_defs_from_node(tree.root_node(), content.as_bytes(), &mut defs);
    Some(defs)
}

fn find_nested_js_function_byte_range(
    content: &str,
    lang: Lang,
    target: &str,
) -> Option<(usize, usize)> {
    collect_nested_js_function_defs(content, lang)?
        .into_iter()
        .find(|def| def.name == target)
        .map(|def| (def.start_byte, def.end_byte))
}

fn collect_nested_js_function_defs_from_node(
    node: tree_sitter::Node,
    content: &[u8],
    defs: &mut Vec<NestedJsDef>,
) {
    if matches!(node.kind(), "function_declaration" | "method_definition") {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                if !name.is_empty() {
                    defs.push(NestedJsDef {
                        name: name.to_string(),
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        signature: artifact_signature(node, content),
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                    });
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nested_js_function_defs_from_node(child, content, defs);
    }
}

fn extract_callee_names_in_byte_range(
    content: &str,
    lang: Lang,
    start_byte: usize,
    end_byte: usize,
) -> Vec<String> {
    let Some(tree) = parse_lang_tree(content, lang) else {
        return Vec::new();
    };
    let Some(ts_lang) = outline_language(lang) else {
        return Vec::new();
    };
    let Some(query_str) = callee_query_str(lang) else {
        return Vec::new();
    };
    let Some(names) = with_callee_query(&ts_lang, query_str, |query| {
        let Some(callee_idx) = query.capture_index_for_name("callee") else {
            return Vec::new();
        };
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content.as_bytes());
        let mut names = Vec::new();
        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != callee_idx {
                    continue;
                }
                if cap.node.start_byte() < start_byte || cap.node.start_byte() >= end_byte {
                    continue;
                }
                if let Ok(text) = cap.node.utf8_text(content.as_bytes()) {
                    names.push(text.to_string());
                }
            }
        }
        names
    }) else {
        return Vec::new();
    };
    let mut names = names;
    names.sort();
    names.dedup();
    names
}

fn parse_lang_tree(content: &str, lang: Lang) -> Option<tree_sitter::Tree> {
    let ts_lang = outline_language(lang)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_lang).ok()?;
    parser.parse(content, None)
}

fn artifact_signature(node: tree_sitter::Node, content: &[u8]) -> Option<String> {
    let text = node.utf8_text(content).ok()?;
    let head = text.split('{').next().unwrap_or(text).trim();
    if head.is_empty() {
        None
    } else {
        Some(head.chars().take(120).collect())
    }
}
