use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::budget;
use crate::cache::OutlineCache;
use crate::commands::decision_flow::resolve_decision_flow_target;
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, render_next_actions, Anchor, EvidenceAtom, EvidenceKind, EvidenceRole,
    EvidenceSource, NextAction,
};
use crate::format;
use crate::lang::outline::outline_language;
use crate::lang::{self, decision_flow, decision_flow::FlowTarget};
use crate::search;
use crate::types::{estimate_tokens, FileType, Lang};

const MAX_SHARED_PER_GROUP: usize = 8;
const MAX_ONLY_FEATURES: usize = 16;
const MAX_SNIPPET_CHARS: usize = 120;

pub(crate) fn run_compare(
    target_a: &str,
    target_b: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    _cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let left = resolve_compare_target(target_a, scope)?;
    let right = resolve_compare_target(target_b, scope)?;
    let comparison = compare_features(&left.features, &right.features);

    let mut out = String::new();
    let confidence = confidence_label_for(EvidenceSource::Ast);
    let _ = writeln!(out, "# Compare: {target_a} <> {target_b}");
    let _ = writeln!(out, "confidence: {confidence}");
    out.push_str(
        "caveat: structural comparison only; not equivalence, runtime, dataflow, ownership, or correctness proof\n\n",
    );
    append_targets(&mut out, &left, &right);
    append_metrics(&mut out, &left, &right, &comparison);
    append_shared_sections(&mut out, &comparison);
    append_only_section(&mut out, "only in A", &comparison.only_left);
    append_only_section(&mut out, "only in B", &comparison.only_right);
    append_footer(&mut out, &left, &right, scope);
    append_token_footer(&mut out);

    Ok(match budget_tokens {
        Some(budget) => budget::apply_preserving_footer(&out, budget),
        None => out,
    })
}

struct CompareTarget {
    original: String,
    path: PathBuf,
    display_path: String,
    display_target: String,
    start_line: u32,
    end_line: u32,
    features: Vec<CompareFeature>,
}

#[derive(Clone, Debug)]
struct CompareFeature {
    group: FeatureGroup,
    key: String,
    label: String,
    atom: EvidenceAtom,
}

impl CompareFeature {
    const fn line(&self) -> u32 {
        self.atom.anchor().start_line()
    }

    fn snippet(&self) -> &str {
        self.atom.snippet()
    }

    const fn role(&self) -> Option<EvidenceRole> {
        self.atom.role()
    }

    const fn kind(&self) -> EvidenceKind {
        self.atom.kind()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum FeatureGroup {
    Access,
    Call,
    Condition,
    Return,
}

struct Comparison<'a> {
    shared_by_group: BTreeMap<FeatureGroup, Vec<SharedFeature<'a>>>,
    only_left: Vec<&'a CompareFeature>,
    only_right: Vec<&'a CompareFeature>,
    shared_count: usize,
}

struct SharedFeature<'a> {
    left: &'a CompareFeature,
    right: &'a CompareFeature,
}

