use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::classify::{self, classify};
use crate::commands::context::{
    with_artifact_note, with_artifact_read_label, ArtifactMode, ExpandedCtx,
};
use crate::commands::multi_scope::{
    parse_multi_symbol_query, unsupported_find_syntax_error, use_files_error,
};
use crate::commands::section_disambiguation::disambiguate_glob_for_section;
use crate::evidence::owner_links::OWNER_NON_GO_CAVEAT;
use crate::evidence::owner_links::{
    build_owner_link_evidence, OwnerAnchor, OwnerCallMechanism, OwnerLinkEvidence,
    OwnerLinkHitInput, OWNER_LINK_CAVEAT, OWNER_LINK_EDGE_CAP, OWNER_LINK_ZERO_EDGE,
};
use crate::evidence::{render_next_actions, NextAction};
use crate::types::{Match, QueryType, RegexCoOccurrenceQuery, RegexTextKind, RegexTextQuery};
use crate::OutlineCache;
use crate::SrcwalkError;
use crate::{artifact, budget, format, index, read, search, session};

const MAX_TEXT_OR_TERMS: usize = 8;
const DEFAULT_TEXT_OR_TERM_LIMIT: usize = 10;
const TEXT_OR_COMPACT_MIN_TERMS: usize = 3;
const TEXT_OR_COMPACT_MIN_MATCHES: usize = 30;
const TEXT_OR_ROLLUP_FILE_LIMIT: usize = 8;
const TEXT_OR_ROLLUP_LINE_LIMIT: usize = 6;
const TEXT_OR_WINDOW_CONTEXT_LINES: u32 = 10;
const TEXT_OR_WINDOW_LIMIT: usize = 3;
const TEXT_OR_WINDOW_MAX_SPAN: u32 = 80;

/// Mechanism tags + `calls NAME` honesty note, in a one-line legend framing.
const OWNER_LINK_MECH_LEGEND: &str =
    "[recv=same package-qualified receiver type; local=single-assignment constructor; calls NAME=call-expression name, not candidate binding; bare=same-package invocation]";

/// Short stable tag for a call mechanism, kept plain-language.
fn owner_call_mechanism_tag(m: OwnerCallMechanism) -> &'static str {
    match m {
        OwnerCallMechanism::SingleAssignmentLocalConstructor => "local",
        OwnerCallMechanism::CrossFileSameQualifiedReceiver
        | OwnerCallMechanism::SameFileSameQualifiedReceiver => "recv",
        OwnerCallMechanism::SamePackageBareInvocation => "bare",
    }
}

fn comma_terms(query: &str) -> Vec<&str> {
    query
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .collect()
}

/// Original 1-based comma-separated term positions (before dedup/drop).
/// Empty slots are dropped but NOT renumbered, so `#N` is honest occurrence
/// identity: `alpha,alpha,,missing,beta` -> [(1,alpha),(2,alpha),(4,missing),(5,beta)].
fn indexed_comma_terms(query: &str) -> Vec<(usize, &str)> {
    query
        .split(',')
        .enumerate()
        .filter_map(|(i, t)| {
            let t = t.trim();
            if t.is_empty() {
                None
            } else {
                Some((i + 1, t))
            }
        })
        .collect()
}

/// classify → match on query type → return formatted string.
pub(crate) fn run(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

pub(crate) fn run_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        false,
        cache,
    )
}

pub(crate) fn run_text_filtered_with_artifact(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_text_filtered_with_artifact_and_hint(
        query,
        scope,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        artifact,
        false,
        cache,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_text_filtered_with_artifact_and_hint(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    literal_comma_hint: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let mut result =
        search::search_content_raw_with_artifact_counting(query, scope, glob, artifact)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    let advisory = search::low_signal_term_advisory(&search::low_signal_term_stats(query, &result));
    search::pagination::paginate(&mut result, limit, offset);
    search::compact_artifact_snippets(&mut result, artifact);
    let mut output = search::format_raw_result(&result, cache)?;
    if literal_comma_hint && result.total_found == 0 {
        output.push_str(
            "\n\n> Hint: treated as one literal text query. Use `--match any --as text` for comma-separated literal OR, or `--match all --as text` for same-file co-occurrence.",
        );
    }
    let output = match advisory {
        Some(note) => search::insert_low_signal_advisories(output, std::slice::from_ref(&note)),
        None => output,
    };
    let output = with_artifact_note(output, artifact);
    match budget_tokens {
        Some(budget) => Ok(budget::apply_preserving_footer(&output, budget)),
        None => Ok(output),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_text_or_filtered_with_artifact(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let terms = comma_terms(query);
    let indexed_terms = indexed_comma_terms(query);
    if terms.len() < 2 {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "discover --match any --as text requires 2-8 comma-separated terms".to_string(),
        });
    }
    if terms.len() > MAX_TEXT_OR_TERMS {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "discover --match any --as text supports 2-8 terms".to_string(),
        });
    }

    let term_limit = limit.unwrap_or(DEFAULT_TEXT_OR_TERM_LIMIT);
    let mut total_found = 0usize;
    let mut total_files = BTreeSet::new();
    let mut term_results = Vec::with_capacity(terms.len());
    let mut advisories = Vec::new();

    for (query_term_index, term) in &indexed_terms {
        let mut result =
            search::search_content_raw_with_artifact_counting(term, scope, glob, artifact)?;
        search::apply_general_filter(&mut result, scope, cache, filter)?;
        if let Some(note) =
            search::low_signal_term_advisory(&search::low_signal_term_stats(term, &result))
        {
            advisories.push(note);
        }
        total_found += result.total_found;
        let file_count = result
            .matches
            .iter()
            .map(|m| {
                total_files.insert(m.path.clone());
                &m.path
            })
            .collect::<BTreeSet<_>>()
            .len();

        search::pagination::paginate(&mut result, Some(term_limit), offset);
        search::compact_artifact_snippets(&mut result, artifact);
        let shown_so_far = result.offset + result.matches.len();
        let omitted = result.total_found.saturating_sub(shown_so_far);

        term_results.push(TextOrTermResult {
            query_term_index: *query_term_index,
            term: (*term).to_string(),
            total_found: result.total_found,
            file_count,
            matches: result.matches,
            omitted,
        });
    }

    let owner_links = if artifact.enabled() {
        OwnerLinkEvidence::default()
    } else {
        let inputs = term_results
            .iter()
            .flat_map(|result| {
                result.matches.iter().map(|matched| OwnerLinkHitInput {
                    path: &matched.path,
                    line: matched.line,
                })
            })
            .collect::<Vec<_>>();
        build_owner_link_evidence(&inputs)
    };

    let compact =
        terms.len() >= TEXT_OR_COMPACT_MIN_TERMS || total_found > TEXT_OR_COMPACT_MIN_MATCHES;
    let (rendered, has_specific_next) = if compact {
        let rollup = render_text_or_file_rollup(&term_results, scope, &owner_links);
        (rollup.body, rollup.has_specific_next)
    } else {
        (
            render_text_or_term_details(&term_results, term_limit, scope, &owner_links),
            false,
        )
    };

    let mut output = format!(
        "# Text OR: \"{}\" in {} — {} terms, {} matches, {} {}\n> Caveat: literal OR text evidence only; not semantic relation proof.{}",
        query,
        format::display_path(scope),
        terms.len(),
        total_found,
        total_files.len(),
        text_or_file_word(total_files.len()),
        rendered
    );
    output.push_str(&render_owner_link_appendix(&owner_links, scope));
    if !advisories.is_empty() {
        output = search::insert_low_signal_advisories(output, &advisories);
    }
    if total_found > 0 && (!has_specific_next || artifact.enabled()) {
        let rendered = render_next_actions(&[NextAction::guidance(
            "read raw hit evidence with `srcwalk show <path>:<line> -C 10`.",
            "text-or hit drilldown",
            40,
        )]);
        if !rendered.is_empty() {
            output.push_str("\n\n");
            output.push_str(&rendered);
        }
    }
    let output = with_artifact_note(output, artifact);
    match budget_tokens {
        Some(budget) => Ok(budget::apply_preserving_footer(&output, budget)),
        None => Ok(output),
    }
}

struct TextOrTermResult {
    /// 1-based position in the ORIGINAL comma-separated user input, before any
    /// dedup/normalization/drop. Zero-hit and empty-invalid slots keep their
    /// original index (never renumbered), so `#N` is honest occurrence identity.
    query_term_index: usize,
    term: String,
    total_found: usize,
    file_count: usize,
    matches: Vec<Match>,
    omitted: usize,
}

fn render_text_or_term_details(
    term_results: &[TextOrTermResult],
    term_limit: usize,
    scope: &Path,
    owner_links: &OwnerLinkEvidence,
) -> String {
    use std::fmt::Write as _;

    let mut rendered = String::new();
    for result in term_results {
        let shown = result.matches.len();
        let _ = write!(
            rendered,
            "\n\n## {} — {shown}/{} matches",
            result.term, result.total_found
        );
        render_text_or_term_matches(&mut rendered, &result.matches, scope, owner_links);
        if result.omitted > 0 {
            let _ = write!(
                rendered,
                "\n  > Note: {} more `{}` matches omitted by per-term limit {term_limit}; increase --limit or narrow terms.",
                result.omitted, result.term
            );
        }
    }
    rendered
}

