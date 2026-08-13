use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, evidence_source_label_for, render_next_actions, EvidenceSource,
    NextAction,
};
use crate::format;
use crate::format::rel_nonempty;
use crate::lang::tsconfig::ConfigCache;
use crate::read;
use crate::session::Session;
use crate::types::{estimate_tokens, FileType, Match, OutlineKind, SearchResult};

use super::{facets, glob};

mod basename;
mod expand;
mod glob_result;
mod match_item;
mod semantic;
mod structural_targets;

pub(super) use expand::{append_expand_budget_note, ExpandBudget};
pub(super) use structural_targets::{
    append_structural_next_targets, has_confirmed_structural_targets,
};

#[derive(Default)]
pub(super) struct RenderedSourceLines {
    lines: HashSet<(PathBuf, u32)>,
}

impl RenderedSourceLines {
    pub(super) fn record_code_block(&mut self, path: &Path, code: &str) {
        for line in code.lines().filter_map(rendered_code_line_number) {
            self.lines.insert((path.to_path_buf(), line));
        }
    }

    fn contains(&self, path: &Path, line: u32) -> bool {
        self.lines.contains(&(path.to_path_buf(), line))
    }

    /// True when every line `start..=end` of `path` was rendered verbatim in
    /// this packet (US-063: an offer for a fully-rendered range is redundant).
    pub(super) fn contains_range(&self, path: &Path, start: u32, end: u32) -> bool {
        (start..=end).all(|line| self.contains(path, line))
    }

    /// True when every part of a multi-range selector (`--section A-B,C-D`)
    /// is fully rendered (US-063): a selector is suppressed only when ALL its
    /// parts are covered; a single uncovered part keeps the whole offer.
    /// Currently exercised by unit tests; trace/context selector wiring is
    /// deferred, so this shared-seam primitive stays available for that path.
    #[allow(dead_code)]
    pub(super) fn contains_all_ranges(&self, path: &Path, ranges: &[(u32, u32)]) -> bool {
        !ranges.is_empty() && ranges.iter().all(|&(s, e)| self.contains_range(path, s, e))
    }
}

fn rendered_code_line_number(segment: &str) -> Option<u32> {
    let fence_pos = segment.find('│')?;
    segment[..fence_pos].trim().parse::<u32>().ok()
}

pub(super) fn shown_name_occurrence_line(m: &Match, rendered: &RenderedSourceLines) -> bool {
    m.is_name_occurrence_candidate()
        && !m.text.contains("--section bytes:")
        && rendered.contains(&m.path, m.line)
        && !crate::artifact::should_auto_artifact_file(&m.path)
}
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

pub(super) fn match_evidence_source(m: &Match) -> EvidenceSource {
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

pub(super) fn displayed_evidence_kind_label(m: &Match) -> &'static str {
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

/// The exact provenance tuple string a non-definition match would print, used to
/// compare against a section default for suppression. Mirrors
/// `append_match_provenance` (no kind override).
fn non_definition_provenance_tuple(m: &Match) -> String {
    let source = match_evidence_source(m);
    format!(
        "source: {} · kind: {} · confidence: {}",
        evidence_source_label_for(source),
        displayed_evidence_kind_label(m),
        confidence_label_for(source)
    )
}

/// Append facet rows (Tests, Comments) grouping two or more visible hits from
/// the same file under a file header. Grouping applies only when it is strictly
/// fewer UTF-8 bytes than the ungrouped inline rows; a tie keeps ungrouped.
/// Single-hit files keep their current one-line shape. Ordering is
/// first-appearance by file, then original order.
fn append_facet_grouped_rows(out: &mut String, matches: &[Match], scope: &Path) {
    use std::collections::HashMap;
    use std::fmt::Write as _;

    let mut files: Vec<(&Path, Vec<&Match>)> = Vec::new();
    for m in matches {
        match files.iter_mut().find(|(p, _)| *p == m.path.as_path()) {
            Some((_, group)) => group.push(m),
            None => files.push((m.path.as_path(), vec![m])),
        }
    }

    // Precompute the per-path decision: grouped when strictly smaller, else
    // inline (single-hit, tie, or loss).
    let mut grouped_by_path: HashMap<&Path, String> = HashMap::new();
    for (path, group) in &files {
        if group.len() < 2 {
            continue;
        }
        let mut grouped_candidate = String::new();
        let _ = write!(
            grouped_candidate,
            "\n  {} [{} matches]",
            crate::format::rel_nonempty(path, scope),
            group.len()
        );
        for m in group {
            let atom = (*m).to_evidence_atom();
            let _ = write!(
                grouped_candidate,
                "\n    :{} — {}",
                atom.anchor().start_line(),
                atom.snippet().trim()
            );
        }

        let mut ungrouped_candidate = String::new();
        append_facet_ungrouped_rows(&mut ungrouped_candidate, group, scope);

        if grouped_candidate.len() < ungrouped_candidate.len() {
            grouped_by_path.insert(path, grouped_candidate);
        }
    }

    // Iterate the original visible slice. Emit a profitable group once at its
    // first occurrence; emit every other (single-hit / unprofitable) row at its
    // original position so rejected/tied file rows keep their relative order.
    let mut emitted: std::collections::HashSet<&Path> = std::collections::HashSet::new();
    for m in matches {
        let path = m.path.as_path();
        if let Some(grouped) = grouped_by_path.get(path) {
            if emitted.insert(path) {
                out.push_str(grouped);
            }
            continue;
        }
        append_facet_ungrouped_rows(out, &[m], scope);
    }
}

/// Render every hit in a file as a one-line inline row (the ungrouped shape).
fn append_facet_ungrouped_rows(out: &mut String, group: &[&Match], scope: &Path) {
    use std::fmt::Write as _;
    for m in group {
        let atom = m.to_evidence_atom();
        let _ = write!(
            out,
            "\n  {} — {}",
            atom.anchor().display_relative_to(scope),
            atom.snippet().trim()
        );
    }
}
pub fn format_raw_result(
    result: &SearchResult,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    let config_cache = ConfigCache::new();
    format_search_result(result, cache, None, &bloom, &config_cache, 0, None)
}

pub fn format_raw_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    header: String,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    let config_cache = ConfigCache::new();
    format_search_result_with_header(result, cache, None, &bloom, &config_cache, 0, None, header)
}

