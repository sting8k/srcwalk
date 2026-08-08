//! Ruby-specific decision-flow / Flow Map helpers.
//!
//! Ruby is a conservative structural tier: it models only what
//! tree-sitter-ruby syntax proves, inside `method`/`singleton_method` bodies.
//! Supported: if/elsif/else, unless (with honest inverted body/after
//! orientation), case/when/else (patterns joined from source order), native
//! while/until/for (incl. modifiers), direct calls (incl. endless method
//! bodies and call-header-only blocks), assignments/operator assignments,
//! explicit returns, and receiverless `raise`/`fail` throws.
//!
//! Hard abstentions: `rescue`/`ensure`/`rescue_modifier` (exception edges),
//! `break`/`next`/`redo`/`retry` (abrupt control). Call `block`/`do_block`
//! subtrees are opaque: the call header is shown, internals are never
//! traversed and never counted as unsupported. `obj.raise`/`Kernel.raise`
//! are ordinary calls, not throws.

use tree_sitter::Node;

use super::evidence;
use super::flow_lang::FlowLanguage;
use super::types::{Branch, FlowNodeKind, IncomingEdge};
use super::{
    branch_body_nodes, clean_label, compact_node_text, condition_node, if_alternative_body,
    if_consequence_body, line_end, line_start, FlowBuilder,
};

/// Stable marker for Ruby Flow Map abstentions. `decision-flow` fails loudly
/// with this exact reason; `context` recognizes only this marker as a Flow Map
/// fallback error (never arbitrary `InvalidQuery` errors).
pub(crate) const ABSTENTION_MARKER: &str = "Ruby Flow Map abstained:";

