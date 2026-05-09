use std::path::Path;

mod batch;
mod comments;
mod definitions;
mod glob_search;
mod suggest;
mod usages;

use comments::tag_comment_matches;
use definitions::find_definitions_with_artifact;
#[cfg(test)]
use definitions::{find_artifact_anchor_defs, find_defs_treesitter};
use usages::find_usages_with_artifact;

use crate::error::SrcwalkError;
use crate::search::rank;
use crate::types::{Match, SearchResult};
use crate::ArtifactMode;
use grep_regex::RegexMatcher;

const MAX_DEFINITION_DEPTH: usize = 8;
const MAX_ARTIFACT_DEFINITION_DEPTH: usize = 64;
const MAX_ARTIFACT_FILE_SIZE: u64 = 25_000_000;

pub fn search_name_glob(
    pattern: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    glob_search::search_name_glob(pattern, scope, cache, context, glob)
}

pub fn search_name_glob_with_artifact(
    pattern: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    glob_search::search_name_glob_with_artifact(pattern, scope, cache, context, glob, artifact)
}

/// Multi-symbol batch search.
/// Single-walk: each file is opened/parsed once; `AhoCorasick` gates by any-query hit;
/// tree-sitter AST walked once with per-query buckets. Same for usages.
/// Returns one `SearchResult` per query in input order.
pub fn search_batch(
    queries: &[&str],
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<Vec<SearchResult>, SrcwalkError> {
    batch::search_batch(queries, scope, cache, context, glob)
}

/// Symbol search: find definitions via tree-sitter, usages via ripgrep, concurrently.
/// Merge results, deduplicate, definitions first.
pub fn search(
    query: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_with_artifact(query, scope, cache, context, glob, ArtifactMode::Source)
}

pub fn search_with_artifact(
    query: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    // Compile regex once, share across both arms
    let word_pattern = format!(r"\b{}\b", regex_syntax::escape(query));
    let matcher = RegexMatcher::new(&word_pattern).map_err(|e| SrcwalkError::InvalidQuery {
        query: query.to_string(),
        reason: e.to_string(),
    })?;

    let (defs, usages) = rayon::join(
        || find_definitions_with_artifact(query, scope, glob, cache, artifact),
        || find_usages_with_artifact(query, &matcher, scope, glob, artifact),
    );

    let defs = defs?;
    let mut usages_vec = vec![usages?];
    tag_comment_matches(&mut usages_vec);
    let usages = usages_vec.into_iter().next().unwrap();

    // Deduplicate: remove usage matches that overlap with definition matches.
    // Linear scan — max ~30 defs from EARLY_QUIT_THRESHOLD, no allocation needed.
    let mut merged: Vec<Match> = defs;
    let def_count = merged.len();

    for m in usages {
        let dominated = merged[..def_count]
            .iter()
            .any(|d| d.path == m.path && d.line == m.line);
        if !dominated {
            merged.push(m);
        }
    }

    let total = merged.len();
    let comment_count = merged.iter().filter(|m| m.in_comment).count();
    let usage_count = total - def_count - comment_count;

    rank::sort(&mut merged, query, scope, context);

    Ok(SearchResult {
        query: query.to_string(),
        scope: scope.to_path_buf(),
        matches: merged,
        total_found: total,
        definitions: def_count,
        usages: usage_count,
        comments: comment_count,
        has_more: false,
        offset: 0,
    })
}

pub fn suggest(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    top_n: usize,
) -> Vec<(String, std::path::PathBuf, u32)> {
    suggest::suggest(query, scope, glob, top_n)
}

#[cfg(test)]
mod tests;
