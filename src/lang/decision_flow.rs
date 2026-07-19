mod evidence;
mod render;
mod types;

use tree_sitter::Node;

use types::{Branch, FlowEdge, FlowGraph, FlowNode, FlowNodeKind, IncomingEdge};
pub(crate) use types::{FlowTarget, TargetSelector};

use crate::error::SrcwalkError;
use crate::lang::outline::outline_language;
use crate::lang::treesitter::{extract_definition_name, js_function_context_name};
use crate::types::Lang;

const DEFAULT_MAX_NODES: usize = 80;
const MIN_BUDGET_MAX_NODES: usize = 12;
const MAX_LABEL_CHARS: usize = 96;

#[derive(Clone, Debug)]
pub(crate) struct RenderedFlowMap {
    pub(crate) entry_start: u32,
    pub(crate) entry_end: u32,
    pub(crate) entry_label: String,
    pub(crate) body: String,
    pub(crate) exits: Vec<String>,
}

struct FlowBuilder<'a> {
    source: &'a str,
    lang: Lang,
    max_nodes: usize,
    focus: Option<(u32, u32)>,
    graph: FlowGraph,
}

pub(crate) fn render_decision_flow(
    target: &FlowTarget,
    source: &str,
    lang: Lang,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let graph = build_target_graph(target, source, lang, budget_tokens)?;
    Ok(render::render_compact_text(&graph))
}

pub(crate) fn render_flow_map(
    target: &FlowTarget,
    source: &str,
    lang: Lang,
    budget_tokens: Option<u64>,
) -> Result<RenderedFlowMap, SrcwalkError> {
    let graph = build_target_graph(target, source, lang, budget_tokens)?;
    Ok(render::render_flow_map(&graph))
}

fn build_target_graph(
    target: &FlowTarget,
    source: &str,
    lang: Lang,
    budget_tokens: Option<u64>,
) -> Result<FlowGraph, SrcwalkError> {
    let Some(ts_lang) = outline_language(lang) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: format!(
                "decision-flow requires tree-sitter source support; {lang:?} is not supported"
            ),
        });
    };

    if !is_supported_decision_flow_lang(lang) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: format!(
                "decision-flow currently supports Rust, JavaScript, TypeScript, TSX, Python, Go, Java, C, C++, and C#; {lang:?} is not supported"
            ),
        });
    }

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&ts_lang)
        .map_err(|e| SrcwalkError::ParseError {
            path: target.path.clone(),
            reason: format!("failed to initialize tree-sitter parser: {e}"),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| SrcwalkError::ParseError {
            path: target.path.clone(),
            reason: "tree-sitter parser returned no tree".to_string(),
        })?;

    let function = find_target_function(tree.root_node(), source, lang, &target.selector)
        .ok_or_else(|| SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: unresolved_target_reason(&target.selector),
        })?;
    Ok(build_graph(
        target,
        source,
        lang,
        function,
        target_focus(&target.selector),
        node_cap_for_budget(budget_tokens),
    ))
}

fn unresolved_target_reason(selector: &TargetSelector) -> String {
    match selector {
        TargetSelector::LineRange { .. } | TargetSelector::FocusedLineRange { .. } => "line/range target must be inside one supported function, method, or constructor; class/module ranges are not supported".to_string(),
        TargetSelector::Symbol(_) => "target did not resolve to a supported function-like AST node".to_string(),
    }
}

fn target_focus(selector: &TargetSelector) -> Option<(u32, u32)> {
    match selector {
        TargetSelector::FocusedLineRange { start, end } => Some((*start, *end)),
        _ => None,
    }
}

fn node_cap_for_budget(budget_tokens: Option<u64>) -> usize {
    budget_tokens.map_or(DEFAULT_MAX_NODES, |budget| {
        ((budget as usize) / 20).clamp(MIN_BUDGET_MAX_NODES, DEFAULT_MAX_NODES)
    })
}

pub(crate) fn is_supported_flow_target_lang(lang: Lang) -> bool {
    is_supported_decision_flow_lang(lang)
}

pub(crate) fn find_flow_target_function<'tree>(
    root: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    find_target_function(root, source, lang, selector)
}