/// Render one term's visible matches, grouping same-file hits >= 2 and folding
/// contiguous equal-owner runs of >= 3 (owner K=3). Single-hit files keep the
/// current one-line shape. For multi-hit files, the grouped candidate wins only
/// when it is strictly fewer UTF-8 bytes than the ungrouped inline rows; a tie
/// keeps the current ungrouped inline shape.
///
/// Ordering semantics: the per-file decision (grouped vs inline) is precomputed
/// once, then the ORIGINAL visible slice is iterated in order. A profitable
/// (grouped) file emits its whole group at the first occurrence of that path and
/// later occurrences are consumed; an unprofitable or single-hit file emits each
/// row at its original position. This keeps rejected/tied file rows in their
/// original relative order instead of clustering them together.
fn render_text_or_term_matches(
    rendered: &mut String,
    matches: &[Match],
    scope: &Path,
    owner_links: &OwnerLinkEvidence,
) {
    use std::collections::HashMap;
    use std::fmt::Write as _;

    // Group by path preserving first-appearance order.
    let mut files: Vec<(&Path, Vec<&Match>)> = Vec::new();
    for m in matches {
        match files.iter_mut().find(|(p, _)| *p == m.path.as_path()) {
            Some((_, group)) => group.push(m),
            None => files.push((m.path.as_path(), vec![m])),
        }
    }

    // Precompute the per-path rendering decision: `Some(grouped)` when the
    // grouped candidate is strictly smaller than the inline rows, `None` when
    // the file is single-hit or grouping ties/loses (emits rows inline).
    let mut grouped_by_path: HashMap<&Path, String> = HashMap::new();
    for (path, group) in &files {
        if group.len() < 2 {
            continue;
        }
        let mut grouped_candidate = String::new();
        let _ = write!(
            grouped_candidate,
            "\n  {} [{} matches]",
            format::rel_nonempty(path, scope),
            group.len()
        );
        render_owner_runs(&mut grouped_candidate, group, owner_links);

        let mut ungrouped_candidate = String::new();
        render_text_or_term_ungrouped(&mut ungrouped_candidate, group, path, scope, owner_links);

        if grouped_candidate.len() < ungrouped_candidate.len() {
            grouped_by_path.insert(path, grouped_candidate);
        }
    }

    // Iterate the original visible slice. Emit a profitable group once at its
    // first occurrence; emit every other row at its original position.
    let mut emitted: std::collections::HashSet<&Path> = std::collections::HashSet::new();
    for m in matches {
        let path = m.path.as_path();
        if let Some(grouped) = grouped_by_path.get(path) {
            if emitted.insert(path) {
                rendered.push_str(grouped);
            }
            // Later occurrences of an emitted profitable group are consumed.
            continue;
        }
        // Single-hit or unprofitable/tied: emit this row at its position.
        render_text_or_term_ungrouped(rendered, &[m], path, scope, owner_links);
    }
}

/// Render every hit in a file as a one-line inline row (the ungrouped shape).
fn render_text_or_term_ungrouped(
    rendered: &mut String,
    group: &[&Match],
    path: &Path,
    scope: &Path,
    owner_links: &OwnerLinkEvidence,
) {
    use std::fmt::Write as _;
    for m in group {
        let owner = owner_tag_inline(owner_links, m);
        let _ = write!(
            rendered,
            "\n  {}:{}{} — {}",
            format::rel_nonempty(path, scope),
            m.line,
            owner,
            m.text.trim()
        );
    }
}

/// Render a multi-hit file group's child rows, folding contiguous runs of >= 3
/// hits with the exact same owner anchor into an owner subgroup header.
fn render_owner_runs(rendered: &mut String, group: &[&Match], owner_links: &OwnerLinkEvidence) {
    use std::fmt::Write as _;

    let mut i = 0;
    while i < group.len() {
        let owner = owner_links.owner_for(&group[i].path, group[i].line);
        // Find the contiguous run sharing the same owner anchor.
        let mut j = i + 1;
        while j < group.len()
            && owners_equal(owner, owner_links.owner_for(&group[j].path, group[j].line))
        {
            j += 1;
        }
        let run_len = j - i;
        if let Some(owner) = owner.filter(|_| run_len >= 3) {
            // K=3 fold: owner once as a subgroup header.
            let _ = write!(
                rendered,
                "\n    [owner {}@{}-{}] [{} hits]",
                owner.qualified_name(),
                owner.start_line,
                owner.end_line,
                run_len
            );
            for m in &group[i..j] {
                let _ = write!(rendered, "\n      :{} — {}", m.line, m.text.trim());
            }
        } else {
            // Runs of 1-2, unique/mixed owners, or unattributed stay inline.
            for m in &group[i..j] {
                let owner = owner_tag_inline(owner_links, m);
                let _ = write!(rendered, "\n    :{}{} — {}", m.line, owner, m.text.trim());
            }
        }
        i = j;
    }
}

/// Owner anchor tag with a leading space for inline hit rows, or empty string.
fn owner_tag_inline(owner_links: &OwnerLinkEvidence, m: &Match) -> String {
    owner_links
        .owner_for(&m.path, m.line)
        .map_or_else(String::new, |owner| {
            format!(
                " [owner {}@{}-{}]",
                owner.qualified_name(),
                owner.start_line,
                owner.end_line
            )
        })
}

/// Whether two owner anchors are the same (same qualified name and range).
fn owners_equal(a: Option<&OwnerAnchor>, b: Option<&OwnerAnchor>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => {
            x.qualified_name() == y.qualified_name()
                && x.start_line == y.start_line
                && x.end_line == y.end_line
        }
        (None, None) => true,
        _ => false,
    }
}

struct TextOrRollupRender {
    body: String,
    has_specific_next: bool,
}