fn resolve_compare_target(target: &str, scope: &Path) -> Result<CompareTarget, SrcwalkError> {
    let resolved = resolve_decision_flow_target(target, scope)?;
    let source =
        std::fs::read_to_string(&resolved.path).map_err(|source| SrcwalkError::IoError {
            path: resolved.path.clone(),
            source,
        })?;
    let FileType::Code(lang) = lang::detect_file_type(&resolved.path) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "compare requires a source code file".to_string(),
        });
    };
    if !decision_flow::is_supported_flow_target_lang(lang) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!("compare does not support {lang:?} targets yet"),
        });
    }

    let Some(ts_lang) = outline_language(lang) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!(
                "compare requires tree-sitter source support; {lang:?} is not supported"
            ),
        });
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&ts_lang)
        .map_err(|e| SrcwalkError::ParseError {
            path: resolved.path.clone(),
            reason: format!("failed to initialize tree-sitter parser: {e}"),
        })?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| SrcwalkError::ParseError {
            path: resolved.path.clone(),
            reason: "tree-sitter parser returned no tree".to_string(),
        })?;
    let root = tree.root_node();
    let function =
        decision_flow::find_flow_target_function(root, &source, lang, &resolved.selector)
            .ok_or_else(|| SrcwalkError::InvalidQuery {
                query: target.to_string(),
                reason: "target did not resolve to a supported function-like source node"
                    .to_string(),
            })?;

    let start_line = line_start(function);
    let end_line = line_end(function);
    let display_path = format::display_path(&resolved.path);
    let display_target = format_display_target(&resolved, &display_path);
    let mut features = extract_features(function, &resolved.path, &source, lang);
    sort_features(&mut features);

    Ok(CompareTarget {
        original: target.to_string(),
        path: resolved.path,
        display_path,
        display_target,
        start_line,
        end_line,
        features,
    })
}

fn format_display_target(resolved: &FlowTarget, display_path: &str) -> String {
    if resolved.display_target.contains(':') {
        resolved.display_target.clone()
    } else {
        format!("{display_path}:{}", resolved.display_target)
    }
}

fn extract_features(
    function: Node<'_>,
    path: &Path,
    source: &str,
    lang: Lang,
) -> Vec<CompareFeature> {
    let mut features = Vec::new();
    let range = Some((line_start(function), line_end(function)));
    for site in search::callees::extract_call_sites(source, lang, range) {
        let snippet = compact_snippet(&site.call_text);
        features.push(CompareFeature {
            group: FeatureGroup::Call,
            key: format!("call:{}", site.callee),
            label: site.callee,
            atom: EvidenceAtom::new(
                EvidenceKind::Call,
                None,
                Anchor::line(path, site.line),
                snippet,
                EvidenceSource::Ast,
            ),
        });
    }
    collect_structural_features(function, function, path, source, &mut features);
    features
}

