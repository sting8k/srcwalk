use std::collections::HashMap;
use std::fmt::Write as _;

use crate::evidence::EvidenceKind;
use crate::format;

use super::types::{FlowAnnotation, FlowEdge, FlowGraph, FlowNode, FlowNodeKind};
use super::RenderedFlowMap;

pub(super) fn render_compact_text(graph: &FlowGraph) -> String {
    let mut out = String::new();
    let rel = format::display_path(&graph.path);
    let _ = writeln!(out, "# Decision-flow: {}", graph.target);
    let _ = writeln!(
        out,
        "\n[target] {rel}:{}-{}  {}",
        graph.entry_start, graph.entry_end, graph.entry_label
    );

    out.push_str("\n[flow]\n");
    // Each node is printed once in document order; outgoing edges inline the
    // target node summary so agents can follow an edge without cross-referencing.
    // Target nodes can therefore appear once per incoming edge plus once in the
    // main listing; that repetition is intentional for readability.
    let nodes_by_id: HashMap<usize, &FlowNode> =
        graph.nodes.iter().map(|node| (node.id, node)).collect();
    let edges_by_from = group_edges_by_source(&graph.edges);
    for node in &graph.nodes {
        let _ = writeln!(out, "{}", node_summary(node));
        if let Some(edges) = edges_by_from.get(&node.id) {
            for edge in edges {
                let _ = writeln!(out, "  {}", edge_summary(edge, &nodes_by_id));
            }
        }
    }

    if graph.truncated {
        out.push_str("\n> Caveat: decision-flow node cap reached; output is truncated. Narrow the target range.\n");
    }
    out
}

pub(super) fn render_flow_map(graph: &FlowGraph) -> RenderedFlowMap {
    let shape = FlowShape::from_graph(graph);
    let mut body = String::new();

    if shape.is_linear() {
        render_linear_flow_map_body(graph, &shape, &mut body);
    } else {
        render_structural_flow_map_body(graph, &shape, &mut body);
    }

    if graph.truncated {
        body.push_str("omitted: node cap reached; narrow the target range.\n");
    }

    let exits = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, FlowNodeKind::Return | FlowNodeKind::Throw))
        .map(flow_map_exit_summary)
        .collect();

    RenderedFlowMap {
        entry_start: graph.entry_start,
        entry_end: graph.entry_end,
        entry_label: graph.entry_label.clone(),
        body,
        exits,
    }
}

#[derive(Default)]
struct FlowShape {
    entries: usize,
    decisions: usize,
    loops: usize,
    exits: usize,
    actions: usize,
    summaries: usize,
}

impl FlowShape {
    fn from_graph(graph: &FlowGraph) -> Self {
        let mut shape = Self::default();
        for node in &graph.nodes {
            match node.kind {
                FlowNodeKind::Entry => shape.entries += 1,
                FlowNodeKind::Decision => shape.decisions += 1,
                FlowNodeKind::Loop => shape.loops += 1,
                FlowNodeKind::Return | FlowNodeKind::Throw => shape.exits += 1,
                FlowNodeKind::Call => shape.actions += 1,
                FlowNodeKind::Summary => shape.summaries += 1,
            }
        }
        shape
    }

    fn is_linear(&self) -> bool {
        self.decisions == 0 && self.loops == 0
    }
}

fn render_structural_flow_map_body(graph: &FlowGraph, shape: &FlowShape, body: &mut String) {
    let edges_by_from = group_edges_by_source(&graph.edges);
    let items = flow_map_items(graph, &edges_by_from);
    let display_by_node_id = flow_map_display_by_node_id(&items);
    let mut annotation_budget = AnnotationBudget::new();

    let _ = writeln!(body, "shape: {}", shape_summary(shape));
    for item in &items {
        let _ = writeln!(body, "{}", item.summary());
        append_item_annotations(body, graph, item, &mut annotation_budget);
        if let Some(edges) = edges_by_from.get(&item.edge_source_id()) {
            for edge in edges {
                let _ = writeln!(
                    body,
                    "  {}",
                    flow_map_edge_display_summary(edge, &display_by_node_id)
                );
            }
        }
    }
}

const MAX_ANNOTATION_LINES: usize = 40;
const MAX_ANNOTATIONS_PER_GROUP: usize = 3;

const ACTION_RUN_MIN: usize = 3;

