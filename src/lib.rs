#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,  // line numbers as u32, token counts — we target 64-bit
    clippy::cast_sign_loss,            // same
    clippy::cast_possible_wrap,        // u32→i32 for tree-sitter APIs
    clippy::module_name_repetitions,   // Rust naming conventions
    clippy::similar_names,             // common in parser/search code
    clippy::too_many_lines,            // one complex function (find_definitions)
    clippy::too_many_arguments,        // internal recursive AST walker
    clippy::unnecessary_wraps,         // Result return for API consistency
    clippy::struct_excessive_bools,    // CLI struct derives clap
    clippy::missing_errors_doc,        // internal pub(crate) fns don't need error docs
    clippy::missing_panics_doc,        // same
)]

pub(crate) mod artifact;
pub(crate) mod budget;
pub mod cache;
pub(crate) mod classify;
pub mod error;
pub(crate) mod format;
pub mod index;
pub(crate) mod lang;
pub mod map;
pub(crate) mod read;
pub(crate) mod search;
pub(crate) mod session;
pub(crate) mod types;

mod commands;

pub use commands::context::ArtifactMode;

use std::path::{Path, PathBuf};

use cache::OutlineCache;
use error::SrcwalkError;

/// classify → match on query type → return formatted string.
pub fn run(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::find::run(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        cache,
    )
}

pub fn run_filtered(
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
    commands::find::run_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        cache,
    )
}

pub fn run_filtered_with_artifact(
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
    commands::find::run_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        artifact,
        cache,
    )
}

/// Full variant — forces full file output, bypassing smart views.
pub fn run_full(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::find::run_full(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        cache,
    )
}

pub fn run_full_filtered(
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
    commands::find::run_full_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        cache,
    )
}

pub fn run_full_filtered_with_artifact(
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
    commands::find::run_full_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        artifact,
        cache,
    )
}

/// Run with expanded search — inline source for top N matches.
#[allow(clippy::too_many_arguments)]
pub fn run_expanded(
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
    commands::find::run_expanded(
        query,
        scope,
        section,
        budget_tokens,
        full,
        expand,
        limit,
        offset,
        glob,
        cache,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_expanded_filtered(
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
    commands::find::run_expanded_filtered(
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
        cache,
    )
}

pub fn run_files(
    pattern: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    commands::find::run_files(pattern, scope, budget_tokens, limit, offset)
}

pub fn run_multi_scope_find_filtered(
    query: &str,
    scopes: &[PathBuf],
    budget_tokens: Option<u64>,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::multi_scope::run_multi_scope_find_filtered(
        query,
        scopes,
        budget_tokens,
        expand,
        limit,
        offset,
        glob,
        filter,
        cache,
    )
}

pub fn run_path_exact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::path::run_path_exact(query, scope, section, budget_tokens, full, cache)
}

pub fn run_path_exact_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::path::run_path_exact_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        full,
        artifact,
        cache,
    )
}

/// Find all callers of a symbol.
#[allow(clippy::too_many_arguments)]
pub fn run_callers(
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
    commands::callers::run_callers(
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
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_callers_with_artifact(
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
    commands::callers::run_callers_with_artifact(
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
        artifact,
    )
}

/// Show what a symbol calls (forward call graph).
pub fn run_callees(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    commands::callees::run_callees(target, scope, budget_tokens, cache, depth, detailed, filter)
}

pub fn run_callees_with_artifact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    commands::callees::run_callees_with_artifact(
        target,
        scope,
        budget_tokens,
        cache,
        depth,
        detailed,
        filter,
        artifact,
    )
}

/// Lab: compact downstream flow slice for a known symbol.
#[allow(clippy::too_many_arguments)]
pub fn run_flow(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    commands::flow::run_flow(target, scope, budget_tokens, cache, depth, filter)
}

/// Lab: compact upstream blast-radius slice for changing a symbol.
pub fn run_impact(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    commands::impact::run_impact(target, scope, budget_tokens, cache)
}

/// Analyze blast-radius dependencies of a file.
pub fn run_deps(
    path: &Path,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    commands::deps::run_deps(path, scope, budget_tokens, cache, limit, offset)
}
