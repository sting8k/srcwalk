use std::path::Path;

use tree_sitter::Node;

use super::types::{FlowAnnotation, FlowNode};
use super::{clean_label, compact_node_text, condition_node, find_first_call, line_start};
use crate::evidence::EvidenceRole;
use crate::types::Lang;

const PARAMETER_SEARCH_MAX_DEPTH: usize = 24;
const IDENTIFIER_DESCENT_MAX_DEPTH: usize = 32;

pub(super) fn add_parameter_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    function: Node<'_>,
    source: &str,
) {
    let Some(parameters) = parameters_node(function) else {
        return;
    };

    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if is_punctuation_or_type_only(parameter.kind()) {
            continue;
        }
        let Some(name) = parameter_name_node(parameter) else {
            continue;
        };
        let text = compact_node_text(name, source);
        if text.is_empty() {
            continue;
        }
        add_annotation(
            flow_node,
            FlowAnnotation::definition(path, text, EvidenceRole::Parameter, line_start(name)),
        );
    }
}

pub(super) fn add_condition_read_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    syntax_node: Node<'_>,
    source: &str,
) {
    if let Some(condition) = condition_node(syntax_node) {
        add_read_annotations(flow_node, path, condition, source, EvidenceRole::Condition);
    }
}

pub(super) fn add_call_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    call: Node<'_>,
    source: &str,
) {
    let text = call_annotation_text(call, source);
    if !text.is_empty() {
        add_annotation(
            flow_node,
            FlowAnnotation::call(path, text, line_start(call)),
        );
    }
    if let Some(arguments) = call_arguments(call) {
        add_read_annotations(flow_node, path, arguments, source, EvidenceRole::CallArg);
    }
}

pub(super) fn add_assignment_write_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    statement: Node<'_>,
    source: &str,
) {
    add_assignment_annotations_with_rhs(flow_node, path, statement, source, false);
}

pub(super) fn add_assignment_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    statement: Node<'_>,
    source: &str,
) {
    add_assignment_annotations_with_rhs(flow_node, path, statement, source, true);
}

fn add_assignment_annotations_with_rhs(
    flow_node: &mut FlowNode,
    path: &Path,
    statement: Node<'_>,
    source: &str,
    include_rhs_reads: bool,
) {
    let Some(parts) = assignment_parts(statement) else {
        return;
    };

    let lhs = compact_node_text(parts.lhs, source);
    if !lhs.is_empty() {
        add_annotation(
            flow_node,
            FlowAnnotation::write(
                path,
                lhs,
                EvidenceRole::AssignmentLhs,
                line_start(parts.lhs),
            ),
        );
    }
    if include_rhs_reads {
        if let Some(rhs) = parts.rhs {
            add_read_annotations(flow_node, path, rhs, source, EvidenceRole::AssignmentRhs);
        }
    }
}

pub(super) fn has_assignment(statement: Node<'_>) -> bool {
    assignment_parts(statement).is_some()
}

pub(super) fn add_return_or_throw_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    statement: Node<'_>,
    source: &str,
    lang: Lang,
) {
    if let Some(call) = find_first_call(statement, lang) {
        add_call_annotations(flow_node, path, call, source);
    } else {
        add_read_annotations(flow_node, path, statement, source, EvidenceRole::Return);
    }
}

fn add_read_annotations(
    flow_node: &mut FlowNode,
    path: &Path,
    syntax_node: Node<'_>,
    source: &str,
    role: EvidenceRole,
) {
    let mut annotations = Vec::new();
    collect_read_annotations(path, syntax_node, source, role, &mut annotations);
    for annotation in annotations {
        add_annotation(flow_node, annotation);
    }
}

fn add_annotation(flow_node: &mut FlowNode, annotation: FlowAnnotation) {
    if flow_node.annotations.iter().any(|existing| {
        existing.kind() == annotation.kind()
            && existing.text() == annotation.text()
            && existing.role() == annotation.role()
            && existing.line() == annotation.line()
    }) {
        return;
    }
    flow_node.annotations.push(annotation);
}