enum FlowMapItem<'a> {
    Node(&'a FlowNode),
    ActionRun(ActionRunDisplay),
}

impl FlowMapItem<'_> {
    fn summary(&self) -> String {
        match self {
            FlowMapItem::Node(node) => flow_map_node_summary(node),
            FlowMapItem::ActionRun(run) => flow_map_action_run_summary(run),
        }
    }

    fn edge_source_id(&self) -> usize {
        match self {
            FlowMapItem::Node(node) => node.id,
            FlowMapItem::ActionRun(run) => run.last_id,
        }
    }
}

struct ActionRunDisplay {
    node_ids: Vec<usize>,
    last_id: usize,
    count: usize,
    start_line: u32,
    end_line: u32,
}

fn flow_map_items<'a>(
    graph: &'a FlowGraph,
    edges_by_from: &HashMap<usize, Vec<&FlowEdge>>,
) -> Vec<FlowMapItem<'a>> {
    let mut items = Vec::new();
    let mut index = 0;
    while index < graph.nodes.len() {
        let node = &graph.nodes[index];
        if node.kind != FlowNodeKind::Call {
            items.push(FlowMapItem::Node(node));
            index += 1;
            continue;
        }

        let end = action_run_end(&graph.nodes, edges_by_from, index);
        let count = end - index + 1;
        if count < ACTION_RUN_MIN {
            for node in &graph.nodes[index..=end] {
                items.push(FlowMapItem::Node(node));
            }
        } else {
            let nodes = &graph.nodes[index..=end];
            let first = nodes.first().expect("action run has at least one node");
            let last = nodes.last().expect("action run has at least one node");
            items.push(FlowMapItem::ActionRun(ActionRunDisplay {
                node_ids: nodes.iter().map(|node| node.id).collect(),
                last_id: last.id,
                count,
                start_line: first.start_line,
                end_line: last.end_line,
            }));
        }
        index = end + 1;
    }
    items
}

fn action_run_end(
    nodes: &[FlowNode],
    edges_by_from: &HashMap<usize, Vec<&FlowEdge>>,
    start: usize,
) -> usize {
    let mut end = start;
    while end + 1 < nodes.len()
        && nodes[end + 1].kind == FlowNodeKind::Call
        && has_single_next_edge(edges_by_from, nodes[end].id, nodes[end + 1].id)
    {
        end += 1;
    }
    end
}

fn has_single_next_edge(
    edges_by_from: &HashMap<usize, Vec<&FlowEdge>>,
    from: usize,
    to: usize,
) -> bool {
    edges_by_from
        .get(&from)
        .is_some_and(|edges| edges.len() == 1 && edges[0].to == to && edges[0].label.is_none())
}

