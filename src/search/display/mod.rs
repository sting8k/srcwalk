use std::collections::{BTreeSet, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, evidence_source_label_for, render_next_actions, Anchor, EvidenceSource,
    NextAction,
};
use crate::format;
use crate::format::rel_nonempty;
use crate::read;
use crate::session::Session;
use crate::types::{estimate_tokens, FileType, Match, OutlineKind, SearchResult};

use super::{facets, glob};

mod basename;
mod expand;
mod glob_result;
mod match_item;
mod semantic;

pub(super) use expand::{append_expand_budget_note, ExpandBudget};
#[cfg(test)]
pub(super) use semantic::best_semantic_candidate;

pub(super) fn match_kind_label(m: &Match, cache: &OutlineCache) -> Option<&'static str> {
    if m.in_comment {
        return Some("comment occurrence");
    }
    if !m.is_definition {
        return Some(non_definition_label(m));
    }
    if m.impl_target.is_some() {
        return Some("impl");
    }
    if m.base_target.is_some() {
        return Some("base");
    }
    semantic::semantic_candidate_for_match(m, cache)
        .map(|candidate| semantic::outline_kind_label(candidate.kind))
}

pub(super) fn is_artifact_anchor_match(m: &Match) -> bool {
    m.is_definition && m.text.starts_with("artifact anchor ")
}

fn match_evidence_source(m: &Match) -> EvidenceSource {
    if is_artifact_anchor_match(m) {
        EvidenceSource::Artifact
    } else if matches!(
        crate::lang::detect_file_type(&m.path),
        FileType::Document(_)
    ) {
        EvidenceSource::Document
    } else {
        m.to_evidence_atom().source()
    }
}

pub(super) fn document_outline_kind_label(kind: OutlineKind) -> Option<&'static str> {
    match kind {
        OutlineKind::Section => Some("section"),
        OutlineKind::Element => Some("element"),
        OutlineKind::CodeBlock => Some("code-block"),
        _ => None,
    }
}

fn displayed_evidence_kind_label(m: &Match) -> &'static str {
    if m.in_comment {
        "comment occurrence"
    } else if m.impl_target.is_some() {
        "impl"
    } else if m.base_target.is_some() {
        "base"
    } else if m.is_definition {
        "definition"
    } else {
        non_definition_label(m)
    }
}

pub(super) fn append_match_provenance_with_kind(
    m: &Match,
    out: &mut String,
    indent: &str,
    kind_override: Option<&'static str>,
) {
    let source = match_evidence_source(m);
    let _ = write!(
        out,
        "\n{indent}source: {} · kind: {} · confidence: {}",
        evidence_source_label_for(source),
        kind_override.unwrap_or_else(|| displayed_evidence_kind_label(m)),
        confidence_label_for(source)
    );
}

pub(super) fn append_match_provenance(m: &Match, out: &mut String, indent: &str) {
    append_match_provenance_with_kind(m, out, indent, None);
}
pub fn format_raw_result(
    result: &SearchResult,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    format_search_result(result, cache, None, &bloom, 0, None)
}

pub fn format_raw_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    header: String,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    format_search_result_with_header(result, cache, None, &bloom, 0, None, header)
}

pub fn search_files_glob(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    search_files_glob_with_exclude(pattern, scope, limit, offset, None)
}

pub fn search_files_glob_with_exclude(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    let result = glob::search_with_exclude(pattern, scope, limit, offset, exclude)?;
    glob_result::format_glob_result(&result, scope, "Files")
}

pub fn search_files_glob_with_scope_filter(
    pattern: &str,
    scope: &Path,
    scope_glob: Option<&str>,
    limit: Option<usize>,
    offset: usize,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    let result = glob::search_with_scope_glob(pattern, scope, scope_glob, limit, offset, exclude)?;
    glob_result::format_glob_result(&result, scope, "Files")
}