fn is_supported_decision_flow_lang(lang: Lang) -> bool {
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
    )
}

pub(crate) fn find_unique_flow_target_definition<'tree>(
    root: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    let mut candidates = Vec::new();
    collect_unique_function_nodes(root, lang, &mut candidates);

    match selector {
        TargetSelector::Symbol(symbol) => {
            candidates.retain(|candidate| {
                function_display_name(*candidate, source, lang).is_some_and(|name| name == *symbol)
            });
        }
        TargetSelector::LineRange { start, end } => {
            let exact = candidates
                .iter()
                .copied()
                .filter(|candidate| {
                    line_start(*candidate) == *start && line_end(*candidate) == *end
                })
                .collect::<Vec<_>>();
            if exact.len() == 1 {
                return exact.into_iter().next();
            }
            candidates.retain(|candidate| {
                declaration_name_node(*candidate)
                    .is_some_and(|name| node_intersects_range(name, (*start, *end)))
            });
        }
        TargetSelector::FocusedLineRange { start, end } => {
            candidates.retain(|candidate| {
                declaration_name_node(*candidate)
                    .is_some_and(|name| node_intersects_range(name, (*start, *end)))
            });
        }
    }

    (candidates.len() == 1).then(|| candidates[0])
}

pub(crate) fn is_function_like_node(node: Node<'_>, lang: Lang) -> bool {
    normalized_function_node(node, lang).is_some()
}

pub(crate) fn function_has_parameter_named(
    function: Node<'_>,
    source: &str,
    expected: &str,
) -> bool {
    evidence::function_has_parameter_named(function, source, expected)
}

fn build_graph(
    target: &FlowTarget,
    source: &str,
    lang: Lang,
    function: Node<'_>,
    focus: Option<(u32, u32)>,
    max_nodes: usize,
) -> FlowGraph {
    let start = line_start(function);
    let end = line_end(function);
    let entry_label = function_display_name(function, source, lang)
        .unwrap_or_else(|| compact_node_text(function, source));
    let mut builder = FlowBuilder {
        source,
        lang,
        max_nodes,
        focus,
        graph: FlowGraph {
            target: target.display_target.clone(),
            path: target.path.clone(),
            entry_label,
            entry_start: start,
            entry_end: end,
            nodes: Vec::new(),
            edges: Vec::new(),
            truncated: false,
        },
    };

    let entry_id = builder.add_node(FlowNodeKind::Entry, "entry", start, end);
    evidence::add_parameter_annotations(
        &mut builder.graph.nodes[entry_id],
        &builder.graph.path,
        function,
        source,
    );
    let body = function_body(function).map_or_else(Vec::new, statement_children);
    let tails = builder.append_sequence(
        &body,
        vec![IncomingEdge {
            from: entry_id,
            label: None,
        }],
    );
    for tail in tails {
        let return_id = builder.add_node(
            FlowNodeKind::Return,
            "end",
            builder.graph.entry_end,
            builder.graph.entry_end,
        );
        builder.connect(tail, return_id);
    }

    builder.graph
}