fn collect_structural_features(
    node: Node<'_>,
    root: Node<'_>,
    path: &Path,
    source: &str,
    features: &mut Vec<CompareFeature>,
) {
    if node.id() != root.id() && is_function_like(node.kind()) {
        return;
    }

    if is_control_node(node.kind()) {
        if let Some(condition) = condition_node(node) {
            let text = compact_node_text(condition, source);
            if !text.is_empty() {
                let line = line_start(condition);
                features.push(CompareFeature {
                    group: FeatureGroup::Condition,
                    key: format!("condition:{text}"),
                    label: text,
                    atom: EvidenceAtom::new(
                        EvidenceKind::Condition,
                        Some(EvidenceRole::Condition),
                        Anchor::line(path, line),
                        line_snippet(source, line),
                        EvidenceSource::Ast,
                    ),
                });
            }
        }
    }

    if is_return_node(node.kind()) {
        let text = compact_node_text(node, source);
        if !text.is_empty() {
            let line = line_start(node);
            features.push(CompareFeature {
                group: FeatureGroup::Return,
                key: format!("return:{text}"),
                label: text,
                atom: EvidenceAtom::new(
                    EvidenceKind::Return,
                    Some(EvidenceRole::Return),
                    Anchor::line(path, line),
                    line_snippet(source, line),
                    EvidenceSource::Ast,
                ),
            });
        }
    }

    if is_access_expression(node.kind()) {
        let text = compact_node_text(node, source);
        if !text.is_empty() {
            let kind = classify_access(node, source);
            let role = classify_access_role(node, kind);
            let member = access_member_key(&text);
            let line = line_start(node);
            features.push(CompareFeature {
                group: FeatureGroup::Access,
                key: format!(
                    "access:{}:{}:{}",
                    kind.as_str(),
                    role.map_or("unknown", EvidenceRole::as_str),
                    member
                ),
                label: member,
                atom: EvidenceAtom::new(
                    kind,
                    role,
                    Anchor::line(path, line),
                    line_snippet(source, line),
                    EvidenceSource::Ast,
                ),
            });
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_structural_features(child, root, path, source, features);
    }
}

fn compare_features<'a>(left: &'a [CompareFeature], right: &'a [CompareFeature]) -> Comparison<'a> {
    let left_by_key = features_by_key(left);
    let right_by_key = features_by_key(right);

    let mut shared_by_group: BTreeMap<FeatureGroup, Vec<SharedFeature<'a>>> = BTreeMap::new();
    let mut only_left = Vec::new();
    let mut only_right = Vec::new();
    let mut shared_count = 0;

    for (key, left_features) in &left_by_key {
        let Some(right_features) = right_by_key.get(*key) else {
            only_left.extend(left_features.iter().copied());
            continue;
        };

        let pair_count = left_features.len().min(right_features.len());
        shared_count += pair_count;
        for index in 0..pair_count {
            let left = left_features[index];
            let right = right_features[index];
            shared_by_group
                .entry(left.group)
                .or_default()
                .push(SharedFeature { left, right });
        }
        only_left.extend(left_features[pair_count..].iter().copied());
        only_right.extend(right_features[pair_count..].iter().copied());
    }

    for (key, right_features) in &right_by_key {
        if !left_by_key.contains_key(*key) {
            only_right.extend(right_features.iter().copied());
        }
    }

    Comparison {
        shared_by_group,
        only_left,
        only_right,
        shared_count,
    }
}

fn features_by_key(features: &[CompareFeature]) -> BTreeMap<&str, Vec<&CompareFeature>> {
    let mut by_key = BTreeMap::new();
    for feature in features {
        by_key
            .entry(feature.key.as_str())
            .or_insert_with(Vec::new)
            .push(feature);
    }
    by_key
}

fn sort_features(features: &mut [CompareFeature]) {
    features.sort_by(|a, b| {
        (a.group, &a.key, a.line(), a.snippet()).cmp(&(b.group, &b.key, b.line(), b.snippet()))
    });
}

fn append_targets(out: &mut String, left: &CompareTarget, right: &CompareTarget) {
    out.push_str("targets:\n");
    let _ = writeln!(
        out,
        "A {} :{}{}",
        left.display_target,
        left.start_line,
        range_suffix(left.start_line, left.end_line)
    );
    let _ = writeln!(
        out,
        "B {} :{}{}\n",
        right.display_target,
        right.start_line,
        range_suffix(right.start_line, right.end_line)
    );
}

fn append_metrics(
    out: &mut String,
    left: &CompareTarget,
    right: &CompareTarget,
    comparison: &Comparison<'_>,
) {
    let _ = writeln!(
        out,
        "metrics: A features={} B features={} shared={} only_A={} only_B={}\n",
        left.features.len(),
        right.features.len(),
        comparison.shared_count,
        comparison.only_left.len(),
        comparison.only_right.len()
    );
}

fn append_shared_sections(out: &mut String, comparison: &Comparison<'_>) {
    for (group, features) in &comparison.shared_by_group {
        let _ = writeln!(out, "{}:", shared_group_heading(*group));
        for shared in features.iter().take(MAX_SHARED_PER_GROUP) {
            append_shared_feature(out, shared);
        }
        if features.len() > MAX_SHARED_PER_GROUP {
            let _ = writeln!(
                out,
                "- omitted: {} more {}",
                features.len() - MAX_SHARED_PER_GROUP,
                shared_group_heading(*group).trim_end_matches(':')
            );
        }
        out.push('\n');
    }
}

fn append_shared_feature(out: &mut String, shared: &SharedFeature<'_>) {
    let _ = writeln!(out, "- {}", feature_title(shared.left));
    let _ = writeln!(
        out,
        "  A :{} | {}",
        shared.left.line(),
        shared.left.snippet()
    );
    let _ = writeln!(
        out,
        "  B :{} | {}",
        shared.right.line(),
        shared.right.snippet()
    );
}

fn append_only_section(out: &mut String, heading: &str, features: &[&CompareFeature]) {
    let _ = writeln!(out, "{heading}:");
    if features.is_empty() {
        out.push_str("- none\n\n");
        return;
    }
    for feature in features.iter().take(MAX_ONLY_FEATURES) {
        let _ = writeln!(out, "- {}", feature_title(feature));
        let _ = writeln!(out, "  :{} | {}", feature.line(), feature.snippet());
    }
    if features.len() > MAX_ONLY_FEATURES {
        let _ = writeln!(
            out,
            "- omitted: {} more features",
            features.len() - MAX_ONLY_FEATURES
        );
    }
    out.push('\n');
}

fn append_footer(out: &mut String, left: &CompareTarget, right: &CompareTarget, scope: &Path) {
    let left_anchor = Anchor::lines(&left.path, left.start_line, left.end_line);
    let right_anchor = Anchor::lines(&right.path, right.start_line, right.end_line);
    let mut actions = vec![
        NextAction::from_evidence(
            format!("srcwalk context {}", left.original),
            "inspect left comparison target context",
            10,
            EvidenceSource::Ast,
            left_anchor.clone(),
        ),
        NextAction::from_evidence(
            format!("srcwalk context {}", right.original),
            "inspect right comparison target context",
            20,
            EvidenceSource::Ast,
            right_anchor.clone(),
        ),
        NextAction::from_evidence(
            format!(
                "srcwalk show {}:{}{}",
                left.display_path,
                left.start_line,
                range_suffix(left.start_line, left.end_line)
            ),
            "read left comparison source range",
            30,
            EvidenceSource::Ast,
            left_anchor.clone(),
        ),
        NextAction::from_evidence(
            format!(
                "srcwalk show {}:{}{}",
                right.display_path,
                right.start_line,
                range_suffix(right.start_line, right.end_line)
            ),
            "read right comparison source range",
            40,
            EvidenceSource::Ast,
            right_anchor.clone(),
        ),
    ];

    let shared_access = shared_access_labels(left, right);
    if let Some(label) = shared_access.first() {
        actions.push(NextAction::from_evidence(
            format!(
                "srcwalk discover {} --as access --scope {}",
                label,
                format::display_path(scope)
            ),
            "inspect shared access evidence",
            50,
            EvidenceSource::Ast,
            left_anchor,
        ));
    }

    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        out.push_str(&rendered);
        out.push('\n');
    }
}