/// Format match entries with optional expansion.
fn format_compact_facet_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
) {
    let mut definitions: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
    let mut grouped: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
    for m in matches {
        if m.is_definition {
            definitions.entry(m.path.as_path()).or_default().push(m);
        } else {
            grouped.entry(m.path.as_path()).or_default().push(m);
        }
    }

    for (path, group) in definitions {
        if group.len() == 1 {
            semantic::format_definition_semantic_match(group[0], scope, cache, out);
            continue;
        }
        let _ = write!(
            out,
            "\n  {} [{} matches]",
            rel_nonempty(path, scope),
            group.len()
        );
        for m in group {
            semantic::format_definition_semantic_match_in_file(m, cache, out);
        }
    }

    for (path, group) in grouped {
        if group.len() == 1 {
            format_compact_non_definition_match(group[0], scope, out);
            continue;
        }
        let noun = non_definition_group_noun(group[0]);
        let _ = write!(
            out,
            "\n  {} [{} {noun}]",
            rel_nonempty(path, scope),
            group.len()
        );
        append_match_provenance(group[0], out, "    ");
        for m in group {
            let atom = m.to_evidence_atom();
            let kind = non_definition_label(m);
            let _ = write!(
                out,
                "\n    [{kind}] :{} | {}",
                atom.anchor().start_line(),
                atom.snippet().trim()
            );
        }
    }
}

fn format_compact_non_definition_match(m: &Match, scope: &Path, out: &mut String) {
    let atom = m.to_evidence_atom();
    let kind = non_definition_label(m);
    let _ = write!(
        out,
        "\n  [{kind}] {} | {}",
        atom.anchor().display_relative_to(scope),
        atom.snippet().trim()
    );
    append_match_provenance(m, out, "  ");
}

/// Groups consecutive usage matches in the same enclosing function to reduce token noise.
/// Shared expand state enables cross-query dedup in multi-symbol search.
pub(super) fn format_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand_remaining: &mut usize,
    expand_budget: &mut ExpandBudget,
    expanded_files: &mut HashSet<PathBuf>,
    context_shown_files: &mut HashSet<PathBuf>,
    smart_truncated: &mut bool,
    out: &mut String,
) {
    // Multi-file: one expand per unique file. Single-file: sequential per-match.
    // expanded_files may contain entries from prior queries (cross-query dedup).
    let multi_file = matches
        .first()
        .is_some_and(|first| matches.iter().any(|m| m.path != first.path));

    let groups = group_matches(matches, cache);

    for group in &groups {
        match group {
            MatchGroup::Single(m) => {
                match_item::format_single_match(
                    m,
                    scope,
                    cache,
                    session,
                    bloom,
                    expand_remaining,
                    expand_budget,
                    expanded_files,
                    context_shown_files,
                    smart_truncated,
                    multi_file,
                    out,
                );
            }
            MatchGroup::FileGroup(usages) => {
                format_file_group(usages, scope, cache, context_shown_files, out);
            }
        }
    }
}

/// Group consecutive non-definition matches by (path, enclosing outline entry).
/// Dedup key for definition matches: (path, line, `def_range`, `def_name`, `impl_target`).
type DefKey<'a> = (
    &'a Path,
    u32,
    Option<(u32, u32)>,
    Option<&'a str>,
    Option<&'a str>,
);

/// Returns a Vec of groups, where each group is a slice of matches.
/// Definitions and impl matches are always singleton groups.
enum MatchGroup<'a> {
    Single(&'a Match),
    FileGroup(Vec<&'a Match>),
}

/// Group matches for rendering: definitions/impls stay individual, usages grouped by file.
fn group_matches<'a>(matches: &'a [Match], _cache: &OutlineCache) -> Vec<MatchGroup<'a>> {
    let mut groups: Vec<MatchGroup<'a>> = Vec::new();
    let mut seen_defs: HashSet<DefKey<'_>> = HashSet::new();
    // Collect non-definitions by file and honest evidence label, preserving first occurrence order.
    let mut file_matches: IndexMap<(&Path, &'static str), Vec<&'a Match>> = IndexMap::new();

    for m in matches {
        if m.is_definition || m.impl_target.is_some() {
            let key = (
                m.path.as_path(),
                m.line,
                m.def_range,
                m.def_name.as_deref(),
                m.impl_target.as_deref(),
            );
            if !seen_defs.insert(key) {
                continue;
            }
            groups.push(MatchGroup::Single(m));
        } else {
            file_matches
                .entry((m.path.as_path(), non_definition_label(m)))
                .or_default()
                .push(m);
        }
    }

    // Emit file-grouped non-definitions after definitions.
    for ((_path, _label), matches) in file_matches {
        if matches.len() == 1 {
            groups.push(MatchGroup::Single(matches[0]));
        } else {
            groups.push(MatchGroup::FileGroup(matches));
        }
    }

    groups
}