fn render_text_or_file_rollup(
    term_results: &[TextOrTermResult],
    scope: &Path,
    owner_links: &OwnerLinkEvidence,
) -> TextOrRollupRender {
    use std::fmt::Write as _;

    let mut by_path: BTreeMap<PathBuf, TextOrFileRollup> = BTreeMap::new();
    for result in term_results {
        for m in &result.matches {
            let entry = by_path
                .entry(m.path.clone())
                .or_insert_with(|| TextOrFileRollup::new(m.path.clone()));
            *entry.term_counts.entry(result.term.clone()).or_insert(0) += 1;
            entry.shown_matches += 1;
            entry.lines.insert(m.line);
            *entry
                .line_terms
                .entry(m.line)
                .or_default()
                .entry(result.term.clone())
                .or_insert(0) += 1;
        }
    }

    let mut files = by_path.into_values().collect::<Vec<_>>();
    files.sort_by(|a, b| {
        b.term_counts
            .len()
            .cmp(&a.term_counts.len())
            .then(a.is_test.cmp(&b.is_test))
            .then(b.shown_matches.cmp(&a.shown_matches))
            .then(a.path.cmp(&b.path))
    });

    let mut rendered = String::new();
    let mut has_specific_next = false;
    rendered.push_str("\n\n## Files ranked by term coverage");
    if files.is_empty() {
        rendered.push_str("\n(no files matched shown terms)");
    }
    for file in files.iter().take(TEXT_OR_ROLLUP_FILE_LIMIT) {
        let rel = format::rel_nonempty(&file.path, scope);
        let _ = write!(
            rendered,
            "\n{} — {} {}, {} shown matches",
            rel,
            file.term_counts.len(),
            text_or_term_word(file.term_counts.len()),
            file.shown_matches
        );
        let terms = file
            .term_counts
            .iter()
            .map(|(term, count)| format!("{term}({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        let lines = file
            .lines
            .iter()
            .take(TEXT_OR_ROLLUP_LINE_LIMIT)
            .map(|line| format!(":{line}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(rendered, "\n  terms: {terms}");
        let _ = write!(rendered, "\n  hits: {lines}");
        if file.lines.len() > TEXT_OR_ROLLUP_LINE_LIMIT {
            let _ = write!(
                rendered,
                ", +{} more shown lines",
                file.lines.len() - TEXT_OR_ROLLUP_LINE_LIMIT
            );
        }
        render_text_or_owner_rollup(&mut rendered, &file.path, owner_links, term_results);
        let windows = text_or_select_hit_windows(file);
        if !windows.is_empty() {
            let summaries = windows
                .iter()
                .map(text_or_window_summary)
                .collect::<Vec<_>>()
                .join("; ");
            let sections = windows
                .iter()
                .map(text_or_window_range)
                .collect::<Vec<_>>()
                .join(",");
            let omitted_windows = text_or_hit_windows(file)
                .len()
                .saturating_sub(windows.len());
            let _ = write!(rendered, "\n  windows: {summaries}");
            if let (Some(rel_arg), Some(section_arg)) = (
                format::shell_quote_arg(&rel),
                format::shell_quote_arg(&sections),
            ) {
                let _ = write!(
                    rendered,
                    "\n  > Next: srcwalk show {rel_arg} --section {section_arg} -C 10"
                );
                has_specific_next = true;
            }
            if omitted_windows > 0 {
                let _ = write!(
                    rendered,
                    "\n  > Note: {omitted_windows} lower-ranked hit windows omitted from next read; use shown hits above or narrow terms."
                );
            }
        }
    }
    if files
        .iter()
        .any(|file| !text_or_hit_windows(file).is_empty())
    {
        rendered.push_str(
            "\n> Caveat: hit-window proximity is literal navigation evidence, not semantic relation proof.",
        );
    }
    if files.len() > TEXT_OR_ROLLUP_FILE_LIMIT {
        let _ = write!(
            rendered,
            "\n> Note: {} more files omitted from rollup; increase --limit or narrow terms.",
            files.len() - TEXT_OR_ROLLUP_FILE_LIMIT
        );
    }

    rendered.push_str("\n\n## Terms");
    for result in term_results {
        let shown = result.matches.len();
        let _ = write!(
            rendered,
            "\n{} — {shown}/{} matches, {} {}",
            result.term,
            result.total_found,
            result.file_count,
            text_or_file_word(result.file_count)
        );
        if result.omitted > 0 {
            let _ = write!(rendered, "; {} omitted by per-term limit", result.omitted);
        }
    }
    TextOrRollupRender {
        body: rendered,
        has_specific_next,
    }
}

fn render_text_or_owner_rollup(
    rendered: &mut String,
    path: &Path,
    owner_links: &OwnerLinkEvidence,
    term_results: &[TextOrTermResult],
) {
    use std::fmt::Write as _;

    // Aggregate owners by each indexed result's matches + owner attribution.
    // `#N` is the ORIGINAL comma-separated position (query_term_index), so
    // exact duplicate terms keep distinct honest indices. `#N` alone = count 1;
    // `#N*K` = count K.
    let mut owners: BTreeMap<OwnerAnchor, BTreeMap<usize, usize>> = BTreeMap::new();
    for result in term_results {
        for m in &result.matches {
            if m.path != *path {
                continue;
            }
            if let Some(owner) = owner_links.owner_for(&m.path, m.line) {
                *owners
                    .entry(owner.clone())
                    .or_default()
                    .entry(result.query_term_index)
                    .or_insert(0) += 1;
            }
        }
    }
    if owners.is_empty() {
        return;
    }
    let summaries = owners
        .into_iter()
        .map(|(owner, terms)| {
            let terms = terms
                .into_iter()
                .map(|(index, count)| {
                    if count <= 1 {
                        format!("#{index}")
                    } else {
                        format!("#{index}*{count}")
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            // The file path is already explicit in the enclosing file-scoped
            // rollup header, so owner anchors use `:start-end` only.
            format!(
                "{}:{}-{}[{terms}]",
                owner.qualified_name(),
                owner.start_line,
                owner.end_line
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let _ = write!(
        rendered,
        "\n  owners (#N=Nth query term; *K=hits): {summaries}"
    );
}

fn render_owner_link_appendix(owner_links: &OwnerLinkEvidence, scope: &Path) -> String {
    use std::fmt::Write as _;

    if !owner_links.has_owners() {
        return String::new();
    }
    let mut rendered = String::new();

    // The Go mechanical-call appendix (zero-edge sentence, "## Mechanical Go
    // calls", and its call-specific caveat) is gated only by an explicit,
    // SUCCESSFUL Go call-analysis attempt PLUS actual Go owner evidence. It is
    // never driven by non-Go owner presence.
    if owner_links.go_call_analysis_attempted && owner_links.has_go_owners() {
        if owner_links.edges.is_empty() {
            // The zero-edge sentence means "no direct call evidence among the
            // Go hit owners". It is only valid for a Go-only result, and only
            // when at least two distinct Go owners exist (so the claim is
            // meaningful). Non-Go owners must not satisfy its threshold.
            if !owner_links.has_non_go_owners() && owner_links.attributed_go_owner_count() >= 2 {
                let _ = write!(rendered, "\n\n{OWNER_LINK_ZERO_EDGE}");
            }
        } else {
            rendered.push_str("\n\n## Mechanical Go calls ");
            rendered.push_str(OWNER_LINK_MECH_LEGEND);
            for edge in owner_links.edges.iter().take(OWNER_LINK_EDGE_CAP) {
                let call_path = format::rel_nonempty(&edge.caller.path, scope);
                let cand_path = format::rel_nonempty(&edge.candidate.path, scope);
                // `@:` explicitly means the same call file; cross-file candidates
                // keep the full repo-relative path.
                let cand_loc = if call_path == cand_path {
                    format!(
                        "@:{}-{}",
                        edge.candidate.start_line, edge.candidate.end_line
                    )
                } else {
                    format!(
                        "@{cand_path}:{}-{}",
                        edge.candidate.start_line, edge.candidate.end_line
                    )
                };
                let _ = write!(
                    rendered,
                    "\n- [{}] {} calls {}@{call_path}:{}; candidate {}{cand_loc}",
                    owner_call_mechanism_tag(edge.mechanism),
                    edge.caller.qualified_name(),
                    edge.callee_name,
                    edge.call_line,
                    edge.candidate.qualified_name()
                );
            }
        }
        let _ = write!(rendered, "\n\n{OWNER_LINK_CAVEAT}");
    }

    // Non-Go owner attribution gets its own concise honesty caveat, gated by
    // actually rendered non-Go owner evidence. It must not imply call analysis
    // ran for non-Go languages.
    if owner_links.has_non_go_owners() {
        let _ = write!(rendered, "\n\n{OWNER_NON_GO_CAVEAT}");
    }
    rendered
}

struct TextOrFileRollup {
    path: PathBuf,
    term_counts: BTreeMap<String, usize>,
    shown_matches: usize,
    lines: BTreeSet<u32>,
    line_terms: BTreeMap<u32, BTreeMap<String, usize>>,
    is_test: bool,
}

impl TextOrFileRollup {
    fn new(path: PathBuf) -> Self {
        let is_test = text_or_is_test_path(&path);
        Self {
            path,
            term_counts: BTreeMap::new(),
            shown_matches: 0,
            lines: BTreeSet::new(),
            line_terms: BTreeMap::new(),
            is_test,
        }
    }
}

#[derive(Debug, Clone)]
struct TextOrHitWindow {
    start: u32,
    end: u32,
    term_counts: BTreeMap<String, usize>,
    hit_count: usize,
}

impl TextOrHitWindow {
    fn new(line: u32, terms: &BTreeMap<String, usize>) -> Self {
        let mut window = Self {
            start: line,
            end: line,
            term_counts: BTreeMap::new(),
            hit_count: 0,
        };
        window.add_line(line, terms);
        window
    }

    fn add_line(&mut self, line: u32, terms: &BTreeMap<String, usize>) {
        self.end = self.end.max(line);
        for (term, count) in terms {
            *self.term_counts.entry(term.clone()).or_insert(0) += *count;
            self.hit_count += *count;
        }
    }

    fn term_count(&self) -> usize {
        self.term_counts.len()
    }

    fn span(&self) -> u32 {
        self.end.saturating_sub(self.start) + 1
    }
}

fn text_or_hit_windows(file: &TextOrFileRollup) -> Vec<TextOrHitWindow> {
    let mut windows = Vec::new();
    let mut current: Option<TextOrHitWindow> = None;
    let merge_gap = TEXT_OR_WINDOW_CONTEXT_LINES
        .saturating_mul(2)
        .saturating_add(1);

    for (line, terms) in &file.line_terms {
        match current.as_mut() {
            Some(window)
                if *line <= window.end.saturating_add(merge_gap)
                    && line.saturating_sub(window.start).saturating_add(1)
                        <= TEXT_OR_WINDOW_MAX_SPAN =>
            {
                window.add_line(*line, terms);
            }
            Some(_) => {
                windows.push(current.take().expect("current window exists"));
                current = Some(TextOrHitWindow::new(*line, terms));
            }
            None => current = Some(TextOrHitWindow::new(*line, terms)),
        }
    }

    if let Some(window) = current {
        windows.push(window);
    }

    windows
}

fn text_or_select_hit_windows(file: &TextOrFileRollup) -> Vec<TextOrHitWindow> {
    let mut remaining = text_or_hit_windows(file);
    let mut selected = Vec::new();
    let mut covered_terms = BTreeSet::new();

    while selected.len() < TEXT_OR_WINDOW_LIMIT
        && !remaining.is_empty()
        && (covered_terms.len() < file.term_counts.len()
            || remaining.iter().any(|window| window.term_count() > 1))
    {
        let best_idx = remaining
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| text_or_compare_windows(a, b, &covered_terms))
            .map(|(idx, _)| idx)
            .expect("remaining is not empty");
        let window = remaining.remove(best_idx);
        covered_terms.extend(window.term_counts.keys().cloned());
        selected.push(window);
    }

    selected.sort_by_key(|window| window.start);
    selected
}

fn text_or_compare_windows(
    a: &TextOrHitWindow,
    b: &TextOrHitWindow,
    covered_terms: &BTreeSet<String>,
) -> std::cmp::Ordering {
    let a_new_terms = a
        .term_counts
        .keys()
        .filter(|term| !covered_terms.contains(*term))
        .count();
    let b_new_terms = b
        .term_counts
        .keys()
        .filter(|term| !covered_terms.contains(*term))
        .count();

    a_new_terms
        .cmp(&b_new_terms)
        .then(a.term_count().cmp(&b.term_count()))
        .then(a.hit_count.cmp(&b.hit_count))
        .then(b.span().cmp(&a.span()))
        .then(b.start.cmp(&a.start))
}

fn text_or_window_range(window: &TextOrHitWindow) -> String {
    format!("{}-{}", window.start, window.end)
}

fn text_or_window_summary(window: &TextOrHitWindow) -> String {
    let terms = window
        .term_counts
        .iter()
        .map(|(term, count)| format!("{term}({count})"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(":{}-{} terms={terms}", window.start, window.end)
}

fn text_or_file_word(count: usize) -> &'static str {
    if count == 1 {
        "file"
    } else {
        "files"
    }
}

fn text_or_term_word(count: usize) -> &'static str {
    if count == 1 {
        "term"
    } else {
        "terms"
    }
}

fn text_or_is_test_path(path: &Path) -> bool {
    path.components().any(|component| {
        let segment = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        segment == "test"
            || segment == "tests"
            || segment == "spec"
            || segment == "specs"
            || segment == "__tests__"
            || segment.starts_with("test_")
            || segment.ends_with("_test")
            || segment.ends_with("_spec")
            || segment.contains("_test.")
            || segment.contains(".test.")
            || segment.contains("_spec.")
            || segment.contains(".spec.")
    })
}
pub(crate) fn run_text_expanded_filtered(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    search::search_content_expanded(
        query,
        scope,
        cache,
        &session::Session::new(),
        expand,
        None,
        limit,
        offset,
        glob,
        filter,
        budget_tokens,
    )
}

pub(crate) fn run_cooccurrence_filtered_with_artifact(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let terms = comma_terms(query);
    if terms.len() < 2 {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "discover --match all requires 2-5 comma-separated terms".to_string(),
        });
    }
    if terms.len() > 5 {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "discover --match all supports 2-5 terms".to_string(),
        });
    }

    let mut by_path: BTreeMap<PathBuf, (BTreeSet<usize>, Vec<crate::types::Match>)> =
        BTreeMap::new();
    for (idx, term) in terms.iter().enumerate() {
        let mut result = search::search_content_raw_with_artifact(term, scope, glob, artifact)?;
        search::apply_general_filter(&mut result, scope, cache, filter)?;
        search::compact_artifact_snippets(&mut result, artifact);
        for m in result.matches {
            let entry = by_path.entry(m.path.clone()).or_default();
            entry.0.insert(idx);
            entry.1.push(m);
        }
    }

    let mut matches = Vec::new();
    for (_path, (seen_terms, mut path_matches)) in by_path {
        if seen_terms.len() == terms.len() {
            matches.append(&mut path_matches);
        }
    }
    matches.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.cmp(&b.line))
            .then(a.text.cmp(&b.text))
    });
    matches.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.text == b.text);

    if matches.is_empty() {
        return Err(SrcwalkError::NoMatches {
            query: query.to_string(),
            scope: scope.to_path_buf(),
            suggestion: None,
            guidance: None,
        });
    }

    let definitions = matches.iter().filter(|m| m.is_definition).count();
    let comments = matches.iter().filter(|m| m.in_comment).count();
    let usages = matches.len().saturating_sub(definitions + comments);
    let file_count = matches
        .iter()
        .map(|m| &m.path)
        .collect::<BTreeSet<_>>()
        .len();
    let mut result = crate::types::SearchResult {
        query: query.to_string(),
        scope: scope.to_path_buf(),
        total_found: matches.len(),
        eligible_files: 0,
        definition_candidates: definitions,
        name_occurrence_candidates: matches
            .iter()
            .filter(|m| m.is_name_occurrence_candidate())
            .count(),
        matches,
        definitions,
        usages,
        comments,
        has_more: false,
        offset: 0,
    };
    search::pagination::paginate(&mut result, limit, offset);
    let header = format!(
        "# Co-occurrence: \"{}\" in {} — {} files contain all {} terms, {} matches\n> Caveat: same-file co-occurrence only; not semantic relation proof.",
        result.query,
        crate::format::display_path(scope),
        file_count,
        terms.len(),
        result.total_found
);
    let output = search::format_raw_result_with_header(&result, cache, header)?;
    let output = with_artifact_note(output, artifact);
    match budget_tokens {
        Some(budget) => Ok(budget::apply_preserving_footer(&output, budget)),
        None => Ok(output),
    }
}

pub(crate) fn run_access_filtered(
    query: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let output = search::access::search_access(query, scope, cache, limit, offset, glob, filter)?;
    match budget_tokens {
        Some(budget) => Ok(budget::apply_preserving_footer(&output, budget)),
        None => Ok(output),
    }
}

pub(crate) fn run_filtered_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        false,
        0,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::from(artifact),
        cache,
    )
}

