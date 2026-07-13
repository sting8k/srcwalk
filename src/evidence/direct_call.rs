use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::evidence::Anchor;
use crate::lang::outline::get_outline_entries;
use crate::search::callees::CallSite;
use crate::types::{FileType, Lang, OutlineEntry, OutlineKind};

const MAX_DIRECT_CALL_EDGES: usize = 32;
const MAX_DIRECT_CALL_UNKNOWNS: usize = 16;
const MAX_RELATED_FILES: usize = 20;
const MAX_UNKNOWN_CANDIDATES: usize = 5;
const MAX_ARG_PARAM_MAPPINGS_PER_EDGE: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectCallEvidenceIndex {
    edges: Vec<DirectCallEvidenceEdge>,
    unknowns: Vec<DirectCallUnknown>,
    omitted_edges: usize,
    omitted_unknowns: usize,
    omitted_related_files: usize,
}

impl DirectCallEvidenceIndex {
    pub(crate) fn edges(&self) -> &[DirectCallEvidenceEdge] {
        &self.edges
    }

    #[cfg(test)]
    pub(crate) fn unknowns(&self) -> &[DirectCallUnknown] {
        &self.unknowns
    }

    pub(crate) const fn omitted_edges(&self) -> usize {
        self.omitted_edges
    }

    pub(crate) const fn omitted_unknowns(&self) -> usize {
        self.omitted_unknowns
    }

    pub(crate) const fn omitted_related_files(&self) -> usize {
        self.omitted_related_files
    }

    pub(crate) fn edge_for_site(
        &self,
        site: &CallSite,
        content: &str,
    ) -> Option<&DirectCallEvidenceEdge> {
        let display = call_site_display(site, content)?;
        self.edges.iter().find(|edge| {
            edge.call_anchor().start_line() == site.line
                && edge.call_callee() == site.callee
                && edge.call_display() == display
        })
    }