impl FlowBuilder<'_> {
    fn append_sequence(
        &mut self,
        statements: &[Node<'_>],
        mut incoming: Vec<IncomingEdge>,
    ) -> Vec<IncomingEdge> {
        if let Some(focus) = self.focus {
            return self.append_focused_sequence(statements, incoming, focus);
        }

        for statement in statements {
            if incoming.is_empty() || self.graph.truncated {
                break;
            }
            incoming = self.append_statement(*statement, incoming);
        }
        incoming
    }

    fn append_focused_sequence(
        &mut self,
        statements: &[Node<'_>],
        mut incoming: Vec<IncomingEdge>,
        focus: (u32, u32),
    ) -> Vec<IncomingEdge> {
        let mut seen_focus = false;
        let mut skipped = FocusSummary::default();

        for statement in statements {
            if incoming.is_empty() || self.graph.truncated {
                break;
            }

            let process_with_focus = if seen_focus {
                false
            } else {
                if !node_intersects_range(*statement, focus) {
                    skipped.record(*statement);
                    continue;
                }
                if skipped.count > 0 {
                    incoming = self.append_summary(
                        incoming,
                        &format!("pre-target statements x{}", skipped.count),
                        skipped.start,
                        skipped.end,
                    );
                }
                seen_focus = true;
                true
            };

            if process_with_focus {
                incoming = self.append_statement(*statement, incoming);
            } else {
                let old_focus = self.focus.take();
                incoming = self.append_statement(*statement, incoming);
                self.focus = old_focus;
            }
        }

        if !seen_focus && skipped.count > 0 {
            let old_focus = self.focus.take();
            let result = self.append_sequence(statements, incoming);
            self.focus = old_focus;
            return result;
        }

        incoming
    }

    fn append_statement(
        &mut self,
        statement: Node<'_>,
        incoming: Vec<IncomingEdge>,
    ) -> Vec<IncomingEdge> {
        if is_transparent_statement(statement.kind()) {
            return self.append_sequence(&statement_children(statement), incoming);
        }

        if is_if_node(statement.kind()) {
            return self.append_if(statement, incoming);
        }
        if is_match_or_switch_node(statement.kind()) {
            return self.append_branching_decision(statement, incoming);
        }
        if is_loop_node(statement.kind()) {
            return self.append_loop(statement, incoming);
        }
        if is_return_node(statement.kind()) {
            let label = compact_node_text(statement, self.source);
            let id = self.add_node(
                FlowNodeKind::Return,
                &label,
                line_start(statement),
                line_end(statement),
            );
            evidence::add_return_or_throw_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                statement,
                self.source,
                self.lang,
            );
            self.connect_all(incoming, id);
            return Vec::new();
        }
        if is_throw_node(statement.kind()) {
            let label = compact_node_text(statement, self.source);
            let id = self.add_node(
                FlowNodeKind::Throw,
                &label,
                line_start(statement),
                line_end(statement),
            );
            evidence::add_return_or_throw_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                statement,
                self.source,
                self.lang,
            );
            self.connect_all(incoming, id);
            return Vec::new();
        }
        if let Some(call) = find_first_call(statement, self.lang) {
            let label = compact_node_text(call, self.source);
            let id = self.add_node(FlowNodeKind::Call, &label, line_start(call), line_end(call));
            evidence::add_call_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                call,
                self.source,
            );
            evidence::add_assignment_write_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                statement,
                self.source,
            );
            self.connect_all(incoming, id);
            return vec![IncomingEdge {
                from: id,
                label: None,
            }];
        }
        if let Some(nested) = find_first_nested_control(statement) {
            return self.append_statement(nested, incoming);
        }

        if evidence::has_assignment(statement) {
            let label = compact_node_text(statement, self.source);
            let id = self.add_node(
                FlowNodeKind::Call,
                &label,
                line_start(statement),
                line_end(statement),
            );
            evidence::add_assignment_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                statement,
                self.source,
            );
            self.connect_all(incoming, id);
            return vec![IncomingEdge {
                from: id,
                label: None,
            }];
        }

        incoming
    }

    fn append_if(&mut self, node: Node<'_>, incoming: Vec<IncomingEdge>) -> Vec<IncomingEdge> {
        let label = condition_label(node, self.source)
            .unwrap_or_else(|| compact_node_text(node, self.source));
        let id = self.add_node(
            FlowNodeKind::Decision,
            &label,
            line_start(node),
            line_end(node),
        );
        self.connect_all(incoming, id);
        evidence::add_condition_read_annotations(
            &mut self.graph.nodes[id],
            &self.graph.path,
            node,
            self.source,
        );

        let old_focus = if self.focus_intersects_condition(node) {
            self.focus.take()
        } else {
            None
        };
        let mut tails = Vec::new();
        let consequence = if_consequence_body(node, self.lang);
        tails.extend(self.append_branch(id, "yes", &consequence));

        if let Some(alternative) = if_alternative_body(node, self.lang) {
            tails.extend(self.append_branch(id, "no", &alternative));
        } else {
            tails.push(IncomingEdge {
                from: id,
                label: Some("no".to_string()),
            });
        }
        if old_focus.is_some() {
            self.focus = old_focus;
        }

        tails
    }

    fn append_branching_decision(
        &mut self,
        node: Node<'_>,
        incoming: Vec<IncomingEdge>,
    ) -> Vec<IncomingEdge> {
        let label = condition_label(node, self.source)
            .unwrap_or_else(|| compact_node_text(node, self.source));
        let id = self.add_node(
            FlowNodeKind::Decision,
            &label,
            line_start(node),
            line_end(node),
        );
        self.connect_all(incoming, id);
        evidence::add_condition_read_annotations(
            &mut self.graph.nodes[id],
            &self.graph.path,
            node,
            self.source,
        );

        let branches = match_or_switch_branches(node, self.source);
        if branches.is_empty() {
            return vec![IncomingEdge {
                from: id,
                label: None,
            }];
        }

        let old_focus = if self.focus_intersects_condition(node) {
            self.focus.take()
        } else {
            None
        };
        let mut tails = Vec::new();
        for branch in branches {
            tails.extend(self.append_branch(id, &branch.label, &branch.body));
        }
        if old_focus.is_some() {
            self.focus = old_focus;
        }

        tails
    }

    fn append_loop(&mut self, node: Node<'_>, incoming: Vec<IncomingEdge>) -> Vec<IncomingEdge> {
        let label = condition_label(node, self.source)
            .unwrap_or_else(|| compact_node_text(node, self.source));
        let id = self.add_node(FlowNodeKind::Loop, &label, line_start(node), line_end(node));
        self.connect_all(incoming, id);
        evidence::add_condition_read_annotations(
            &mut self.graph.nodes[id],
            &self.graph.path,
            node,
            self.source,
        );

        let body = node
            .child_by_field_name("body")
            .or_else(|| node.child_by_field_name("consequence"))
            .map_or_else(Vec::new, branch_body_nodes);
        let body_tails = self.append_branch(id, "body", &body);
        for tail in body_tails {
            let edges_before = self.graph.edges.len();
            self.connect(tail, id);
            if self.graph.edges.len() == edges_before {
                continue;
            }
            if let Some(last) = self.graph.edges.last_mut() {
                last.label = Some("repeat".to_string());
            }
        }
        vec![IncomingEdge {
            from: id,
            label: Some("after".to_string()),
        }]
    }

    fn focus_intersects_condition(&self, node: Node<'_>) -> bool {
        self.focus.is_some_and(|focus| {
            condition_node(node).is_some_and(|condition| node_intersects_range(condition, focus))
        })
    }

    fn append_branch(&mut self, from: usize, label: &str, body: &[Node<'_>]) -> Vec<IncomingEdge> {
        if body.is_empty() {
            return vec![IncomingEdge {
                from,
                label: Some(label.to_string()),
            }];
        }

        if let Some(focus) = self.focus {
            if !nodes_intersect_range(body, focus) {
                return Vec::new();
            }
        }

        self.append_sequence(
            body,
            vec![IncomingEdge {
                from,
                label: Some(label.to_string()),
            }],
        )
    }

    fn append_summary(
        &mut self,
        incoming: Vec<IncomingEdge>,
        label: &str,
        start_line: u32,
        end_line: u32,
    ) -> Vec<IncomingEdge> {
        let id = self.add_node(FlowNodeKind::Summary, label, start_line, end_line);
        self.connect_all(incoming, id);
        vec![IncomingEdge {
            from: id,
            label: None,
        }]
    }

    fn add_node(
        &mut self,
        kind: FlowNodeKind,
        label: &str,
        start_line: u32,
        end_line: u32,
    ) -> usize {
        if self.graph.nodes.len() >= self.max_nodes {
            if !self.graph.truncated {
                self.graph.truncated = true;
                let id = self.graph.nodes.len();
                self.graph.nodes.push(FlowNode {
                    id,
                    kind: FlowNodeKind::Return,
                    label: "… truncated".to_string(),
                    start_line,
                    end_line,
                    annotations: Vec::new(),
                });
                return id;
            }
            return self.graph.nodes.last().map_or(0, |node| node.id);
        }
        let id = self.graph.nodes.len();
        self.graph.nodes.push(FlowNode {
            id,
            kind,
            label: clean_label(label),
            start_line,
            end_line,
            annotations: Vec::new(),
        });
        id
    }

    fn connect_all(&mut self, incoming: Vec<IncomingEdge>, to: usize) {
        for edge in incoming {
            self.connect(edge, to);
        }
    }

    fn connect(&mut self, incoming: IncomingEdge, to: usize) {
        if incoming.from == to {
            return;
        }
        self.graph.edges.push(FlowEdge {
            from: incoming.from,
            to,
            label: incoming.label,
        });
    }
}