/// Hard-abstention precheck: scan the selected Ruby function subtree for
/// direct-flow constructs the IR cannot represent. Skips nested
/// `method`/`singleton_method`/`class`/`module` definitions and opaque
/// `block`/`do_block` subtrees. Returns a stable reason on the first unsupported
/// construct in source order.
pub(super) fn unsupported_direct_construct_reason(function: Node<'_>) -> Option<String> {
    let mut cursor = function.walk();
    for child in function.named_children(&mut cursor) {
        if let Some(hit) = scan_for_unsupported(child) {
            return Some(format!(
                "{} unsupported {} at line {} requires {}",
                ABSTENTION_MARKER, hit.kind, hit.line, hit.why
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

fn scan_for_unsupported(node: Node<'_>) -> Option<UnsupportedHit> {
    match node.kind() {
        // Nested definitions and opaque call blocks are not direct-flow
        // concerns of the selected method.
        "method" | "singleton_method" | "class" | "module" | "singleton_class" | "block"
        | "do_block" => return None,
        _ => {}
    }
    if let Some(why) = unsupported_kind_reason(node.kind()) {
        return Some(UnsupportedHit {
            kind: node.kind(),
            line: line_start(node),
            why,
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(hit) = scan_for_unsupported(child) {
            return Some(hit);
        }
    }
    None
}

fn unsupported_kind_reason(kind: &str) -> Option<&'static str> {
    match kind {
        "rescue" | "rescue_modifier" => Some("exception-handling edges"),
        "ensure" => Some("exception-ensure propagation"),
        "break" => Some("abrupt loop exit edges"),
        "next" => Some("abrupt-control edges"),
        "redo" => Some("abrupt loop restart edges"),
        "retry" => Some("exception-retry edges"),
        _ => None,
    }
}

/// Ruby condition/loop/match node-kind recognition delegated from the generic
/// control helpers so Ruby-specific kinds stay with the Ruby language.
pub(super) fn is_if_node(kind: &str) -> bool {
    matches!(
        kind,
        "if" | "elsif" | "unless" | "if_modifier" | "unless_modifier"
    )
}

pub(super) fn is_case_node(kind: &str) -> bool {
    kind == "case"
}

/// Nested definition kinds are separate scopes: their bodies must not leak
/// into the enclosing method's flow map (matches the scan, which skips them).
pub(super) fn is_nested_definition_kind(kind: &str) -> bool {
    matches!(
        kind,
        "method" | "singleton_method" | "class" | "module" | "singleton_class"
    )
}

pub(super) fn is_loop_node(kind: &str) -> bool {
    matches!(
        kind,
        "while" | "until" | "for" | "while_modifier" | "until_modifier"
    )
}

/// True for a direct receiverless `raise`/`fail` call (a supported Throw
/// exit). `obj.raise`/`Kernel.raise` keep a receiver and stay ordinary calls.
pub(super) fn is_receiverless_raise_or_fail(statement: Node<'_>, source: &str) -> bool {
    if statement.kind() != "call" {
        return false;
    }
    if statement.child_by_field_name("receiver").is_some() {
        return false;
    }
    let Some(method) = statement.child_by_field_name("method") else {
        return false;
    };
    matches!(
        method.utf8_text(source.as_bytes()).unwrap_or_default(),
        "raise" | "fail"
    )
}

/// Decision label for Ruby if/elsif/unless (incl. modifiers). `unless` is
/// always labeled `unless <condition>` so inverted orientation is explicit.
pub(super) fn if_label(node: Node<'_>, source: &str) -> String {
    let is_unless = matches!(node.kind(), "unless" | "unless_modifier");
    let keyword = if is_unless { "unless" } else { "if" };
    match condition_node(node) {
        Some(condition) => format!("{keyword} {}", compact_node_text(condition, source)),
        None => keyword.to_string(),
    }
}

/// Ruby if/elsif/unless (incl. modifiers). `unless` body edge reflects
/// condition FALSE ("no"); alternative/after reflects TRUE ("yes"). Plain
/// `if`/`elsif`/`if_modifier` keep body on TRUE ("yes") / after on FALSE
/// ("no"). Labels are honest (`unless <condition>`, `if <condition>`).
pub(super) fn append_ruby_if(
    builder: &mut FlowBuilder<'_>,
    node: Node<'_>,
    incoming: Vec<IncomingEdge>,
) -> Vec<IncomingEdge> {
    let is_unless = matches!(node.kind(), "unless" | "unless_modifier");
    let label = if_label(node, builder.source);
    let id = builder.add_node(
        FlowNodeKind::Decision,
        &label,
        line_start(node),
        line_end(node),
    );
    builder.connect_all(incoming, id);
    evidence::add_condition_read_annotations(
        &mut builder.graph.nodes[id],
        &builder.graph.path,
        node,
        builder.source,
    );

    let old_focus = if builder.focus_intersects_condition(node) {
        builder.focus.take()
    } else {
        None
    };
    let mut tails = Vec::new();

    let consequence = if_consequence_body(node, &builder.language);
    let (body_edge, alt_edge) = if is_unless {
        ("no", "yes")
    } else {
        ("yes", "no")
    };
    tails.extend(builder.append_branch(id, body_edge, &consequence));

    if let Some(alternative) = if_alternative_body(node, &builder.language) {
        tails.extend(builder.append_branch(id, alt_edge, &alternative));
    } else {
        tails.push(IncomingEdge {
            from: id,
            label: Some(alt_edge.to_string()),
        });
    }
    if old_focus.is_some() {
        builder.focus = old_focus;
    }
    tails
}

/// Ruby `case` label: `case <value>` or `case` for value-less case.
pub(super) fn case_label(node: Node<'_>, source: &str) -> String {
    match node.child_by_field_name("value") {
        Some(value) => format!("case {}", compact_node_text(value, source)),
        None => "case".to_string(),
    }
}

/// Ruby `case`/`when`/`else` branches, plus whether an explicit `else` was
/// present. Each `when` joins its direct pattern children in source order
/// (excluding the `then` body); `else` is default.
fn case_branches<'tree>(
    node: Node<'tree>,
    source: &str,
    language: &FlowLanguage,
) -> (Vec<Branch<'tree>>, bool) {
    let mut branches = Vec::new();
    let mut has_else = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "when" => {
                let patterns: Vec<String> = child
                    .named_children(&mut child.walk())
                    .filter(|c| c.kind() != "then" && c.kind() != "comment")
                    .map(|p| compact_node_text(p, source))
                    .collect();
                let label = patterns.join(", ");
                let body = child
                    .child_by_field_name("body")
                    .map_or_else(Vec::new, |b| branch_body_nodes(b, language));
                branches.push(Branch { label, body });
            }
            "else" => {
                has_else = true;
                branches.push(Branch {
                    label: "default".to_string(),
                    body: branch_body_nodes(child, language),
                });
            }
            _ => {}
        }
    }
    (branches, has_else)
}

/// Ruby `case` dispatch (value-less `case` included).
pub(super) fn append_ruby_case(
    builder: &mut FlowBuilder<'_>,
    node: Node<'_>,
    incoming: Vec<IncomingEdge>,
) -> Vec<IncomingEdge> {
    let label = case_label(node, builder.source);
    let id = builder.add_node(
        FlowNodeKind::Decision,
        &label,
        line_start(node),
        line_end(node),
    );
    builder.connect_all(incoming, id);
    evidence::add_condition_read_annotations(
        &mut builder.graph.nodes[id],
        &builder.graph.path,
        node,
        builder.source,
    );

    let (branches, has_else) = case_branches(node, builder.source, &builder.language);
    if branches.is_empty() {
        return vec![IncomingEdge {
            from: id,
            label: None,
        }];
    }
    let old_focus = if builder.focus_intersects_condition(node) {
        builder.focus.take()
    } else {
        None
    };
    let mut tails = Vec::new();
    for branch in branches {
        tails.extend(builder.append_branch(id, &branch.label, &branch.body));
    }
    // No `else`: an unmatched value falls through with no exception, so a
    // runtime path reaches whatever follows the `case` even if every
    // explicit `when` branch terminates (returns/raises/exhausts its tails).
    // Mirrors `append_ruby_if`'s synthetic edge for a missing alternative.
    if !has_else {
        tails.push(IncomingEdge {
            from: id,
            label: Some("no match".to_string()),
        });
    }
    if old_focus.is_some() {
        builder.focus = old_focus;
    }
    tails
}

/// Honest loop label: `while <condition>`, `until <condition>`, or
/// `for <pattern> in <value>`.
pub(super) fn loop_label(node: Node<'_>, source: &str) -> Option<String> {
    let condition = condition_node(node).map(|c| compact_node_text(c, source));
    match node.kind() {
        "while" | "while_modifier" => condition.map(|c| format!("while {c}")),
        "until" | "until_modifier" => condition.map(|c| format!("until {c}")),
        "for" => {
            let pattern = node
                .child_by_field_name("pattern")
                .map(|p| compact_node_text(p, source))
                .unwrap_or_default();
            let value = node
                .child_by_field_name("value")
                .and_then(|in_node| {
                    let mut cursor = in_node.walk();
                    let mut children = in_node.named_children(&mut cursor);
                    children.next()
                })
                .map(|v| compact_node_text(v, source))
                .unwrap_or_default();
            Some(format!("for {pattern} in {value}"))
        }
        _ => None,
    }
}

/// Call label limited to receiver/method/arguments before any attached block:
/// `block`/`do_block` internals are opaque and never collapsed into the label.
pub(super) fn call_label(call: Node<'_>, source: &str) -> String {
    if let Some(block) = call.child_by_field_name("block") {
        if let Some(header) = source.get(call.start_byte()..block.start_byte()) {
            return clean_label(header);
        }
    }
    compact_node_text(call, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Lang;
    use tree_sitter::Parser;

    fn parse(source: &str) -> (tree_sitter::Tree, String) {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("ruby grammar");
        let tree = parser.parse(source, None).expect("parse");
        (tree, source.to_string())
    }

    fn method_body(root: Node<'_>) -> Node<'_> {
        let mut cursor = root.walk();
        let mut method = None;
        for child in root.named_children(&mut cursor) {
            if matches!(child.kind(), "method" | "singleton_method") {
                method = Some(child);
                break;
            }
        }
        method.expect("method node")
    }

    /// DFS: first named descendant (or self) whose kind matches.
    fn find_first<'a>(root: Node<'a>, kind: &str) -> Option<Node<'a>> {
        if root.kind() == kind {
            return Some(root);
        }
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if let Some(found) = find_first(child, kind) {
                return Some(found);
            }
        }
        None
    }

    /// Collect every descendant call node in source order.
    fn collect_calls<'a>(root: Node<'a>, out: &mut Vec<Node<'a>>) {
        if root.kind() == "call" {
            out.push(root);
        }
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            collect_calls(child, out);
        }
    }

    #[test]
    fn scan_skips_nested_definitions_and_opaque_blocks() {
        let (tree, _src) = parse(
            "def outer\n  def inner\n    next\n  end\n  items.each do |it|\n    next\n  end\n  work\nend\n",
        );
        let func = method_body(tree.root_node());
        assert_eq!(unsupported_direct_construct_reason(func), None);
    }

    #[test]
    fn scan_reports_rescue_in_source_order() {
        let (tree, _) = parse(
            "def a\n  x = 1\n  begin\n    work\n  rescue StandardError => e\n    handle(e)\n  end\nend\n",
        );
        let func = method_body(tree.root_node());
        let reason = unsupported_direct_construct_reason(func).expect("rescue hit");
        assert!(reason.contains("unsupported rescue at line 5"), "{reason}");
        assert!(reason.starts_with(ABSTENTION_MARKER), "{reason}");
    }

    #[test]
    fn scan_reports_next_at_exact_line() {
        let (tree, _) = parse("def a\n  while cond\n    next\n  end\nend\n");
        let func = method_body(tree.root_node());
        let reason = unsupported_direct_construct_reason(func).expect("next hit");
        assert!(reason.contains("unsupported next at line 3"), "{reason}");
        assert!(reason.contains("abrupt-control edges"), "{reason}");
    }

    #[test]
    fn raise_fail_detection_is_receiverless_only() {
        let (tree, src) =
            parse("def a\n  raise 'x'\n  fail 'y'\n  obj.raise 'z'\n  Kernel.raise 'w'\nend\n");
        let func = method_body(tree.root_node());
        let mut calls = Vec::new();
        collect_calls(func, &mut calls);
        assert!(is_receiverless_raise_or_fail(calls[0], &src));
        assert!(is_receiverless_raise_or_fail(calls[1], &src));
        assert!(!is_receiverless_raise_or_fail(calls[2], &src));
        assert!(!is_receiverless_raise_or_fail(calls[3], &src));
    }

    #[test]
    fn call_label_stops_before_block() {
        let (tree, src) = parse("def a\n  items.each do |it|\n    run(it)\n  end\nend\n");
        let func = method_body(tree.root_node());
        let call = find_first(func, "call").expect("each call");
        let label = call_label(call, &src);
        assert_eq!(label, "items.each");
        assert!(!label.contains("run"));
    }

    #[test]
    fn call_label_handles_parenless_call() {
        // Paren-less calls with args parse as `call` nodes and are traced.
        let (tree, src) = parse("def a\n  work 5 while more?\n  save x, y\nend\n");
        let func = method_body(tree.root_node());
        let work = find_first(func, "call").expect("work 5 call");
        let label = call_label(work, &src);
        assert_eq!(label, "work 5");
        assert!(!label.contains("while"));
    }

    #[test]
    fn if_label_marks_unless() {
        let (tree, src) = parse("def a\n  unless ready?\n    work\n  end\nend\n");
        let func = method_body(tree.root_node());
        let unless = find_first(func, "unless").expect("unless node");
        let label = if_label(unless, &src);
        assert_eq!(label, "unless ready?");
    }

    #[test]
    fn loop_labels_are_honest() {
        let (tree, src) = parse(
            "def a\n  while cond\n    work\n  end\n  until done?\n    work\n  end\n  for item in items\n    work(item)\n  end\nend\n",
        );
        let func = method_body(tree.root_node());
        let while_node = find_first(func, "while").unwrap();
        assert_eq!(loop_label(while_node, &src).as_deref(), Some("while cond"));
        let until_node = find_first(func, "until").unwrap();
        assert_eq!(loop_label(until_node, &src).as_deref(), Some("until done?"));
        let for_node = find_first(func, "for").unwrap();
        assert_eq!(
            loop_label(for_node, &src).as_deref(),
            Some("for item in items")
        );
    }

    #[test]
    fn case_branches_join_patterns_in_order() {
        let (tree, src) = parse(
            "def a(v)\n  case v\n  when 'x', 'y'\n    hit(v)\n  when 'z'\n    miss\n  else\n    default_action\n  end\nend\n",
        );
        let func = method_body(tree.root_node());
        let case = find_first(func, "case").expect("case node");
        let language = super::super::flow_lang::active_flow_language(Lang::Ruby).expect("language");
        let (branches, has_else) = case_branches(case, &src, &language);
        assert_eq!(branches.len(), 3);
        assert_eq!(branches[0].label, "'x', 'y'");
        assert_eq!(branches[1].label, "'z'");
        assert_eq!(branches[2].label, "default");
        assert!(has_else);
    }

    #[test]
    fn case_branches_reports_no_else() {
        let (tree, src) = parse("def a(v)\n  case v\n  when 1\n    one\n  end\nend\n");
        let func = method_body(tree.root_node());
        let case = find_first(func, "case").expect("case node");
        let language = super::super::flow_lang::active_flow_language(Lang::Ruby).expect("language");
        let (branches, has_else) = case_branches(case, &src, &language);
        assert_eq!(branches.len(), 1);
        assert!(!has_else);
    }

    #[test]
    fn value_less_case_gets_case_label() {
        let (tree, src) =
            parse("def a\n  case\n  when x > 1\n    big\n  else\n    small\n  end\nend\n");
        let func = method_body(tree.root_node());
        let case = find_first(func, "case").expect("case node");
        assert_eq!(case_label(case, &src), "case");
    }

    #[test]
    fn scan_reports_ensure_and_rescue_modifier_at_exact_lines() {
        let cases = [
            (
                "def a\n  x = 1\n  begin\n    work\n  ensure\n    work2\n  end\nend\n",
                "unsupported ensure at line 5 requires exception-ensure propagation",
            ),
            (
                "def a\n  x = risky rescue 0\n  x\nend\n",
                "unsupported rescue_modifier at line 2 requires exception-handling edges",
            ),
        ];
        for (source, expected) in cases {
            let (tree, _) = parse(source);
            let func = method_body(tree.root_node());
            let reason =
                unsupported_direct_construct_reason(func).expect("expected an abstention hit");
            assert!(reason.starts_with(ABSTENTION_MARKER), "{reason}");
            assert!(
                reason.contains(expected),
                "expected {expected:?} in {reason}"
            );
        }
    }

    #[test]
    fn scan_reports_abrupt_kinds_break_redo_retry_at_exact_lines() {
        let cases = [
            (
                "def a\n  while c\n    break\n  end\nend\n",
                "unsupported break at line 3 requires abrupt loop exit edges",
            ),
            (
                "def a\n  while c\n    redo\n  end\nend\n",
                "unsupported redo at line 3 requires abrupt loop restart edges",
            ),
            (
                "def a\n  begin\n    retry\n  end\nend\n",
                "unsupported retry at line 3 requires exception-retry edges",
            ),
        ];
        for (source, expected) in cases {
            let (tree, _) = parse(source);
            let func = method_body(tree.root_node());
            let reason =
                unsupported_direct_construct_reason(func).expect("expected an abstention hit");
            assert!(reason.starts_with(ABSTENTION_MARKER), "{reason}");
            assert!(
                reason.contains(expected),
                "expected {expected:?} in {reason}"
            );
        }
    }

    #[test]
    fn scan_skips_abrupt_kinds_inside_opaque_block_and_label_excludes_body() {
        // A block body is opaque: nested if/next must not abstain and must not
        // leak into the call label.
        let (tree, src) = parse(
            "def a\n  out = []\n  items.each do |item|\n    next if item < 0\n    out << item if item > 0\n  end\n  out\nend\n",
        );
        let func = method_body(tree.root_node());
        assert_eq!(unsupported_direct_construct_reason(func), None);
        let call = find_first(func, "call").expect("each call");
        let label = call_label(call, &src);
        assert_eq!(label, "items.each");
        assert!(!label.contains("next"));
        assert!(!label.contains("out"));
    }
}
