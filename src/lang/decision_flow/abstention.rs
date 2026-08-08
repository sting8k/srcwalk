//! Language-aware hard-abstention checks for built-in Flow Maps.
//!
//! The scanner reports only direct constructs the structural IR cannot represent.
use super::line_start;
use crate::types::Lang;
use tree_sitter::Node;

const ABSTENTION_MARKERS: [&str; 5] = [
    "Ruby Flow Map abstained:",
    "JavaScript Flow Map abstained:",
    "TypeScript Flow Map abstained:",
    "TSX Flow Map abstained:",
    "Python Flow Map abstained:",
];

pub(crate) fn is_abstention_reason(reason: &str) -> bool {
    ABSTENTION_MARKERS
        .iter()
        .any(|marker| reason.starts_with(marker))
}

fn marker(lang: Lang) -> Option<&'static str> {
    match lang {
        Lang::Ruby => Some("Ruby Flow Map abstained:"),
        Lang::JavaScript => Some("JavaScript Flow Map abstained:"),
        Lang::TypeScript => Some("TypeScript Flow Map abstained:"),
        Lang::Tsx => Some("TSX Flow Map abstained:"),
        Lang::Python => Some("Python Flow Map abstained:"),
        _ => None,
    }
}

pub(super) fn unsupported_direct_construct_reason(
    function: Node<'_>,
    lang: Lang,
) -> Option<String> {
    let marker = marker(lang)?;
    if lang == Lang::Ruby {
        return None;
    }
    let mut cursor = function.walk();
    for child in function.named_children(&mut cursor) {
        if let Some(hit) = scan_for_unsupported(child, lang) {
            return Some(format!(
                "{marker} unsupported {} at line {} requires {}",
                hit.kind, hit.line, hit.why
            ));
        }
    }
    None
}

struct UnsupportedHit {
    kind: &'static str,
    line: u32,
    why: &'static str,
}

fn scan_for_unsupported(node: Node<'_>, lang: Lang) -> Option<UnsupportedHit> {
    if is_nested_scope_kind(lang, node.kind()) {
        return None;
    }
    if is_switch_case_terminator_break(node, lang) {
        return None;
    }
    if let Some(why) = unsupported_kind_reason(lang, node.kind()) {
        return Some(UnsupportedHit {
            kind: node.kind(),
            line: line_start(node),
            why,
        });
    }
    let mut cursor = node.walk();
    let nested = node
        .named_children(&mut cursor)
        .find_map(|child| scan_for_unsupported(child, lang));
    nested
}

fn is_switch_case_terminator_break(node: Node<'_>, lang: Lang) -> bool {
    if !matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx)
        || node.kind() != "break_statement"
        || node.child_by_field_name("label").is_some()
    {
        return false;
    }
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if matches!(parent.kind(), "switch_case" | "switch_default") {
            return true;
        }
        if parent.kind() != "statement_block" {
            return false;
        }
        ancestor = parent.parent();
    }
    false
}

fn is_nested_scope_kind(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => matches!(
            kind,
            "arrow_function"
                | "function_declaration"
                | "function_expression"
                | "generator_function"
                | "generator_function_declaration"
                | "method_definition"
                | "class_declaration"
                | "class"
                | "abstract_class_declaration"
                | "class_static_block"
                | "object"
        ),
        Lang::Python => matches!(kind, "function_definition" | "class_definition" | "lambda"),
        Lang::Ruby => matches!(
            kind,
            "method"
                | "singleton_method"
                | "class"
                | "module"
                | "singleton_class"
                | "block"
                | "do_block"
        ),
        _ => false,
    }
}

fn unsupported_kind_reason(lang: Lang, kind: &str) -> Option<&'static str> {
    match lang {
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Python => match kind {
            "try_statement" => Some("exception-handling/finally propagation edges"),
            "break_statement" => Some("abrupt loop-exit edges"),
            "continue_statement" => Some("abrupt loop-continue edges"),
            _ => None,
        },
        _ => None,
    }
}