#[derive(Default)]
struct FocusSummary {
    count: usize,
    start: u32,
    end: u32,
}

impl FocusSummary {
    fn record(&mut self, node: Node<'_>) {
        self.count += 1;
        if self.start == 0 {
            self.start = line_start(node);
        }
        self.end = line_end(node);
    }
}

fn node_intersects_range(node: Node<'_>, range: (u32, u32)) -> bool {
    line_start(node) <= range.1 && line_end(node) >= range.0
}

fn nodes_intersect_range(nodes: &[Node<'_>], range: (u32, u32)) -> bool {
    nodes.iter().any(|node| node_intersects_range(*node, range))
}

fn find_target_function<'tree>(
    root: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    let mut matches = Vec::new();
    collect_function_nodes(root, source, lang, selector, &mut matches);
    matches.sort_by_key(|node| (node.end_byte() - node.start_byte(), line_start(*node)));
    matches.into_iter().next()
}

fn collect_unique_function_nodes<'tree>(
    node: Node<'tree>,
    lang: Lang,
    candidates: &mut Vec<Node<'tree>>,
) {
    if let Some(candidate) = normalized_function_node(node, lang) {
        if !candidates
            .iter()
            .any(|existing| existing.id() == candidate.id())
        {
            candidates.push(candidate);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_unique_function_nodes(child, lang, candidates);
    }
}

fn declaration_name_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("declarator"))
}

