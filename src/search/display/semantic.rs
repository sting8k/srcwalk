use std::fmt::Write;
use std::fs;
use std::path::Path;

use super::{extract_line_range, get_outline_str};
use crate::cache::OutlineCache;
use crate::format::rel_nonempty;
use crate::types::{Match, OutlineEntry, OutlineKind};

pub(super) fn enclosing_fn_name(path: &Path, line: u32, cache: &OutlineCache) -> Option<String> {
    let outline_str = get_outline_str(path, cache)?;
    let mut best: Option<(&str, u32, u32)> = None;
    for ol in outline_str.lines() {
        if let Some((s, e)) = extract_line_range(ol) {
            if line >= s && line <= e {
                // Pick tightest enclosing range
                if best.is_none() || (e - s) < (best.unwrap().2 - best.unwrap().1) {
                    best = Some((ol, s, e));
                }
            }
        }
    }
    let entry = best?.0.trim();
    // Outline lines look like "  [45-79]      fn foo_bar"
    entry.split_whitespace().last().map(String::from)
}

#[derive(Debug, Clone)]
pub(in crate::search) struct SemanticCandidate {
    pub(in crate::search) kind: OutlineKind,
    pub(in crate::search) name: String,
    pub(in crate::search) start_line: u32,
    pub(in crate::search) end_line: u32,
    pub(in crate::search) parents: Vec<String>,
    pub(in crate::search) children: Vec<SemanticChild>,
}