pub fn search_files_glob(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    search_files_glob_with_exclude(pattern, scope, limit, offset, None)
}

/// Path-fragment search (US-059): relative paths containing the fragment, ≤20 rows.
pub fn search_files_fragment(
    fragment: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    file_search_with_scope_miss(scope, "Path fragments", |root| {
        super::glob::search_path_fragment(fragment, root, limit, offset)
    })
}

pub fn search_files_glob_with_exclude(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    file_search_with_scope_miss(scope, "Files", |root| {
        glob::search_with_exclude(pattern, root, limit, offset, exclude)
    })
}

pub fn search_files_glob_with_scope_filter(
    pattern: &str,
    scope: &Path,
    scope_glob: Option<&str>,
    limit: Option<usize>,
    offset: usize,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    file_search_with_scope_miss(scope, "Files", |root| {
        glob::search_with_scope_glob(pattern, root, scope_glob, limit, offset, exclude)
    })
}

/// US-062: discover file-target queries widen to the repo root only on a zero
/// in-scope match. When the in-scope search finds nothing and `scope` is below
/// the repo root, rerun the same file search over the repo root and, on
/// success, append an outside-scope hint + a corrected `> Try:` scope. Any
/// in-scope match, no match anywhere, or scope == repo root returns the normal
/// (byte-identical) result.
fn file_search_with_scope_miss(
    scope: &Path,
    label: &str,
    run: impl Fn(&Path) -> Result<glob::GlobResult, SrcwalkError>,
) -> Result<String, SrcwalkError> {
    let in_scope = run(scope)?;
    if in_scope.total_found > 0 {
        return glob_result::format_glob_result(&in_scope, scope, label);
    }
    let root = repo_root(scope);
    // US-062: `git rev-parse --show-toplevel` returns an absolute path while
    // the user may pass a relative `--scope .`; canonicalize both sides so a
    // zero-match at the repo root does not run a redundant widened pass. When
    // canonicalization fails (e.g. a deleted dir), fall back to raw equality.
    if paths_equivalent(&root, scope) {
        return glob_result::format_glob_result(&in_scope, scope, label);
    }
    let expanded = run(&root)?;
    if expanded.total_found == 0 {
        return glob_result::format_glob_result(&in_scope, scope, label);
    }
    let mut out = glob_result::format_glob_result(&in_scope, scope, label)?;
    out.push_str(&format_scope_miss(&expanded, &root, &in_scope.pattern));
    Ok(out)
}

/// Git top-level ancestor of `scope`, or `scope` itself when not in a git
/// repo. Without git there is no reliable repo root, so a gitless miss stays a
/// miss (no second pass) rather than widening to a broad filesystem walk.
fn repo_root(scope: &Path) -> PathBuf {
    git_toplevel(scope).unwrap_or_else(|| scope.to_path_buf())
}