fn collect_function_nodes<'tree>(
    node: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
    matches: &mut Vec<Node<'tree>>,
) {
    let candidate = normalized_function_node(node, lang);
    if let Some(candidate) = candidate {
        let is_match = match selector {
            TargetSelector::Symbol(symbol) => {
                function_display_name(candidate, source, lang).is_some_and(|name| name == *symbol)
            }
            TargetSelector::LineRange { start, end }
            | TargetSelector::FocusedLineRange { start, end } => {
                line_start(candidate) <= *start && line_end(candidate) >= *end
            }
        };
        if is_match {
            matches.push(candidate);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_function_nodes(child, source, lang, selector, matches);
    }
}

fn normalized_function_node(node: Node<'_>, lang: Lang) -> Option<Node<'_>> {
    if node.kind() == "decorated_definition" && lang == Lang::Python {
        return first_named_child_kind(node, "function_definition");
    }
    is_function_like(node.kind(), lang).then_some(node)
}

fn is_function_like(kind: &str, lang: Lang) -> bool {
    match lang {
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
        _ => false,
    }
}

fn function_display_name(node: Node<'_>, source: &str, lang: Lang) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    if matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) {
        return js_function_context_name(node, &lines)
            .or_else(|| extract_definition_name(node, &lines));
    }
    extract_definition_name(node, &lines).or_else(|| {
        node.child_by_field_name("name")
            .map(|name| compact_node_text(name, source))
    })
}

fn function_body(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = node.walk();
    let body = node
        .children(&mut cursor)
        .find(|child| is_block_like(child.kind()));
    body
}

fn statement_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !is_punctuation_or_delimiter(child.kind()))
        .collect()
}

