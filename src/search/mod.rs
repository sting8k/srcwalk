mod artifact_snippet;
pub mod callees;
pub mod callers;
pub mod content;
pub mod deps;
mod display;
pub mod facets;
mod filter;
pub mod glob;
pub mod io;
pub mod pagination;
pub mod rank;
pub mod siblings;
pub mod strip;
pub mod symbol;
pub mod truncate;

use std::collections::HashSet;
use std::fmt::Write;
use std::path::Path;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format;
use crate::format::rel;
use crate::session::Session;
use crate::types::SearchResult;
use crate::ArtifactMode;

pub use self::artifact_snippet::compact_artifact_snippets;
pub use self::display::{format_raw_result, format_raw_result_with_header, search_files_glob};
pub use self::filter::apply_general_filter;
pub(crate) use self::io::{file_metadata, read_file_bytes};
use self::io::{parse_pattern, walker};
use self::pagination::paginate;

use self::display::{
    append_expand_budget_note, format_matches, format_search_result,
    format_search_result_with_header, ExpandBudget,
};

/// Append a `> Did you mean: …` line when a symbol search returned 0 hits and
/// at least one spelling-similar symbol exists in scope.
fn append_did_you_mean(
    out: &mut String,
    result: &SearchResult,
    scope: &Path,
    glob: Option<&str>,
    filter: Option<&str>,
) {
    if !result.matches.is_empty() {
        return;
    }
    let suggestions = symbol::suggest(&result.query, scope, glob, 3);
    if suggestions.is_empty() {
        return;
    }
    let _ = write!(out, "\n\n> Did you mean: ");
    for (i, (spelling, path, line)) in suggestions.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, ", ");
        }
        let rel_path = rel(path, scope);
        let display = if rel_path.is_empty() {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        } else {
            rel_path
        };
        let _ = write!(out, "{spelling} ({display}:{line})");
    }
    out.push('?');
    if has_kind_filter(filter) {
        out.push_str(
            "\n> Note: kind filters only match symbols inside --scope; broaden --scope for imported/out-of-scope definitions.",
        );
    }
}

fn has_kind_filter(filter: Option<&str>) -> bool {
    filter.is_some_and(|filter| {
        filter
            .split_whitespace()
            .any(|part| part.trim_start().starts_with("kind:"))
    })
}
#[allow(dead_code)]
pub fn search_symbol(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    search_symbol_with_artifact(
        query,
        scope,
        cache,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::Source,
    )
}

pub fn search_symbol_with_artifact(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let mut result = symbol::search_with_artifact(query, scope, Some(cache), None, glob, artifact)?;
    apply_general_filter(&mut result, scope, cache, filter)?;
    paginate(&mut result, limit, offset);
    compact_artifact_snippets(&mut result, artifact);
    let bloom = crate::index::bloom::BloomFilterCache::new();
    let mut out = format_search_result(&result, cache, None, &bloom, 0, None)?;
    append_did_you_mean(&mut out, &result, scope, glob, filter);
    // Contextual hints
    if result.definitions > 0 {
        out.push_str("\n\n> Next: use --expand to inline definition source");
    }
    if result.usages >= 5 {
        out.push_str("\n> Next: for precise call sites use `srcwalk callers <symbol>` instead of text-based usages");
    }
    Ok(out)
}

#[allow(dead_code)]
pub fn search_symbol_glob(
    pattern: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    search_symbol_glob_with_artifact(
        pattern,
        scope,
        cache,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::Source,
    )
}

pub fn search_symbol_glob_with_artifact(
    pattern: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let mut result =
        symbol::search_name_glob_with_artifact(pattern, scope, Some(cache), None, glob, artifact)?;
    apply_general_filter(&mut result, scope, cache, filter)?;
    paginate(&mut result, limit, offset);
    compact_artifact_snippets(&mut result, artifact);
    let header = search_names_header(&result, scope);
    format_search_result_with_header(
        &result,
        cache,
        None,
        &crate::index::bloom::BloomFilterCache::new(),
        0,
        None,
        header,
    )
}

pub fn search_symbol_glob_expanded(
    pattern: &str,
    scope: &Path,
    cache: &OutlineCache,
    session: &Session,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let mut result = symbol::search_name_glob(pattern, scope, Some(cache), context, glob)?;
    apply_general_filter(&mut result, scope, cache, filter)?;
    paginate(&mut result, limit, offset);
    let header = search_names_header(&result, scope);
    format_search_result_with_header(
        &result,
        cache,
        Some(session),
        bloom,
        expand,
        budget_tokens,
        header,
    )
}

fn search_names_header(result: &SearchResult, scope: &Path) -> String {
    format!(
        "# Search names: \"{}\" in {} — {} matches",
        result.query,
        format::display_path(scope),
        result.matches.len()
    )
}