/// Format a file-level group of usages: one header, outline once, compact list with fn names.
pub(super) fn non_definition_label(m: &Match) -> &'static str {
    if m.impl_target.is_some() {
        "impl"
    } else if m.in_comment {
        "comment occurrence"
    } else {
        m.to_evidence_atom().kind().as_str()
    }
}

fn non_definition_group_noun(m: &Match) -> &'static str {
    match non_definition_label(m) {
        "name occurrence" => "name occurrences",
        "comment occurrence" => "comment occurrences",
        _ => "text matches",
    }
}

fn non_definition_facet_heading(matches: &[Match], same_package: bool) -> &'static str {
    let has_text = matches.iter().any(|m| non_definition_label(m) == "text");
    let has_name_occurrence = matches
        .iter()
        .any(|m| non_definition_label(m) == "name occurrence");

    match (has_text, has_name_occurrence, same_package) {
        (true, false, true) => "Text matches — same package",
        (true, false, false) => "Text matches — other",
        (false, true, true) => "Name occurrences — same package",
        (false, true, false) => "Name occurrences — other",
        (_, _, true) => "Matches — same package",
        (_, _, false) => "Matches — other",
    }
}

fn format_file_group(
    group: &[&Match],
    scope: &Path,
    cache: &OutlineCache,
    context_shown_files: &mut HashSet<PathBuf>,
    out: &mut String,
) {
    let first = group[0];
    let path_str = rel_nonempty(&first.path, scope);

    let noun = non_definition_group_noun(first);
    let _ = write!(out, "\n\n## {path_str} [{} {noun}]", group.len());

    append_match_provenance(first, out, "");

    // Show outline context once per file
    if context_shown_files.insert(first.path.clone()) {
        if let Some(context) = outline_context_for_match(&first.path, first.line, cache) {
            out.push_str(&context);
        }
    }

    // Compact list: one line per hit with enclosing fn annotation
    for m in group {
        let atom = m.to_evidence_atom();
        let fn_name = semantic::enclosing_fn_name(&m.path, atom.anchor().start_line(), cache);
        if let Some(name) = fn_name {
            let _ = write!(
                out,
                "\n- :{:<6} {} ← {name}",
                atom.anchor().start_line(),
                atom.snippet().trim()
            );
        } else {
            let _ = write!(
                out,
                "\n- :{:<6} {}",
                atom.anchor().start_line(),
                atom.snippet().trim()
            );
        }
    }
}

fn append_context_next_targets(
    out: &mut String,
    result: &SearchResult,
    cache: &OutlineCache,
) -> bool {
    let mut actions = Vec::new();
    let mut seen = BTreeSet::new();
    for m in &result.matches {
        let Some(target) = semantic::context_target_for_match(m, cache) else {
            continue;
        };
        let key = (m.path.clone(), target.start_line, target.end_line);
        if !seen.insert(key) {
            continue;
        }

        let anchor = Anchor::lines(&m.path, target.start_line, target.end_line);
        actions.push(NextAction::from_evidence(
            format!(
                "srcwalk context {}",
                anchor.display_relative_to(&result.scope)
            ),
            "confirmed structural context target",
            10 + actions.len() as u16,
            EvidenceSource::Ast,
            anchor,
        ));
        if actions.len() == 3 {
            break;
        }
    }

    if actions.is_empty() {
        return false;
    }

    out.push_str("\n\n## Confirmed next context targets");
    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        out.push('\n');
        out.push_str(&rendered);
    }
    true
}

fn append_next_action(footer: &mut String, action: NextAction) {
    if !footer.is_empty() {
        footer.push('\n');
    }
    footer.push_str(&render_next_actions(&[action]));
}

pub(super) fn append_symbol_ambiguity_caveat(out: &mut String, result: &SearchResult) {
    if result.definition_candidates > 1 && result.name_occurrence_candidates > 0 {
        let _ = write!(
            out,
            "\n> Caveat: {} definition candidates share this name; text-matched name occurrences are not binding-resolved and may belong to different scopes.",
            result.definition_candidates
        );
    }
}

