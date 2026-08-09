mod evidence;
mod flow_lang;
mod render;
mod ruby;
mod types;

mod abstention;
use tree_sitter::Node;

pub(crate) use abstention::is_abstention_reason;
use types::{Branch, FlowEdge, FlowGraph, FlowNode, FlowNodeKind, IncomingEdge};
pub(crate) use types::{FlowTarget, TargetSelector};

use crate::error::SrcwalkError;
use crate::types::Lang;
use flow_lang::{active_flow_language, supports_flow_lang, FlowLanguage};

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
    language: FlowLanguage,
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
    let Some(language) = active_flow_language(lang) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: format!(
                "decision-flow currently supports Rust, JavaScript, TypeScript, TSX, Python, Go, Java, C, C++, and C#, as well as Ruby; {lang:?} is not supported"
            ),
        });
    };

    let Some(ts_lang) = flow_lang::flow_language(lang) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: format!(
                "decision-flow requires tree-sitter source support; {lang:?} is not supported"
            ),
        });
    };

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

    let function = find_target_function(tree.root_node(), source, &language, &target.selector)
        .ok_or_else(|| SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason: unresolved_target_reason(&target.selector),
        })?;
    if let Some(reason) = unsupported_direct_construct_reason(function, lang) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.display_target.clone(),
            reason,
        });
    }
    Ok(build_graph(
        target,
        source,
        &language,
        function,
        target_focus(&target.selector),
        node_cap_for_budget(budget_tokens),
    ))
}

fn unsupported_direct_construct_reason(function: Node<'_>, lang: Lang) -> Option<String> {
    if lang == Lang::Ruby {
        ruby::unsupported_direct_construct_reason(function)
    } else {
        abstention::unsupported_direct_construct_reason(function, lang)
    }
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
    supports_flow_lang(lang)
}

pub(crate) fn find_flow_target_function<'tree>(
    root: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    let language = active_flow_language(lang)?;
    find_target_function(root, source, &language, selector)
}

pub(crate) fn find_unique_flow_target_definition<'tree>(
    root: Node<'tree>,
    source: &str,
    lang: Lang,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    let language = active_flow_language(lang)?;
    let lines = source.lines().collect::<Vec<_>>();
    let mut candidates = Vec::new();
    collect_unique_function_nodes(root, &language, &mut candidates);

    match selector {
        TargetSelector::Symbol(symbol) => {
            candidates.retain(|candidate| {
                flow_lang::function_display_name(&language, *candidate, source, &lines)
                    .is_some_and(|name| name == *symbol)
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
    active_flow_language(lang)
        .and_then(|language| normalized_function_node(node, &language))
        .is_some()
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
    language: &FlowLanguage,
    function: Node<'_>,
    focus: Option<(u32, u32)>,
    max_nodes: usize,
) -> FlowGraph {
    let start = line_start(function);
    let end = line_end(function);
    let lines: Vec<&str> = source.lines().collect();
    let entry_label = flow_lang::function_display_name(language, function, source, &lines)
        .unwrap_or_else(|| compact_node_text(function, source));
    let mut builder = FlowBuilder {
        source,
        language: *language,
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
    let body = flow_lang::function_body(language, function)
        .map_or_else(Vec::new, |b| branch_body_nodes(b, language));
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
        let language = &self.language;
        if self.language.lang == Lang::Ruby && ruby::is_nested_definition_kind(statement.kind()) {
            // Nested class/module/method definitions are separate scopes: skip
            // them entirely instead of descending into their bodies.
            return incoming;
        }
        if flow_lang::is_transparent_statement(language, statement.kind()) {
            return self.append_sequence(&statement_children(statement), incoming);
        }
        if flow_lang::is_if_node(language, statement.kind()) {
            return self.append_if(statement, incoming);
        }
        if flow_lang::is_match_or_switch_node(language, statement.kind()) {
            return self.append_branching_decision(statement, incoming);
        }
        if flow_lang::is_loop_node(language, statement.kind()) {
            return self.append_loop(statement, incoming);
        }
        if flow_lang::is_return_node(language, statement.kind()) {
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
                self.language.lang,
            );
            self.connect_all(incoming, id);
            return Vec::new();
        }
        if flow_lang::is_throw_node(language, statement.kind())
            || (self.language.lang == Lang::Ruby
                && ruby::is_receiverless_raise_or_fail(statement, self.source))
        {
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
                self.language.lang,
            );
            self.connect_all(incoming, id);
            return Vec::new();
        }
        if let Some(call) = find_first_call(statement, language) {
            let label = if self.language.lang == Lang::Ruby {
                ruby::call_label(call, self.source)
            } else {
                compact_node_text(call, self.source)
            };
            let id = self.add_node(FlowNodeKind::Call, &label, line_start(call), line_end(call));
            evidence::add_call_annotations(
                &mut self.graph.nodes[id],
                &self.graph.path,
                call,
                self.source,
                self.language.lang,
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
        if self.language.lang == Lang::Ruby {
            return ruby::append_ruby_if(self, node, incoming);
        }
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
        let consequence = if_consequence_body(node, &self.language);
        tails.extend(self.append_branch(id, "yes", &consequence));

        if let Some(alternative) = if_alternative_body(node, &self.language) {
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
        if self.language.lang == Lang::Ruby {
            return ruby::append_ruby_case(self, node, incoming);
        }
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

        let branches = match_or_switch_branches(node, self.source, &self.language);
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
        let label = if self.language.lang == Lang::Ruby {
            ruby::loop_label(node, self.source)
        } else {
            condition_label(node, self.source)
        }
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
            .map_or_else(Vec::new, |n| branch_body_nodes(n, &self.language));
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
    language: &FlowLanguage,
    selector: &TargetSelector,
) -> Option<Node<'tree>> {
    let mut matches = Vec::new();
    collect_function_nodes(root, source, language, selector, &mut matches);
    matches.sort_by_key(|node| (node.end_byte() - node.start_byte(), line_start(*node)));
    matches.into_iter().next()
}

fn collect_unique_function_nodes<'tree>(
    node: Node<'tree>,
    language: &FlowLanguage,
    candidates: &mut Vec<Node<'tree>>,
) {
    if let Some(candidate) = normalized_function_node(node, language) {
        if !candidates
            .iter()
            .any(|existing| existing.id() == candidate.id())
        {
            candidates.push(candidate);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_unique_function_nodes(child, language, candidates);
    }
}

fn declaration_name_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("declarator"))
}

fn collect_function_nodes<'tree>(
    node: Node<'tree>,
    source: &str,
    language: &FlowLanguage,
    selector: &TargetSelector,
    matches: &mut Vec<Node<'tree>>,
) {
    let candidate = normalized_function_node(node, language);
    if let Some(candidate) = candidate {
        let is_match = match selector {
            TargetSelector::Symbol(symbol) => flow_lang::function_display_name(
                language,
                candidate,
                source,
                &source.lines().collect::<Vec<_>>(),
            )
            .is_some_and(|name| name == *symbol),
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
        collect_function_nodes(child, source, language, selector, matches);
    }
}

fn normalized_function_node<'tree>(
    node: Node<'tree>,
    language: &FlowLanguage,
) -> Option<Node<'tree>> {
    if node.kind() == "decorated_definition" && language.lang == Lang::Python {
        return first_named_child_kind(node, "function_definition");
    }
    flow_lang::is_function_like(language, node.kind()).then_some(node)
}