fn flow_map_display_by_node_id(items: &[FlowMapItem<'_>]) -> HashMap<usize, String> {
    let mut display = HashMap::new();
    for item in items {
        match item {
            FlowMapItem::Node(node) => {
                display.insert(node.id, flow_map_node_summary(node));
            }
            FlowMapItem::ActionRun(run) => {
                let summary = flow_map_action_run_summary(run);
                for id in &run.node_ids {
                    display.insert(*id, summary.clone());
                }
            }
        }
    }
    display
}

#[derive(Default)]
struct AnnotationBudget {
    rendered_lines: usize,
    omitted_notice_rendered: bool,
}

impl AnnotationBudget {
    fn new() -> Self {
        Self::default()
    }

    fn take_line(&mut self, body: &mut String) -> bool {
        if self.rendered_lines < MAX_ANNOTATION_LINES {
            self.rendered_lines += 1;
            return true;
        }
        if !self.omitted_notice_rendered {
            body.push_str("  annotations omitted: cap reached; narrow the target range.\n");
            self.omitted_notice_rendered = true;
        }
        false
    }
}

fn append_item_annotations(
    body: &mut String,
    graph: &FlowGraph,
    item: &FlowMapItem<'_>,
    budget: &mut AnnotationBudget,
) {
    match item {
        FlowMapItem::Node(node) => append_node_annotations(body, &node.annotations, budget),
        FlowMapItem::ActionRun(run) => {
            let annotations: Vec<&FlowAnnotation> = run
                .node_ids
                .iter()
                .filter_map(|id| graph.nodes.get(*id))
                .flat_map(|node| node.annotations.iter())
                .collect();
            append_annotations(body, &annotations, budget);
        }
    }
}

fn append_node_annotations(
    body: &mut String,
    annotations: &[FlowAnnotation],
    budget: &mut AnnotationBudget,
) {
    let annotations: Vec<&FlowAnnotation> = annotations.iter().collect();
    append_annotations(body, &annotations, budget);
}

fn append_annotations(
    body: &mut String,
    annotations: &[&FlowAnnotation],
    budget: &mut AnnotationBudget,
) {
    for kind in [
        EvidenceKind::Definition,
        EvidenceKind::Call,
        EvidenceKind::Write,
        EvidenceKind::Read,
    ] {
        let group: Vec<&FlowAnnotation> = annotations
            .iter()
            .copied()
            .filter(|annotation| annotation.kind() == kind)
            .collect();
        if group.is_empty() {
            continue;
        }
        if !budget.take_line(body) {
            break;
        }
        let _ = writeln!(
            body,
            "  {}: {}",
            annotation_group_label(kind),
            annotation_group_summary(&group)
        );
    }
}

fn annotation_group_label(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::Definition => "definitions",
        EvidenceKind::Usage => "usages",
        EvidenceKind::Text => "text",
        EvidenceKind::File => "files",
        EvidenceKind::Call => "calls",
        EvidenceKind::Read => "reads",
        EvidenceKind::Write => "writes",
        EvidenceKind::Reset => "resets",
        EvidenceKind::Condition => "conditions",
        EvidenceKind::Return => "returns",
        EvidenceKind::Dependency => "dependencies",
        EvidenceKind::UnknownAccess => "unknown",
    }
}

fn annotation_group_summary(annotations: &[&FlowAnnotation]) -> String {
    let mut parts: Vec<String> = annotations
        .iter()
        .take(MAX_ANNOTATIONS_PER_GROUP)
        .map(|annotation| annotation_summary(annotation))
        .collect();
    if annotations.len() > MAX_ANNOTATIONS_PER_GROUP {
        parts.push(format!(
            "+{} more",
            annotations.len() - MAX_ANNOTATIONS_PER_GROUP
        ));
    }
    parts.join("; ")
}

fn annotation_summary(annotation: &FlowAnnotation) -> String {
    match annotation.role() {
        Some(role) => format!(
            "{} {} :{}",
            annotation.text(),
            role.as_str(),
            annotation.line()
        ),
        None => format!("{} :{}", annotation.text(), annotation.line()),
    }
}

fn render_linear_flow_map_body(graph: &FlowGraph, shape: &FlowShape, body: &mut String) {
    body.push_str("shape: linear structural flow; no branch nodes detected by supported parser\n");

    let mut annotation_budget = AnnotationBudget::new();
    if let Some(entry) = graph
        .nodes
        .iter()
        .find(|node| node.kind == FlowNodeKind::Entry && !node.annotations.is_empty())
    {
        let _ = writeln!(body, "entry: {}", flow_map_node_summary(entry));
        append_node_annotations(body, &entry.annotations, &mut annotation_budget);
    }

    for summary in graph
        .nodes
        .iter()
        .filter(|node| node.kind == FlowNodeKind::Summary)
    {
        let _ = writeln!(body, "summary: {}", flow_map_node_summary(summary));
    }

    let actions: Vec<&FlowNode> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == FlowNodeKind::Call)
        .collect();
    match actions.as_slice() {
        [] => body.push_str("actions: none structurally detected\n"),
        [action] => {
            let _ = writeln!(body, "action: {}", flow_map_node_summary(action));
            append_node_annotations(body, &action.annotations, &mut annotation_budget);
        }
        [first, second] => {
            let _ = writeln!(body, "action: {}", flow_map_node_summary(first));
            append_node_annotations(body, &first.annotations, &mut annotation_budget);
            let _ = writeln!(body, "action: {}", flow_map_node_summary(second));
            append_node_annotations(body, &second.annotations, &mut annotation_budget);
        }
        actions => {
            let start = actions
                .first()
                .map_or(graph.entry_start, |node| node.start_line);
            let end = actions.last().map_or(graph.entry_end, |node| node.end_line);
            let _ = writeln!(
                body,
                "actions summarized :{}{} {} action nodes",
                start,
                display_end_suffix(start, end),
                shape.actions
            );
            let annotations: Vec<&FlowAnnotation> = actions
                .iter()
                .flat_map(|node| node.annotations.iter())
                .collect();
            append_annotations(body, &annotations, &mut annotation_budget);
        }
    }

    if shape.exits == 0 {
        body.push_str("exits: none structurally detected in linear flow\n");
    }
}

