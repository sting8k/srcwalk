use tree_sitter::{Language, Node};

use crate::lang::outline::outline_language;
use crate::types::Lang;

#[derive(Clone, Copy)]
pub(crate) struct FlowLanguage {
    pub(crate) lang: Lang,
}

pub(crate) fn active_flow_language(lang: Lang) -> Option<FlowLanguage> {
    is_builtin_flow_language(lang).then_some(FlowLanguage { lang })
}

pub(crate) fn flow_language(lang: Lang) -> Option<Language> {
    active_flow_language(lang).and_then(|_| outline_language(lang))
}

pub(crate) fn supports_flow_lang(lang: Lang) -> bool {
    active_flow_language(lang).is_some()
}

fn is_builtin_flow_language(lang: Lang) -> bool {
    matches!(
        lang,
        Lang::Rust
            | Lang::JavaScript
            | Lang::TypeScript
            | Lang::Tsx
            | Lang::Python
            | Lang::Go
            | Lang::Java
            | Lang::C
            | Lang::Cpp
            | Lang::CSharp
            | Lang::Ruby
    )
}

pub(crate) fn is_function_like(language: &FlowLanguage, kind: &str) -> bool {
    match language.lang {
        Lang::Rust => kind == "function_item",
        Lang::Go => matches!(kind, "function_declaration" | "method_declaration"),
        Lang::Java => matches!(kind, "method_declaration" | "constructor_declaration"),
        Lang::C | Lang::Cpp | Lang::Python => kind == "function_definition",
        Lang::CSharp => matches!(kind, "method_declaration" | "constructor_declaration"),
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => matches!(
            kind,
            "function_declaration"
                | "function_expression"
                | "generator_function"
                | "arrow_function"
                | "method_definition"
        ),
        Lang::Ruby => matches!(kind, "method" | "singleton_method"),
        _ => false,
    }
}

pub(crate) fn function_display_name(
    language: &FlowLanguage,
    node: Node<'_>,
    source: &str,
    lines: &[&str],
) -> Option<String> {
    use crate::lang::treesitter::{extract_definition_name, js_function_context_name};

    if matches!(
        language.lang,
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx
    ) {
        return js_function_context_name(node, lines)
            .or_else(|| extract_definition_name(node, lines));
    }
    extract_definition_name(node, lines).or_else(|| {
        node.child_by_field_name("name")
            .map(|name| compact_node_text(name, source))
    })
}

pub(crate) fn is_if_node(language: &FlowLanguage, kind: &str) -> bool {
    matches!(kind, "if_expression" | "if_statement" | "elif_clause")
        || (language.lang == Lang::Ruby
            && matches!(
                kind,
                "if" | "elsif" | "unless" | "if_modifier" | "unless_modifier"
            ))
}

pub(crate) fn is_loop_node(language: &FlowLanguage, kind: &str) -> bool {
    matches!(
        kind,
        "loop_expression"
            | "while_expression"
            | "for_expression"
            | "while_statement"
            | "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "do_statement"
            | "enhanced_for_statement"
            | "foreach_statement"
            | "for_each_statement"
    ) || (language.lang == Lang::Ruby
        && matches!(
            kind,
            "while" | "until" | "for" | "while_modifier" | "until_modifier"
        ))
}

pub(crate) fn is_return_node(language: &FlowLanguage, kind: &str) -> bool {
    matches!(kind, "return_expression" | "return_statement")
        || (language.lang == Lang::Ruby && kind == "return")
}

pub(crate) fn is_throw_node(_language: &FlowLanguage, kind: &str) -> bool {
    matches!(kind, "throw_statement" | "raise_statement")
}

pub(crate) fn is_call_node(language: &FlowLanguage, kind: &str) -> bool {
    matches!(kind, "call_expression")
        || matches!(language.lang, Lang::Python | Lang::Ruby) && kind == "call"
        || matches!(kind, "method_invocation" | "invocation_expression")
        || (language.lang == Lang::Rust && kind == "macro_invocation")
        || matches!(kind, "await_expression")
}

pub(crate) fn is_transparent_statement(language: &FlowLanguage, kind: &str) -> bool {
    matches!(
        kind,
        "expression_statement" | "parenthesized_expression" | "else_clause"
    ) || is_block_like(language, kind)
}

pub(crate) fn is_block_like(language: &FlowLanguage, kind: &str) -> bool {
    matches!(
        kind,
        "block"
            | "statement_block"
            | "compound_statement"
            | "declaration_list"
            | "switch_block"
            | "switch_body"
    ) || (language.lang == Lang::Ruby
        && matches!(kind, "body_statement" | "then" | "else" | "begin" | "do"))
}

pub(crate) fn is_match_or_switch_node(language: &FlowLanguage, kind: &str) -> bool {
    matches!(
        kind,
        "match_expression"
            | "match_statement"
            | "switch_statement"
            | "switch_expression"
            | "expression_switch_statement"
            | "type_switch_statement"
    ) || (language.lang == Lang::Ruby && kind == "case")
}

pub(crate) fn function_body<'tree>(
    language: &FlowLanguage,
    node: Node<'tree>,
) -> Option<Node<'tree>> {
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| is_block_like(language, child.kind()));
    found
}

fn compact_node_text(node: Node<'_>, source: &str) -> String {
    let range = node.byte_range();
    let text = source.get(range).unwrap_or_default();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
