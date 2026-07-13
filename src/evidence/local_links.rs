use std::collections::BTreeSet;
use std::path::Path;

use tree_sitter::Node;

use crate::evidence::Anchor;
use crate::lang::outline::outline_language;
use crate::search::callees::extract_call_sites;
use crate::types::Lang;

pub(crate) const DEFAULT_LOCAL_LINK_MAX_HOPS: usize = 2;
pub(crate) const MAX_LOCAL_LINK_HOPS: usize = 3;
const MAX_LOCAL_LINKS: usize = 256;
const MAX_LOCAL_LINK_CHAINS: usize = 16;
const LOCAL_LINK_CONFIDENCE: &str = "local structural syntax";
const TRAVERSAL_DEPTH_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LocalSubject {
    identity: String,
}

impl LocalSubject {
    pub(crate) fn new(identity: impl Into<String>) -> Option<Self> {
        let identity = identity.into().trim().to_string();
        (!identity.is_empty()).then_some(Self { identity })
    }

    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LocalLinkKind {
    AssignmentAlias,
    FieldRead,
    FieldWrite,
    CallResult,
    ArgumentUse,
    ReturnValue,
    Parameter,
}

impl LocalLinkKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AssignmentAlias => "assignment/alias",
            Self::FieldRead => "field_read",
            Self::FieldWrite => "field_write",
            Self::CallResult => "call_result",
            Self::ArgumentUse => "argument_use",
            Self::ReturnValue => "return_value",
            Self::Parameter => "parameter",
        }
    }
}