/// Full variant — forces full file output, bypassing smart views.
pub(crate) fn run_full(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_full_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

pub(crate) fn run_full_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_full_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        false,
        cache,
    )
}

pub(crate) fn run_full_filtered_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        true,
        0,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::from(artifact),
        cache,
    )
}

/// Run with expanded search — inline source for top N matches.
pub(crate) fn run_expanded(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_expanded_filtered(
        query,
        scope,
        section,
        budget_tokens,
        full,
        expand,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_expanded_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        full,
        expand,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::Source,
        cache,
    )
}

pub(crate) fn run_files(
    pattern: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    let output = search::search_files_glob_with_exclude(pattern, scope, limit, offset, exclude)?;
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}

pub(crate) fn run_files_with_scope_filter(
    pattern: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    scope_glob: Option<&str>,
    exclude: Option<&str>,
) -> Result<String, SrcwalkError> {
    let output = search::search_files_glob_with_scope_filter(
        pattern, scope, scope_glob, limit, offset, exclude,
    )?;
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}

fn run_inner(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    if let Some(err) = unsupported_find_syntax_error(query) {
        return Err(err);
    }

    // US-071 Step 4 (amendment): pre-parse comma-separated multi-symbol
    // candidates BEFORE automatic classification. A dotted part can make the
    // whole string look like a filename (QueryType::Glob), which the old
    // classify-first gate then wrongly excluded. `parse_multi_symbol_query`
    // already rejects real globs (`*?{[`), regexes, paths, and section syntax
    // because it requires every part to be a bare/dotted identifier, so a real
    // file/glob/regex/path/section query falls through to `classify` unchanged.
    // Explicit `--as text|glob|regex|path` modes are dispatched before
    // `run_inner` and never reach this route.
    if let Some(parts) = parse_multi_symbol_query(query)? {
        let session = session::Session::new();
        let sym_index = index::SymbolIndex::new();
        let bloom = index::bloom::BloomFilterCache::new();
        let expand = if expand > 0 { expand } else { 2 };
        let output = search::search_multi_symbol_expanded(
            &parts,
            scope,
            cache,
            &session,
            &sym_index,
            &bloom,
            expand,
            None,
            limit,
            offset,
            glob,
            filter,
            budget_tokens,
        )?;
        return match budget_tokens {
            Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
            None => Ok(output),
        };
    }

    let config_cache = crate::lang::tsconfig::ConfigCache::new();

    let query_type = classify(query, scope);
    // P1.2 — disambiguate bare-filename + --section.
    // Glob classification swallows `--section` silently for bare filenames like
    // `Cart.php`. When section is set, resolve the glob now: pick the prod
    // candidate if exactly one survives test/vendor filtering, else fail loud.
    let mut resolution_note: Option<String> = None;
    let query_type = if section.is_some() {
        if let QueryType::Glob(pattern) = &query_type {
            match disambiguate_glob_for_section(pattern, scope, query)? {
                Some((picked, note)) => {
                    resolution_note = note;
                    QueryType::FilePath(picked)
                }
                None => query_type,
            }
        } else {
            query_type
        }
    } else {
        query_type
    };

    let use_expanded = expand > 0
        && !matches!(
            query_type,
            QueryType::FilePath(_)
                | QueryType::FilePathLine(_, _)
                | QueryType::FilePathSection(_, _)
                | QueryType::Glob(_)
        );

    let output_result = match query_type {
        QueryType::FilePath(_) | QueryType::FilePathLine(_, _) | QueryType::Glob(_)
            if filter.is_some() =>
        {
            Err(SrcwalkError::InvalidQuery {
                query: query.to_string(),
                reason:
                    "--filter applies to discover results and direct trace callers, not file/glob reads"
                        .to_string(),
            })
        }
        QueryType::FilePath(path) => {
            let mut out = if artifact.enabled() {
                if let Some(symbol) = section {
                    if let Some(result) =
                        artifact::read_js_ts_symbol_section(&path, symbol, budget_tokens)
                    {
                        result?
                    } else {
                        read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
                    }
                } else {
                    read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
                }
            } else {
                read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
            };
            out = with_artifact_read_label(out, artifact);
            if section.is_none() && !full {
                out = artifact::add_anchors(out, &path, artifact);
            }
            if section.is_none()
                && !full
                && read::would_outline(&path)
                && !artifact.enabled()
                && !crate::capabilities::is_binary_artifact_path(&path)
            {
                let related = fs::read_to_string(&path)
                    .ok()
                    .map(|content| {
                        read::imports::resolve_related_files_with_content_and_scope(
                            &path,
                            &content,
                            scope,
                            &config_cache,
                            Some(8),
                        )
                    })
                    .unwrap_or_default();
                if !related.is_empty() {
                    let hints: Vec<String> = related
                        .iter()
                        .map(|p| format::rel_nonempty(p, scope))
                        .collect();
                    out.push_str("\n\n> Related: ");
                    out.push_str(&hints.join(", "));
                }
                let rendered = render_next_actions(&[NextAction::guidance(
                    "use `srcwalk deps <file>` to see imports and dependents",
                    "file dependency drilldown",
                    40,
                )]);
                if !rendered.is_empty() {
                    out.push('\n');
                    out.push_str(&rendered);
                }
            }
            Ok(out)
        }
        QueryType::FilePathLine(path, line) => {
            let line_section = line.to_string();
            let effective_section = section.unwrap_or(&line_section);
            let out = if artifact.enabled() {
                if let Some(result) =
                    artifact::read_js_ts_symbol_section(&path, effective_section, budget_tokens)
                {
                    result?
                } else {
                    read::read_file_with_budget(
                        &path,
                        Some(effective_section),
                        full,
                        budget_tokens,
                        cache,
                    )?
                }
            } else {
                read::read_file_with_budget(
                    &path,
                    Some(effective_section),
                    full,
                    budget_tokens,
                    cache,
                )?
            };
            Ok(with_artifact_read_label(out, artifact))
        }
        QueryType::FilePathSection(path, path_section) => {
            let effective_section = section.unwrap_or(&path_section);
            let out = if artifact.enabled() {
                if let Some(result) =
                    artifact::read_js_ts_symbol_section(&path, effective_section, budget_tokens)
                {
                    result?
                } else {
                    read::read_file_with_budget(
                        &path,
                        Some(effective_section),
                        full,
                        budget_tokens,
                        cache,
                    )?
                }
            } else {
                read::read_file_with_budget(
                    &path,
                    Some(effective_section),
                    full,
                    budget_tokens,
                    cache,
                )?
            };
            Ok(with_artifact_read_label(out, artifact))
        }
        QueryType::RegexText(q) => run_regex_text(
            &q, scope, cache, limit, offset, glob, filter, artifact,
        ),
        QueryType::RegexCoOccurrence(q) => run_regex_cooccurrence(
            &q, scope, cache, limit, offset, glob, filter,
        ),
        QueryType::Glob(_) if classify::has_glob_chars(query) => Err(use_files_error(query)),
        QueryType::Glob(pattern) => search::search_files_glob(&pattern, scope, limit, offset),
        _ if use_expanded => {
            let ctx = ExpandedCtx {
                session: session::Session::new(),
                sym_index: index::SymbolIndex::new(),
                bloom: index::bloom::BloomFilterCache::new(),
                expand,
                budget_tokens,
            };
            run_query_expanded(&query_type, scope, cache, &ctx, limit, offset, glob, filter)
        }
        _ => run_query_basic(
            &query_type,
            scope,
            cache,
            limit,
            offset,
            glob,
            filter,
            artifact,
        ),
    };

    let output = match output_result {
        Ok(output) => output,
        Err(err) => {
            return Err(match resolution_note {
                Some(note) => SrcwalkError::WithNote {
                    note,
                    source: Box::new(err),
                },
                None => err,
            });
        }
    };
    let output = with_artifact_note(output, artifact);

    let final_out = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    };
    Ok(match resolution_note {
        Some(note) => format!("{note}\n\n{final_out}"),
        None => final_out,
    })
}

fn should_error_missing_path_like_query(query: &str) -> bool {
    classify::looks_like_path_with_separator(query)
}
/// Dispatch search queries in expanded mode (inline source for top N matches).
/// Only called for search query types — FilePath/Glob are handled before this.
fn run_query_expanded(
    query_type: &QueryType,
    scope: &Path,
    cache: &OutlineCache,
    ctx: &ExpandedCtx,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) => search::search_symbol_expanded(
            name,
            scope,
            cache,
            &ctx.session,
            &ctx.sym_index,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::SymbolGlob(pattern) => search::search_symbol_glob_expanded(
            pattern,
            scope,
            cache,
            &ctx.session,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::Concept(text) if text.contains(' ') => search::search_content_expanded(
            text,
            scope,
            cache,
            &ctx.session,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::Concept(text) | QueryType::Fallthrough(text) => search::search_symbol_expanded(
            text,
            scope,
            cache,
            &ctx.session,
            &ctx.sym_index,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        // FilePath/Glob/Glob never reach here (gated by use_expanded)
        QueryType::RegexText(_)
        | QueryType::RegexCoOccurrence(_)
        | QueryType::FilePath(_)
        | QueryType::FilePathLine(_, _)
        | QueryType::FilePathSection(_, _)
        | QueryType::Glob(_) => {
            unreachable!("non-search query type in expanded path")
        }
    }
}

/// Dispatch search queries in basic mode (no expansion).
/// Only called for search query types — FilePath/Glob are handled before this.
fn run_query_basic(
    query_type: &QueryType,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) if artifact.enabled() => single_query_search(
            name, scope, cache, true, limit, offset, glob, filter, artifact,
        ),
        QueryType::Symbol(name) => search::search_symbol_with_artifact(
            name, scope, cache, limit, offset, glob, filter, artifact,
        ),
        QueryType::SymbolGlob(pattern) => search::search_symbol_glob_with_artifact(
            pattern, scope, cache, limit, offset, glob, filter, artifact,
        ),
        QueryType::Concept(text) if text.contains(' ') => {
            multi_word_concept_search(text, scope, cache, limit, offset, glob, filter, artifact)
        }
        QueryType::Concept(text) => single_query_search(
            text, scope, cache, true, limit, offset, glob, filter, artifact,
        ),
        QueryType::Fallthrough(text) => single_query_search(
            text, scope, cache, false, limit, offset, glob, filter, artifact,
        ),
        QueryType::RegexText(_)
        | QueryType::RegexCoOccurrence(_)
        | QueryType::FilePath(_)
        | QueryType::FilePathLine(_, _)
        | QueryType::FilePathSection(_, _)
        | QueryType::Glob(_) => {
            unreachable!("non-search query type in basic path")
        }
    }
}

