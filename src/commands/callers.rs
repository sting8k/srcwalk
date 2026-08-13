use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::ArtifactMode;
use crate::commands::pathsymbol::{self, PathSymbolOutcome};
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
                "--filter and --count-by currently apply to direct trace callers only; omit --depth"
                    .to_string(),
        });
    }

    let session = session::Session::new();
    let bloom = index::bloom::BloomFilterCache::new();

    // Resolve an exact `path:symbol` root first. The path pins WHICH definition
    // the agent asked about; the terminal callable key is what a direct by-name
    // search can actually match. An unresolved / ambiguous / missing / `::` root
    // is an explicit error, never a silent bare-name search.
    // The root file may sit outside --scope (it is only being identified); the
    // relation search itself stays bounded by --scope.
    let exact_root = match pathsymbol::resolve_path_symbol_outcome(target, scope) {
        PathSymbolOutcome::Error(err) | PathSymbolOutcome::UnresolvedInNamedFile(err) => {
            return Err(err)
        }
        PathSymbolOutcome::Unique { path, symbol, .. } => Some((path, symbol)),
        PathSymbolOutcome::FallThrough => None,
    };
    let lookup = exact_root.as_ref().map_or(target, |(_, symbol)| {
        crate::lang::qualified::terminal_callable_key(symbol)
    });
    let root = search::callers::CallerRoot {
        lookup,
        display: target,
        from_exact_path: exact_root.is_some(),
    };
    let outside_scope_note = exact_root
        .as_ref()
        .filter(|(path, _)| !path.starts_with(scope))
        .map(|(path, _)| {
            format!(
                "\n> Note: the exact root {} lies outside --scope {}; call sites are searched inside --scope only.",
                crate::format::display_path(path),
                crate::format::display_path(scope)
            )
        });

    // BFS path when --depth N (N >= 2). Otherwise use compact direct-call rows by default.
    let output = match depth {
        Some(d) if d >= 2 => search::callers::search_callers_bfs(
            root,
            scope,
            cache,
            &bloom,
            d.min(5),
            max_frontier.unwrap_or(50),
            max_edges.unwrap_or(500),
            glob,
            skip_hubs,
        )?,
        _ => search::callers::search_callers_expanded_with_artifact(
            root, scope, cache, &session, &bloom, expand, None, limit, offset, glob, filter,
            count_by, artifact,
        )?,
    };
    let output = match outside_scope_note {
        Some(note) => format!("{output}{note}"),
        None => output,
    };
    match budget_tokens {
        Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
        None => Ok(output),
    }
}