impl LocalLinkKind {
    fn is_binding_like(self) -> bool {
        matches!(
            self,
            Self::AssignmentAlias | Self::FieldRead | Self::FieldWrite | Self::CallResult
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalLink {
    kind: LocalLinkKind,
    from: LocalSubject,
    to: LocalSubject,
    anchor: Anchor,
    snippet: String,
    confidence: &'static str,
}

impl LocalLink {
    fn new(
        kind: LocalLinkKind,
        from: LocalSubject,
        to: LocalSubject,
        anchor: Anchor,
        snippet: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            from,
            to,
            anchor,
            snippet: snippet.into(),
            confidence: LOCAL_LINK_CONFIDENCE,
        }
    }

    pub(crate) fn kind(&self) -> LocalLinkKind {
        self.kind
    }

    pub(crate) fn from(&self) -> &LocalSubject {
        &self.from
    }

    pub(crate) fn to(&self) -> &LocalSubject {
        &self.to
    }

    pub(crate) fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    pub(crate) fn snippet(&self) -> &str {
        &self.snippet
    }

    pub(crate) fn confidence(&self) -> &'static str {
        self.confidence
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LocalLinkGraph {
    links: Vec<LocalLink>,
    budget_exceeded: bool,
}

impl LocalLinkGraph {
    pub(crate) fn new() -> Self {
        Self {
            links: Vec::new(),
            budget_exceeded: false,
        }
    }

    pub(crate) fn from_links(mut links: Vec<LocalLink>) -> Self {
        links.sort_by_key(local_link_sort_key);
        links.dedup_by(|left, right| {
            local_link_semantic_key(left) == local_link_semantic_key(right)
        });
        let budget_exceeded = links.len() > MAX_LOCAL_LINKS;
        if budget_exceeded {
            links.truncate(MAX_LOCAL_LINKS);
        }
        Self {
            links,
            budget_exceeded,
        }
    }

    pub(crate) fn links(&self) -> &[LocalLink] {
        &self.links
    }

    pub(crate) fn budget_exceeded(&self) -> bool {
        self.budget_exceeded
    }

    pub(crate) fn unique_chain(
        &self,
        start: &str,
        end: &str,
        max_hops: usize,
    ) -> Option<Vec<LocalLink>> {
        let start = start.trim();
        let end = end.trim();
        if start.is_empty() || end.is_empty() {
            return None;
        }
        if start == end {
            return Some(Vec::new());
        }
        let max_hops = max_hops.clamp(1, MAX_LOCAL_LINK_HOPS);
        if self.links.is_empty()
            || self.budget_exceeded
            || self.has_ambiguous_binding_target(start)
            || self.has_ambiguous_binding_target(end)
        {
            return None;
        }

        let mut chains: Vec<Vec<usize>> = Vec::new();
        let mut path = Vec::new();
        let mut seen = BTreeSet::new();
        seen.insert(start.to_string());
        self.collect_chains(start, end, max_hops, &mut seen, &mut path, &mut chains);
        if chains.len() == 1 {
            let chain = chains.pop().unwrap();
            if chain
                .iter()
                .any(|index| self.has_ambiguous_binding_target(self.links[*index].to.identity()))
            {
                return None;
            }
            Some(
                chain
                    .into_iter()
                    .map(|index| self.links[index].clone())
                    .collect(),
            )
        } else {
            None
        }
    }

    pub(crate) fn unique_predecessor_chain(
        &self,
        end: &str,
        max_hops: usize,
    ) -> Option<Vec<LocalLink>> {
        let end = end.trim();
        if end.is_empty() || self.budget_exceeded {
            return None;
        }

        let max_hops = max_hops.clamp(1, MAX_LOCAL_LINK_HOPS);
        let mut current = end.to_string();
        let mut seen = BTreeSet::from([current.clone()]);
        let mut hops = 0;

        while hops < max_hops {
            let mut incoming = self
                .links
                .iter()
                .filter(|link| link.to.identity() == current && link.kind.is_binding_like());
            let Some(link) = incoming.next() else {
                break;
            };
            if incoming.next().is_some() {
                return None;
            }

            let predecessor = link.from.identity().to_string();
            if !seen.insert(predecessor.clone()) {
                return None;
            }
            current = predecessor;
            hops += 1;
        }

        self.unique_chain(&current, end, max_hops)
    }

    fn has_ambiguous_binding_target(&self, target: &str) -> bool {
        let mut incoming = self
            .links
            .iter()
            .filter(|link| link.to.identity() == target && link.kind.is_binding_like());
        let Some(first) = incoming.next() else {
            return false;
        };
        incoming.any(|link| {
            link.from.identity() != first.from.identity()
                || link.anchor.display() != first.anchor.display()
        })
    }

    fn collect_chains(
        &self,
        current: &str,
        end: &str,
        max_hops: usize,
        seen: &mut BTreeSet<String>,
        path: &mut Vec<usize>,
        chains: &mut Vec<Vec<usize>>,
    ) {
        if chains.len() >= MAX_LOCAL_LINK_CHAINS || path.len() >= max_hops {
            return;
        }

        for (index, link) in self.links.iter().enumerate() {
            if link.from.identity() != current {
                continue;
            }
            let next = link.to.identity();
            if seen.contains(next) {
                continue;
            }

            path.push(index);
            if next == end {
                chains.push(path.clone());
            } else {
                seen.insert(next.to_string());
                self.collect_chains(next, end, max_hops, seen, path, chains);
                seen.remove(next);
            }
            path.pop();

            if chains.len() >= MAX_LOCAL_LINK_CHAINS {
                return;
            }
        }
    }
}

fn local_link_sort_key(link: &LocalLink) -> (usize, String, String, String, String) {
    (
        link.kind as usize,
        link.from.identity().to_string(),
        link.to.identity().to_string(),
        link.anchor().display(),
        link.snippet().to_string(),
    )
}

fn local_link_semantic_key(link: &LocalLink) -> (usize, String, String) {
    (
        link.kind as usize,
        link.from.identity().to_string(),
        link.to.identity().to_string(),
    )
}

#[cfg(test)]
pub(crate) fn collect_local_links_for_function(
    path: &Path,
    content: &str,
    lang: Lang,
    scope_label: &str,
    start_line: u32,
    end_line: u32,
) -> LocalLinkGraph {
    let spans = [(scope_label, start_line, end_line)];
    collect_local_links_for_function_spans(path, content, lang, &spans)
        .into_iter()
        .next()
        .unwrap_or_default()
}

pub(crate) fn collect_local_links_for_function_spans(
    path: &Path,
    content: &str,
    lang: Lang,
    spans: &[(&str, u32, u32)],
) -> Vec<LocalLinkGraph> {
    if spans.is_empty() {
        return Vec::new();
    }

    let empty_graphs = || vec![LocalLinkGraph::new(); spans.len()];
    let Some(ts_lang) = outline_language(lang) else {
        return empty_graphs();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return empty_graphs();
    }
    let Some(tree) = parser.parse(content, None) else {
        return empty_graphs();
    };

    let bytes = content.as_bytes();
    let root = tree.root_node();
    let call_sites = extract_call_sites(content, lang, None);

    spans
        .iter()
        .map(|(scope_label, start_line, end_line)| {
            if *start_line == 0 || *end_line == 0 || *end_line < *start_line {
                return LocalLinkGraph::new();
            }

            let mut links = Vec::new();
            collect_structure_links(
                root,
                bytes,
                path,
                scope_label,
                *start_line,
                *end_line,
                &mut links,
                0,
            );

            for call_site in call_sites
                .iter()
                .filter(|call_site| (*start_line..=*end_line).contains(&call_site.line))
            {
                collect_call_site_links(path, bytes, call_site, &mut links);
            }

            LocalLinkGraph::from_links(links)
        })
        .collect()
}

fn collect_structure_links(
    node: Node<'_>,
    source: &[u8],
    path: &Path,
    scope_label: &str,
    start_line: u32,
    end_line: u32,
    links: &mut Vec<LocalLink>,
    depth: usize,
) {
    if depth > TRAVERSAL_DEPTH_LIMIT || !node_overlaps_span(node, start_line, end_line) {
        return;
    }

    if let Some(link) = assignment_link(node, source, path) {
        links.push(link);
        return;
    }

    if let Some(link) = return_link(node, source, path) {
        links.push(link);
        return;
    }

    if let Some(parameter_links) = parameter_links(node, source, path, scope_label) {
        links.extend(parameter_links);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_structure_links(
            child,
            source,
            path,
            scope_label,
            start_line,
            end_line,
            links,
            depth + 1,
        );
    }
}

fn assignment_link(node: Node<'_>, source: &[u8], path: &Path) -> Option<LocalLink> {
    let parts = assignment_parts(node)?;
    let snippet = compact_node_text(node, source);
    let anchor = Anchor::line(path, line_start(node));
    let lhs_text = compact_node_text(parts.lhs, source);
    let rhs = parts.rhs?;
    let rhs_text = compact_node_text(rhs, source);

    let lhs_subject = subject_from_expression(parts.lhs, source)?;
    if is_call_like(rhs.kind(), &rhs_text) {
        return subject_from_text(&rhs_text).map(|from| {
            LocalLink::new(
                LocalLinkKind::CallResult,
                from,
                lhs_subject,
                anchor,
                snippet,
            )
        });
    }

    let rhs_subject = subject_from_expression(rhs, source)?;
    if is_field_like(&lhs_text) && !is_field_like(&rhs_text) {
        return Some(LocalLink::new(
            LocalLinkKind::FieldWrite,
            rhs_subject,
            lhs_subject,
            anchor,
            snippet,
        ));
    }
    if is_field_like(&rhs_text) && !is_field_like(&lhs_text) {
        return Some(LocalLink::new(
            LocalLinkKind::FieldRead,
            rhs_subject,
            lhs_subject,
            anchor,
            snippet,
        ));
    }
    if rhs_subject.identity() != lhs_subject.identity() {
        return Some(LocalLink::new(
            LocalLinkKind::AssignmentAlias,
            rhs_subject,
            lhs_subject,
            anchor,
            snippet,
        ));
    }
    None
}

fn return_link(node: Node<'_>, source: &[u8], path: &Path) -> Option<LocalLink> {
    if !node.kind().contains("return") {
        return None;
    }
    let value = node
        .named_child(0)
        .or_else(|| first_identifier_like_descendant(node))?;
    let from = subject_from_expression(value, source)
        .or_else(|| subject_from_text(compact_node_text(value, source)))?;
    let to = subject_from_text(format!("return@{}", line_start(node)))?;
    Some(LocalLink::new(
        LocalLinkKind::ReturnValue,
        from,
        to,
        Anchor::line(path, line_start(node)),
        compact_node_text(node, source),
    ))
}

fn collect_call_site_links(
    path: &Path,
    source: &[u8],
    call_site: &crate::search::callees::CallSite,
    links: &mut Vec<LocalLink>,
) {
    let call_text = compact_call_site_text(call_site, source);
    let Some(call_subject) = subject_from_text(&call_text) else {
        return;
    };
    let anchor = Anchor::line(path, call_site.line);
    for arg in &call_site.args {
        if let Some(from) = subject_from_expression_text(arg) {
            links.push(LocalLink::new(
                LocalLinkKind::ArgumentUse,
                from,
                call_subject.clone(),
                anchor.clone(),
                call_text.clone(),
            ));
        }
    }
    if let Some(return_var) = call_site
        .return_var
        .as_deref()
        .and_then(subject_from_expression_text)
    {
        links.push(LocalLink::new(
            LocalLinkKind::CallResult,
            call_subject.clone(),
            return_var,
            anchor.clone(),
            call_text.clone(),
        ));
    }
    if call_site.is_return {
        if let Some(ret) = subject_from_text(format!("return@{}", call_site.line)) {
            links.push(LocalLink::new(
                LocalLinkKind::ReturnValue,
                call_subject,
                ret,
                anchor,
                call_text,
            ));
        }
    }
}

fn compact_call_site_text(call_site: &crate::search::callees::CallSite, source: &[u8]) -> String {
    call_site
        .call_byte_range
        .and_then(|(start, end)| source.get(start..end))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map_or_else(|| compact_text(&call_site.call_text), compact_text)
}

fn parameter_links(
    node: Node<'_>,
    source: &[u8],
    path: &Path,
    scope_label: &str,
) -> Option<Vec<LocalLink>> {
    let parameters = parameters_node(node)?;
    let function_subject = subject_from_text(format!(
        "function:{}@{}-{}",
        scope_label,
        line_start(node),
        line_end(node)
    ))?;

    let mut links = Vec::new();
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if is_punctuation_or_type_only(parameter.kind()) {
            continue;
        }
        let Some(name) = parameter_name_node(parameter) else {
            continue;
        };
        let Some(subject) = subject_from_expression(name, source)
            .or_else(|| subject_from_text(compact_node_text(name, source)))
        else {
            continue;
        };
        links.push(LocalLink::new(
            LocalLinkKind::Parameter,
            subject,
            function_subject.clone(),
            Anchor::line(path, line_start(name)),
            compact_node_text(parameter, source),
        ));
    }

    (!links.is_empty()).then_some(links)
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
            .or_else(|| first_identifier_like_descendant(node))?;
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

fn parameters_node(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(parameters) = node.child_by_field_name("parameters") {
        return Some(parameters);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "parameters" | "formal_parameters" | "parameter_list"
        ) {
            return Some(child);
        }
    }
    None
}

fn parameter_name_node(parameter: Node<'_>) -> Option<Node<'_>> {
    parameter
        .child_by_field_name("pattern")
        .and_then(first_identifier_like_descendant)
        .or_else(|| {
            parameter
                .child_by_field_name("name")
                .and_then(first_identifier_like_descendant)
        })
        .or_else(|| {
            parameter
                .child_by_field_name("declarator")
                .and_then(first_identifier_like_descendant)
        })
        .or_else(|| first_identifier_like_child(parameter))
}

fn first_identifier_like_descendant(node: Node<'_>) -> Option<Node<'_>> {
    if is_identifier_like(node.kind()) {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(identifier) = first_identifier_like_descendant(child) {
            return Some(identifier);
        }
    }
    None
}

fn first_identifier_like_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|child| is_identifier_like(child.kind()));
    found
}

fn is_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "property_identifier"
            | "field_identifier"
            | "simple_identifier"
            | "constant"
            | "self_parameter"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
    )
}

