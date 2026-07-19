use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::classify::{self, classify};
use crate::commands::context::{
    with_artifact_note, with_artifact_read_label, ArtifactMode, ExpandedCtx,
};
use crate::commands::multi_scope::{
    parse_multi_symbol_query, unsupported_find_syntax_error, use_files_error,
};
use crate::commands::section_disambiguation::disambiguate_glob_for_section;
use crate::evidence::{render_next_actions, NextAction};
use crate::types::{Match, QueryType};
use crate::OutlineCache;
use crate::SrcwalkError;
use crate::{artifact, budget, format, index, read, search, session};

const MAX_TEXT_OR_TERMS: usize = 8;
const DEFAULT_TEXT_OR_TERM_LIMIT: usize = 10;
const TEXT_OR_COMPACT_MIN_TERMS: usize = 3;
const TEXT_OR_COMPACT_MIN_MATCHES: usize = 30;
const TEXT_OR_ROLLUP_FILE_LIMIT: usize = 8;
const TEXT_OR_ROLLUP_LINE_LIMIT: usize = 6;

fn comma_terms(query: &str) -> Vec<&str> {
    query
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
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
    let mut result = search::search_content_raw_with_artifact(query, scope, glob, artifact)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    search::pagination::paginate(&mut result, limit, offset);
    search::compact_artifact_snippets(&mut result, artifact);
    let mut output = search::format_raw_result(&result, cache)?;
    if literal_comma_hint && result.total_found == 0 {
        output.push_str(
            "\n\n> Hint: treated as one literal text query. Use `--match any --as text` for comma-separated literal OR, or `--match all --as text` for same-file co-occurrence.",
        );
    }
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

    for term in &terms {
        let mut result = search::search_content_raw_with_artifact(term, scope, glob, artifact)?;
        search::apply_general_filter(&mut result, scope, cache, filter)?;
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
            term: (*term).to_string(),
            total_found: result.total_found,
            file_count,
            matches: result.matches,
            omitted,
        });
    }

    let compact =
        terms.len() >= TEXT_OR_COMPACT_MIN_TERMS || total_found > TEXT_OR_COMPACT_MIN_MATCHES;
    let rendered = if compact {
        render_text_or_file_rollup(&term_results, scope)
    } else {
        render_text_or_term_details(&term_results, term_limit, scope)
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
    if total_found > 0 {
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
        for m in &result.matches {
            let _ = write!(
                rendered,
                "\n  {}:{} — {}",
                format::rel_nonempty(&m.path, scope),
                m.line,
                m.text.trim()
            );
        }
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

fn render_text_or_file_rollup(term_results: &[TextOrTermResult], scope: &Path) -> String {
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
        if let (Some(start), Some(end)) = (file.lines.first(), file.lines.last()) {
            let _ = write!(
                rendered,
                "\n  > Next: srcwalk show {rel}:{start}-{end} -C 20"
            );
        }
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
    rendered
}

struct TextOrFileRollup {
    path: PathBuf,
    term_counts: BTreeMap<String, usize>,
    shown_matches: usize,
    lines: BTreeSet<u32>,
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
            is_test,
        }
    }
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

    // Multi-symbol: comma-separated identifiers, 2..=5 items.
    // Check before main dispatch. Only activate when all parts look like identifiers
    // to avoid hijacking regex (/foo,bar/) or glob (*.{rs,ts}) queries.
    if !matches!(
        query_type,
        QueryType::Glob(_)
            | QueryType::SymbolGlob(_)
            | QueryType::FilePath(_)
            | QueryType::FilePathLine(_, _)
            | QueryType::FilePathSection(_, _)
    ) {
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
    }

    // FilePath and Glob are read operations, not search — handle before expanded dispatch
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
                let related = read::imports::resolve_related_files(&path);
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
        QueryType::FilePath(_)
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
        QueryType::FilePath(_)
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