fn shape_summary(shape: &FlowShape) -> String {
    let mut parts = vec![
        count_label(shape.entries, "entry", "entries"),
        count_label(shape.decisions, "decision", "decisions"),
        count_label(shape.loops, "loop", "loops"),
        count_label(shape.exits, "exit", "exits"),
        count_label(shape.actions, "action", "actions"),
    ];
    if shape.summaries > 0 {
        parts.push(count_label(shape.summaries, "summary", "summaries"));
    }
    parts.join(", ")
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn flow_map_edge_display_summary(
    edge: &FlowEdge,
    display_by_node_id: &HashMap<usize, String>,
) -> String {
    let label = edge.label.as_deref().map_or("next", flow_map_edge_label);
    match display_by_node_id.get(&edge.to) {
        Some(target) => format!("{label} -> {target}"),
        None => format!("{label} -> N{} missing", edge.to + 1),
    }
}

fn flow_map_node_summary(node: &FlowNode) -> String {
    format!(
        "N{} {} :{}{} {}",
        node.id + 1,
        flow_map_kind_label(node.kind),
        node.start_line,
        line_range_suffix(node),
        node.label
    )
}

fn flow_map_action_run_summary(run: &ActionRunDisplay) -> String {
    format!(
        "actions summarized :{}{} {} action nodes",
        run.start_line,
        display_end_suffix(run.start_line, run.end_line),
        run.count
    )
}

fn flow_map_exit_summary(node: &FlowNode) -> String {
    format!(
        ":{}{} {}",
        node.start_line,
        line_range_suffix(node),
        node.label
    )
}

fn flow_map_edge_label(label: &str) -> &str {
    match label {
        "yes" => "true",
        "no" => "false",
        "repeat" => "loop_back",
        "after" => "next",
        other => other,
    }
}

fn flow_map_kind_label(kind: FlowNodeKind) -> &'static str {
    match kind {
        FlowNodeKind::Entry => "entry",
        FlowNodeKind::Decision => "decision",
        FlowNodeKind::Call => "action",
        FlowNodeKind::Return => "return",
        FlowNodeKind::Throw => "throw",
        FlowNodeKind::Loop => "loop",
        FlowNodeKind::Summary => "summary",
    }
}

fn group_edges_by_source(edges: &[FlowEdge]) -> HashMap<usize, Vec<&FlowEdge>> {
    let mut grouped: HashMap<usize, Vec<&FlowEdge>> = HashMap::new();
    for edge in edges {
        grouped.entry(edge.from).or_default().push(edge);
    }
    grouped
}

fn edge_summary(edge: &FlowEdge, nodes_by_id: &HashMap<usize, &FlowNode>) -> String {
    let prefix = edge
        .label
        .as_ref()
        .map_or_else(|| "=>".to_string(), |label| format!("{label} =>"));
    match nodes_by_id.get(&edge.to) {
        Some(node) => format!("{prefix} {}", node_summary(node)),
        None => format!("{prefix} N{} [missing]", edge.to),
    }
}

fn node_summary(node: &FlowNode) -> String {
    format!(
        "N{} [{}] @:{}{}  {}",
        node.id,
        kind_label(node.kind),
        node.start_line,
        line_range_suffix(node),
        node.label
    )
}

fn line_range_suffix(node: &FlowNode) -> String {
    display_end_suffix(node.start_line, node.end_line)
}

fn display_end_suffix(start: u32, end: u32) -> String {
    if end > start {
        format!("-{end}")
    } else {
        String::new()
    }
}

fn kind_label(kind: FlowNodeKind) -> &'static str {
    match kind {
        FlowNodeKind::Entry => "entry",
        FlowNodeKind::Decision => "decision",
        FlowNodeKind::Call => "call",
        FlowNodeKind::Return => "return",
        FlowNodeKind::Throw => "throw",
        FlowNodeKind::Loop => "loop",
        FlowNodeKind::Summary => "summary",
    }
}