fn append_token_footer(out: &mut String) {
    let tokens = estimate_tokens(out.len() as u64);
    let _ = write!(out, "\n(~{tokens} tokens)");
}

fn shared_access_labels(left: &CompareTarget, right: &CompareTarget) -> Vec<String> {
    let left_access: BTreeSet<&str> = left
        .features
        .iter()
        .filter(|feature| feature.group == FeatureGroup::Access)
        .map(|feature| feature.label.as_str())
        .collect();
    let right_access: BTreeSet<&str> = right
        .features
        .iter()
        .filter(|feature| feature.group == FeatureGroup::Access)
        .map(|feature| feature.label.as_str())
        .collect();
    left_access
        .intersection(&right_access)
        .map(|label| (*label).to_string())
        .collect()
}

fn feature_title(feature: &CompareFeature) -> String {
    match feature.group {
        FeatureGroup::Access => format!(
            "{} {} {}",
            feature.label,
            feature.kind().as_str(),
            feature.role().map_or("expression", EvidenceRole::as_str)
        ),
        FeatureGroup::Call | FeatureGroup::Condition => {
            format!("{} {}", feature.kind().as_str(), feature.label)
        }
        FeatureGroup::Return => format!(
            "{} {}",
            feature.kind().as_str(),
            feature
                .label
                .strip_prefix("return ")
                .unwrap_or(feature.label.as_str())
        ),
    }
}