/// Shared cascade for single-word queries: symbol → content → not found.
///
/// When `prefer_definitions` is true (Concept path), only accept symbol results
/// that contain actual definitions; fall back to content otherwise.
/// When false (Fallthrough path), accept any symbol match immediately.
fn filter_zero_guidance(filter: Option<&str>) -> Option<String> {
    let filter = filter?.trim();
    if filter.is_empty() {
        return None;
    }

    let kind_hint = if filter
        .split_whitespace()
        .any(|part| part.trim_start().starts_with("kind:"))
    {
        " kind filters match result row kinds such as fn, class, usage, or comment."
    } else {
        ""
    };

    Some(format!(
        "no matches after --filter {filter}; the unfiltered search had matches, but the filter removed them all.{kind_hint} Try --as symbol for definitions, --as text for content, or remove the filter."
    ))
}

fn single_query_search(
    text: &str,
    scope: &Path,
    cache: &OutlineCache,
    prefer_definitions: bool,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let mut sym_result = search::search_symbol_raw_with_artifact(text, scope, glob, artifact)?;
    let sym_unfiltered = sym_result.total_found;
    search::apply_general_filter(&mut sym_result, scope, cache, filter)?;
    let mut filtered_to_zero =
        filter.is_some() && sym_unfiltered > 0 && sym_result.total_found == 0;
    let accept_sym = if prefer_definitions {
        sym_result.definitions > 0
    } else {
        sym_result.total_found > 0
    };

    if accept_sym {
        search::pagination::paginate(&mut sym_result, limit, offset);
        search::compact_artifact_snippets(&mut sym_result, artifact);
        return search::format_raw_result(&sym_result, cache);
    }

    let mut content_result = search::search_content_raw_with_artifact(text, scope, glob, artifact)?;
    let content_unfiltered = content_result.total_found;
    search::apply_general_filter(&mut content_result, scope, cache, filter)?;
    filtered_to_zero |=
        filter.is_some() && content_unfiltered > 0 && content_result.total_found == 0;
    if content_result.total_found > 0 {
        search::pagination::paginate(&mut content_result, limit, offset);
        search::compact_artifact_snippets(&mut content_result, artifact);
        return search::format_raw_result(&content_result, cache);
    }

    // For concept queries: if symbol had usages but no definitions, show those
    if prefer_definitions && sym_result.total_found > 0 {
        search::pagination::paginate(&mut sym_result, limit, offset);
        search::compact_artifact_snippets(&mut sym_result, artifact);
        return search::format_raw_result(&sym_result, cache);
    }

    if !artifact.enabled() && should_error_missing_path_like_query(text) {
        // US-059: an unresolvable path-like query becomes a path-fragment match
        // (relative paths containing the fragment) instead of a dead-end error.
        // Explicit `./` / `../` paths are unambiguous and still fail loudly.
        if !text.starts_with("./") && !text.starts_with("../") {
            let fragment_out = search::search_files_fragment(text, scope, limit, offset)?;
            return Ok(format!(
                "> interpreted as path fragment `{text}` (no exact file resolved).\n\n{fragment_out}",
            ));
        }
        return Err(SrcwalkError::PathLikeNotFound {
            path: scope.join(text),
            scope: scope.to_path_buf(),
            basename: std::path::Path::new(text)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        });
    }

    Err(SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, text, glob),
        guidance: filtered_to_zero
            .then(|| filter_zero_guidance(filter))
            .flatten(),
    })
}

/// Render a regex-escaped `discover` query (US-059).
fn run_regex_text(
    q: &RegexTextQuery,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write as _;
    let scope_path = format::display_path(scope);

    // `models\.json` → bare filename → glob branch (≡ `discover models.json`).
    if matches!(q.kind, RegexTextKind::BareFilename) {
        // Regex-escaped filenames are file/glob reads under the hood; reject
        // --filter with the same message the plain file/glob route uses,
        // instead of silently ignoring it.
        if filter.is_some() {
            return Err(SrcwalkError::InvalidQuery {
                query: q.original.clone(),
                reason:
                    "--filter applies to discover results and direct trace callers, not file/glob reads"
                        .to_string(),
            });
        }
        let pattern = format!("**/{}", q.literal);
        let out = search::search_files_glob(&pattern, scope, limit, offset)?;
        return Ok(format!(
            "> interpreted as filename `{}` for regex-escaped `{}`\n\n{}",
            q.literal, q.original, out
        ));
    }

    // Symbol + text dual sections.
    let mut sym = search::search_symbol_raw_with_artifact(&q.symbol_core, scope, glob, artifact)?;
    search::apply_general_filter(&mut sym, scope, cache, filter)?;
    let mut text = search::search_content_raw_with_artifact(&q.literal, scope, glob, artifact)?;
    search::apply_general_filter(&mut text, scope, cache, filter)?;

    let mut out = format!(
        "# Regex-dialect: \"{}\" in {} — interpreted as `{}`\n> Caveat: regex escapes de-escaped for literal + symbol search; not a regex engine.",
        q.original, scope_path, q.literal
    );
    if sym.total_found > 0 {
        search::pagination::paginate(&mut sym, limit, offset);
        search::compact_artifact_snippets(&mut sym, artifact);
        let rendered = search::format_raw_result(&sym, cache)?;
        let _ = write!(out, "\n\n## Symbol: {}\n{}", q.symbol_core, rendered);
    }
    if text.total_found > 0 {
        search::pagination::paginate(&mut text, limit, offset);
        search::compact_artifact_snippets(&mut text, artifact);
        let rendered = search::format_raw_result(&text, cache)?;
        let _ = write!(out, "\n\n## Text: {}\n{}", q.literal, rendered);
    }
    if sym.total_found == 0 && text.total_found == 0 {
        let _ = write!(
            out,
            "\n> Try: `srcwalk discover '{}*' --scope {}` for prefix symbols, or `rg '{}'` for raw regex.",
            q.symbol_core, scope_path, q.original
        );
    }
    Ok(out)
}

/// Render a `.*`/`.+` two-term same-line co-occurrence query (US-059).
fn run_regex_cooccurrence(
    q: &RegexCoOccurrenceQuery,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write as _;
    let mut result = search::cooccurrence::search_same_line_ordered(
        &q.term1,
        &q.term2,
        scope,
        glob,
        crate::ArtifactMode::Source,
    )?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;

    let mut header = format!(
        "# Regex co-occurrence: \"{}\" in {} — interpreted as same-line `{}` ⇒ `{}`",
        q.original,
        format::display_path(scope),
        q.term1,
        q.term2
    );
    if q.simplified {
        header.push_str("\n> Note: pattern had 3+ terms; only the first two were used.");
    }
    header.push_str("\n> Caveat: same-line ordered co-occurrence; not semantic relation proof.");

    if result.total_found == 0 {
        let _ = write!(
            header,
            "\n> Try: `srcwalk discover '{}' --scope {}` for literal text, or `rg '{}'` for real regex.",
            q.term1, format::display_path(scope), q.original
        );
        return Ok(header);
    }

    search::pagination::paginate(&mut result, limit, offset);
    search::format_raw_result_with_header(&result, cache, header)
}

/// Multi-word concept search: exact phrase first, then relaxed word proximity.
fn multi_word_concept_search(
    text: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    // Try structural definitions first. Document headings often contain spaces;
    // if we have a source-backed section/element definition, prefer that over
    // a lower-confidence text phrase hit on the heading line.
    let mut sym_result = search::search_symbol_raw_with_artifact(text, scope, glob, artifact)?;
    search::apply_general_filter(&mut sym_result, scope, cache, filter)?;
    if sym_result.definitions > 0 {
        search::pagination::paginate(&mut sym_result, limit, offset);
        search::compact_artifact_snippets(&mut sym_result, artifact);
        return search::format_raw_result(&sym_result, cache);
    }

    // Try exact phrase match first
    let mut content_result = search::search_content_raw_with_artifact(text, scope, glob, artifact)?;
    search::apply_general_filter(&mut content_result, scope, cache, filter)?;
    content_result.query = text.to_string();
    if content_result.total_found > 0 {
        search::pagination::paginate(&mut content_result, limit, offset);
        return search::format_raw_result(&content_result, cache);
    }

    // Relaxed: match all words in any order
    let words: Vec<&str> = text.split_whitespace().collect();
    let relaxed = if words.len() == 2 {
        format!(
            "{}.*{}|{}.*{}",
            regex_syntax::escape(words[0]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[0]),
        )
    } else {
        // 3+ words: match any word (OR), rely on multi_word_boost in ranking
        words
            .iter()
            .map(|w| regex_syntax::escape(w))
            .collect::<Vec<_>>()
            .join("|")
    };

    let mut relaxed_result =
        search::search_regex_raw_with_artifact(&relaxed, scope, glob, artifact)?;
    search::apply_general_filter(&mut relaxed_result, scope, cache, filter)?;
    relaxed_result.query = text.to_string();
    if relaxed_result.total_found > 0 {
        search::pagination::paginate(&mut relaxed_result, limit, offset);
        return search::format_raw_result(&relaxed_result, cache);
    }

    let first_word = words.first().copied().unwrap_or(text);
    Err(SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, first_word, glob),
        guidance: None,
    })
}

