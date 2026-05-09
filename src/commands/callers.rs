use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::ArtifactMode;
use crate::error::SrcwalkError;
use crate::{budget, index, search, session};

/// Find all callers of a symbol.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_callers(
    target: &str,
    scope: &Path,
    expand: usize,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
    depth: Option<usize>,
    max_frontier: Option<usize>,
    max_edges: Option<usize>,
    skip_hubs: Option<&str>,
    filter: Option<&str>,
    count_by: Option<&str>,
    json: bool,
) -> Result<String, SrcwalkError> {
    run_callers_with_artifact(
        target,
        scope,
        expand,
        budget_tokens,
        limit,
        offset,
        glob,
        cache,
        depth,
        max_frontier,
        max_edges,
        skip_hubs,
        filter,
        count_by,
        json,
        ArtifactMode::Source,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_callers_with_artifact(
    target: &str,
    scope: &Path,
    expand: usize,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
    depth: Option<usize>,
    max_frontier: Option<usize>,
    max_edges: Option<usize>,
    skip_hubs: Option<&str>,
    filter: Option<&str>,
    count_by: Option<&str>,
    json: bool,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    if artifact.enabled() && matches!(depth, Some(d) if d >= 2) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "--artifact callers currently supports direct call sites only; omit --depth"
                .to_string(),
        });
    }
    if matches!(depth, Some(d) if d >= 2) && (filter.is_some() || count_by.is_some()) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason:
                "--filter and --count-by currently apply to direct --callers only; omit --depth"
                    .to_string(),
        });
    }

    let session = session::Session::new();
    let bloom = index::bloom::BloomFilterCache::new();

    // BFS path when --depth N (N >= 2). Otherwise use compact direct-call rows by default.
    let output = match depth {
        Some(d) if d >= 2 => search::callers::search_callers_bfs(
            target,
            scope,
            cache,
            &bloom,
            d.min(5),
            max_frontier.unwrap_or(50),
            max_edges.unwrap_or(500),
            glob,
            skip_hubs,
            json,
            budget_tokens.map(|b| b as usize),
        )?,
        _ => search::callers::search_callers_expanded_with_artifact(
            target, scope, cache, &session, &bloom, expand, None, limit, offset, glob, filter,
            count_by, artifact,
        )?,
    };
    if json {
        // BFS JSON handles its own budget internally (edges array cap).
        // Legacy callers JSON returns unmodified for machine-readable output.
        return Ok(output);
    }
    match budget_tokens {
        Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
        None => Ok(output),
    }
}