fn shared_group_heading(group: FeatureGroup) -> &'static str {
    match group {
        FeatureGroup::Access => "shared field access",
        FeatureGroup::Call => "shared calls",
        FeatureGroup::Condition => "shared conditions",
        FeatureGroup::Return => "shared returns",
    }
}

fn range_suffix(start: u32, end: u32) -> String {
    if end > start {
        format!("-{end}")
    } else {
        String::new()
    }
}

fn classify_access(access: Node<'_>, source: &str) -> EvidenceKind {
    if is_address_taken(access, source) {
        return EvidenceKind::UnknownAccess;
    }

    let mut current = access;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "assignment" | "assignment_expression" | "assignment_statement"
                if is_assignment_left(parent, access) =>
            {
                return assignment_right(parent)
                    .filter(|right| is_zero_like(*right, source))
                    .map_or(EvidenceKind::Write, |_| EvidenceKind::Reset);
            }
            "augmented_assignment_expression" | "augmented_assignment_statement"
                if is_assignment_left(parent, access) =>
            {
                return EvidenceKind::Write;
            }
            "update_expression" | "inc_statement" | "dec_statement" => {
                return EvidenceKind::Write;
            }
            kind if is_function_like(kind) || kind.contains("declaration") => break,
            _ => {}
        }
        current = parent;
    }

    EvidenceKind::Read
}

fn classify_access_role(access: Node<'_>, kind: EvidenceKind) -> Option<EvidenceRole> {
    if matches!(kind, EvidenceKind::Write | EvidenceKind::Reset) {
        return Some(EvidenceRole::AssignmentLhs);
    }

    let mut current = access;
    while let Some(parent) = current.parent() {
        let parent_kind = parent.kind();
        if is_function_like(parent_kind) {
            break;
        }
        if is_condition_context(parent, access) {
            return Some(EvidenceRole::Condition);
        }
        if is_call_argument_context(parent, access) {
            return Some(EvidenceRole::CallArg);
        }
        if is_assignment_right(parent, access) {
            return Some(EvidenceRole::AssignmentRhs);
        }
        if is_return_node(parent_kind) {
            return Some(EvidenceRole::Return);
        }
        if is_receiver_context(parent, current) {
            return Some(EvidenceRole::Receiver);
        }
        current = parent;
    }

    Some(EvidenceRole::Expression)
}

fn is_condition_context(parent: Node<'_>, access: Node<'_>) -> bool {
    is_control_node(parent.kind())
        && condition_node(parent).is_some_and(|condition| contains_node(condition, access))
}

fn is_call_argument_context(parent: Node<'_>, access: Node<'_>) -> bool {
    match parent.kind() {
        "argument_list" | "arguments" | "value_arguments" | "call_arguments" => true,
        "call_expression" | "invocation_expression" | "call" => parent
            .child_by_field_name("arguments")
            .is_some_and(|args| contains_node(args, access)),
        _ => false,
    }
}

fn is_assignment_right(parent: Node<'_>, access: Node<'_>) -> bool {
    matches!(
        parent.kind(),
        "assignment" | "assignment_expression" | "assignment_statement"
    ) && !is_assignment_left(parent, access)
        && assignment_right(parent).is_some_and(|right| contains_node(right, access))
}

fn is_assignment_left(parent: Node<'_>, access: Node<'_>) -> bool {
    parent
        .child_by_field_name("left")
        .or_else(|| first_named_child(parent))
        .is_some_and(|left| is_write_target_path(left, access))
}