/// True when two paths point at the same directory, tolerating relative vs
/// absolute spellings (canonical first; raw equality as fallback).
fn paths_equivalent(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

fn git_toplevel(scope: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(scope)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// `> Found outside scope (N): p1, p2` (cap 5, shortest-first, deterministic)
/// + a `> Try:` command whose scope finds all listed files.
fn format_scope_miss(expanded: &glob::GlobResult, root: &Path, pattern: &str) -> String {
    let mut paths: Vec<String> = expanded
        .files
        .iter()
        .map(|f| rel_nonempty(&f.path, root))
        .collect();
    paths.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    paths.truncate(5);
    let mut out = format!(
        "\n> Found outside scope ({}): {}",
        expanded.total_found,
        paths.join(", ")
    );
    let _ = write!(
        out,
        "\n> Try: `srcwalk discover '{}' --scope {}`",
        pattern,
        format::display_path(root)
    );
    out
}

/// Format compact facet matches. When `hoist_provenance` is true (used only for
/// the `Matches — same package` `usages_local` facet), the most frequent
/// non-definition provenance tuple is hoisted once as a section default and
/// identical per-group provenance is suppressed; deviations print locally.
fn format_compact_facet_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    hoist_provenance: bool,
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

    render_compact_definitions(&definitions, scope, cache, out);

    // Hoist the most frequent non-definition provenance tuple as a section
    // default when requested (usages_local only). Only suppress when the
    // section has >= 2 provenance carriers. The default is chosen from the
    // actual rendered provenance carriers (one per path group / single row,
    // i.e. `group[0]`), NOT weighted by the number of child hits — a group
    // renders one provenance line regardless of how many matches it contains.
    // Ties resolve by first-appearance.
    let carriers: Vec<&Match> = grouped.values().map(|g| g[0]).collect();
    let suppress = if hoist_provenance && carriers.len() >= 2 {
        pick_non_definition_provenance_default(&carriers)
    } else {
        None
    };
    if let Some(tuple) = &suppress {
        let _ = write!(out, "\n  {tuple}");
    }

    for (path, group) in grouped {
        if group.len() == 1 {
            format_compact_non_definition_match_suppressed(
                group[0],
                scope,
                out,
                suppress.as_deref(),
            );
            continue;
        }
        let noun = non_definition_group_noun(group[0]);
        let _ = write!(
            out,
            "\n  {} [{} {noun}]",
            rel_nonempty(path, scope),
            group.len()
        );
        append_match_provenance_if_not_default(group[0], out, "    ", suppress.as_deref());
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

/// Render the compact Definitions facet, hoisting the most frequent provenance
/// tuple as a section default and suppressing identical per-entry provenance.
/// The default is page-local and deterministic: most frequent tuple, ties by
/// first occurrence; never cached or carried across `--offset` pages.
fn render_compact_definitions(
    definitions: &IndexMap<&Path, Vec<&Match>>,
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
) {
    use std::fmt::Write as _;

    // Flatten visible definitions in first-appearance order to pick the default.
    let mut flat: Vec<&Match> = Vec::new();
    for group in definitions.values() {
        flat.extend(group.iter().copied());
    }
    let default = pick_definition_provenance_default(&flat, cache);

    // Hoist the default tuple once below the section header only when the
    // section has >= 2 visible definitions; a single definition keeps its
    // current per-entry provenance byte-identical.
    let suppress = default.filter(|_| flat.len() >= 2);
    if let Some(tuple) = &suppress {
        let _ = write!(out, "\n  {tuple}");
    }

    for (path, group) in definitions {
        if group.len() == 1 {
            semantic::format_definition_semantic_match_suppressed(
                group[0],
                scope,
                cache,
                out,
                suppress.as_deref(),
            );
            continue;
        }
        let _ = write!(
            out,
            "\n  {} [{} matches]",
            rel_nonempty(path, scope),
            group.len()
        );
        for m in group {
            semantic::format_definition_semantic_match_in_file_suppressed(
                m,
                cache,
                out,
                suppress.as_deref(),
            );
        }
    }
}

/// Select the most frequent visible definition provenance tuple as the section
/// default; ties resolve by first occurrence. Returns `None` when there is no
/// computable tuple (e.g. all relation/impl entries), so nothing is hoisted.
fn pick_definition_provenance_default(matches: &[&Match], cache: &OutlineCache) -> Option<String> {
    pick_most_frequent(
        matches
            .iter()
            .filter_map(|m| semantic::definition_provenance_tuple(m, cache)),
    )
}

/// Select the most frequent non-definition provenance tuple as the section
/// default; ties resolve by first occurrence.
fn pick_non_definition_provenance_default(matches: &[&Match]) -> Option<String> {
    pick_most_frequent(matches.iter().map(|m| non_definition_provenance_tuple(m)))
}

/// Most frequent tuple; ties resolve by first occurrence (iteration order).
fn pick_most_frequent(tuples: impl Iterator<Item = String>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for tuple in tuples {
        match counts.iter_mut().find(|(t, _)| *t == tuple) {
            Some((_, count)) => *count += 1,
            None => counts.push((tuple, 1)),
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (tuple, count) in counts {
        if best
            .as_ref()
            .is_none_or(|(_, best_count)| count > *best_count)
        {
            best = Some((tuple, count));
        }
    }
    best.map(|(tuple, _)| tuple)
}

/// Compact single-row non-definition match. When `suppress` equals the entry's
/// exact provenance tuple, the per-entry provenance line is omitted (hoisted to
/// the section default computed by the caller); a deviation prints locally.
fn format_compact_non_definition_match_suppressed(
    m: &Match,
    scope: &Path,
    out: &mut String,
    suppress: Option<&str>,
) {
    let atom = m.to_evidence_atom();
    let kind = non_definition_label(m);
    let _ = write!(
        out,
        "\n  [{kind}] {} | {}",
        atom.anchor().display_relative_to(scope),
        atom.snippet().trim()
    );
    append_match_provenance_if_not_default(m, out, "  ", suppress);
}

/// Append provenance only when it deviates from the section default (or there is
/// no default). Exactly matches `append_match_provenance` otherwise.
fn append_match_provenance_if_not_default(
    m: &Match,
    out: &mut String,
    indent: &str,
    suppress: Option<&str>,
) {
    if let Some(default) = suppress {
        if non_definition_provenance_tuple(m) == default {
            return;
        }
    }
    append_match_provenance(m, out, indent);
}

/// Groups consecutive usage matches in the same enclosing function to reduce token noise.
/// Shared expand state enables cross-query dedup in multi-symbol search.
pub(super) fn format_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    config_cache: &ConfigCache,
    expand_remaining: &mut usize,
    expand_budget: &mut ExpandBudget,
    expanded_files: &mut HashSet<PathBuf>,
    context_shown_files: &mut HashSet<PathBuf>,
    rendered_source_lines: &mut RenderedSourceLines,
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
                    config_cache,
                    expand_remaining,
                    expand_budget,
                    expanded_files,
                    context_shown_files,
                    rendered_source_lines,
                    smart_truncated,
                    multi_file,
                    out,
                );
            }
            MatchGroup::FileGroup(usages) => {
                format_file_group(
                    usages,
                    scope,
                    cache,
                    context_shown_files,
                    rendered_source_lines,
                    out,
                );
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
    rendered_source_lines: &RenderedSourceLines,
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
        let line = atom.anchor().start_line();
        let snippet = if shown_name_occurrence_line(m, rendered_source_lines) {
            "[name occurrence · source shown above]"
        } else {
            atom.snippet().trim()
        };
        let fn_name = semantic::enclosing_fn_name(&m.path, line, cache);
        if let Some(name) = fn_name {
            let _ = write!(out, "\n- :{line:<6} {snippet} ← {name}");
        } else {
            let _ = write!(out, "\n- :{line:<6} {snippet}");
        }
    }
}

fn append_next_action(footer: &mut String, action: NextAction) {
    if !footer.is_empty() {
        footer.push('\n');
    }
    footer.push_str(&render_next_actions(&[action]));
}

pub(super) fn append_symbol_ambiguity_caveat(
    out: &mut String,
    result: &SearchResult,
    cache: &OutlineCache,
) {
    if result.definition_candidates > 1 && result.name_occurrence_candidates > 0 {
        let _ = write!(
            out,
            "\n> Caveat: {} definition candidates share this name; text-matched name occurrences are not binding-resolved and may belong to different scopes.",
            result.definition_candidates
        );
        // US-064 rule 6: known distinct qualifiers give the retry forms.
        let mut qualified_forms = definition_qualifier_forms(result, cache);
        if qualified_forms.len() >= 2 {
            qualified_forms.truncate(4);
            let _ = write!(
                out,
                "\n> Qualify: '{}' | '{}'",
                qualified_forms[0], qualified_forms[1]
            );
            if qualified_forms.len() > 2 {
                let rest: Vec<&str> = qualified_forms[2..].iter().map(String::as_str).collect();
                let _ = write!(out, " | '{}'", rest.join("' | '"));
            }
        }
    }
}

/// US-064 rule 6: deterministic, capped set of `Q.N` retry forms for a plain
/// symbol query with multiple definition candidates. Only qualifiers that are
/// structurally KNOWN (Go receiver or outline container parent) are emitted;
/// nothing is invented.
fn definition_qualifier_forms(result: &SearchResult, cache: &OutlineCache) -> Vec<String> {
    let mut forms = Vec::new();
    for m in result.matches.iter().filter(|m| m.is_definition) {
        if let Some(qualifier) = definition_qualifier_for(m, cache) {
            if let Some(name) = m.def_name.as_deref() {
                let plain = crate::search::symbol::split_dot_symbol_query(name)
                    .map_or(name, |(_, plain)| plain);
                let form = format!("{qualifier}.{plain}");
                if !forms.contains(&form) {
                    forms.push(form);
                }
            }
        }
    }
    forms.sort();
    forms
}

/// US-064: known qualifier for one definition match — Go receiver (parsed from
/// the declaration line) or the outline container chain for other languages.
fn definition_qualifier_for(m: &Match, cache: &OutlineCache) -> Option<String> {
    if matches!(
        crate::lang::detect_file_type(&m.path),
        crate::types::FileType::Code(crate::types::Lang::Go)
    ) {
        if let Some(q) = go_receiver_from_line(&m.text) {
            return Some(q);
        }
    }
    crate::search::display::semantic::semantic_candidate_for_match(m, cache)
        .filter(|c| !c.parents.is_empty())
        .map(|c| c.parents.join("."))
}

/// US-064: parse the receiver type out of a Go declaration line like
/// `func (b *Batch) Set(...)` → `Batch`. Only method form (receiver before the
/// name) is accepted; a bare `func Set(...)` yields None.
fn go_receiver_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("func")?.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let rest = &rest[1..];
    let close = rest.find(')')?;
    let receiver = &rest[..close];
    let inner = receiver.trim();
    if inner.is_empty() {
        return None;
    }
    // `b *Batch` / `q syncQueue[T]` — the type is the part after the receiver
    // variable name. No whitespace means no receiver variable (anonymous).
    let space = inner.find(char::is_whitespace)?;
    let ty = inner[space..].trim();
    if ty.is_empty() {
        return None;
    }
    Some(crate::search::symbol::normalize_receiver_type(ty))
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
    config_cache: &ConfigCache,
    expand: usize,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let header = format::search_header(
        &result.query,
        &result.scope,
        result.matches.len(),
        result.page_evidence_counts(),
    );
    format_search_result_with_header(
        result,
        cache,
        session,
        bloom,
        config_cache,
        expand,
        budget_tokens,
        header,
    )
}

pub(super) fn format_search_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    config_cache: &ConfigCache,
    expand: usize,
    budget_tokens: Option<u64>,
    header: String,
) -> Result<String, SrcwalkError> {
    let mut out = header;
    append_symbol_ambiguity_caveat(&mut out, result, cache);
    let mut expand_remaining = expand;
    let mut expand_budget = ExpandBudget::new(expand, budget_tokens);
    let mut expanded_files = HashSet::new();
    let mut context_shown_files = HashSet::new();
    let mut rendered_source_lines = RenderedSourceLines::default();
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
                format_compact_facet_matches(
                    &faceted.definitions,
                    &result.scope,
                    cache,
                    false,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.definitions,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    config_cache,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut rendered_source_lines,
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
                    false,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.implementations,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    config_cache,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut rendered_source_lines,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.bases.is_empty() {
            let _ = write!(out, "\n\n### Base relationships ({})", faceted.bases.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.bases, &result.scope, cache, false, &mut out);
            } else {
                format_matches(
                    &faceted.bases,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    config_cache,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut rendered_source_lines,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.tests.is_empty() {
            let _ = write!(out, "\n\n### Tests ({})", faceted.tests.len());
            append_match_provenance(&faceted.tests[0], &mut out, "");
            // Compact test format — one line per match, no expand budget consumed
            append_facet_grouped_rows(&mut out, &faceted.tests, &result.scope);
        }

        if !faceted.comments.is_empty() {
            let _ = write!(out, "\n\n### Comments ({})", faceted.comments.len());
            append_match_provenance(&faceted.comments[0], &mut out, "");
            append_facet_grouped_rows(&mut out, &faceted.comments, &result.scope);
        }

        if !faceted.usages_local.is_empty() {
            let header = non_definition_facet_heading(&faceted.usages_local, true);
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_local.len());
            if compact_facets {
                format_compact_facet_matches(
                    &faceted.usages_local,
                    &result.scope,
                    cache,
                    true,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.usages_local,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    config_cache,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut rendered_source_lines,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.usages_cross.is_empty() {
            let header = non_definition_facet_heading(&faceted.usages_cross, false);
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_cross.len());
            if compact_facets {
                format_compact_facet_matches(
                    &faceted.usages_cross,
                    &result.scope,
                    cache,
                    false,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.usages_cross,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    config_cache,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut rendered_source_lines,
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
            config_cache,
            &mut expand_remaining,
            &mut expand_budget,
            &mut expanded_files,
            &mut context_shown_files,
            &mut rendered_source_lines,
            &mut smart_truncated,
            &mut out,
        );
    }

    let has_structural_next_targets = structural_targets::append_structural_next_targets(
        &mut out,
        result,
        cache,
        &rendered_source_lines,
    );

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

    // When a confirmed-structural-target block exists, it already owns the
    // next action(s); appending a generic guidance `> Next:` here would create
    // a competing primary action. Only add the numeric exact-hit guidance when
    // there is no structural target (text/access/name occurrence evidence).
    if result.total_found > 0 && !has_structural_next_targets {
        append_next_action(
            &mut footer,
            NextAction::guidance(
                "read exact hit evidence with `srcwalk show <path>:<line> -C 10`.",
                "read exact hit evidence",
                50,
            ),
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
                "read omitted hits with `srcwalk show <path>:<line> -C 10` or `srcwalk show <path>:<symbol|start-end>`.",
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

#[cfg(test)]
mod scope_miss_tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "us062_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ))
    }

    #[test]
    fn repo_root_prefers_git_toplevel() {
        let dir = temp_dir("root_git");
        let scope = dir.join("packages/coding-agent/src");
        fs::create_dir_all(&scope).unwrap();
        // A real repo requires `git init`; skip if git is unavailable.
        let init = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&dir)
            .output();
        match init {
            Ok(out) if out.status.success() => {
                let expected = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
                let actual =
                    fs::canonicalize(repo_root(&scope)).unwrap_or_else(|_| repo_root(&scope));
                assert_eq!(actual, expected);
            }
            _ => {
                // No git in this environment: the safe fallback is the scope.
                assert_eq!(repo_root(&scope), scope);
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_root_returns_scope_without_git() {
        let dir = temp_dir("root_nogit");
        let scope = dir.join("packages/coding-agent/src");
        fs::create_dir_all(&scope).unwrap();
        // No .git anywhere: the safe fallback is the scope itself (no widening).
        assert_eq!(repo_root(&scope), scope);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_miss_hint_caps_at_five_and_sorts_shortest_first() {
        let root = temp_dir("hint");
        fs::create_dir_all(&root).unwrap();
        let mut files = Vec::new();
        for (i, name) in [
            "zzz/quite_long_name.json",
            "a.json",
            "bb.json",
            "ccc.json",
            "dddd.json",
            "eeeee.json",
            "ffffff.json",
        ]
        .iter()
        .enumerate()
        {
            let p = root.join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"{}").unwrap();
            files.push(glob::GlobFileEntry {
                path: p,
                preview: None,
            });
            let _ = i;
        }
        let result = glob::GlobResult {
            pattern: "*.json".to_string(),
            total_found: files.len(),
            files,
            available_extensions: Vec::new(),
            offset: 0,
            limit: 20,
            path_symbol_target: None,
            oversized: false,
        };
        let hint = format_scope_miss(&result, &root, "*.json");
        // Shortest-first and capped: only 5 of the 7 paths.
        let listed = hint
            .split("): ")
            .nth(1)
            .and_then(|s| s.lines().next())
            .unwrap_or("");
        let paths: Vec<&str> = listed.split(", ").collect();
        assert_eq!(paths.len(), 5, "{hint}");
        assert_eq!(paths[0], "a.json");
        assert_eq!(paths[1], "bb.json");
        assert!(hint.contains("(7)"), "{hint}");
        assert!(hint.contains("> Try: `srcwalk discover '*.json'"), "{hint}");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod rendered_lines_tests {
    use super::*;

    #[test]
    fn only_all_parts_covered_suppresses_a_multirange_selector() {
        let mut rendered = RenderedSourceLines::default();
        rendered.record_code_block(
            Path::new("p.rs"),
            "1 │ a\n2 │ b\n3 │ c\n4 │ d\n5 │ e\n6 │ f\n7 │ g\n",
        );
        // All parts (1-2 and 5-7) covered -> true (selector suppressed).
        assert!(rendered.contains_all_ranges(Path::new("p.rs"), &[(1, 2), (5, 7)]));
        // One part (8-9) uncovered -> false (selector kept).
        assert!(!rendered.contains_all_ranges(Path::new("p.rs"), &[(1, 2), (8, 9)]));
        // Empty selector -> never suppressed.
        assert!(!rendered.contains_all_ranges(Path::new("p.rs"), &[]));
    }

    #[test]
    fn partial_overlap_keeps_the_offer() {
        let mut rendered = RenderedSourceLines::default();
        rendered.record_code_block(Path::new("p.rs"), "1 │ a\n2 │ b\n3 │ c\n");
        assert!(rendered.contains_range(Path::new("p.rs"), 1, 3));
        assert!(!rendered.contains_range(Path::new("p.rs"), 1, 6)); // 4-6 missing
        assert!(!rendered.contains_range(Path::new("p.rs"), 4, 6)); // no overlap
    }
}

#[cfg(test)]
mod us064_receiver_line_tests {
    use super::go_receiver_from_line;

    #[test]
    fn pointer_value_and_generic_receivers() {
        assert_eq!(
            go_receiver_from_line("func (b *Batch) Set(key, value []byte) error { return nil }"),
            Some("Batch".to_string())
        );
        assert_eq!(
            go_receiver_from_line("func (q *syncQueue[T]) Set(v T) {}"),
            Some("syncQueue".to_string())
        );
        assert_eq!(
            go_receiver_from_line("func (q syncQueue[T]) pop() T { var z T; return z }"),
            Some("syncQueue".to_string())
        );
        assert_eq!(
            go_receiver_from_line("func (c Config) Load() {}"),
            Some("Config".to_string())
        );
    }

    #[test]
    fn plain_functions_and_anonymous_receivers_rejected() {
        assert_eq!(
            go_receiver_from_line("func Set(x int) int { return x }"),
            None
        );
        assert_eq!(go_receiver_from_line("func (Batch) Method() {}"), None);
        assert_eq!(go_receiver_from_line("var x = 1"), None);
        assert_eq!(go_receiver_from_line(""), None);
    }
}

#[cfg(test)]
mod us062_paths_equivalent_tests {
    use super::paths_equivalent;

    #[test]
    fn different_spellings_of_same_dir_are_equivalent() {
        let dir = std::env::temp_dir().join(format!(
            "us062_pathseq_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        // Two spellings of the same directory: plain vs `..` round-trip.
        // Canonicalization resolves the dot-dot; raw equality would not.
        // (No cwd mutation: tests run in parallel in one process.)
        let child = dir.join("a/b");
        std::fs::create_dir_all(&child).unwrap();
        let dotted = dir.join("a/b/../b");
        assert_ne!(child, dotted, "raw paths must differ for a meaningful test");
        assert!(
            paths_equivalent(&child, &dotted),
            "{} vs {}",
            child.display(),
            dotted.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn distinct_dirs_are_not_equivalent() {
        let base = std::env::temp_dir().join(format!(
            "us062_pathseq_diff_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(base.join("one")).unwrap();
        std::fs::create_dir_all(base.join("two")).unwrap();
        assert!(!paths_equivalent(&base.join("one"), &base.join("two")));
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod provenance_tuple_tests {
    use super::*;
    use crate::types::Match;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn def_match(name: &str, line: u32) -> Match {
        Match {
            path: PathBuf::from(format!("/repo/{name}.go")),
            line,
            text: format!("func {name}() {{}}"),
            is_definition: true,
            exact: true,
            file_lines: 40,
            mtime: SystemTime::now(),
            def_range: Some((line, line + 1)),
            def_name: Some(name.to_string()),
            def_weight: 80,
            impl_target: None,
            base_target: None,
            in_comment: false,
        }
    }

    fn tuple_of(m: &[&Match], cache: &OutlineCache) -> Option<String> {
        pick_definition_provenance_default(m, cache)
    }

    #[test]
    fn uniform_ast_definitions_produce_ast_tuple() {
        let cache = OutlineCache::default();
        let a = def_match("A", 1);
        let b = def_match("B", 5);
        let c = def_match("C", 9);
        let got = tuple_of(&[&a, &b, &c], &cache).unwrap();
        assert_eq!(
            got,
            "source: ast · kind: definition · confidence: structural syntax"
        );
    }

    #[test]
    fn most_frequent_tuple_wins_and_ties_by_first_occurrence() {
        let cache = OutlineCache::default();
        let ast_a = def_match("A", 1);
        let ast_b = def_match("B", 5);
        // A deviation: a plain (non-definition) name occurrence in a code file
        // yields a text tuple different from the AST definition tuple.
        let dev = Match {
            path: PathBuf::from("/repo/usage.go"),
            line: 3,
            text: "A()".to_string(),
            is_definition: false,
            exact: true,
            file_lines: 40,
            mtime: SystemTime::now(),
            def_range: None,
            def_name: None,
            def_weight: 0,
            impl_target: None,
            base_target: None,
            in_comment: false,
        };
        // 3 AST defs vs 2 deviations -> AST wins (most frequent).
        let ast_c = def_match("C", 9);
        let got = tuple_of(&[&ast_a, &ast_b, &ast_c, &dev, &dev], &cache).unwrap();
        assert_eq!(
            got,
            "source: ast · kind: definition · confidence: structural syntax"
        );
        // Single deviation only -> the deviation tuple is the default (honest).
        let dev_only = tuple_of(&[&dev], &cache).unwrap();
        assert_eq!(
            dev_only,
            "source: text · kind: name occurrence · confidence: text evidence"
        );
    }

    #[test]
    fn impl_base_entries_are_excluded_from_default() {
        let cache = OutlineCache::default();
        let a = def_match("A", 1);
        let impl_match = Match {
            impl_target: Some("Trait".to_string()),
            ..def_match("B", 5)
        };
        let got = tuple_of(&[&a, &impl_match], &cache).unwrap();
        assert_eq!(
            got,
            "source: ast · kind: definition · confidence: structural syntax"
        );
    }

    fn name_occurrence_match(path: &str, line: u32) -> Match {
        Match {
            path: PathBuf::from(path),
            line,
            text: "a.A()".to_string(),
            is_definition: false,
            exact: true,
            file_lines: 40,
            mtime: SystemTime::now(),
            def_range: None,
            def_name: None,
            def_weight: 0,
            impl_target: None,
            base_target: None,
            in_comment: false,
        }
    }

    #[test]
    fn non_definition_default_picks_most_frequent_usage_tuple() {
        // usages_local provenance hoist: uniform name occurrences pick the
        // text/name-occurrence tuple as the default.
        let a = name_occurrence_match("/repo/x.go", 1);
        let b = name_occurrence_match("/repo/y.go", 5);
        let c = name_occurrence_match("/repo/z.go", 9);
        let got = pick_non_definition_provenance_default(&[&a, &b, &c]).unwrap();
        assert_eq!(
            got,
            "source: text · kind: name occurrence · confidence: text evidence"
        );
    }

    #[test]
    fn non_definition_deviation_keeps_own_tuple() {
        // A document/text deviation (different tuple) among usages_local is not
        // folded into the default; the majority name-occurrence tuple wins.
        let a = name_occurrence_match("/repo/x.go", 1);
        let b = name_occurrence_match("/repo/y.go", 5);
        // Deviation: a comment occurrence (kind differs).
        let dev = Match {
            in_comment: true,
            ..name_occurrence_match("/repo/x.go", 3)
        };
        let got = pick_non_definition_provenance_default(&[&a, &b, &dev]).unwrap();
        assert_eq!(
            got,
            "source: text · kind: name occurrence · confidence: text evidence"
        );
        // Single deviation only -> the deviation tuple is the (honest) default.
        let dev_only = pick_non_definition_provenance_default(&[&dev]).unwrap();
        assert_eq!(
            dev_only,
            "source: text · kind: comment occurrence · confidence: text evidence"
        );
    }

    #[test]
    fn conditional_grouping_tie_keeps_ungrouped() {
        // A 2-hit short-path group is a tie/no-gain: the inline shape is chosen.
        let out = "";
        let _ = out;
        // Compare the two candidate byte lengths directly for a short path.
        let path = std::path::Path::new("/repo/a.go");
        let scope = std::path::Path::new("/repo");
        let mut grouped = String::new();
        let mut ungrouped = String::new();
        let m1 = name_occurrence_match("/repo/a.go", 1);
        let m2 = name_occurrence_match("/repo/a.go", 2);
        let group: Vec<&Match> = vec![&m1, &m2];
        let _ = write!(
            grouped,
            "\n  {} [{} matches]\n    :1 — t\n    :2 — t",
            crate::format::rel_nonempty(path, scope),
            2
        );
        for m in &group {
            let atom = m.to_evidence_atom();
            let _ = write!(
                ungrouped,
                "\n  {} — {}",
                atom.anchor().display_relative_to(scope),
                atom.snippet().trim()
            );
        }
        // Short path: grouped is not strictly smaller, so ungrouped wins.
        assert!(
            grouped.len() >= ungrouped.len(),
            "short-path 2-hit group should not be strictly smaller"
        );
    }

    fn artifact_anchor_match(name: &str, line: u32) -> Match {
        Match {
            path: PathBuf::from(format!("/repo/bundle.min.js")),
            line,
            text: format!("artifact anchor export {name}"),
            is_definition: true,
            exact: true,
            file_lines: 40,
            mtime: SystemTime::now(),
            def_range: Some((line, line + 1)),
            def_name: Some(name.to_string()),
            def_weight: 80,
            impl_target: None,
            base_target: None,
            in_comment: false,
        }
    }

    #[test]
    fn artifact_anchor_default_hoisted_once() {
        // Blocker fix: an artifact anchor's definition provenance tuple is
        // `source: artifact · kind: anchor ...`, so when it is the section
        // default it must be suppressed per-entry (printed once as the default).
        let cache = OutlineCache::default();
        let a1 = artifact_anchor_match("A1", 1);
        let a2 = artifact_anchor_match("A2", 1);
        let a3 = artifact_anchor_match("A3", 1);
        let a4 = artifact_anchor_match("A4", 1);
        let a5 = artifact_anchor_match("A5", 1);
        let a6 = artifact_anchor_match("A6", 1);
        // All artifact anchors share one provenance tuple -> it is the default.
        let default = tuple_of(&[&a1, &a2, &a3, &a4, &a5, &a6], &cache).unwrap();
        assert_eq!(
            default,
            "source: artifact · kind: anchor · confidence: artifact-level"
        );
        // Rendering the compact Definitions facet must not repeat the default
        // after every anchor entry.
        let mut map: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
        map.insert(a1.path.as_path(), vec![&a1, &a2, &a3, &a4, &a5, &a6]);
        let scope = Path::new("/repo");
        let mut out = String::new();
        render_compact_definitions(&map, scope, &cache, &mut out);
        assert_eq!(
            out.matches("kind: anchor · confidence: artifact-level")
                .count(),
            1,
            "artifact anchor default must be printed once (hoisted), got:\n{out}"
        );
    }

    #[test]
    fn artifact_anchor_mixed_keeps_deviation_tuple() {
        // Blocker fix: when the section default is an artifact-anchor tuple, a
        // non-anchor definition deviation must still print its own tuple.
        let cache = OutlineCache::default();
        let a1 = artifact_anchor_match("A1", 1);
        let a2 = artifact_anchor_match("A2", 1);
        let a3 = artifact_anchor_match("A3", 1);
        let a4 = artifact_anchor_match("A4", 1);
        let a5 = artifact_anchor_match("A5", 1);
        // Deviation: a Go AST definition (different provenance tuple).
        let go = Match {
            path: PathBuf::from("/repo/lib.go"),
            line: 5,
            text: "func Lib() {}".to_string(),
            is_definition: true,
            exact: true,
            file_lines: 40,
            mtime: SystemTime::now(),
            def_range: Some((5, 6)),
            def_name: Some("Lib".to_string()),
            def_weight: 80,
            impl_target: None,
            base_target: None,
            in_comment: false,
        };
        // 5 anchors > 1 AST -> anchor tuple is the default.
        let mut map: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
        map.insert(a1.path.as_path(), vec![&a1, &a2, &a3, &a4, &a5]);
        map.insert(go.path.as_path(), vec![&go]);
        let scope = Path::new("/repo");
        let mut out = String::new();
        render_compact_definitions(&map, scope, &cache, &mut out);
        // The hoisted default (anchor) appears once as the section default.
        assert_eq!(
            out.matches("kind: anchor · confidence: artifact-level")
                .count(),
            1,
            "anchor default should be printed once, got:\n{out}"
        );
        // The Go definition deviation keeps its own tuple.
        assert!(
            out.contains("source: ast · kind: definition · confidence: structural syntax"),
            "AST deviation must keep its own provenance tuple, got:\n{out}"
        );
    }

    #[test]
    fn usages_local_default_uses_carriers_not_child_count() {
        // Blocker fix: the usages_local default is chosen from the actual
        // rendered provenance carriers (one `group[0]` per path group), NOT
        // weighted by the number of child hits. Here one path group has many
        // name-occurrence children but there are two comment-only singleton
        // carriers: the comment tuple wins as the default even though the
        // child count favors name occurrences.
        let cache = OutlineCache::default();
        // Carrier A: a path group with 6 name-occurrence children (only its
        // `group[0]` is a carrier).
        let mut group_a = Vec::new();
        for i in 0..6 {
            group_a.push(name_occurrence_match("/repo/a.go", i + 1));
        }
        // Carriers B and C: two comment-occurrence singletons (each a carrier).
        let b = Match {
            in_comment: true,
            path: PathBuf::from("/repo/b.go"),
            ..name_occurrence_match("/repo/b.go", 1)
        };
        let c = Match {
            in_comment: true,
            path: PathBuf::from("/repo/c.go"),
            ..name_occurrence_match("/repo/c.go", 1)
        };
        // Carriers = [a.go -> name, b.go -> comment, c.go -> comment].
        // The comment tuple wins 2 vs 1 (carrier-based).
        let mut map: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
        map.insert(
            group_a.first().unwrap().path.as_path(),
            group_a.iter().collect(),
        );
        map.insert(b.path.as_path(), vec![&b]);
        map.insert(c.path.as_path(), vec![&c]);
        let carriers: Vec<&Match> = map.values().map(|g| g[0]).collect();
        let default = pick_non_definition_provenance_default(&carriers).unwrap();
        assert_eq!(
            default, "source: text · kind: comment occurrence · confidence: text evidence",
            "carrier-majority default should be the comment tuple"
        );
        // The name-occurrence group (deviation) keeps its own tuple.
        let non_def_default = pick_non_definition_provenance_default(&[&group_a[0]]).unwrap();
        assert_eq!(
            non_def_default,
            "source: text · kind: name occurrence · confidence: text evidence"
        );
    }

    #[test]
    fn interleaved_ungrouped_rows_preserve_original_order() {
        // Blocker fix: when a multi-hit file's group ISN'T profitable, its rows
        // must stay at their original positions in the visible slice rather than
        // being clustered together. Order A1, B1, A2 (A unprofitable short path)
        // must render A1, B1, A2 — not A1, A2, B1.
        let scope = Path::new("/repo");
        // A is a short-path 2-hit group (grouping ties -> stays inline).
        let a1 = name_occurrence_match("/repo/a.go", 1);
        let a2 = name_occurrence_match("/repo/a.go", 3);
        // B is an unrelated single-hit file.
        let b1 = name_occurrence_match("/repo/b.go", 2);
        // Visible slice order: A1, B1, A2.
        let matches = vec![a1, b1, a2];
        let mut out = String::new();
        append_facet_grouped_rows(&mut out, &matches, scope);
        // a.go is short (2 chars) -> grouping is a tie/loss, so its rows stay
        // inline at their original positions: a.go:1, b.go:2, a.go:3.
        let a1_pos = out.find("a.go:1").unwrap();
        let b1_pos = out.find("b.go:2").unwrap();
        let a2_pos = out.find("a.go:3").unwrap();
        assert!(
            a1_pos < b1_pos && b1_pos < a2_pos,
            "ungrouped rows must keep original interleaved order A1,B1,A2, got:\n{out}"
        );
        assert!(
            !out.contains("a.go [2 matches]"),
            "short-path 2-hit group must not be grouped, got:\n{out}"
        );
    }

    #[test]
    fn interleaved_profitable_group_emits_at_first_occurrence() {
        // Blocker fix: a PROFITABLE group emits its whole group at the first
        // occurrence of its path, consuming later matches. Order A1, B1, A2 where
        // A is a long-path multi-hit profitable group renders A[2], B1 — the A
        // group at A1's position, B1 after.
        let scope = Path::new("/repo");
        // A uses a long path so grouping is strictly smaller.
        let a1 = name_occurrence_match(
            "/repo/very/long/path/that/makes/grouping/profitable/a.go",
            1,
        );
        let a2 = name_occurrence_match(
            "/repo/very/long/path/that/makes/grouping/profitable/a.go",
            3,
        );
        let b1 = name_occurrence_match("/repo/b.go", 2);
        let matches = vec![a1, b1, a2];
        let mut out = String::new();
        append_facet_grouped_rows(&mut out, &matches, scope);
        // The A group is emitted at A1's position (before B1); A2 is consumed.
        let group_pos = out.find("[2 matches]").unwrap();
        let b1_pos = out.find("b.go:2").unwrap();
        assert!(
            group_pos < b1_pos,
            "profitable group must emit at first occurrence before B1, got:\n{out}"
        );
        assert!(
            out.contains(":1") && out.contains(":3"),
            "profitable group must contain both child rows, got:\n{out}"
        );
    }
}