pub fn search_symbol_expanded(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    session: &Session,
    index: &crate::index::SymbolIndex,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let _ = index;

    let mut result = symbol::search(query, scope, Some(cache), context, glob)?;
    apply_general_filter(&mut result, scope, cache, filter)?;
    paginate(&mut result, limit, offset);
    let mut out =
        format_search_result(&result, cache, Some(session), bloom, expand, budget_tokens)?;
    append_did_you_mean(&mut out, &result, scope, glob, filter);
    Ok(out)
}

pub fn search_multi_symbol_expanded(
    queries: &[&str],
    scope: &Path,
    cache: &OutlineCache,
    session: &Session,
    index: &crate::index::SymbolIndex,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let _ = index; // Available but not yet used for search fast-path

    let mut sections: Vec<String> = Vec::with_capacity(queries.len());
    let expand_per_query = if expand == 0 { 0 } else { expand.max(1) };

    // Phase 1: single-walk batch search — one file I/O per file, one ts parse,
    // AhoCorasick-gated, per-query buckets. Much faster than N independent walkers.
    let mut results = symbol::search_batch(queries, scope, Some(cache), context, glob)?;

    // Sort by match count ascending — fewer matches = rarer/more specific.
    // Rare symbols are higher value and shouldn't be starved by common ones.
    results.sort_by_key(|r| r.matches.len());

    // Phase 2: format sequentially (format_matches touches the session mutex
    // and shared sets — cheap, keep single-threaded).
    let mut expanded_files = HashSet::new();
    let mut context_shown_files = HashSet::new();
    let mut expand_budget = ExpandBudget::new(expand_per_query * queries.len(), budget_tokens);
    for mut result in results {
        let mut smart_truncated = false;
        apply_general_filter(&mut result, scope, cache, filter)?;
        paginate(&mut result, limit, offset);
        let mut out = format::search_header(
            &result.query,
            &result.scope,
            result.matches.len(),
            result.definitions,
            result.usages,
            result.comments,
        );
        let mut budget = expand_per_query;
        format_matches(
            &result.matches,
            &result.scope,
            cache,
            Some(session),
            bloom,
            &mut budget,
            &mut expand_budget,
            &mut expanded_files,
            &mut context_shown_files,
            &mut smart_truncated,
            &mut out,
        );
        if result.total_found > result.matches.len() {
            let omitted = result.total_found - result.matches.len();
            let next_offset = result.offset + result.matches.len();
            let page_size = result.matches.len().max(1);
            let _ = write!(
                out,
                "\n\n> Next: {omitted} more matches available. Continue with --offset {next_offset} --limit {page_size}."
            );
        }
        if smart_truncated {
            out.push_str("\n\n> Caveat: expanded source truncated.\n> Next: use shown line range with --section <start-end>.");
        }
        append_did_you_mean(&mut out, &result, scope, glob, filter);
        sections.push(out);
    }
    let mut out = sections.join("\n\n---\n");
    append_expand_budget_note(&mut out, &expand_budget);
    Ok(out)
}

pub fn search_content_expanded(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    session: &Session,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let (pattern, is_regex) = parse_pattern(query);
    let mut result = content::search(pattern, scope, is_regex, context, glob)?;
    apply_general_filter(&mut result, scope, cache, filter)?;
    paginate(&mut result, limit, offset);
    let bloom = crate::index::bloom::BloomFilterCache::new();
    format_search_result(&result, cache, Some(session), &bloom, expand, budget_tokens)
}

/// Raw symbol search — returns structured result for programmatic inspection.
pub fn search_symbol_raw(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_symbol_raw_with_artifact(query, scope, glob, ArtifactMode::Source)
}

pub fn search_symbol_raw_with_artifact(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    symbol::search_with_artifact(query, scope, None, None, glob, artifact)
}

pub fn search_symbol_glob_raw(
    pattern: &str,
    scope: &Path,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_symbol_glob_raw_with_artifact(pattern, scope, glob, ArtifactMode::Source)
}

pub fn search_symbol_glob_raw_with_artifact(
    pattern: &str,
    scope: &Path,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    symbol::search_name_glob_with_artifact(pattern, scope, None, None, glob, artifact)
}

/// Raw content search — returns structured result for programmatic inspection.
pub fn search_content_raw(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_content_raw_with_artifact(query, scope, glob, ArtifactMode::Source)
}

pub fn search_content_raw_with_artifact(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    let (pattern, is_regex) = parse_pattern(query);
    content::search_with_artifact(pattern, scope, is_regex, None, glob, artifact)
}

/// Raw regex search — returns structured result for programmatic inspection.
pub fn search_regex_raw(
    pattern: &str,
    scope: &Path,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_regex_raw_with_artifact(pattern, scope, glob, ArtifactMode::Source)
}

pub fn search_regex_raw_with_artifact(
    pattern: &str,
    scope: &Path,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    content::search_with_artifact(pattern, scope, true, None, glob, artifact)
}

#[cfg(test)]
mod tests;