/// Cross-convention symbol suggest first (P1.3 infra), then file-name fallback.
/// Used by symbol→content miss paths so users get a useful "Did you mean: ...".
pub(crate) fn symbol_or_file_suggestion(
    scope: &Path,
    query: &str,
    glob: Option<&str>,
) -> Option<String> {
    let hits = search::symbol::suggest(query, scope, glob, 1);
    if let Some((name, path, line)) = hits.into_iter().next() {
        // Skip case-only variants to avoid suggest loops (foo→Foo→foo).
        let q_low: String = query
            .chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        let n_low: String = name
            .chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        if q_low == n_low {
            return None;
        }
        let rel = format::rel_nonempty(&path, scope);
        return Some(format!("{name} ({rel}:{line})"));
    }
    read::suggest_similar_file(scope, query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::owner_links::{OwnedTextHit, OwnerAnchor, OwnerCallEvidence};
    use std::path::PathBuf;

    /// Single parser that recovers (`tag`, `caller`, `callee`, `call_file`,
    /// `call_line`, `candidate`, `def_range`) from one rendered bullet. The candidate location is
    /// `@:start-end` (same call file) or `@full/path:start-end` (cross-file).
    /// Location separators are resolved rightmost (`rsplit_once(':')`) so windows
    /// drive paths like `C:\repo\pkg\a.go:22` parse correctly.
    struct ParsedEdge {
        tag: String,
        caller: String,
        callee: String,
        call_file: String,
        call_line: u32,
        candidate: String,
        cand_file: String,
        def_range: String,
    }

    fn parse_edge(b: &str) -> Option<ParsedEdge> {
        let rest = b.strip_prefix("- [")?;
        let (tag, rest) = rest.split_once(']')?;
        let rest = rest.strip_prefix(' ')?;
        let (caller, rest) = rest.split_once(" calls ")?;
        // rest = `CALLEE@PATH:LINE; candidate CANDIDATE@LOC`
        let (callee, rest) = rest.split_once('@')?;
        let (call_site, cand_site) = rest.split_once("; candidate ")?;
        let (call_file, call_line) = rsplit_once_colon(call_site)?;
        let (candidate, cand_loc) = cand_site.split_once('@')?;
        let (cand_file, def_range) = if let Some(r) = cand_loc.strip_prefix(':') {
            (call_file.to_string(), r.to_string())
        } else {
            let (f, r) = rsplit_once_colon(cand_loc)?;
            (f.to_string(), r.to_string())
        };
        Some(ParsedEdge {
            tag: tag.to_string(),
            caller: caller.to_string(),
            callee: callee.to_string(),
            call_file: call_file.to_string(),
            call_line: call_line.to_string().parse().ok()?,
            candidate: candidate.to_string(),
            cand_file,
            def_range,
        })
    }

    fn rsplit_once_colon(s: &str) -> Option<(&str, &str)> {
        let idx = s.rfind(':')?;
        Some((&s[..idx], &s[idx + 1..]))
    }

    fn anchor(path: &str, name: &str, receiver: &str, s: u32, e: u32) -> OwnerAnchor {
        let display_name = if receiver.is_empty() {
            name.to_string()
        } else {
            format!("{receiver}.{name}")
        };
        OwnerAnchor {
            path: PathBuf::from(path),
            name: name.into(),
            receiver_var: None,
            receiver_type: (!receiver.is_empty()).then(|| receiver.to_string()),
            package_dir: PathBuf::from("."),
            start_line: s,
            end_line: e,
            language: crate::types::Lang::Go,
            display_name,
        }
    }

    fn render(edges: &[OwnerCallEvidence]) -> String {
        // Seed one hit so has_owners() holds; the appendix renders edges
        // independently of hit content.
        let anchor = anchor("seed.go", "Seed", "S", 1, 1);
        let hits = vec![OwnedTextHit {
            path: anchor.path.clone(),
            line: 1,
            owner: anchor,
        }];
        let ev = OwnerLinkEvidence {
            hits,
            edges: edges.to_vec(),
            go_call_analysis_attempted: true,
        };
        render_owner_link_appendix(&ev, Path::new(""))
    }

    #[test]
    fn edge_bullets_round_trip_same_file_elision_and_cross_file() {
        let same_file = OwnerCallEvidence {
            caller: anchor("pkg/a.go", "Set", "DB", 10, 40),
            call_line: 22,
            callee_name: "Apply".into(),
            candidate: anchor("pkg/a.go", "Apply", "DB", 30, 60),
            mechanism: OwnerCallMechanism::SameFileSameQualifiedReceiver,
        };
        let cross_file = OwnerCallEvidence {
            caller: anchor("pkg/a.go", "SyncPod", "Kubelet", 5, 50),
            call_line: 12,
            callee_name: "killPod".into(),
            candidate: anchor("pkg/kubelet/sub.go", "killPod", "Kubelet", 7, 20),
            mechanism: OwnerCallMechanism::CrossFileSameQualifiedReceiver,
        };
        let bare = OwnerCallEvidence {
            caller: anchor("pkg/b.go", "Run", "", 1, 9),
            call_line: 3,
            callee_name: "cleanup".into(),
            candidate: anchor("pkg/b.go", "cleanup", "", 2, 4),
            mechanism: OwnerCallMechanism::SamePackageBareInvocation,
        };
        let local = OwnerCallEvidence {
            caller: anchor("pkg/c.go", "Setup", "", 1, 9),
            call_line: 5,
            callee_name: "Connect".into(),
            candidate: anchor("pkg/c.go", "Connect", "Pool", 2, 6),
            mechanism: OwnerCallMechanism::SingleAssignmentLocalConstructor,
        };

        let out = render(&[same_file, cross_file, bare, local]);
        let bullets: Vec<&str> = out.lines().filter(|l| l.starts_with("- ")).collect();
        assert_eq!(bullets.len(), 4, "{out}");

        let e0 = parse_edge(bullets[0]).unwrap();
        assert_eq!(
            (e0.tag.as_str(), e0.caller.as_str(), e0.callee.as_str()),
            ("recv", "DB.Set", "Apply")
        );
        assert_eq!(e0.call_file, "pkg/a.go");
        assert_eq!(e0.call_line, 22);
        assert_eq!(e0.candidate, "DB.Apply");
        assert_eq!(e0.cand_file, "pkg/a.go");
        assert_eq!(e0.def_range, "30-60");

        let e1 = parse_edge(bullets[1]).unwrap();
        assert_eq!(
            (e1.tag.as_str(), e1.caller.as_str()),
            ("recv", "Kubelet.SyncPod")
        );
        assert_eq!(e1.call_file, "pkg/a.go");
        assert_eq!(e1.call_line, 12);
        assert_eq!(e1.candidate, "Kubelet.killPod");
        assert_eq!(e1.cand_file, "pkg/kubelet/sub.go");
        assert_eq!(e1.def_range, "7-20");

        let e2 = parse_edge(bullets[2]).unwrap();
        assert_eq!(
            (e2.tag.as_str(), e2.caller.as_str(), e2.callee.as_str()),
            ("bare", "Run", "cleanup")
        );
        assert_eq!(e2.call_file, "pkg/b.go");
        assert_eq!(e2.call_line, 3);
        assert_eq!(e2.cand_file, "pkg/b.go");
        assert_eq!(e2.def_range, "2-4");

        // `[local]` = single-assignment constructor local.
        let e3 = parse_edge(bullets[3]).unwrap();
        assert_eq!(
            (e3.tag.as_str(), e3.caller.as_str(), e3.callee.as_str()),
            ("local", "Setup", "Connect")
        );
        assert_eq!(e3.call_file, "pkg/c.go");
        assert_eq!(e3.call_line, 5);
        assert_eq!(e3.cand_file, "pkg/c.go");
        assert_eq!(e3.def_range, "2-6");

        // Legend must be present exactly once.
        assert_eq!(
            out.matches("[recv=same package-qualified receiver type")
                .count(),
            1,
            "{out}"
        );
    }

    #[test]
    fn edge_bullets_parse_windows_drive_paths_and_colon_safe() {
        // Windows drive path in the call site; `@:` sentinel for same-file candidate.
        let same_win = OwnerCallEvidence {
            caller: anchor(r"C:\repo\pkg\a.go", "Run", "DB", 10, 40),
            call_line: 22,
            callee_name: "Apply".into(),
            candidate: anchor(r"C:\repo\pkg\a.go", "Apply", "DB", 30, 60),
            mechanism: OwnerCallMechanism::SameFileSameQualifiedReceiver,
        };
        // Cross-file windows candidate keeps full path.
        let cross_win = OwnerCallEvidence {
            caller: anchor(r"C:\repo\pkg\a.go", "SyncPod", "Kubelet", 5, 50),
            call_line: 12,
            callee_name: "killPod".into(),
            candidate: anchor(r"C:\repo\pkg\sub.go", "killPod", "Kubelet", 7, 20),
            mechanism: OwnerCallMechanism::CrossFileSameQualifiedReceiver,
        };
        let out = render(&[same_win, cross_win]);
        let bullets: Vec<&str> = out.lines().filter(|l| l.starts_with("- ")).collect();
        assert_eq!(bullets.len(), 2, "{out}");

        let expected_call_file = if cfg!(windows) {
            "C:/repo/pkg/a.go"
        } else {
            r"C:\repo\pkg\a.go"
        };
        let expected_candidate_file = if cfg!(windows) {
            "C:/repo/pkg/sub.go"
        } else {
            r"C:\repo\pkg\sub.go"
        };

        let e0 = parse_edge(bullets[0]).unwrap();
        assert_eq!(e0.call_file, expected_call_file);
        assert_eq!(e0.call_line, 22);
        assert_eq!(e0.cand_file, expected_call_file);
        assert_eq!(e0.def_range, "30-60");

        let e1 = parse_edge(bullets[1]).unwrap();
        assert_eq!(e1.call_file, expected_call_file);
        assert_eq!(e1.call_line, 12);
        assert_eq!(e1.cand_file, expected_candidate_file);
        assert_eq!(e1.def_range, "7-20");
    }

    #[test]
    fn owner_rollup_uses_original_term_index_order_and_counts() {
        // Three query terms in original positions #1,#3 (position #2 is a
        // zero-hit/empty slot that must NOT renumber). Owner `Set` hits term #1
        // once and term #3 twice.
        let path = PathBuf::from("pkg/a.go");
        let owner = anchor("pkg/a.go", "Set", "DB", 10, 40);
        let mk_result = |index: usize, term: &str, lines: Vec<u32>| TextOrTermResult {
            query_term_index: index,
            term: term.into(),
            total_found: lines.len(),
            file_count: 1,
            matches: lines
                .into_iter()
                .map(|line| Match {
                    path: path.clone(),
                    line,
                    text: String::new(),
                    is_definition: false,
                    exact: false,
                    file_lines: 100,
                    mtime: std::time::SystemTime::UNIX_EPOCH,
                    def_range: None,
                    def_name: None,
                    def_weight: 0,
                    impl_target: None,
                    base_target: None,
                    in_comment: false,
                })
                .collect(),
            omitted: 0,
        };
        let terms = vec![
            mk_result(1, "alpha", vec![12]),
            mk_result(3, "beta", vec![14, 15]), // original position 3 (slot 2 empty)
        ];
        let hits = vec![
            OwnedTextHit {
                path: path.clone(),
                line: 12,
                owner: owner.clone(),
            },
            OwnedTextHit {
                path: path.clone(),
                line: 14,
                owner: owner.clone(),
            },
            OwnedTextHit {
                path: path.clone(),
                line: 15,
                owner: owner.clone(),
            },
        ];
        let ev = OwnerLinkEvidence {
            hits,
            edges: Vec::new(),
            go_call_analysis_attempted: true,
        };
        let mut rendered = String::new();
        render_text_or_owner_rollup(&mut rendered, &path, &ev, &terms);
        assert!(
            rendered.contains("owners (#N=Nth query term; *K=hits): DB.Set:10-40[#1,#3*2]"),
            "{rendered}"
        );
        // Determinism: rendering twice yields identical output.
        let mut again = String::new();
        render_text_or_owner_rollup(&mut again, &path, &ev, &terms);
        assert_eq!(rendered, again);
    }

    #[test]
    fn indexed_comma_terms_keep_original_positions_with_gaps_and_dups() {
        // `alpha,alpha,,missing,beta` -> indices 1,2,4,5 (empty #3 retained as a
        // gap; duplicates keep distinct identity).
        let got = indexed_comma_terms("alpha,alpha,,missing,beta");
        let expected: Vec<(usize, &str)> =
            vec![(1, "alpha"), (2, "alpha"), (4, "missing"), (5, "beta")];
        assert_eq!(got, expected);
        // Leading/trailing empties and whitespace also do not renumber.
        let got2 = indexed_comma_terms(", ,alpha,beta,");
        assert_eq!(got2, vec![(3, "alpha"), (4, "beta")]);
    }

    #[test]
    fn owner_rollup_duplicate_zero_hit_and_count_multiplier() {
        // Query `alpha,alpha,,missing,beta` over a single owner who matches all
        // four surviving terms on distinct lines. Duplicate #1,#2 both attribute;
        // zero-hit #4 (missing) must NOT shift beta to a lower index; beta (#5)
        // hits twice so it renders `#5*2`.
        let path = PathBuf::from("pkg/a.go");
        let owner = anchor("pkg/a.go", "Set", "DB", 10, 40);
        let mk_result = |index: usize, term: &str, lines: Vec<u32>| TextOrTermResult {
            query_term_index: index,
            term: term.into(),
            total_found: lines.len(),
            file_count: 1,
            matches: lines
                .into_iter()
                .map(|line| Match {
                    path: path.clone(),
                    line,
                    text: String::new(),
                    is_definition: false,
                    exact: false,
                    file_lines: 100,
                    mtime: std::time::SystemTime::UNIX_EPOCH,
                    def_range: None,
                    def_name: None,
                    def_weight: 0,
                    impl_target: None,
                    base_target: None,
                    in_comment: false,
                })
                .collect(),
            omitted: 0,
        };
        // alpha#1 line 12, alpha#2 line 13, missing#4 (no hits, omitted), beta#5 lines 14,15.
        let terms = vec![
            mk_result(1, "alpha", vec![12]),
            mk_result(2, "alpha", vec![13]),
            mk_result(4, "missing", vec![]),
            mk_result(5, "beta", vec![14, 15]),
        ];
        let hits = vec![12, 13, 14, 15]
            .into_iter()
            .map(|line| OwnedTextHit {
                path: path.clone(),
                line,
                owner: owner.clone(),
            })
            .collect();
        let ev = OwnerLinkEvidence {
            hits,
            edges: Vec::new(),
            go_call_analysis_attempted: true,
        };
        let mut rendered = String::new();
        render_text_or_owner_rollup(&mut rendered, &path, &ev, &terms);
        // Duplicate #1,#2 both attribute; zero-hit #4 gap keeps beta at #5 with
        // count multiplier 2. Order within owner is by ascending index.
        assert!(rendered.contains("DB.Set:10-40[#1,#2,#5*2]"), "{rendered}");
    }
    fn py_anchor(path: &str, name: &str, s: u32, e: u32) -> OwnerAnchor {
        OwnerAnchor {
            path: PathBuf::from(path),
            name: name.into(),
            receiver_var: None,
            receiver_type: None,
            package_dir: PathBuf::from("."),
            start_line: s,
            end_line: e,
            language: crate::types::Lang::Python,
            display_name: name.to_string(),
        }
    }

    fn go_hit(anchor: OwnerAnchor, line: u32) -> OwnedTextHit {
        OwnedTextHit {
            path: anchor.path.clone(),
            line,
            owner: anchor,
        }
    }

    #[test]
    fn non_go_only_result_never_renders_go_zero_edge_or_caveat() {
        // Two Python owners, no Go analysis attempted -> no Go zero-edge, no
        // Go mechanical caveat; only the non-Go honesty caveat may appear.
        let py1 = py_anchor("app.py", "apply", 1, 2);
        let py2 = py_anchor("app.py", "set", 3, 4);
        let ev = OwnerLinkEvidence {
            hits: vec![go_hit(py1, 1), go_hit(py2, 3)],
            edges: Vec::new(),
            go_call_analysis_attempted: false,
        };
        let out = render_owner_link_appendix(&ev, Path::new(""));
        assert!(!out.contains("No direct name-level call evidence"), "{out}");
        assert!(
            !out.contains("structural owner and mechanically filtered"),
            "{out}"
        );
        assert!(!out.contains("## Mechanical Go calls"), "{out}");
        assert!(
            out.contains("structural lexical ownership candidates"),
            "{out}"
        );
    }

    #[test]
    fn single_go_owner_with_python_owners_suppresses_zero_edge_sentence() {
        // 1 Go owner + 2 Python owners: Go analysis ran, but the zero-edge
        // sentence must NOT render (it would be a measured-zero claim across
        // languages it cannot support). The Go caveat and non-Go caveat may.
        let go = anchor("a.go", "Run", "DB", 10, 40);
        let py1 = py_anchor("app.py", "apply", 1, 2);
        let py2 = py_anchor("app.py", "set", 3, 4);
        let ev = OwnerLinkEvidence {
            hits: vec![go_hit(go, 12), go_hit(py1, 1), go_hit(py2, 3)],
            edges: Vec::new(),
            go_call_analysis_attempted: true,
        };
        let out = render_owner_link_appendix(&ev, Path::new(""));
        assert!(!out.contains("No direct name-level call evidence"), "{out}");
        assert!(
            out.contains("structural owner and mechanically filtered"),
            "{out}"
        );
        assert!(
            out.contains("structural lexical ownership candidates"),
            "{out}"
        );
    }

    #[test]
    fn two_go_owners_no_edges_preserves_zero_edge_and_go_caveat() {
        // >=2 Go owners, Go-only, no edges -> zero-edge + Go caveat (byte
        // identity preserved for the pure-Go case).
        let go1 = anchor("a.go", "Run", "DB", 10, 40);
        let go2 = anchor("a.go", "Apply", "DB", 50, 80);
        let ev = OwnerLinkEvidence {
            hits: vec![go_hit(go1, 12), go_hit(go2, 52)],
            edges: Vec::new(),
            go_call_analysis_attempted: true,
        };
        let out = render_owner_link_appendix(&ev, Path::new(""));
        assert!(out.contains("No direct name-level call evidence"), "{out}");
        assert!(
            out.contains("structural owner and mechanically filtered"),
            "{out}"
        );
        // Pure Go: no non-Go caveat.
        assert!(
            !out.contains("structural lexical ownership candidates"),
            "{out}"
        );
    }

    // ---------- US-067: deterministic owner replay audit harness (test-only) ----------
    //
    // Taps the same production seam as `run_text_or_*` (find.rs ~210-252): raw per-term
    // `search_content_raw_with_artifact` results are filtered, paginated with a no-omission
    // assertion, and fed ONCE into `build_owner_link_evidence`. Output is deterministic
    // TSV (no timestamps/absolute machine paths) so an external pinned-repo replay can be
    // diffed. No barrier/error classification and no AST/name logic live here.

    /// One raw term-hit row with an optional attached owner anchor.
    #[derive(Clone)]
    struct AuditRow {
        term_index: usize,
        term: String,
        /// Repo-relative, `/`-normalized path (used for emission/sorting).
        rel: String,
        /// Original path, used only for owner-anchor lookup before emission.
        path: PathBuf,
        line: u32,
        owner: Option<(String, u32, u32)>, // (qualified_name, start, end)
    }

    /// Fail-fast field guard: a value containing `\t`, `\r`, or `\n` would corrupt
    /// a TSV column (no escaping scheme is used), so reject it before serialization.
    fn assert_tsv_safe(field: &str, what: &str) {
        assert!(
            !field.contains('\t') && !field.contains('\r') && !field.contains('\n'),
            "TSV-unsafe {what} field corruption: {field:?}",
        );
    }

    /// Deterministic serialization of the three audit outputs. Factored so the
    /// ignored replay harness and the synthetic dedupe test share one emitter.
    #[must_use]
    fn audit_tsv_files(
        rows: &[AuditRow],
        per_term: &[(usize, String, usize)], // (term_index, term, total_found)
    ) -> (String, String, String) {
        // raw.tsv sorted by (term_index, rel, line).
        let mut raw = rows.to_vec();
        raw.sort_by(|a, b| {
            a.term_index
                .cmp(&b.term_index)
                .then(a.rel.cmp(&b.rel))
                .then(a.line.cmp(&b.line))
        });
        let mut raw_tsv = String::from(
            "term_index\tterm\trelpath\tline\towner_or_empty\tstart_or_empty\tend_or_empty\n",
        );
        for r in &raw {
            assert_tsv_safe(&r.term, "term");
            assert_tsv_safe(&r.rel, "relpath");
            let (qn, start, end) = r.owner.as_ref().map_or(
                (String::new(), String::new(), String::new()),
                |(qn, s, e)| {
                    assert_tsv_safe(qn, "qualified_name");
                    (qn.clone(), s.to_string(), e.to_string())
                },
            );
            raw_tsv.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                r.term_index, r.term, r.rel, r.line, qn, start, end
            ));
        }

        // tuples.tsv: DISTINCT sorted (rel, line, qualified_name, start, end).
        let mut tuple_set: BTreeSet<(String, u32, String, u32, u32)> = BTreeSet::new();
        for r in rows {
            if let Some((qn, s, e)) = &r.owner {
                tuple_set.insert((r.rel.clone(), r.line, qn.clone(), *s, *e));
            }
        }
        let mut tuples_tsv = String::from("relpath\tline\tqualified_name\tstart\tend\n");
        for (rel, line, qn, s, e) in &tuple_set {
            tuples_tsv.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", rel, line, qn, s, e));
        }

        // summary.tsv (key/value, no header noise).
        let mut summary = String::new();
        for (idx, term, total) in per_term {
            assert_tsv_safe(term, "term");
            let raw_count = rows.iter().filter(|r| r.term_index == *idx).count();
            summary.push_str(&format!(
                "per-term\t{}\t{}\t{}\t{}\n",
                idx, term, total, raw_count
            ));
        }
        let total_raw = rows.len();
        let unique_pl = rows
            .iter()
            .map(|r| (r.rel.clone(), r.line))
            .collect::<BTreeSet<_>>()
            .len();
        let distinct_tuples = tuple_set.len();
        let distinct_qn = tuple_set
            .iter()
            .map(|t| t.2.clone())
            .collect::<BTreeSet<_>>()
            .len();
        let unattributed = rows.iter().filter(|r| r.owner.is_none()).count();
        summary.push_str(&format!("aggregate\ttotal_raw_term_rows\t{}\n", total_raw));
        summary.push_str(&format!(
            "aggregate\tunique_path_line_rows\t{}\n",
            unique_pl
        ));
        summary.push_str(&format!(
            "aggregate\tdistinct_attributed_tuples\t{}\n",
            distinct_tuples
        ));
        summary.push_str(&format!(
            "aggregate\tdistinct_qualified_names\t{}\n",
            distinct_qn
        ));
        summary.push_str(&format!(
            "aggregate\tunattributed_raw_term_rows\t{}\n",
            unattributed
        ));

        (raw_tsv, tuples_tsv, summary)
    }

    /// Repo-relative, `/`-normalized path under the canonical scope. Fails on
    /// non-UTF8 or any path outside the scope (never emits absolute machine paths).
    #[must_use]
    fn audit_relpath(canonical_scope: &Path, path: &Path) -> String {
        let rel = path
            .strip_prefix(canonical_scope)
            .expect("match path is not under the canonical scope");
        let s = rel
            .to_str()
            .expect("non-UTF8 repo-relative path in US-067 audit");
        s.replace('\\', "/")
    }

    /// Deterministic (path, line) -> owner map, asserting any duplicate key has a
    /// byte-identical anchor (one owner per hit location).
    fn owner_anchor_map(evidence: &OwnerLinkEvidence) -> BTreeMap<(PathBuf, u32), OwnerAnchor> {
        let mut map = BTreeMap::new();
        for hit in &evidence.hits {
            let key = (hit.path.clone(), hit.line);
            if let Some(prev) = map.get(&key) {
                assert_eq!(
                    prev, &hit.owner,
                    "duplicate (path,line) owner mismatch at {:?}:{}",
                    hit.path, hit.line
                );
            }
            map.insert(key, hit.owner.clone());
        }
        map
    }

    /// Drives the production search + owner pipeline and writes the three
    /// deterministic TSV outputs into a FRESH `out_dir`. Panics on any protocol
    /// breach (non-[1,2,3] term positions, TSV-unsafe term, per-term omission,
    /// non-UTF8/outside-scope path, duplicate (path,line) owner mismatch, or a
    /// pre-existing output dir).
    fn run_us067_replay(scope: &Path, query: &str, limit: usize, out_dir: &Path) {
        let indexed = indexed_comma_terms(query);
        let positions: Vec<usize> = indexed.iter().map(|(i, _)| *i).collect();
        assert_eq!(
            positions,
            vec![1, 2, 3],
            "SRCWALK_US067_AUDIT_QUERY must be exactly 3 comma terms with original positions [1,2,3] (no empty gaps); got {positions:?}: {query:?}"
        );
        for (_, term) in &indexed {
            assert_tsv_safe(term, "term");
        }

        let canonical_scope = scope
            .canonicalize()
            .expect("SRCWALK_US067_AUDIT_SCOPE must exist and be canonicalizable");
        // One shared outline cache across all terms, matching the production command.
        let cache = OutlineCache::default();

        let mut rows: Vec<AuditRow> = Vec::new();
        let mut per_term: Vec<(usize, String, usize)> = Vec::new();

        for (idx, term) in &indexed {
            let mut result = search::search_content_raw_with_artifact(
                term,
                &canonical_scope,
                None,
                ArtifactMode::Source,
            )
            .expect("search_content_raw_with_artifact");
            search::apply_general_filter(&mut result, &canonical_scope, &cache, None)
                .expect("apply_general_filter");
            let total_found = result.total_found;
            search::pagination::paginate(&mut result, Some(limit), 0);
            assert_eq!(
                total_found,
                result.matches.len(),
                "per-term omission: term {} total_found {} != matches.len() {}; raise SRCWALK_US067_AUDIT_LIMIT >= {}",
                term, total_found, result.matches.len(), total_found
            );
            per_term.push((*idx, term.to_string(), total_found));
            for m in &result.matches {
                rows.push(AuditRow {
                    term_index: *idx,
                    term: term.to_string(),
                    rel: audit_relpath(&canonical_scope, &m.path),
                    path: m.path.clone(),
                    line: m.line,
                    owner: None,
                });
            }
        }

        // Build inputs from the stable `rows` paths in a short scope; drop them
        // before mutating rows so no duplicate path vector is kept alive.
        let anchors = {
            let inputs: Vec<OwnerLinkHitInput> = rows
                .iter()
                .map(|r| OwnerLinkHitInput {
                    path: &r.path,
                    line: r.line,
                })
                .collect();
            let evidence = build_owner_link_evidence(&inputs);
            owner_anchor_map(&evidence)
        };
        for r in &mut rows {
            r.owner = anchors
                .get(&(r.path.clone(), r.line))
                .map(|a| (a.qualified_name(), a.start_line, a.end_line));
        }

        let (raw_tsv, tuples_tsv, summary_tsv) = audit_tsv_files(&rows, &per_term);
        assert!(
            !out_dir.exists(),
            "SRCWALK_US067_AUDIT_OUT must be a fresh (non-existent) directory; refusing to overwrite {}",
            out_dir.display()
        );
        std::fs::create_dir_all(out_dir).expect("create audit out dir");
        std::fs::write(out_dir.join("raw.tsv"), raw_tsv).expect("write raw.tsv");
        std::fs::write(out_dir.join("tuples.tsv"), tuples_tsv).expect("write tuples.tsv");
        std::fs::write(out_dir.join("summary.tsv"), summary_tsv).expect("write summary.tsv");
    }

    #[test]
    fn us067_audit_tsv_deterministic_and_tuples_dedupe() {
        let rows = vec![
            AuditRow {
                term_index: 2,
                term: "return".into(),
                rel: "a.ts".into(),
                path: PathBuf::from("a.ts"),
                line: 10,
                owner: Some(("A.f".into(), 5, 12)),
            },
            AuditRow {
                term_index: 1,
                term: "pipe".into(),
                rel: "a.ts".into(),
                path: PathBuf::from("a.ts"),
                line: 10,
                owner: Some(("A.f".into(), 5, 12)), // duplicate tuple
            },
            AuditRow {
                term_index: 1,
                term: "pipe".into(),
                rel: "b.ts".into(),
                path: PathBuf::from("b.ts"),
                line: 3,
                owner: None,
            },
        ];
        let per_term = vec![(1, "pipe".into(), 2), (2, "return".into(), 1)];
        let (raw, tuples, summary) = audit_tsv_files(&rows, &per_term);

        // raw.tsv sorted by (term_index, rel, line).
        let rl: Vec<&str> = raw.lines().collect();
        assert_eq!(rl.len(), 4, "header + 3 rows: {raw}");
        assert!(
            rl[1].starts_with("1\tpipe\ta.ts\t10\tA.f\t5\t12"),
            "{}",
            rl[1]
        );
        assert!(rl[2].starts_with("1\tpipe\tb.ts\t3\t\t\t"), "{}", rl[2]);
        assert!(
            rl[3].starts_with("2\treturn\ta.ts\t10\tA.f\t5\t12"),
            "{}",
            rl[3]
        );

        // tuples.tsv dedupe: one distinct tuple despite two same-key raw rows.
        let tl: Vec<&str> = tuples.lines().collect();
        assert_eq!(tl.len(), 2, "header + 1 distinct tuple: {tuples}");
        assert!(tl[1].starts_with("a.ts\t10\tA.f\t5\t12"), "{}", tl[1]);

        assert!(summary.contains("per-term\t1\tpipe\t2\t2\n"), "{summary}");
        assert!(summary.contains("per-term\t2\treturn\t1\t1\n"), "{summary}");
        assert!(
            summary.contains("aggregate\ttotal_raw_term_rows\t3\n"),
            "{summary}"
        );
        assert!(
            summary.contains("aggregate\tunique_path_line_rows\t2\n"),
            "{summary}"
        );
        assert!(
            summary.contains("aggregate\tdistinct_attributed_tuples\t1\n"),
            "{summary}"
        );
        assert!(
            summary.contains("aggregate\tdistinct_qualified_names\t1\n"),
            "{summary}"
        );
        assert!(
            summary.contains("aggregate\tunattributed_raw_term_rows\t1\n"),
            "{summary}"
        );
    }

    #[test]
    #[should_panic(expected = "TSV-unsafe term field corruption")]
    fn us067_tsv_guard_rejects_delimiter_in_term() {
        let rows = vec![AuditRow {
            term_index: 1,
            term: "pi\tpe".into(),
            rel: "a.ts".into(),
            path: PathBuf::from("a.ts"),
            line: 1,
            owner: None,
        }];
        let _ = audit_tsv_files(&rows, &[(1, "pipe".into(), 1)]);
    }

    #[test]
    #[should_panic(expected = "TSV-unsafe qualified_name field corruption")]
    fn us067_tsv_guard_rejects_delimiter_in_qualified_name() {
        let rows = vec![AuditRow {
            term_index: 1,
            term: "pipe".into(),
            rel: "a.ts".into(),
            path: PathBuf::from("a.ts"),
            line: 1,
            owner: Some(("A\nB".into(), 5, 12)),
        }];
        let _ = audit_tsv_files(&rows, &[(1, "pipe".into(), 1)]);
    }

    #[test]
    #[ignore = "requires pinned external repo; US-067 replay"]
    fn us067_owner_replay_audit() {
        let scope =
            std::env::var("SRCWALK_US067_AUDIT_SCOPE").expect("SRCWALK_US067_AUDIT_SCOPE required");
        let query =
            std::env::var("SRCWALK_US067_AUDIT_QUERY").expect("SRCWALK_US067_AUDIT_QUERY required");
        let limit: usize = std::env::var("SRCWALK_US067_AUDIT_LIMIT")
            .expect("SRCWALK_US067_AUDIT_LIMIT required")
            .parse()
            .expect("SRCWALK_US067_AUDIT_LIMIT must be a usize");
        assert!(limit > 0, "SRCWALK_US067_AUDIT_LIMIT must be positive");
        let out =
            std::env::var("SRCWALK_US067_AUDIT_OUT").expect("SRCWALK_US067_AUDIT_OUT required");
        run_us067_replay(Path::new(&scope), &query, limit, Path::new(&out));
    }
}