/// Format a symbol/content search result.
/// When an outline cache is available, wraps each match in the file's outline context.
/// When `expand > 0`, the top N matches inline actual code (def body or ±10 lines).
/// When there are >5 matches, groups them into facets for easier navigation.
pub(super) fn format_search_result(
    result: &SearchResult,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let header = format::search_header(
        &result.query,
        &result.scope,
        result.matches.len(),
        result.page_evidence_counts(),
    );
    format_search_result_with_header(result, cache, session, bloom, expand, budget_tokens, header)
}

pub(super) fn format_search_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    budget_tokens: Option<u64>,
    header: String,
) -> Result<String, SrcwalkError> {
    let mut out = header;
    append_symbol_ambiguity_caveat(&mut out, result);
    let mut expand_remaining = expand;
    let mut expand_budget = ExpandBudget::new(expand, budget_tokens);
    let mut expanded_files = HashSet::new();
    let mut context_shown_files = HashSet::new();
    let mut smart_truncated = false;

    let compact_facets = result.matches.len() > 5 && expand == 0;

    // File-level retrieval: when a file basename matches the query exactly,
    // prepend a compact outline so the agent gets file-level context first.
    // Semantic-compact facets render kind/parent/children inline, so the
    // basename outline would duplicate the same facts and cost tokens.
    if !compact_facets {
        if let Some(file_outline) =
            basename::basename_file_outline(&result.query, &result.matches, &result.scope, cache)
        {
            let _ = write!(out, "\n\n{file_outline}");
        }
    }

    // Apply faceting when there are many matches (>5)
    if result.matches.len() > 5 {
        let faceted = facets::facet_matches(result.matches.clone(), &result.scope);

        // Format each non-empty facet with section headers
        if !faceted.definitions.is_empty() {
            let _ = write!(out, "\n\n### Definitions ({})", faceted.definitions.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.definitions, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.definitions,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.implementations.is_empty() {
            let _ = write!(
                out,
                "\n\n### Implementations ({})",
                faceted.implementations.len()
            );
            if compact_facets {
                format_compact_facet_matches(
                    &faceted.implementations,
                    &result.scope,
                    cache,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.implementations,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.bases.is_empty() {
            let _ = write!(out, "\n\n### Base relationships ({})", faceted.bases.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.bases, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.bases,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.tests.is_empty() {
            let _ = write!(out, "\n\n### Tests ({})", faceted.tests.len());
            append_match_provenance(&faceted.tests[0], &mut out, "");
            // Compact test format — one line per match, no expand budget consumed
            for m in &faceted.tests {
                let atom = m.to_evidence_atom();
                let _ = write!(
                    out,
                    "\n  {} — {}",
                    atom.anchor().display_relative_to(&result.scope),
                    atom.snippet().trim()
                );
            }
        }

        if !faceted.comments.is_empty() {
            let _ = write!(out, "\n\n### Comments ({})", faceted.comments.len());
            append_match_provenance(&faceted.comments[0], &mut out, "");
            for m in &faceted.comments {
                let atom = m.to_evidence_atom();
                let _ = write!(
                    out,
                    "\n  {} — {}",
                    atom.anchor().display_relative_to(&result.scope),
                    atom.snippet().trim()
                );
            }
        }

        if !faceted.usages_local.is_empty() {
            let header = non_definition_facet_heading(&faceted.usages_local, true);
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_local.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.usages_local, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.usages_local,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.usages_cross.is_empty() {
            let header = non_definition_facet_heading(&faceted.usages_cross, false);
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_cross.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.usages_cross, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.usages_cross,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }
    } else {
        // Linear display for ≤5 matches
        format_matches(
            &result.matches,
            &result.scope,
            cache,
            session,
            bloom,
            &mut expand_remaining,
            &mut expand_budget,
            &mut expanded_files,
            &mut context_shown_files,
            &mut smart_truncated,
            &mut out,
        );
    }

    let has_context_next_targets = append_context_next_targets(&mut out, result, cache);

    let mut footer = String::new();
    if result.has_more {
        let omitted = result.total_found - result.matches.len() - result.offset;
        let next_offset = result.offset + result.matches.len();
        let page_size = result.matches.len().max(1);
        append_next_action(
            &mut footer,
            NextAction::metadata(
                format!("{omitted} more matches available. Continue with --offset {next_offset} --limit {page_size}."),
                "result pagination",
                10,
            ),
        );
    } else if result.offset > 0 {
        let _ = write!(
            footer,
            "> Note: end of results at offset {}.",
            result.offset
        );
    } else if result.total_found > result.matches.len() {
        let omitted = result.total_found - result.matches.len();
        append_next_action(
            &mut footer,
            NextAction::metadata(
                format!(
                    "{omitted} more matches hidden by display limits. Narrow with --scope <dir>."
                ),
                "display limit omitted matches",
                20,
            ),
        );
    }

    if result.total_found > 0 {
        let guidance = if has_context_next_targets {
            "choose a confirmed context target above, or read exact hit evidence with `srcwalk show <path>:<line> -C 10`."
        } else {
            "read exact hit evidence with `srcwalk show <path>:<line> -C 10`."
        };
        append_next_action(
            &mut footer,
            NextAction::guidance(guidance, "read exact hit evidence", 50),
        );
    }

    if smart_truncated {
        if !footer.is_empty() {
            footer.push('\n');
        }
        footer.push_str("> Caveat: expanded source truncated.");
        append_next_action(
            &mut footer,
            NextAction::guidance(
                "use shown line range with `srcwalk show <path>:<start-end>`.",
                "expanded source was truncated",
                60,
            ),
        );
    }

    if expand_budget.omitted > 0 {
        if !footer.is_empty() {
            footer.push('\n');
        }
        let expanded = expand_budget.expanded;
        let omitted = expand_budget.omitted;
        let used = expand_budget
            .cap_tokens
            .saturating_sub(expand_budget.remaining_tokens);
        let cap = expand_budget.cap_tokens;
        let _ = write!(
            footer,
            "> Note: expand cap ~{used}/{cap} tokens; expanded {expanded}, omitted {omitted}."
        );
        append_next_action(
            &mut footer,
            NextAction::guidance(
                "read omitted hits with `srcwalk show <path>:<line> -C 10` or `srcwalk show <path> --section <symbol|range>`.",
                "expanded hits omitted by budget",
                70,
            ),
        );
    }

    let tokens = estimate_tokens(out.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(out, "\n\n({token_str} tokens)");
    if !footer.is_empty() {
        let _ = write!(out, "\n\n{footer}");
    }

    Ok(out)
}

/// Get cached outline string for a file. Returns None for non-code or huge files.
fn get_outline_str(path: &std::path::Path, cache: &OutlineCache) -> Option<std::sync::Arc<str>> {
    let file_type = crate::lang::detect_file_type(path);
    if !matches!(file_type, FileType::Code(_)) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    if meta.len() > 500_000 {
        return None;
    }
    Some(cache.get_or_compute(path, mtime, || {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let buf = content.as_bytes();
        read::outline::generate(path, file_type, &content, buf, false)
    }))
}

/// Build outline context around a match — ±2 entries around the enclosing one.
fn outline_context_for_match(
    path: &std::path::Path,
    match_line: u32,
    cache: &OutlineCache,
) -> Option<String> {
    let outline_str = get_outline_str(path, cache)?;
    let outline_lines: Vec<&str> = outline_str.lines().collect();
    if outline_lines.is_empty() {
        return None;
    }

    let match_idx = outline_lines.iter().position(|line| {
        extract_line_range(line).is_some_and(|(s, e)| match_line >= s && match_line <= e)
    })?;

    let start = match_idx.saturating_sub(2);
    let end = (match_idx + 3).min(outline_lines.len());

    let mut context = String::new();
    for (i, line) in outline_lines.iter().enumerate().take(end).skip(start) {
        if i == match_idx {
            let _ = write!(context, "\n→ {line}");
        } else {
            let _ = write!(context, "\n  {line}");
        }
    }
    Some(context)
}

/// Extract (`start_line`, `end_line`) from an outline entry like "[20-115]" or "[16]".
fn extract_line_range(line: &str) -> Option<(u32, u32)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let range_str = &trimmed[1..end];
    if let Some((a, b)) = range_str.split_once('-') {
        let start: u32 = a.trim().parse().ok()?;
        // Handle import ranges like "[1-]"
        let end: u32 = if b.trim().is_empty() {
            start
        } else {
            b.trim().parse().ok()?
        };
        Some((start, end))
    } else {
        let n: u32 = range_str.trim().parse().ok()?;
        Some((n, n))
    }
}