fn parameters_node(function: Node<'_>) -> Option<Node<'_>> {
    if let Some(parameters) = function.child_by_field_name("parameters") {
        return Some(parameters);
    }

    find_parameters_node(function, 0)
}

fn find_parameters_node(node: Node<'_>, depth: usize) -> Option<Node<'_>> {
    if depth > PARAMETER_SEARCH_MAX_DEPTH {
        return None;
    }
    if matches!(
        node.kind(),
        "parameters" | "formal_parameters" | "parameter_list"
    ) {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_block_like_for_parameter_search(child.kind()) {
            continue;
        }
        if let Some(parameters) = find_parameters_node(child, depth + 1) {
            return Some(parameters);
        }
    }
    None
}

fn is_block_like_for_parameter_search(kind: &str) -> bool {
    matches!(
        kind,
        "body" | "block" | "compound_statement" | "statement_block" | "function_body" | "suite"
    )
}

fn parameter_name_node(parameter: Node<'_>) -> Option<Node<'_>> {
    parameter
        .child_by_field_name("pattern")
        .and_then(identifier_like_descendant)
        .or_else(|| {
            parameter
                .child_by_field_name("name")
                .and_then(identifier_like_descendant)
        })
        .or_else(|| {
            parameter
                .child_by_field_name("declarator")
                .and_then(identifier_like_descendant)
        })
        .or_else(|| first_identifier_like_child(parameter))
}

fn identifier_like_descendant(node: Node<'_>) -> Option<Node<'_>> {
    identifier_like_descendant_inner(node, 0)
}

fn identifier_like_descendant_inner(node: Node<'_>, depth: usize) -> Option<Node<'_>> {
    if depth > IDENTIFIER_DESCENT_MAX_DEPTH {
        return None;
    }
    if is_identifier_like(node.kind()) || matches!(node.kind(), "self_parameter") {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(identifier) = identifier_like_descendant_inner(child, depth + 1) {
            return Some(identifier);
        }
    }
    None
}

fn first_identifier_like_child(node: Node<'_>) -> Option<Node<'_>> {
    if is_identifier_like(node.kind()) || matches!(node.kind(), "self_parameter") {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_identifier_like(child.kind()) || matches!(child.kind(), "self_parameter") {
            return Some(child);
        }
    }
    None
}

fn is_punctuation_or_type_only(kind: &str) -> bool {
    matches!(kind, "," | ":" | "type_identifier" | "primitive_type")
}

fn call_arguments(call: Node<'_>) -> Option<Node<'_>> {
    if let Some(arguments) = call.child_by_field_name("arguments") {
        return Some(arguments);
    }

    let mut cursor = call.walk();
    for child in call.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "arguments" | "argument_list" | "value_arguments" | "call_arguments"
        ) {
            return Some(child);
        }
    }
    None
}

fn call_annotation_text(call: Node<'_>, source: &str) -> String {
    let text = compact_node_text(call, source);
    let before_args = text
        .split_once('(')
        .map_or(text.as_str(), |(prefix, _)| prefix);
    clean_label(before_args.trim().trim_start_matches("await "))
}

struct AssignmentParts<'tree> {
    lhs: Node<'tree>,
    rhs: Option<Node<'tree>>,
}

fn assignment_parts(node: Node<'_>) -> Option<AssignmentParts<'_>> {
    if is_assignment_node(node.kind()) {
        let lhs = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("pattern"))
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| node.child_by_field_name("declarator"))
            .or_else(|| first_identifier_like_child(node))?;
        let rhs = node
            .child_by_field_name("right")
            .or_else(|| node.child_by_field_name("value"));
        return Some(AssignmentParts { lhs, rhs });
    }

    if is_declaration_wrapper(node.kind()) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if let Some(parts) = assignment_parts(child) {
                return Some(parts);
            }
        }
    }

    None
}

fn is_assignment_node(kind: &str) -> bool {
    matches!(
        kind,
        "assignment"
            | "let_declaration"
            | "assignment_expression"
            | "assignment_statement"
            | "augmented_assignment"
            | "short_var_declaration"
            | "var_spec"
            | "variable_declarator"
            | "init_declarator"
            | "variable_declaration"
            | "local_variable_declaration"
    )
}

fn is_declaration_wrapper(kind: &str) -> bool {
    matches!(
        kind,
        "lexical_declaration" | "const_declaration" | "var_declaration" | "declaration"
    )
}

fn collect_read_annotations(
    path: &Path,
    node: Node<'_>,
    source: &str,
    role: EvidenceRole,
    annotations: &mut Vec<FlowAnnotation>,
) {
    if is_access_expression(node.kind()) {
        let text = compact_node_text(node, source);
        if !text.is_empty() {
            annotations.push(FlowAnnotation::read(path, text, role, line_start(node)));
        }
        return;
    }

    if is_identifier_like(node.kind()) {
        let text = compact_node_text(node, source);
        if !text.is_empty() {
            annotations.push(FlowAnnotation::read(path, text, role, line_start(node)));
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_read_annotations(path, child, source, role, annotations);
    }
}

fn is_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "property_identifier"
            | "field_identifier"
            | "constant"
            | "simple_identifier"
    )
}

fn is_access_expression(kind: &str) -> bool {
    matches!(
        kind,
        "field_expression"
            | "selector_expression"
            | "member_expression"
            | "member_access_expression"
            | "field_access"
            | "attribute"
            | "navigation_expression"
            | "navigation_suffix"
            | "dot"
            | "instance_variable"
    )
}