fn branch_body_nodes(node: Node<'_>) -> Vec<Node<'_>> {
    if is_block_like(node.kind()) {
        statement_children(node)
    } else {
        vec![node]
    }
}

fn is_block_like(kind: &str) -> bool {
    matches!(
        kind,
        "block"
            | "statement_block"
            | "compound_statement"
            | "declaration_list"
            | "switch_block"
            | "switch_body"
    )
}

fn if_consequence_body(node: Node<'_>, lang: Lang) -> Vec<Node<'_>> {
    if lang == Lang::Python {
        return first_child_block_body(node);
    }
    node.child_by_field_name("consequence")
        .or_else(|| node.child_by_field_name("body"))
        .map_or_else(|| first_child_block_body(node), branch_body_nodes)
}

fn if_alternative_body(node: Node<'_>, lang: Lang) -> Option<Vec<Node<'_>>> {
    if lang != Lang::Python {
        if let Some(alternative) = node.child_by_field_name("alternative") {
            return Some(branch_body_nodes(alternative));
        }
    }
    let mut cursor = node.walk();
    let alternative = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "else_clause" | "elif_clause"))
        .map(branch_body_nodes);
    alternative
}

fn first_child_block_body(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    let body = node
        .named_children(&mut cursor)
        .find(|child| is_block_like(child.kind()))
        .map_or_else(Vec::new, branch_body_nodes);
    body
}

fn match_or_switch_branches<'tree>(node: Node<'tree>, source: &str) -> Vec<Branch<'tree>> {
    let mut branches = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "match_arm" => branches.push(rust_match_branch(child, source)),
            "switch_case" | "switch_default" => branches.push(js_switch_branch(child, source)),
            "expression_case"
            | "default_case"
            | "case_statement"
            | "switch_block_statement_group"
            | "switch_section"
            | "case_clause"
            | "default_clause" => branches.push(generic_case_branch(child, source)),
            _ => {
                branches.extend(match_or_switch_branches(child, source));
            }
        }
    }
    branches
}

fn rust_match_branch<'tree>(node: Node<'tree>, source: &str) -> Branch<'tree> {
    let label = node.child_by_field_name("pattern").map_or_else(
        || first_named_child_text(node, source),
        |pattern| compact_node_text(pattern, source),
    );
    let body = node.child_by_field_name("body").map_or_else(
        || last_named_child(node).into_iter().collect(),
        branch_body_nodes,
    );
    Branch { label, body }
}

fn js_switch_branch<'tree>(node: Node<'tree>, source: &str) -> Branch<'tree> {
    let label = if node.kind() == "switch_default" {
        "default".to_string()
    } else {
        node.child_by_field_name("value").map_or_else(
            || first_named_child_text(node, source),
            |value| compact_node_text(value, source),
        )
    };
    let mut cursor = node.walk();
    let value = node.child_by_field_name("value");
    let body = node
        .named_children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "comment"))
        .filter(|child| value.is_none_or(|value| child.id() != value.id()))
        .collect();
    Branch { label, body }
}

fn generic_case_branch<'tree>(node: Node<'tree>, source: &str) -> Branch<'tree> {
    let children = statement_children(node);
    if node.kind().contains("default") {
        return Branch {
            label: "default".to_string(),
            body: children,
        };
    }

    if let Some(value) = node.child_by_field_name("value") {
        let body = children
            .into_iter()
            .filter(|child| child.id() != value.id())
            .collect();
        return Branch {
            label: compact_node_text(value, source),
            body,
        };
    }

    let Some((first, rest)) = children.split_first() else {
        return Branch {
            label: "case".to_string(),
            body: Vec::new(),
        };
    };
    if is_case_label_node(first.kind()) {
        Branch {
            label: compact_node_text(*first, source),
            body: rest.to_vec(),
        }
    } else {
        Branch {
            label: "default".to_string(),
            body: children,
        }
    }
}

fn is_case_label_node(kind: &str) -> bool {
    matches!(
        kind,
        "switch_label"
            | "constant_pattern"
            | "expression_list"
            | "number_literal"
            | "string_literal"
            | "interpreted_string_literal"
    )
}

fn condition_label(node: Node<'_>, source: &str) -> Option<String> {
    condition_node(node).map(|condition| compact_node_text(condition, source))
}

fn condition_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("condition")
        .or_else(|| node.child_by_field_name("value"))
}

fn is_if_node(kind: &str) -> bool {
    matches!(kind, "if_expression" | "if_statement" | "elif_clause")
}

fn is_match_or_switch_node(kind: &str) -> bool {
    matches!(
        kind,
        "match_expression"
            | "match_statement"
            | "switch_statement"
            | "switch_expression"
            | "expression_switch_statement"
            | "type_switch_statement"
    )
}

fn is_loop_node(kind: &str) -> bool {
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
    )
}