fn is_write_target_path(left: Node<'_>, access: Node<'_>) -> bool {
    if !contains_node(left, access) {
        return false;
    }

    let mut current = access;
    loop {
        if same_node(current, left) {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if same_node(parent, left) {
            return true;
        }
        if !contains_node(left, parent) || !is_write_target_wrapper(parent, current) {
            return false;
        }
        current = parent;
    }
}

fn is_write_target_wrapper(parent: Node<'_>, child: Node<'_>) -> bool {
    match parent.kind() {
        kind if is_access_expression(kind) => true,
        "subscript_expression" | "index_expression" | "element_access_expression" => {
            is_first_named_child(parent, child)
        }
        "parenthesized_expression"
        | "directly_assignable_expression"
        | "unary_expression"
        | "pointer_expression" => true,
        _ => false,
    }
}

fn is_receiver_context(parent: Node<'_>, current: Node<'_>) -> bool {
    is_access_expression(parent.kind()) && is_first_named_child(parent, current)
}

fn assignment_right(parent: Node<'_>) -> Option<Node<'_>> {
    parent.child_by_field_name("right").or_else(|| {
        let count = parent.named_child_count();
        (count >= 2)
            .then(|| parent.named_child(count - 1))
            .flatten()
    })
}

fn first_named_child(parent: Node<'_>) -> Option<Node<'_>> {
    (parent.named_child_count() > 0)
        .then(|| parent.named_child(0))
        .flatten()
}

fn is_first_named_child(parent: Node<'_>, child: Node<'_>) -> bool {
    first_named_child(parent).is_some_and(|first| same_node(first, child))
}

fn same_node(a: Node<'_>, b: Node<'_>) -> bool {
    a.kind() == b.kind() && a.start_byte() == b.start_byte() && a.end_byte() == b.end_byte()
}

fn contains_node(parent: Node<'_>, child: Node<'_>) -> bool {
    parent.start_byte() <= child.start_byte() && child.end_byte() <= parent.end_byte()
}

fn is_address_taken(access: Node<'_>, source: &str) -> bool {
    access.parent().is_some_and(|parent| {
        parent.kind() == "unary_expression"
            && parent
                .utf8_text(source.as_bytes())
                .is_ok_and(|text| text.trim_start().starts_with('&'))
    })
}

fn is_zero_like(node: Node<'_>, source: &str) -> bool {
    let Ok(text) = node.utf8_text(source.as_bytes()) else {
        return false;
    };
    let value = strip_wrapping_parens(text.trim().trim_end_matches(';')).to_ascii_lowercase();
    matches!(
        value.as_str(),
        "0" | "false" | "null" | "nullptr" | "nil" | "none"
    )
}

fn strip_wrapping_parens(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('(') && trimmed.ends_with(')') {
            value = &trimmed[1..trimmed.len() - 1];
        } else {
            return trimmed;
        }
    }
}

fn is_control_node(kind: &str) -> bool {
    matches!(
        kind,
        "if_expression"
            | "if_statement"
            | "elif_clause"
            | "while_expression"
            | "while_statement"
            | "for_expression"
            | "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "match_expression"
            | "match_statement"
            | "switch_statement"
            | "switch_expression"
            | "expression_switch_statement"
            | "type_switch_statement"
    )
}

fn condition_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("condition")
        .or_else(|| node.child_by_field_name("value"))
}

fn is_return_node(kind: &str) -> bool {
    matches!(kind, "return_expression" | "return_statement")
}

fn is_function_like(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_item"
            | "function_declarator"
            | "function_body_declaration"
            | "method"
            | "method_declaration"
            | "method_definition"
            | "function"
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

fn access_member_key(text: &str) -> String {
    text.rsplit(['.', '>', ':'])
        .next()
        .unwrap_or(text)
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string()
}

fn compact_node_text(node: Node<'_>, source: &str) -> String {
    let text = node.utf8_text(source.as_bytes()).unwrap_or("");
    compact_snippet(text)
}

fn line_snippet(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .map(compact_snippet)
        .unwrap_or_default()
}

fn compact_snippet(text: &str) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= MAX_SNIPPET_CHARS {
        return cleaned;
    }
    let mut truncated = cleaned
        .chars()
        .take(MAX_SNIPPET_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn line_start(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn line_end(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}
