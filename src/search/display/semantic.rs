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

    if m.is_definition {
        if let Some(candidate) = semantic_candidate_for_match(m, cache) {
            if candidate.kind == OutlineKind::Function {
                return Some(ContextTarget {
                    start_line: candidate.start_line,
                    end_line: candidate.end_line,
                });
            }
        }
        if let Some((start_line, end_line)) = function_definition_range_from_outline(m, cache) {
            return Some(ContextTarget {
                start_line,
                end_line,
            });
        }
        return None;
    }

    if !m.exact {
        return None;
    }

    let entries = structured_outline_entries(&m.path, cache)?;
    let candidate = best_enclosing_function(&entries, m.line)?;
    Some(ContextTarget {
        start_line: candidate.start_line,
        end_line: candidate.end_line,
    })
}

pub(super) fn format_definition_semantic_match(
    m: &Match,
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
) {
    let path = rel_nonempty(&m.path, scope);
    format_definition_semantic_match_with_path(m, Some(&path), cache, out, "  ");
}

pub(super) fn format_definition_semantic_match_in_file(
    m: &Match,
    cache: &OutlineCache,
    out: &mut String,
) {
    format_definition_semantic_match_with_path(m, None, cache, out, "    ");
}

fn format_definition_semantic_match_with_path(
    m: &Match,
    path: Option<&str>,
    cache: &OutlineCache,
    out: &mut String,
    indent: &str,
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
        super::append_match_provenance_with_kind(m, out, indent, Some("anchor"));
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
        let qualified_name = if candidate.parents.is_empty() {
            candidate.name.clone()
        } else {
            format!("{}.{}", candidate.parents.join("."), candidate.name)
        };
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
        super::append_match_provenance_with_kind(m, out, indent, kind_override);
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
        super::append_match_provenance(m, out, indent);
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
        super::append_match_provenance(m, out, indent);
    }
    append_artifact_definition_snippet(m, out);
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
    let wanted = m.def_name.as_deref();
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

fn function_definition_range_from_outline(m: &Match, cache: &OutlineCache) -> Option<(u32, u32)> {
    let range = m.def_range?;
    let wanted = m.def_name.as_deref();
    let entries = structured_outline_entries(&m.path, cache)?;
    find_function_definition_range(&entries, range, wanted)
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