    pub(crate) fn unknown_for_site(
        &self,
        site: &CallSite,
        content: &str,
    ) -> Option<&DirectCallUnknown> {
        let display = call_site_display(site, content)?;
        self.unknowns.iter().find(|unknown| {
            unknown.call_anchor().start_line() == site.line
                && unknown.call_callee() == site.callee
                && unknown.call_display() == display
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectCallEvidenceEdge {
    call_anchor: Anchor,
    call_callee: String,
    call_display: String,
    target_name: String,
    target_anchor: Anchor,
    confidence: DirectCallResolutionConfidence,
    arg_param_mappings: Vec<ArgParamMapping>,
    omitted_arg_param_mappings: usize,
    mapping_unknown: Option<ArgParamMappingUnknownReason>,
}

impl DirectCallEvidenceEdge {
    pub(crate) const fn call_anchor(&self) -> &Anchor {
        &self.call_anchor
    }

    pub(crate) fn call_callee(&self) -> &str {
        &self.call_callee
    }

    pub(crate) fn call_display(&self) -> &str {
        &self.call_display
    }

    pub(crate) fn target_name(&self) -> &str {
        &self.target_name
    }

    pub(crate) const fn target_anchor(&self) -> &Anchor {
        &self.target_anchor
    }

    pub(crate) const fn confidence(&self) -> DirectCallResolutionConfidence {
        self.confidence
    }

    pub(crate) fn arg_param_mappings(&self) -> &[ArgParamMapping] {
        &self.arg_param_mappings
    }

    pub(crate) const fn omitted_arg_param_mappings(&self) -> usize {
        self.omitted_arg_param_mappings
    }

    pub(crate) const fn mapping_unknown(&self) -> Option<ArgParamMappingUnknownReason> {
        self.mapping_unknown
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArgParamMapping {
    arg_index: usize,
    arg_display: String,
    param_index: usize,
    param_name: String,
}

impl ArgParamMapping {
    pub(crate) const fn arg_index(&self) -> usize {
        self.arg_index
    }

    pub(crate) fn arg_display(&self) -> &str {
        &self.arg_display
    }

    pub(crate) const fn param_index(&self) -> usize {
        self.param_index
    }

    pub(crate) fn param_name(&self) -> &str {
        &self.param_name
    }

    pub(crate) const fn confidence() -> &'static str {
        "syntactic positional arg/param"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArgParamMappingUnknownReason {
    UnreliableSignature,
    NonPositionalArguments,
    ArityMismatch,
}

impl ArgParamMappingUnknownReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UnreliableSignature => "callee signature is not structurally reliable",
            Self::NonPositionalArguments => {
                "named, spread, splat, or otherwise non-positional arguments"
            }
            Self::ArityMismatch => "argument and parameter counts differ",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectCallResolutionConfidence {
    SameFileStructural,
    ExplicitRelatedFileStructural,
}

impl DirectCallResolutionConfidence {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SameFileStructural => "same-file structural direct call",
            Self::ExplicitRelatedFileStructural => "explicit related-file structural direct call",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectCallUnknown {
    call_anchor: Anchor,
    call_callee: String,
    call_display: String,
    reason: DirectCallUnknownReason,
    candidates: Vec<Anchor>,
    omitted_candidates: usize,
}

impl DirectCallUnknown {
    pub(crate) const fn call_anchor(&self) -> &Anchor {
        &self.call_anchor
    }

    pub(crate) fn call_callee(&self) -> &str {
        &self.call_callee
    }

    pub(crate) fn call_display(&self) -> &str {
        &self.call_display
    }

    pub(crate) const fn reason(&self) -> DirectCallUnknownReason {
        self.reason
    }

    pub(crate) fn candidates(&self) -> &[Anchor] {
        &self.candidates
    }

    pub(crate) const fn omitted_candidates(&self) -> usize {
        self.omitted_candidates
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DirectCallUnknownReason {
    AmbiguousTarget,
    SelfRecursiveCall,
}

impl DirectCallUnknownReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguousTarget => "ambiguous structural target",
            Self::SelfRecursiveCall => "self-recursive call omitted from one-hop evidence",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectCallTarget {
    name: String,
    path: PathBuf,
    start_line: u32,
    end_line: u32,
    signature: Option<String>,
    confidence: DirectCallResolutionConfidence,
}

pub(crate) fn build_direct_call_evidence_index(
    source_path: &Path,
    content: &str,
    lang: Lang,
    caller_range: Option<(u32, u32)>,
    sites: &[CallSite],
) -> DirectCallEvidenceIndex {
    if sites.is_empty() {
        return DirectCallEvidenceIndex::default();
    }

    let names = sites
        .iter()
        .map(|site| site.callee.as_str())
        .collect::<BTreeSet<_>>();
    let same_file = collect_targets(
        source_path,
        &get_outline_entries(content, lang),
        &names,
        DirectCallResolutionConfidence::SameFileStructural,
    );

    let related_paths =
        crate::read::imports::resolve_all_related_files_with_content(source_path, content);
    let omitted_related_files = related_paths.len().saturating_sub(MAX_RELATED_FILES);
    let mut related = Vec::new();
    for related_path in related_paths.into_iter().take(MAX_RELATED_FILES) {
        let Ok(related_content) = std::fs::read_to_string(&related_path) else {
            continue;
        };
        let FileType::Code(related_lang) = crate::lang::detect_file_type(&related_path) else {
            continue;
        };
        related.extend(collect_targets(
            &related_path,
            &get_outline_entries(&related_content, related_lang),
            &names,
            DirectCallResolutionConfidence::ExplicitRelatedFileStructural,
        ));
    }

    let mut edges = Vec::new();
    let mut unknowns = Vec::new();
    for site in sites {
        let Some(call_display) = call_site_display(site, content) else {
            continue;
        };
        if site
            .call_prefix
            .as_deref()
            .is_some_and(|prefix| prefix.trim() != site.callee)
        {
            continue;
        }
        let call_anchor = Anchor::line(source_path, site.line);
        let mut candidates = matching_targets(&same_file, &site.callee);
        candidates.extend(matching_targets(&related, &site.callee));

        let target = match candidates.as_slice() {
            [target] => *target,
            [] => continue,
            candidates => {
                let (anchors, omitted_candidates) = capped_candidate_anchors(candidates);
                unknowns.push(DirectCallUnknown {
                    call_anchor,
                    call_callee: site.callee.clone(),
                    call_display,
                    reason: DirectCallUnknownReason::AmbiguousTarget,
                    candidates: anchors,
                    omitted_candidates,
                });
                continue;
            }
        };

        if caller_range.is_some_and(|(start, end)| {
            same_path(source_path, &target.path)
                && start == target.start_line
                && end == target.end_line
        }) {
            unknowns.push(DirectCallUnknown {
                call_anchor,
                call_callee: site.callee.clone(),
                call_display,
                reason: DirectCallUnknownReason::SelfRecursiveCall,
                candidates: vec![Anchor::lines(
                    &target.path,
                    target.start_line,
                    target.end_line,
                )],
                omitted_candidates: 0,
            });
            continue;
        }

        let (arg_param_mappings, omitted_arg_param_mappings, mapping_unknown) =
            arg_param_mappings(source_path, &site.args, target);
        edges.push(DirectCallEvidenceEdge {
            call_anchor,
            call_callee: site.callee.clone(),
            call_display,
            target_name: target.name.clone(),
            target_anchor: Anchor::lines(&target.path, target.start_line, target.end_line),
            confidence: target.confidence,
            arg_param_mappings,
            omitted_arg_param_mappings,
            mapping_unknown,
        });
    }

    edges.sort_by_key(edge_sort_key);
    edges.dedup_by(|left, right| edge_sort_key(left) == edge_sort_key(right));
    unknowns.sort_by_key(unknown_sort_key);
    unknowns.dedup_by(|left, right| unknown_sort_key(left) == unknown_sort_key(right));

    let omitted_edges = edges.len().saturating_sub(MAX_DIRECT_CALL_EDGES);
    edges.truncate(MAX_DIRECT_CALL_EDGES);
    let omitted_unknowns = unknowns.len().saturating_sub(MAX_DIRECT_CALL_UNKNOWNS);
    unknowns.truncate(MAX_DIRECT_CALL_UNKNOWNS);

    DirectCallEvidenceIndex {
        edges,
        unknowns,
        omitted_edges,
        omitted_unknowns,
        omitted_related_files,
    }
}

pub(crate) fn call_site_display(site: &CallSite, content: &str) -> Option<String> {
    let text = site
        .call_byte_range
        .and_then(|(start, end)| content.get(start..end))
        .unwrap_or(&site.call_text);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!compact.is_empty()).then_some(compact)
}

fn collect_targets(
    path: &Path,
    entries: &[OutlineEntry],
    names: &BTreeSet<&str>,
    confidence: DirectCallResolutionConfidence,
) -> Vec<DirectCallTarget> {
    fn visit(
        path: &Path,
        entries: &[OutlineEntry],
        names: &BTreeSet<&str>,
        confidence: DirectCallResolutionConfidence,
        targets: &mut BTreeMap<(String, u32, u32, String), DirectCallTarget>,
    ) {
        for entry in entries {
            if entry.kind == OutlineKind::Function && names.contains(entry.name.as_str()) {
                let target = DirectCallTarget {
                    name: entry.name.clone(),
                    path: path.to_path_buf(),
                    start_line: entry.start_line,
                    end_line: entry.end_line,
                    signature: entry.signature.clone(),
                    confidence,
                };
                targets.insert(target_sort_key(&target), target);
            }
            visit(path, &entry.children, names, confidence, targets);
        }
    }

    let mut targets = BTreeMap::new();
    visit(path, entries, names, confidence, &mut targets);
    targets.into_values().collect()
}

fn matching_targets<'a>(targets: &'a [DirectCallTarget], name: &str) -> Vec<&'a DirectCallTarget> {
    targets
        .iter()
        .filter(|target| target.name == name)
        .collect()
}

fn capped_candidate_anchors(candidates: &[&DirectCallTarget]) -> (Vec<Anchor>, usize) {
    let omitted = candidates.len().saturating_sub(MAX_UNKNOWN_CANDIDATES);
    let anchors = candidates
        .iter()
        .take(MAX_UNKNOWN_CANDIDATES)
        .map(|target| Anchor::lines(&target.path, target.start_line, target.end_line))
        .collect();
    (anchors, omitted)
}

fn arg_param_mappings(
    source_path: &Path,
    args: &[String],
    target: &DirectCallTarget,
) -> (
    Vec<ArgParamMapping>,
    usize,
    Option<ArgParamMappingUnknownReason>,
) {
    if args
        .iter()
        .any(|arg| !is_positional_call_arg(source_path, arg))
    {
        return (
            Vec::new(),
            0,
            Some(ArgParamMappingUnknownReason::NonPositionalArguments),
        );
    }

    let Some(signature) = target.signature.as_deref() else {
        return (
            Vec::new(),
            0,
            Some(ArgParamMappingUnknownReason::UnreliableSignature),
        );
    };
    let Some(params) = parse_function_parameters(&target.path, signature) else {
        return (
            Vec::new(),
            0,
            Some(ArgParamMappingUnknownReason::UnreliableSignature),
        );
    };
    if args.len() != params.len() {
        return (
            Vec::new(),
            0,
            Some(ArgParamMappingUnknownReason::ArityMismatch),
        );
    }

    let mut mappings = args
        .iter()
        .zip(params)
        .enumerate()
        .map(|(index, (arg, param_name))| ArgParamMapping {
            arg_index: index,
            arg_display: arg.clone(),
            param_index: index,
            param_name,
        })
        .collect::<Vec<_>>();
    let omitted = mappings
        .len()
        .saturating_sub(MAX_ARG_PARAM_MAPPINGS_PER_EDGE);
    mappings.truncate(MAX_ARG_PARAM_MAPPINGS_PER_EDGE);
    (mappings, omitted, None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignatureParameterStyle {
    NameFirst,
    NameLast,
    Conservative,
}

fn parse_function_parameters(path: &Path, signature: &str) -> Option<Vec<String>> {
    let style = signature_parameter_style(path);
    let (start, end) = parameter_range(signature, style)?;
    let params = signature[start + 1..end].trim();
    if params.is_empty() {
        return Some(Vec::new());
    }

    let parts = split_top_level_params(params)?;
    let mut parsed = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() || is_varargs_param(part) {
            return None;
        }
        let name = parameter_name(part, style)?;
        if is_receiver_param(&name) {
            if is_syntactic_receiver_param(path, part, style) {
                continue;
            }
            return None;
        }
        parsed.push(name);
    }
    Some(parsed)
}

fn signature_parameter_style(path: &Path) -> SignatureParameterStyle {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "go" => SignatureParameterStyle::NameFirst,
        "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" | "java" | "cs" => {
            SignatureParameterStyle::NameLast
        }
        _ => SignatureParameterStyle::Conservative,
    }
}

fn parameter_range(signature: &str, style: SignatureParameterStyle) -> Option<(usize, usize)> {
    let ranges = top_level_parenthesis_ranges(signature)?;
    if style == SignatureParameterStyle::NameFirst
        && signature.trim_start().starts_with("func (")
        && ranges.len() >= 2
    {
        return Some(ranges[1]);
    }
    ranges.first().copied()
}

fn top_level_parenthesis_ranges(signature: &str) -> Option<Vec<(usize, usize)>> {
    let mut ranges = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in signature.char_indices() {
        match ch {
            '(' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    ranges.push((start?, index));
                    start = None;
                }
            }
            _ => {}
        }
    }
    (depth == 0).then_some(ranges)
}

fn split_top_level_params(params: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in params.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                parts.push(&params[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    parts.push(&params[start..]);
    Some(parts)
}

fn is_varargs_param(param: &str) -> bool {
    let trimmed = param.trim_start();
    trimmed.contains("...") || trimmed.starts_with('*')
}

fn parameter_name(param: &str, style: SignatureParameterStyle) -> Option<String> {
    let before_default = param.split('=').next()?.trim();
    if before_default.eq_ignore_ascii_case("void") {
        return None;
    }
    if style != SignatureParameterStyle::NameLast && before_default.contains(':') {
        return colon_parameter_name(before_default);
    }
    match style {
        SignatureParameterStyle::NameFirst => first_named_parameter_identifier(before_default),
        SignatureParameterStyle::NameLast => last_named_parameter_identifier(before_default),
        SignatureParameterStyle::Conservative => conservative_parameter_name(before_default),
    }
}

fn colon_parameter_name(param: &str) -> Option<String> {
    let before_type = strip_binding_prefixes(param.split(':').next()?.trim());
    is_identifier(before_type).then(|| before_type.to_string())
}

fn first_named_parameter_identifier(value: &str) -> Option<String> {
    (value.split_whitespace().count() >= 2)
        .then(|| value.split_whitespace().find_map(identifier_from_token))?
}

fn last_named_parameter_identifier(value: &str) -> Option<String> {
    let tokens = value.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 || tokens.first().is_some_and(|token| *token == "this") {
        return None;
    }
    let name = identifier_from_token(tokens.last()?)?;
    if is_type_only_identifier(&name)
        || is_name_last_qualifier_or_tag(&name)
        || tokens[..tokens.len() - 1]
            .iter()
            .all(|token| is_name_last_qualifier_or_tag(token))
    {
        None
    } else {
        Some(name)
    }
}

fn is_name_last_qualifier_or_tag(value: &str) -> bool {
    matches!(
        value,
        "const"
            | "volatile"
            | "final"
            | "static"
            | "readonly"
            | "unsigned"
            | "signed"
            | "struct"
            | "enum"
            | "union"
            | "class"
    )
}

fn is_type_only_identifier(value: &str) -> bool {
    matches!(
        value,
        "char"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "void"
            | "bool"
            | "boolean"
            | "byte"
            | "string"
            | "String"
    )
}

fn conservative_parameter_name(value: &str) -> Option<String> {
    let value = strip_binding_prefixes(value);
    is_identifier(value).then(|| value.to_string())
}

fn strip_binding_prefixes(mut value: &str) -> &str {
    value = value.trim();
    for prefix in ["&mut ", "&", "mut ", "ref "] {
        if let Some(stripped) = value.strip_prefix(prefix) {
            return stripped.trim();
        }
    }
    value
}

fn identifier_from_token(token: &str) -> Option<String> {
    let token = token
        .trim()
        .trim_start_matches(['&', '*'])
        .split('[')
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| ch != '_' && !ch.is_ascii_alphanumeric());
    is_identifier(token).then(|| token.to_string())
}

fn is_syntactic_receiver_param(path: &Path, param: &str, style: SignatureParameterStyle) -> bool {
    if !is_rust_path(path)
        || matches!(
            style,
            SignatureParameterStyle::NameFirst | SignatureParameterStyle::NameLast
        )
    {
        return false;
    }
    let before_default = param.split('=').next().unwrap_or_default().trim();
    let before_type = before_default
        .split(':')
        .next()
        .unwrap_or(before_default)
        .trim();
    before_type == "self"
        || before_type == "mut self"
        || (before_type.starts_with('&') && strip_binding_prefixes(before_type) == "self")
}

fn is_rust_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
}

fn is_receiver_param(name: &str) -> bool {
    matches!(name, "self" | "this")
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_positional_call_arg(path: &Path, arg: &str) -> bool {
    let arg = arg.trim();
    if arg.is_empty() || arg.starts_with("...") || arg.starts_with("**") {
        return false;
    }
    if arg.starts_with('*') && is_python_or_ruby_splat_path(path) {
        return false;
    }
    !arg.contains('=') && !arg.contains(':')
}

fn is_python_or_ruby_splat_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "py" | "pyi" | "rb"))
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "Rakefile" | "Vagrantfile"))
}

fn same_path(left: &Path, right: &Path) -> bool {
    crate::format::display_path(left) == crate::format::display_path(right)
}

fn target_sort_key(target: &DirectCallTarget) -> (String, u32, u32, String) {
    (
        crate::format::display_path(&target.path),
        target.start_line,
        target.end_line,
        target.name.clone(),
    )
}

fn edge_sort_key(edge: &DirectCallEvidenceEdge) -> (u32, String, String, String) {
    (
        edge.call_anchor.start_line(),
        edge.call_callee.clone(),
        edge.target_anchor.display(),
        edge.call_display.clone(),
    )
}

fn unknown_sort_key(unknown: &DirectCallUnknown) -> (u32, String, DirectCallUnknownReason, String) {
    (
        unknown.call_anchor.start_line(),
        unknown.call_callee.clone(),
        unknown.reason,
        unknown.call_display.clone(),
    )
}

#[cfg(test)]
#[path = "direct_call/tests.rs"]
mod tests;