fn statement_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !is_punctuation_or_delimiter(child.kind()))
        .collect()
}

fn branch_body_nodes<'tree>(node: Node<'tree>, language: &FlowLanguage) -> Vec<Node<'tree>> {
    if flow_lang::is_block_like(language, node.kind()) {
        statement_children(node)
    } else {
        vec![node]
    }
}

fn if_consequence_body<'tree>(node: Node<'tree>, language: &FlowLanguage) -> Vec<Node<'tree>> {
    if language.lang == Lang::Python {
        return first_child_block_body(node, language);
    }
    node.child_by_field_name("consequence")
        .or_else(|| node.child_by_field_name("body"))
        .map_or_else(
            || first_child_block_body(node, language),
            |n| branch_body_nodes(n, language),
        )
}

fn if_alternative_body<'tree>(
    node: Node<'tree>,
    language: &FlowLanguage,
) -> Option<Vec<Node<'tree>>> {
    if language.lang != Lang::Python {
        if let Some(alternative) = node.child_by_field_name("alternative") {
            return Some(branch_body_nodes(alternative, language));
        }
    }
    let mut cursor = node.walk();
    let alternative = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "else_clause" | "elif_clause"))
        .map(|n| branch_body_nodes(n, language));
    alternative
}

fn first_child_block_body<'tree>(node: Node<'tree>, language: &FlowLanguage) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    let body = node
        .named_children(&mut cursor)
        .find(|child| flow_lang::is_block_like(language, child.kind()))
        .map_or_else(Vec::new, |n| branch_body_nodes(n, language));
    body
}

fn match_or_switch_branches<'tree>(
    node: Node<'tree>,
    source: &str,
    language: &FlowLanguage,
) -> Vec<Branch<'tree>> {
    let mut branches = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "match_arm" => branches.push(rust_match_branch(child, source, language)),
            "switch_case" | "switch_default" => branches.push(js_switch_branch(child, source)),
            "expression_case"
            | "default_case"
            | "case_statement"
            | "switch_block_statement_group"
            | "switch_section"
            | "case_clause"
            | "default_clause" => branches.push(generic_case_branch(child, source)),
            _ => {
                branches.extend(match_or_switch_branches(child, source, language));
            }
        }
    }
    branches
}

fn rust_match_branch<'tree>(
    node: Node<'tree>,
    source: &str,
    language: &FlowLanguage,
) -> Branch<'tree> {
    let label = node.child_by_field_name("pattern").map_or_else(
        || first_named_child_text(node, source),
        |pattern| compact_node_text(pattern, source),
    );
    let body = node.child_by_field_name("body").map_or_else(
        || last_named_child(node).into_iter().collect(),
        |body| branch_body_nodes(body, language),
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
    matches!(kind, "if_expression" | "if_statement" | "elif_clause") || ruby::is_if_node(kind)
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
    ) || ruby::is_case_node(kind)
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
    ) || ruby::is_loop_node(kind)
}

fn find_first_call<'tree>(node: Node<'tree>, language: &FlowLanguage) -> Option<Node<'tree>> {
    if flow_lang::is_call_node(language, node.kind()) {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if is_control_node(child.kind()) {
            continue;
        }
        if let Some(call) = find_first_call(child, language) {
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
            resolved_symbol: None,
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
        let source = r"
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
";
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