fn is_return_node(kind: &str) -> bool {
    matches!(kind, "return_expression" | "return_statement")
}

fn is_throw_node(kind: &str) -> bool {
    matches!(kind, "throw_statement" | "raise_statement")
}

fn is_transparent_statement(kind: &str) -> bool {
    matches!(
        kind,
        "expression_statement" | "parenthesized_expression" | "else_clause"
    ) || is_block_like(kind)
}

fn is_call_node(kind: &str, lang: Lang) -> bool {
    matches!(kind, "call_expression")
        || (lang == Lang::Python && kind == "call")
        || matches!(kind, "method_invocation" | "invocation_expression")
        || (lang == Lang::Rust && matches!(kind, "macro_invocation"))
        || matches!(kind, "await_expression")
}

fn find_first_call(node: Node<'_>, lang: Lang) -> Option<Node<'_>> {
    if is_call_node(node.kind(), lang) {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_control_node(child.kind()) {
            continue;
        }
        if let Some(call) = find_first_call(child, lang) {
            return Some(call);
        }
    }
    None
}

fn find_first_nested_control(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_control_node(child.kind()) {
            return Some(child);
        }
        if let Some(nested) = find_first_nested_control(child) {
            return Some(nested);
        }
    }
    None
}

fn is_control_node(kind: &str) -> bool {
    is_if_node(kind) || is_match_or_switch_node(kind) || is_loop_node(kind)
}

fn first_named_child_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn first_named_child_text(node: Node<'_>, source: &str) -> String {
    let mut cursor = node.walk();
    let first = node.named_children(&mut cursor).next();
    first.map_or_else(
        || compact_node_text(node, source),
        |child| compact_node_text(child, source),
    )
}

fn last_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).last()
}

fn is_punctuation_or_delimiter(kind: &str) -> bool {
    matches!(kind, ";" | "," | ":" | "{" | "}" | "(" | ")")
}

fn compact_node_text(node: Node<'_>, source: &str) -> String {
    let range = node.byte_range();
    let text = source.get(range).unwrap_or_default();
    clean_label(text)
}

fn clean_label(text: &str) -> String {
    let label = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_label(&label, MAX_LABEL_CHARS)
}

fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }

    let keep = max_chars.saturating_sub(1);
    let mut truncated = label.chars().take(keep).collect::<String>();
    truncated.push('…');
    truncated
}

fn line_start(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn line_end(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, selector: TargetSelector) -> FlowTarget {
        FlowTarget {
            path: std::path::Path::new(path).to_path_buf(),
            display_target: path.to_string(),
            selector,
        }
    }

    #[test]
    fn label_truncation_is_utf8_safe() {
        let label = format!("{}💥{}", "a".repeat(94), "b".repeat(10));
        let truncated = truncate_label(&label, MAX_LABEL_CHARS);
        assert!(truncated.ends_with('…'));
        assert!(truncated.contains('💥'));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn rust_if_and_match_emit_decisions() {
        let source = r#"
fn route(mode: Mode) {
    if matches!(mode, Mode::Files) {
        run_files();
        return;
    }
    match mode {
        Mode::Text => run_text(),
        _ => run_symbol(),
    }
}
"#;
        let out = render_decision_flow(
            &target("src/lib.rs:route", TargetSelector::Symbol("route".into())),
            source,
            Lang::Rust,
            None,
        )
        .unwrap();
        assert!(out.contains("[decision]"), "{out}");
        assert!(out.contains("matches!(mode, Mode::Files)"), "{out}");
        assert!(out.contains("Mode::Text"), "{out}");
        assert!(out.contains("run_files"), "{out}");
    }

    #[test]
    fn python_if_and_raise_emit_decisions() {
        let source = r#"
def route(value):
    if value:
        call_a()
    else:
        raise RuntimeError("bad")
    return value
"#;
        let out = render_decision_flow(
            &target("app.py:route", TargetSelector::Symbol("route".into())),
            source,
            Lang::Python,
            None,
        )
        .unwrap();
        assert!(out.contains("value"), "{out}");
        assert!(out.contains("call_a"), "{out}");
        assert!(out.contains("[throw]"), "{out}");
    }
}
