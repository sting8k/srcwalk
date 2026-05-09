use std::path::Path;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::{index, search};

/// Analyze blast-radius dependencies of a file.
pub(crate) fn run_deps(
    path: &Path,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    let bloom = index::bloom::BloomFilterCache::new();
    let result = search::deps::analyze_deps(path, scope, cache, &bloom)?;
    let budget_usize = budget_tokens.map(|b| b as usize);
    Ok(search::deps::format_deps(
        &result,
        scope,
        budget_usize,
        limit,
        offset,
    ))
}