#[derive(Debug, Clone)]
pub(in crate::search) struct SemanticChild {
    pub(in crate::search) kind: OutlineKind,
    pub(in crate::search) name: String,
    pub(in crate::search) start_line: u32,
    pub(in crate::search) end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ContextTarget {
    pub(super) start_line: u32,
    pub(super) end_line: u32,
    /// Parser-backed symbol selector (bare name or `Type.method`) that resolves
    /// back to this target's range, or the closest stable selector when
    /// `symbol_backed` is false.
    pub(super) selector: String,
    /// True when `selector` round-trips to exactly `(start_line, end_line)` in
    /// this file, so the `show path --section <selector>` footer is safe to
    /// emit. False targets fall back to a numeric range command.
    pub(super) symbol_backed: bool,
}

pub(super) fn context_target_for_match(m: &Match, cache: &OutlineCache) -> Option<ContextTarget> {
    if m.in_comment || crate::artifact::should_auto_artifact_file(&m.path) {
        return None;
    }
    let crate::types::FileType::Code(lang) = crate::lang::detect_file_type(&m.path) else {
        return None;
    };
    if !crate::lang::decision_flow::is_supported_flow_target_lang(lang) {
        return None;
    }

    let entries = structured_outline_entries(&m.path, cache)?;

    if m.is_definition {
        let range = m.def_range.unwrap_or((m.line, m.line));
        if let Some(candidate) = best_semantic_candidate(&entries, m) {
            if candidate.kind == OutlineKind::Function {
                // US-064: `m.def_name` retains the parser-backed `Q.N` form for
                // qualified matches, so the footer carries the same selector
                // the discover query used instead of reconstructing it.
                let selector = m.def_name.clone().unwrap_or_else(|| candidate.name.clone());
                return Some(build_target(
                    &m.path,
                    &entries,
                    candidate.start_line,
                    candidate.end_line,
                    selector,
                ));
            }
        }
        if let Some((start_line, end_line)) =
            find_function_definition_range(&entries, range, m.def_name.as_deref())
        {
            let selector = m.def_name.clone().unwrap_or_else(|| start_line.to_string());
            return Some(build_target(
                &m.path, &entries, start_line, end_line, selector,
            ));
        }
        return None;
    }

    if !m.exact {
        return None;
    }

    let candidate = best_enclosing_function(&entries, m.line)?;
    let selector = candidate.name.clone();
    Some(build_target(
        &m.path,
        &entries,
        candidate.start_line,
        candidate.end_line,
        selector,
    ))
}

/// Build a `ContextTarget` from a candidate range and parser-backed selector.
/// `symbol_backed` is true only when the selector resolves back to exactly this
/// range (round-trip check), so the footer never emits a symbol command that
/// would read a different body.
fn build_target(
    path: &Path,
    entries: &[OutlineEntry],
    start_line: u32,
    end_line: u32,
    selector: String,
) -> ContextTarget {
    let lang = crate::lang::detect_file_type(path).structural_lang();
    let symbol_backed = crate::lang::qualified::resolve_selector_first(entries, lang, &selector)
        == Some((start_line, end_line));
    ContextTarget {
        start_line,
        end_line,
        selector,
        symbol_backed,
    }
}

pub(super) fn format_definition_semantic_match(
    m: &Match,
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
) {
    let path = rel_nonempty(&m.path, scope);
    format_definition_semantic_match_with_path(m, Some(&path), cache, out, "  ", None);
}

/// Compact-facet variant: when `suppress` is the exact provenance tuple of the
/// entry, the per-entry provenance line is omitted (hoisted to the section
/// default computed by the caller).
pub(super) fn format_definition_semantic_match_suppressed(
    m: &Match,
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
    suppress: Option<&str>,
) {
    let path = rel_nonempty(&m.path, scope);
    format_definition_semantic_match_with_path(m, Some(&path), cache, out, "  ", suppress);
}

pub(super) fn format_definition_semantic_match_in_file_suppressed(
    m: &Match,
    cache: &OutlineCache,
    out: &mut String,
    suppress: Option<&str>,
) {
    format_definition_semantic_match_with_path(m, None, cache, out, "    ", suppress);
}

/// Compute the exact provenance tuple string `source · kind · confidence` for a
/// definition match, mirroring `append_match_provenance_with_kind`. Returns
/// `None` for relation/impl/base artifacts that do not print a provenance line.
pub(super) fn definition_provenance_tuple(m: &Match, cache: &OutlineCache) -> Option<String> {
    if m.impl_target.is_some() || m.base_target.is_some() {
        return None;
    }
    let source = super::match_evidence_source(m);
    let kind_override = if super::is_artifact_anchor_match(m) {
        Some("anchor")
    } else if matches!(
        crate::lang::detect_file_type(&m.path),
        crate::types::FileType::Document(_)
    ) && semantic_candidate_for_match(m, cache).is_some()
    {
        let candidate = semantic_candidate_for_match(m, cache).unwrap();
        super::document_outline_kind_label(candidate.kind)
    } else {
        None
    };
    Some(format!(
        "source: {} · kind: {} · confidence: {}",
        super::evidence_source_label_for(source),
        kind_override.unwrap_or_else(|| super::displayed_evidence_kind_label(m)),
        super::confidence_label_for(source)
    ))
}

fn format_definition_semantic_match_with_path(
    m: &Match,
    path: Option<&str>,
    cache: &OutlineCache,
    out: &mut String,
    indent: &str,
    suppress: Option<&str>,
) {
    let atom = m.to_evidence_atom();
    if super::is_artifact_anchor_match(m) {
        let label = m
            .def_name
            .as_deref()
            .unwrap_or_else(|| atom.snippet().trim());
        let _ = write!(
            out,
            "\n{indent}[anchor] {label} {}",
            format_loc(path, atom.anchor().start_line())
        );
        append_provenance_if_not_default(m, out, indent, Some("anchor"), cache, suppress);
        return;
    }
    if m.impl_target.is_some() {
        format_relation_definition_match(m, "impl", path, out, indent);
        append_artifact_definition_snippet(m, out);
        return;
    }
    if m.base_target.is_some() {
        format_relation_definition_match(m, "base", path, out, indent);
        append_artifact_definition_snippet(m, out);
        return;
    }
    if let Some(candidate) = semantic_candidate_for_match(m, cache) {
        // US-064: a qualified `Q.N` match renders its own dotted form so the
        // receiver/container choice is visible even when the outline parent
        // chain is empty (Go methods are top-level declarations).
        let qualified_name = m
            .def_name
            .as_deref()
            .and_then(crate::search::symbol::split_dot_symbol_query)
            .map_or_else(
                || {
                    if candidate.parents.is_empty() {
                        candidate.name.clone()
                    } else {
                        format!("{}.{}", candidate.parents.join("."), candidate.name)
                    }
                },
                |(qualifier, plain)| format!("{qualifier}.{plain}"),
            );
        let _ = write!(
            out,
            "\n{indent}[{}] {} {}",
            outline_kind_label(candidate.kind),
            qualified_name,
            format_range(path, candidate.start_line, candidate.end_line)
        );
        let kind_override = if matches!(
            crate::lang::detect_file_type(&m.path),
            crate::types::FileType::Document(_)
        ) {
            super::document_outline_kind_label(candidate.kind)
        } else {
            None
        };
        append_provenance_if_not_default(m, out, indent, kind_override, cache, suppress);
        for child in candidate.children.iter().take(2) {
            let _ = write!(
                out,
                "\n{indent}  +[{}] {} {}-{}",
                outline_kind_label(child.kind),
                child.name,
                child.start_line,
                child.end_line
            );
        }
        if candidate.children.len() > 2 {
            let _ = write!(out, "\n    +{} more members", candidate.children.len() - 2);
        }
    } else if let Some((start, end)) = m.def_range {
        let kind = if m.impl_target.is_some() {
            "impl"
        } else {
            "definition"
        };
        if let Some(name) = m.def_name.as_deref() {
            let _ = write!(
                out,
                "\n{indent}[{kind}] {name} {}",
                format_range(path, start, end)
            );
        } else {
            let _ = write!(out, "\n{indent}[{kind}] {}", format_range(path, start, end));
        }
        append_provenance_if_not_default(m, out, indent, None, cache, suppress);
    } else {
        let kind = if m.impl_target.is_some() {
            "impl"
        } else {
            "definition"
        };
        if let Some(name) = m.def_name.as_deref() {
            let _ = write!(
                out,
                "\n{indent}[{kind}] {name} {}",
                format_loc(path, atom.anchor().start_line())
            );
        } else {
            let _ = write!(
                out,
                "\n{indent}[{kind}] {}",
                format_loc(path, atom.anchor().start_line())
            );
        }
        append_provenance_if_not_default(m, out, indent, None, cache, suppress);
    }
    append_artifact_definition_snippet(m, out);
}

/// Append a match provisioning line unless it equals the supplied section
/// default tuple (hoisted provenance).
fn append_provenance_if_not_default(
    m: &Match,
    out: &mut String,
    indent: &str,
    kind_override: Option<&'static str>,
    cache: &OutlineCache,
    suppress: Option<&str>,
) {
    if let Some(default) = suppress {
        if definition_provenance_tuple(m, cache).as_deref() == Some(default) {
            return;
        }
    }
    super::append_match_provenance_with_kind(m, out, indent, kind_override);
}

fn format_loc(path: Option<&str>, line: u32) -> String {
    match path {
        Some(path) => format!("{path}:{line}"),
        None => format!(":{line}"),
    }
}

fn format_range(path: Option<&str>, start: u32, end: u32) -> String {
    match path {
        Some(path) => format!("{path}:{start}-{end}"),
        None => format!(":{start}-{end}"),
    }
}

fn append_artifact_definition_snippet(m: &Match, out: &mut String) {
    if !crate::artifact::is_artifact_js_ts_file(&m.path)
        || (!m.text.contains('…') && m.text.len() <= 220)
    {
        return;
    }
    let snippet = m.text.trim();
    if snippet.is_empty() {
        return;
    }
    let _ = write!(out, "\n    → {snippet}");
}

pub(super) fn format_relation_definition_match(
    m: &Match,
    kind: &str,
    path: Option<&str>,
    out: &mut String,
    indent: &str,
) {
    let atom = m.to_evidence_atom();
    let label = m
        .def_name
        .as_deref()
        .unwrap_or_else(|| atom.snippet().trim());
    if let Some((start, end)) = m.def_range {
        let _ = write!(
            out,
            "\n{indent}[{kind}] {label} {}",
            format_range(path, start, end)
        );
    } else {
        let _ = write!(
            out,
            "\n{indent}[{kind}] {label} {}",
            format_loc(path, atom.anchor().start_line())
        );
    }
    super::append_match_provenance(m, out, indent);
}

pub(super) fn semantic_candidate_for_match(
    m: &Match,
    cache: &OutlineCache,
) -> Option<SemanticCandidate> {
    let entries = structured_outline_entries(&m.path, cache)?;
    best_semantic_candidate(&entries, m)
}

fn structured_outline_entries(path: &Path, cache: &OutlineCache) -> Option<Vec<OutlineEntry>> {
    let file_type = crate::lang::detect_file_type(path);
    let lang = file_type.structural_lang()?;
    let meta = fs::metadata(path).ok()?;
    if meta.len() > 500_000 {
        return None;
    }
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let content = fs::read_to_string(path).ok()?;
    if crate::lang::document::is_document_lang(lang) {
        return Some(crate::lang::outline::get_outline_entries(&content, lang));
    }
    if let Some(entries) = crate::capabilities::outline_entries(lang, &content) {
        return Some(entries);
    }
    let ts_lang = crate::lang::outline::outline_language(lang)?;
    let tree = cache.get_or_parse(path, mtime, &content, &ts_lang)?;
    let lines: Vec<&str> = content.lines().collect();
    Some(crate::lang::outline::walk_top_level(
        tree.root_node(),
        &lines,
        lang,
    ))
}

pub(in crate::search) fn best_semantic_candidate(
    entries: &[OutlineEntry],
    m: &Match,
) -> Option<SemanticCandidate> {
    // US-064: a qualified `Q.N` def_name is matched semantically via its plain
    // name (`N`) so the outline candidate (kind/parents) is still found; the
    // display layer re-renders the qualifier from the dotted def_name.
    let wanted = m
        .def_name
        .as_deref()
        .and_then(crate::search::symbol::split_dot_symbol_query)
        .map(|(_, plain)| plain)
        .or(m.def_name.as_deref());
    let range = m.def_range.unwrap_or((m.line, m.line));
    let mut candidates = Vec::new();
    collect_semantic_candidates(entries, &mut Vec::new(), range, wanted, &mut candidates);
    if let Some(wanted) = wanted {
        if !candidates.iter().any(|(candidate, _, _)| {
            candidate.name == wanted
                || crate::lang::css::outline_name_matches(candidate.kind, &candidate.name, wanted)
                || crate::lang::document::outline_name_matches(
                    candidate.kind,
                    &candidate.name,
                    wanted,
                )
        }) {
            return None;
        }
    }
    candidates
        .into_iter()
        .min_by_key(|(_, score, size)| (*score, *size))
        .map(|(candidate, _, _)| candidate)
}

fn collect_semantic_candidates(
    entries: &[OutlineEntry],
    parents: &mut Vec<String>,
    match_range: (u32, u32),
    wanted: Option<&str>,
    out: &mut Vec<(SemanticCandidate, u32, u32)>,
) {
    for entry in entries {
        let overlaps = ranges_overlap((entry.start_line, entry.end_line), match_range);
        let contains_line = match_range.0 >= entry.start_line && match_range.0 <= entry.end_line;
        if overlaps || contains_line {
            let name_match = wanted.is_some_and(|name| {
                entry.name == name
                    || crate::lang::css::outline_name_matches(entry.kind, &entry.name, name)
                    || crate::lang::document::outline_name_matches(entry.kind, &entry.name, name)
            });
            let is_module = entry.kind == OutlineKind::Module;
            let kind_penalty = if is_module && !name_match { 25 } else { 0 };
            let name_penalty = if name_match { 0 } else { 100 };
            let exact_penalty = if (entry.start_line, entry.end_line) == match_range {
                0
            } else if entry.start_line <= match_range.0 && entry.end_line >= match_range.1 {
                10
            } else {
                20
            };
            let size = entry.end_line.saturating_sub(entry.start_line);
            out.push((
                SemanticCandidate {
                    kind: entry.kind,
                    name: entry.name.clone(),
                    start_line: entry.start_line,
                    end_line: entry.end_line,
                    parents: parents.clone(),
                    children: entry
                        .children
                        .iter()
                        .filter(|child| child.kind != OutlineKind::Import)
                        .map(|child| SemanticChild {
                            kind: child.kind,
                            name: child.name.clone(),
                            start_line: child.start_line,
                            end_line: child.end_line,
                        })
                        .collect(),
                },
                name_penalty + exact_penalty + kind_penalty,
                size,
            ));
        }

        let pushed_parent = if entry.kind == OutlineKind::Module {
            parents.push(entry.name.clone());
            true
        } else {
            false
        };
        collect_semantic_candidates(&entry.children, parents, match_range, wanted, out);
        if pushed_parent {
            parents.pop();
        }
    }
}

fn find_function_definition_range(
    entries: &[OutlineEntry],
    range: (u32, u32),
    wanted: Option<&str>,
) -> Option<(u32, u32)> {
    for entry in entries {
        if entry.kind == OutlineKind::Function
            && (entry.start_line, entry.end_line) == range
            && wanted.is_none_or(|name| entry.name == name)
        {
            return Some(range);
        }
        if let Some(found) = find_function_definition_range(&entry.children, range, wanted) {
            return Some(found);
        }
    }
    None
}

fn best_enclosing_function(entries: &[OutlineEntry], line: u32) -> Option<SemanticCandidate> {
    let mut candidates = Vec::new();
    collect_enclosing_functions(entries, line, &mut Vec::new(), &mut candidates);
    candidates
        .into_iter()
        .min_by_key(|candidate| candidate.end_line.saturating_sub(candidate.start_line))
}

fn collect_enclosing_functions(
    entries: &[OutlineEntry],
    line: u32,
    parents: &mut Vec<String>,
    out: &mut Vec<SemanticCandidate>,
) {
    for entry in entries {
        let contains_line = line >= entry.start_line && line <= entry.end_line;
        if contains_line && entry.kind == OutlineKind::Function {
            out.push(SemanticCandidate {
                kind: entry.kind,
                name: entry.name.clone(),
                start_line: entry.start_line,
                end_line: entry.end_line,
                parents: parents.clone(),
                children: entry
                    .children
                    .iter()
                    .filter(|child| child.kind != OutlineKind::Import)
                    .map(|child| SemanticChild {
                        kind: child.kind,
                        name: child.name.clone(),
                        start_line: child.start_line,
                        end_line: child.end_line,
                    })
                    .collect(),
            });
        }

        let pushed_parent = if entry.kind == OutlineKind::Module {
            parents.push(entry.name.clone());
            true
        } else {
            false
        };
        if contains_line {
            collect_enclosing_functions(&entry.children, line, parents, out);
        }
        if pushed_parent {
            parents.pop();
        }
    }
}

fn ranges_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

pub(super) fn outline_kind_label(kind: OutlineKind) -> &'static str {
    match kind {
        OutlineKind::Import => "import",
        OutlineKind::Function => "fn",
        OutlineKind::Class => "class",
        OutlineKind::Struct => "struct",
        OutlineKind::Interface => "interface",
        OutlineKind::TypeAlias => "type",
        OutlineKind::Enum => "enum",
        OutlineKind::Constant => "const",
        OutlineKind::Variable | OutlineKind::ImmutableVariable => "var",
        OutlineKind::Export => "export",
        OutlineKind::Provider(kind) => kind.semantic_label(),
        OutlineKind::Selector => "selector",
        OutlineKind::AtRule => "at-rule",
        OutlineKind::Section => "section",
        OutlineKind::Element => "element",
        OutlineKind::CodeBlock => "code-block",
        OutlineKind::Mixin => "mixin",
        OutlineKind::Property => "property",
        OutlineKind::Module => "mod",
        OutlineKind::TestSuite => "test_suite",
        OutlineKind::TestCase => "test_case",
    }
}
