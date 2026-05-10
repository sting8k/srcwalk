use std::path::{Path, PathBuf};
use std::process;

use crate::cli::{Mode, RunConfig};
use crate::output;

fn canonicalize_scopes_or_exit(scopes: Vec<PathBuf>) -> Vec<PathBuf> {
    scopes
        .into_iter()
        .map(|scope| {
            let meta = match std::fs::metadata(&scope) {
                Ok(meta) => meta,
                Err(e) => {
                    eprintln!("error: invalid scope: {} [{e}]", scope.display());
                    process::exit(2);
                }
            };
            if !meta.is_dir() {
                eprintln!(
                    "error: invalid scope: {} [not a directory]",
                    scope.display()
                );
                process::exit(2);
            }
            scope.canonicalize().unwrap_or(scope)
        })
        .collect()
}

pub(crate) fn run(config: RunConfig) {
    // Effective budget: explicit --budget wins, --no-budget disables,
    // otherwise default to 5000 tokens for deterministic agent/script output.
    let effective_budget = if config.no_budget {
        None
    } else {
        config.budget.or(Some(5_000))
    };

    let cache = srcwalk::cache::OutlineCache::new();
    let scopes = canonicalize_scopes_or_exit(config.scopes);
    if scopes.len() > 1 && !config.allow_multi_scope {
        eprintln!("error: repeated --scope is currently supported only by `srcwalk find`");
        process::exit(2);
    }
    let scope = scopes
        .first()
        .expect("at least one scope from clap default")
        .clone();

    if config.artifact.enabled() && scopes.len() > 1 {
        eprintln!(
            "error: --artifact currently supports one --scope; run per scope for artifact evidence"
        );
        process::exit(2);
    }
    if config.artifact.enabled()
        && !matches!(
            config.mode,
            Mode::Search
                | Mode::PathExact
                | Mode::Callers
                | Mode::Callees
                | Mode::Map
                | Mode::Flow
                | Mode::Impact
        )
    {
        eprintln!("error: --artifact currently supports file reads, find/search, map, flow, impact, direct callers, and direct callees only");
        process::exit(2);
    }
    if config.artifact.enabled() && config.expand > 0 && !matches!(config.mode, Mode::Callers) {
        eprintln!("error: --artifact --expand currently applies to callers only");
        process::exit(2);
    }

    if matches!(config.mode, Mode::Map) {
        if config.budget.is_some() || config.no_budget {
            eprintln!(
                "error: map has a fixed 15k token cap; narrow --scope or lower --depth instead"
            );
            process::exit(2);
        }
        match srcwalk::map::generate_for_cli(
            &scope,
            config.depth,
            &cache,
            config.symbols,
            config.glob.as_deref(),
            config.artifact,
        ) {
            Ok(output) => output::emit_output(&output),
            Err(e) => {
                eprintln!("{e}");
                process::exit(e.exit_code());
            }
        }
        return;
    }

    let query = if let Some(q) = config.query {
        q
    } else {
        eprintln!("usage: srcwalk <query> [--scope DIR] [--section N-M] [--budget N]");
        process::exit(3);
    };

    let effective_limit = config.limit;

    if matches!(config.mode, Mode::Files) {
        let result = srcwalk::run_files(
            &query,
            &scope,
            effective_budget,
            effective_limit,
            config.offset,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    if matches!(config.mode, Mode::PathExact) {
        let result = srcwalk::run_path_exact_with_artifact(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            config.full,
            config.artifact.enabled(),
            &cache,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    if config.filter.is_some() && matches!(config.mode, Mode::Deps | Mode::Impact)
        || (config.filter.is_some() && matches!(config.mode, Mode::Callees) && !config.detailed)
    {
        eprintln!(
            "error: --filter applies to search results, direct callers, flow, and detailed callees"
        );
        process::exit(2);
    }

    if matches!(config.mode, Mode::Flow) {
        let result = srcwalk::run_flow_with_artifact(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.depth,
            config.filter.as_deref(),
            config.artifact,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    if matches!(config.mode, Mode::Impact) {
        let result = srcwalk::run_impact_with_artifact(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.artifact,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    if matches!(config.mode, Mode::Callers) {
        let bfs_json = config.json && matches!(config.depth, Some(d) if d >= 2);
        let result = srcwalk::run_callers_with_artifact(
            &query,
            &scope,
            config.expand,
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            &cache,
            config.depth,
            config.max_frontier,
            config.max_edges,
            config.skip_hubs.as_deref(),
            config.filter.as_deref(),
            config.count_by.as_deref(),
            bfs_json,
            config.artifact,
        );
        if bfs_json {
            // run_callers already returns pretty JSON; skip the generic wrapper.
            match result {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(e.exit_code());
                }
            }
            return;
        }
        output::emit_result(result, &query, config.json);
        return;
    }

    if matches!(config.mode, Mode::Callees) {
        let result = srcwalk::run_callees_with_artifact(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.depth,
            config.detailed,
            config.filter.as_deref(),
            config.artifact,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    if matches!(config.mode, Mode::Deps) {
        let path = if Path::new(&query).is_absolute() {
            PathBuf::from(&query)
        } else {
            let scope_path = scope.join(&query);
            if scope_path.exists() {
                scope_path
            } else {
                let cwd_path = std::env::current_dir().unwrap_or_default().join(&query);
                if cwd_path.exists() {
                    cwd_path
                } else {
                    scope_path // fall back, let analyze_deps report the error
                }
            }
        };
        let result = srcwalk::run_deps(
            &path,
            &scope,
            effective_budget,
            &cache,
            config.limit,
            config.offset,
        );
        output::emit_result(result, &query, config.json);
        return;
    }

    let result = if scopes.len() > 1 {
        srcwalk::run_multi_scope_find_filtered(
            &query,
            &scopes,
            effective_budget,
            config.expand,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    } else if config.expand > 0 {
        srcwalk::run_expanded_filtered(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            config.full,
            config.expand,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    } else if config.full {
        srcwalk::run_full_filtered_with_artifact(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            config.artifact.enabled(),
            &cache,
        )
    } else {
        srcwalk::run_filtered_with_artifact(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            config.artifact.enabled(),
            &cache,
        )
    };

    output::emit_result(result, &query, config.json);
}