fn is_punctuation_or_type_only(kind: &str) -> bool {
    matches!(kind, "," | ":" | "type_identifier" | "primitive_type")
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

fn is_call_like(kind: &str, text: &str) -> bool {
    kind.contains("call")
        || kind.contains("invocation")
        || kind.contains("creation")
        || text.contains('(')
}

fn is_field_like(text: &str) -> bool {
    text.contains('.')
}

fn subject_from_text(text: impl Into<String>) -> Option<LocalSubject> {
    LocalSubject::new(text)
}

fn subject_from_expression(node: Node<'_>, source: &[u8]) -> Option<LocalSubject> {
    subject_from_expression_text(&compact_node_text(node, source))
}

fn subject_from_expression_text(text: &str) -> Option<LocalSubject> {
    let text = compact_text(text);
    let normalized = normalize_path_like_expression(&text)?;
    LocalSubject::new(normalized)
}

fn compact_node_text(node: Node<'_>, source: &[u8]) -> String {
    node.utf8_text(source).map(compact_text).unwrap_or_default()
}

fn compact_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_path_like_expression(expression: &str) -> Option<String> {
    let mut value = expression.trim();
    while let Some(stripped) = value.strip_prefix('&').or_else(|| value.strip_prefix('*')) {
        value = stripped.trim_start();
    }
    let normalized = value
        .replace(" .", ".")
        .replace(". ", ".")
        .replace("?.", ".")
        .replace(" ->", "->")
        .replace("-> ", "->")
        .replace("->", ".")
        .replace(" ::", "::")
        .replace(":: ", "::")
        .replace("::", ".");
    if normalized.is_empty()
        || normalized.contains('(')
        || normalized.contains(')')
        || normalized.contains('[')
        || normalized.contains(']')
        || normalized.contains('{')
        || normalized.contains('}')
        || normalized.contains(',')
        || normalized.contains(' ')
    {
        return None;
    }
    let mut parts = normalized.split('.');
    let root = parts.next()?;
    if !is_identifier_segment(root)
        || root.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        || parts.any(|part| !is_identifier_segment(part))
    {
        return None;
    }
    Some(normalized)
}

fn is_identifier_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
}

fn node_overlaps_span(node: Node<'_>, start_line: u32, end_line: u32) -> bool {
    let node_start = node.start_position().row as u32 + 1;
    let node_end = node.end_position().row as u32 + 1;
    node_start <= end_line && node_end >= start_line
}

fn line_start(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn line_end(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}

#[cfg(test)]
mod tests;
