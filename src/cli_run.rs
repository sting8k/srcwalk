use std::path::{Path, PathBuf};
use std::process;

use crate::cli::{DiscoverAs, Mode, RunConfig};
use crate::output;
use srcwalk::ArtifactMode;

pub(crate) const MAX_SHOW_TARGETS: usize = 8;
const MAX_CONTEXT_TARGETS: usize = 3;
const MAX_MULTI_CONTEXT_LINES: usize = 10;
// Below this floor, a context packet cannot reliably retain both source evidence
// and its exact next-read cues; reject instead of returning a misleading fragment.
const MIN_MULTI_CONTEXT_PACKET_TOKENS: u64 = 160;

fn display_path(path: &Path) -> String {
    let path = path.display().to_string();
    if !cfg!(windows) {
        return path;
    }

    let path = path.replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = path.strip_prefix("//?/") {
        rest.to_string()
    } else {
        path
    }
}

fn merge_glob_and_exclude(
    scope_glob: Option<&str>,
    glob: Option<&str>,
    exclude: Option<&str>,
) -> Option<String> {
    let mut patterns = Vec::new();
    if let Some(scope_glob) = scope_glob.filter(|s| !s.is_empty()) {
        patterns.push(scope_glob.to_string());
    }
    if let Some(glob) = glob.filter(|s| !s.is_empty()) {
        patterns.push(glob.to_string());
    }
    if let Some(exclude) = exclude.filter(|s| !s.is_empty()) {
        patterns.push(format!("!{exclude}"));
    }
    (!patterns.is_empty()).then(|| patterns.join("\n"))
}

fn mode_scope_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Search => "discover symbol/search",
        Mode::Text => "discover --as text",
        Mode::MatchAll => "discover --match all",
        Mode::Files => "discover --as file",
        Mode::Show => "show/root path reads",
        Mode::Overview => "overview",
        Mode::Context => "context",
        Mode::DecisionFlow => "decision-flow",
        Mode::Diff => "diff",
        Mode::Review => "review",
        Mode::Compare => "compare",
        Mode::Assess => "assess",
        Mode::Callers => "trace callers",
        Mode::Callees => "trace callees",
        Mode::Deps => "deps",
    }
}

struct NormalizedScopes {
    scopes: Vec<PathBuf>,
    glob: Option<String>,
    line_range: Option<(u32, u32)>,
}

fn parse_scope_line_range(section: &str) -> Option<(u32, u32)> {
    if let Ok(line) = section.parse::<u32>() {
        return (line > 0).then_some((line, line));
    }

    let (start, end) = section.split_once('-')?;
    let start = start.parse::<u32>().ok()?;
    let end = end.parse::<u32>().ok()?;
    (start > 0 && end >= start).then_some((start, end))
}

fn looks_like_scope_line_range(section: &str) -> bool {
    section.chars().any(|c| c.is_ascii_digit())
        && section.chars().all(|c| c.is_ascii_digit() || c == '-')
}

fn resolve_existing_scope_file(raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    std::fs::metadata(&resolved)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| PathBuf::from(raw))
}

fn scope_path_exists(raw: &str) -> bool {
    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let Ok(cwd) = std::env::current_dir() else {
            return false;
        };
        cwd.join(path)
    };
    resolved.try_exists().unwrap_or(false)
}

fn comma_scope_hint(scope: &Path) -> Option<String> {
    let raw = scope.to_str()?;
    if scope_path_exists(raw) {
        return None;
    }
    let parts = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 || !parts.iter().all(|part| scope_path_exists(part)) {
        return None;
    }
    let flags = parts
        .iter()
        .map(|part| {
            let display = display_path(Path::new(part));
            srcwalk::format::shell_quote_arg(&display).map(|quoted| format!("--scope {quoted}"))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(format!(
        "hint: --scope does not accept a comma-separated list; repeat the flag:\n  {}",
        flags.join(" ")
    ))
}

fn split_scope_line_range_or_exit(scope: PathBuf) -> (PathBuf, Option<(u32, u32)>) {
    let raw = scope.to_string_lossy().into_owned();
    let Some((path_part, section)) = raw.rsplit_once(':') else {
        return (scope, None);
    };
    if path_part.is_empty() {
        return (scope, None);
    }

    if let Some(range) = parse_scope_line_range(section) {
        if let Some(path) = resolve_existing_scope_file(path_part) {
            return (path, Some(range));
        }
        if scope_path_exists(&raw) {
            return (scope, None);
        }
        eprintln!(
            "error: invalid scope: {} [No such file or directory]",
            display_path(Path::new(path_part))
        );
        process::exit(2);
    }

    if looks_like_scope_line_range(section) && resolve_existing_scope_file(path_part).is_some() {
        eprintln!("error: invalid scope line range: {section}");
        process::exit(2);
    }

    (scope, None)
}

fn line_range_filter(range: Option<(u32, u32)>) -> Option<String> {
    range.map(|(start, end)| {
        if start == end {
            format!("line:{start}")
        } else {
            format!("line:{start}-{end}")
        }
    })
}

fn has_line_filter(filter: &str) -> bool {
    filter
        .split_whitespace()
        .any(|part| part.trim_start().starts_with("line:"))
}

fn merge_filter_and_line_range(filter: Option<&str>, range: Option<(u32, u32)>) -> Option<String> {
    let filter = filter.filter(|value| !value.trim().is_empty());
    if range.is_some_and(|_| filter.is_some_and(has_line_filter)) {
        eprintln!(
            "error: --scope <file>:<range> cannot be combined with --filter line:<range>; put the intended range in one place"
        );
        process::exit(2);
    }

    match (filter, line_range_filter(range)) {
        (Some(filter), Some(range_filter)) => Some(format!("{filter} {range_filter}")),
        (Some(filter), None) => Some(filter.to_string()),
        (None, Some(range_filter)) => Some(range_filter),
        (None, None) => None,
    }
}

fn mode_allows_discover_scope(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::Search | Mode::Text | Mode::MatchAll | Mode::Files | Mode::Diff | Mode::Review
    )
}

fn mode_allows_exact_scope_auto_artifact(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::Search
            | Mode::Text
            | Mode::MatchAll
            | Mode::Callers
            | Mode::Callees
            | Mode::Context
            | Mode::Assess
    )
}

fn is_text_or(
    mode: Mode,
    discover_as: Option<DiscoverAs>,
    match_explicit: bool,
    inferred_text_or: bool,
    query: &str,
) -> bool {
    (match_explicit || inferred_text_or)
        && matches!(mode, Mode::Text)
        && matches!(discover_as, Some(DiscoverAs::Text))
        && query.contains(',')
}

fn has_glob_chars(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
}

fn split_scope_glob(scope: &Path) -> (PathBuf, String) {
    let raw = scope.to_string_lossy();
    let first_glob = raw
        .char_indices()
        .find_map(|(idx, c)| matches!(c, '*' | '?' | '[' | ']' | '{' | '}').then_some(idx))
        .expect("caller checked glob chars");
    let Some(separator) = raw[..first_glob].rfind(['/', '\\']) else {
        return (PathBuf::from("."), raw.replace('\\', "/"));
    };

    let base = if separator == 0 {
        PathBuf::from(&raw[..=separator])
    } else {
        PathBuf::from(&raw[..separator])
    };
    let pattern = raw[separator + 1..].replace('\\', "/");
    (base, format!("/{pattern}"))
}

fn canonicalize_scope_or_exit(scope: PathBuf, require_dir: bool, mode: Mode) -> PathBuf {
    let meta = match std::fs::metadata(&scope) {
        Ok(meta) => meta,
        Err(e) => {
            eprintln!("error: invalid scope: {} [{e}]", display_path(&scope));
            if let Some(hint) = comma_scope_hint(&scope) {
                eprintln!("{hint}");
            }
            process::exit(2);
        }
    };
    if require_dir && !meta.is_dir() {
        if matches!(mode, Mode::Overview) && meta.is_file() {
            eprintln!(
                "error: overview expects a directory scope; use `srcwalk show {}` or `srcwalk context {}:<line>` for a file target",
                display_path(&scope),
                display_path(&scope)
            );
        } else {
            eprintln!(
                "error: invalid scope: {} [not a directory]",
                display_path(&scope)
            );
        }
        process::exit(2);
    }
    if !require_dir && !meta.is_dir() && !meta.is_file() {
        eprintln!(
            "error: invalid scope: {} [not a file or directory]",
            display_path(&scope)
        );
        process::exit(2);
    }
    scope.canonicalize().unwrap_or(scope)
}

fn canonicalize_scopes_or_exit(
    scopes: Vec<PathBuf>,
    mode: Mode,
    explicit_artifact: bool,
) -> NormalizedScopes {
    let allow_file_or_glob = mode_allows_discover_scope(mode);
    let mut normalized = Vec::with_capacity(scopes.len());
    let mut scope_glob = None;
    let mut line_range = None;
    let mut uses_file_or_glob = false;

    for scope in scopes {
        let (scope, scope_range) = split_scope_line_range_or_exit(scope);
        if let Some(scope_range) = scope_range {
            if !allow_file_or_glob {
                eprintln!(
                    "error: line-range --scope is currently supported by discover only; use a directory scope or `srcwalk show <file>:<range>`"
                );
                process::exit(2);
            }
            line_range = Some(scope_range);
            uses_file_or_glob = true;
        }

        if allow_file_or_glob && has_glob_chars(&scope) {
            let (base, pattern) = split_scope_glob(&scope);
            normalized.push(canonicalize_scope_or_exit(base, true, mode));
            scope_glob = Some(pattern);
            uses_file_or_glob = true;
            continue;
        }

        let allow_auto_artifact_file = mode_allows_exact_scope_auto_artifact(mode);
        let require_dir = !(allow_file_or_glob || allow_auto_artifact_file);
        let canonical = canonicalize_scope_or_exit(scope, require_dir, mode);
        if canonical.is_file() {
            if allow_file_or_glob
                || explicit_artifact
                || srcwalk::should_auto_artifact_file(&canonical)
            {
                uses_file_or_glob = true;
            } else if allow_auto_artifact_file {
                eprintln!(
                    "error: exact file --scope for this mode is currently artifact-only; use a directory scope for source evidence"
                );
                process::exit(2);
            }
        }
        normalized.push(canonical);
    }

    if uses_file_or_glob && normalized.len() > 1 {
        eprintln!(
            "error: file/glob scope currently accepts one --scope; use a directory scope with --filter path:<file> for multi-scope narrowing"
        );
        process::exit(2);
    }

    NormalizedScopes {
        scopes: normalized,
        glob: scope_glob,
        line_range,
    }
}

fn resolve_output_budget(configured: Option<u64>, no_budget: bool) -> Option<u64> {
    if no_budget {
        None
    } else {
        configured.or(Some(crate::cli::DEFAULT_OUTPUT_BUDGET))
    }
}

pub(crate) fn run(mut config: RunConfig) {
    // Explicit --budget wins, --no-budget disables the cap, and all other
    // commands share one deterministic default.
    let effective_budget = resolve_output_budget(config.budget, config.no_budget);

    let cache = srcwalk::cache::OutlineCache::new();
    let normalized_scopes =
        canonicalize_scopes_or_exit(config.scopes, config.mode, config.artifact.enabled());
    let scope_glob = normalized_scopes.glob;
    let scope_line_range = normalized_scopes.line_range;
    let scopes = normalized_scopes.scopes;
    if scopes.len() > 1 && !config.allow_multi_scope {
        eprintln!(
            "error: repeated --scope is not supported for {}; this mode currently accepts one --scope",
            mode_scope_label(config.mode)
        );
        process::exit(2);
    }
    let scope = scopes
        .first()
        .expect("at least one scope from clap default")
        .clone();

    if !config.artifact.enabled()
        && scopes.len() == 1
        && scope_glob.is_none()
        && scope_line_range.is_none()
        && mode_allows_exact_scope_auto_artifact(config.mode)
        && srcwalk::should_auto_artifact_file(&scope)
    {
        config.artifact = ArtifactMode::Artifact;
    }

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
                | Mode::Text
                | Mode::MatchAll
                | Mode::Show
                | Mode::Callers
                | Mode::Callees
                | Mode::Overview
                | Mode::Context
                | Mode::Assess
        )
    {
        eprintln!("error: --artifact currently supports file reads, discover/search, overview, context, assess, direct callers, and direct callees only");
        process::exit(2);
    }
    if config.artifact.enabled() && config.expand > 0 && !matches!(config.mode, Mode::Callers) {
        eprintln!("error: --artifact --expand currently applies to trace callers only");
        process::exit(2);
    }

    if matches!(config.mode, Mode::Diff) {
        if scope_line_range.is_some() {
            eprintln!("error: diff does not support line-range --scope; use a directory, exact file, or glob scope");
            process::exit(2);
        }
        let result = srcwalk::run_diff(
            config.query.as_deref(),
            config.diff_staged,
            &scope,
            scope_glob.as_deref(),
            effective_budget,
            config.limit,
            config.offset,
            &cache,
        );
        output::emit_result(result);
        return;
    }
    if matches!(config.mode, Mode::Review) {
        if scope_line_range.is_some() {
            eprintln!("error: review does not support line-range --scope; use a directory, exact file, or glob scope");
            process::exit(2);
        }
        let result = srcwalk::run_review(
            config.query.as_deref(),
            config.diff_staged,
            &scope,
            scope_glob.as_deref(),
            effective_budget,
            config.limit,
            config.offset,
            &cache,
        );
        output::emit_result(result);
        return;
    }
    if matches!(config.mode, Mode::Overview) {
        if config.budget.is_some() || config.no_budget {
            eprintln!(
                "error: overview has a fixed 15k token cap; narrow --scope or lower --depth instead"
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
        eprintln!("usage: srcwalk <path> | srcwalk discover <query> | srcwalk guide");
        process::exit(3);
    };

    let effective_limit = config.limit;
    if scope_glob.is_some() && config.glob.is_some() {
        eprintln!(
            "error: discover glob scope cannot be combined with --glob; put the full file pattern in --scope or use --exclude to subtract files"
);
        process::exit(2);
    }
    let glob_filter = merge_glob_and_exclude(
        scope_glob.as_deref(),
        config.glob.as_deref(),
        config.exclude.as_deref(),
    );
    let discover_filter = merge_filter_and_line_range(config.filter.as_deref(), scope_line_range);

    if matches!(config.mode, Mode::Compare) {
        if scope_line_range.is_some() {
            eprintln!("error: compare does not support line-range --scope; use a directory scope and exact target arguments");
            process::exit(2);
        }
        let target_b = config
            .compare_target_b
            .as_deref()
            .expect("compare command supplies target_b");
        let result = srcwalk::run_compare(&query, target_b, &scope, effective_budget, &cache);
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Show) {
        let result = run_show(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            config.full,
            config.artifact.enabled(),
            config.context_lines,
            &cache,
        );
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Files) {
        if scope_line_range.is_some() {
            eprintln!(
                "error: discover --as file does not support line-range scopes; use an exact file, directory, or glob scope"
            );
            process::exit(2);
        }
        if config.filter.is_some() || config.expand > 0 || config.glob.is_some() {
            eprintln!(
                "error: --as file supports --limit/--offset/--exclude, not --filter, --expand, or --glob"
            );
            process::exit(2);
        }
        let result = srcwalk::run_files_with_scope_filter(
            &query,
            &scope,
            effective_budget,
            effective_limit,
            config.offset,
            scope_glob.as_deref(),
            config.exclude.as_deref(),
        );
        output::emit_result(result);
        return;
    }

    if config.filter.is_some() && matches!(config.mode, Mode::Deps | Mode::Assess)
        || (config.filter.is_some() && matches!(config.mode, Mode::Callees) && !config.detailed)
    {
        eprintln!(
            "error: --filter applies to discover results, direct trace callers, context, and detailed trace callees"
        );
        process::exit(2);
    }

    if matches!(config.mode, Mode::DecisionFlow) {
        if config.filter.is_some()
            || config.expand > 0
            || config.glob.is_some()
            || config.exclude.is_some()
        {
            eprintln!("error: decision-flow supports --scope and --budget only; filters, glob/exclude, and expand are not supported");
            process::exit(2);
        }
        if config.artifact.enabled() {
            eprintln!("error: decision-flow is source-only; remove --artifact");
            process::exit(2);
        }
        let result = srcwalk::run_decision_flow(&query, &scope, effective_budget, &cache);
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Context) {
        let result = run_context(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.depth,
            config.filter.as_deref(),
            config.artifact,
        );
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Assess) {
        let result = srcwalk::run_impact_with_artifact(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.artifact,
        );
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Callers) {
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
            config.artifact,
        );
        output::emit_result(result);
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
        output::emit_result(result);
        return;
    }

    if config.access && !matches!(config.mode, Mode::MatchAll) {
        if config.artifact.enabled() {
            eprintln!("error: --as access is source-only; remove --artifact");
            process::exit(2);
        }
        if scopes.len() > 1 {
            eprintln!("error: --as access currently supports one --scope; run per scope");
            process::exit(2);
        }
        let result = srcwalk::run_access_filtered(
            &query,
            &scope,
            effective_budget,
            effective_limit,
            config.offset,
            glob_filter.as_deref(),
            discover_filter.as_deref(),
            &cache,
        );
        output::emit_result(result);
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
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::Text) {
        let text_or = is_text_or(
            config.mode,
            config.discover_as,
            config.match_explicit,
            config.inferred_text_or,
            &query,
        );
        if text_or && config.expand > 0 {
            eprintln!(
                "error: discover --match any --as text does not support --expand for comma OR yet"
            );
            process::exit(2);
        }
        if text_or && config.offset > 0 {
            eprintln!(
                "error: discover --match any --as text does not support --offset; increase --limit or narrow the terms instead"
            );
            process::exit(2);
        }
        let result = if text_or {
            srcwalk::run_text_or_filtered_with_artifact(
                &query,
                &scope,
                effective_budget,
                effective_limit,
                config.offset,
                glob_filter.as_deref(),
                discover_filter.as_deref(),
                config.artifact,
                &cache,
            )
        } else if config.expand > 0 {
            srcwalk::run_text_expanded_filtered(
                &query,
                &scope,
                effective_budget,
                config.expand,
                effective_limit,
                config.offset,
                glob_filter.as_deref(),
                discover_filter.as_deref(),
                &cache,
            )
        } else {
            srcwalk::run_text_filtered_with_artifact_and_hint(
                &query,
                &scope,
                effective_budget,
                effective_limit,
                config.offset,
                glob_filter.as_deref(),
                discover_filter.as_deref(),
                config.artifact,
                !config.match_explicit && query.contains(','),
                &cache,
            )
        };
        output::emit_result(result);
        return;
    }

    if matches!(config.mode, Mode::MatchAll) {
        if matches!(
            config.discover_as,
            Some(DiscoverAs::File | DiscoverAs::Access)
        ) {
            eprintln!("error: discover --match all supports symbol/text co-occurrence, not --as file or --as access");
            process::exit(2);
        }
        if config.expand > 0 {
            eprintln!("error: discover --match all does not support --expand yet");
            process::exit(2);
        }
        let result = srcwalk::run_cooccurrence_filtered_with_artifact(
            &query,
            &scope,
            effective_budget,
            effective_limit,
            config.offset,
            glob_filter.as_deref(),
            discover_filter.as_deref(),
            config.artifact,
            &cache,
        );
        output::emit_result(result);
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
            glob_filter.as_deref(),
            discover_filter.as_deref(),
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
            glob_filter.as_deref(),
            discover_filter.as_deref(),
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
            glob_filter.as_deref(),
            discover_filter.as_deref(),
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
            glob_filter.as_deref(),
            discover_filter.as_deref(),
            config.artifact.enabled(),
            &cache,
        )
    };

    output::emit_result(result);
}

#[allow(clippy::too_many_arguments)]
fn run_context(
    target: &str,
    scope: &Path,
    budget: Option<u64>,
    cache: &srcwalk::cache::OutlineCache,
    depth: Option<usize>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, srcwalk::error::SrcwalkError> {
    if !target.contains(',') {
        return srcwalk::run_flow_with_artifact(
            target, scope, budget, cache, depth, filter, artifact,
        );
    }

    let raw_targets: Vec<&str> = target.split(',').collect();
    if raw_targets.iter().any(|part| part.trim().is_empty()) {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "empty context target in comma-separated list".to_string(),
        });
    }
    let targets: Vec<&str> = raw_targets.iter().map(|part| part.trim()).collect();
    if targets.is_empty() {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "empty context target list".to_string(),
        });
    }
    if targets.len() > MAX_CONTEXT_TARGETS {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!(
                "context accepts at most {MAX_CONTEXT_TARGETS} comma-separated exact targets"
            ),
        });
    }
    if targets
        .iter()
        .any(|target| !looks_like_exact_context_target(target))
    {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "multi-target context requires comma-separated exact path:symbol or path:range targets; repeat the file path for each range".to_string(),
        });
    }

    let header = format!(
        "# Context: {} exact targets\n> Caveat: multi-target context splits one global budget across independent exact evidence packets.\n\n",
        targets.len()
    );
    let separator = "\n\n---\n\n";
    let target_headers: Vec<String> = targets
        .iter()
        .map(|target| format!("## Target: {target}\n"))
        .collect();
    let wrapper_bytes = header.len()
        + separator.len() * targets.len().saturating_sub(1)
        + target_headers.iter().map(String::len).sum::<usize>();
    let wrapper_tokens = estimate_tokens(wrapper_bytes as u64);
    let available_target_budget = budget.map(|cap| cap.saturating_sub(wrapper_tokens));

    if let (Some(cap), Some(available)) = (budget, available_target_budget) {
        let minimum_target_budget = MIN_MULTI_CONTEXT_PACKET_TOKENS * targets.len() as u64;
        if available < minimum_target_budget {
            let minimum_budget = wrapper_tokens + minimum_target_budget;
            return Err(srcwalk::error::SrcwalkError::InvalidQuery {
                query: target.to_string(),
                reason: format!(
                    "multi-target context --budget {cap} is too small to show all {count} target packets; raise --budget to at least {minimum_budget} or run targets separately",
                    count = targets.len()
                ),
            });
        }
    }

    let probe_budget = available_target_budget.map(|cap| cap.max(1));
    let mut probed_outputs = Vec::with_capacity(targets.len());
    for target in &targets {
        probed_outputs.push(srcwalk::run_flow_with_artifact(
            target,
            scope,
            probe_budget,
            cache,
            depth,
            filter,
            artifact,
        )?);
    }

    let output = assemble_labeled_batch(&header, separator, &target_headers, &probed_outputs);
    if estimate_tokens(output.len() as u64) <= budget.unwrap_or(u64::MAX) {
        return Ok(output);
    }

    let Some(available_target_budget) = available_target_budget else {
        return Ok(output);
    };
    let wanted_budgets: Vec<u64> = probed_outputs
        .iter()
        .map(|output| estimate_tokens(output.len() as u64))
        .collect();
    let allocated_budgets = allocate_batch_target_budgets(&wanted_budgets, available_target_budget);

    let mut outputs = Vec::with_capacity(targets.len());
    for (target, target_budget) in targets.iter().zip(allocated_budgets) {
        let output = srcwalk::run_flow_with_artifact(
            target,
            scope,
            Some(target_budget),
            cache,
            depth,
            filter,
            artifact,
        )?;
        outputs.push(apply_cli_token_budget(output, Some(target_budget)));
    }

    let output = assemble_labeled_batch(&header, separator, &target_headers, &outputs);
    Ok(apply_cli_token_budget(output, budget))
}

fn looks_like_exact_context_target(target: &str) -> bool {
    target.rsplit_once(':').is_some_and(|(path_part, _)| {
        !path_part.is_empty()
            && (path_part.contains('/')
                || path_part.contains('\\')
                || Path::new(path_part).is_absolute()
                || Path::new(path_part).extension().is_some())
    })
}
fn apply_cli_token_budget(output: String, budget: Option<u64>) -> String {
    apply_cli_token_budget_inner(output, budget, false)
}

fn apply_cli_token_budget_preserving_footer(output: String, budget: Option<u64>) -> String {
    apply_cli_token_budget_inner(output, budget, true)
}

fn apply_cli_token_budget_inner(
    mut output: String,
    budget: Option<u64>,
    preserve_footer: bool,
) -> String {
    let Some(budget) = budget else {
        return output;
    };
    if estimate_tokens(output.len() as u64) <= budget {
        return output;
    }

    if preserve_footer {
        if let Some((body, footer)) = srcwalk::format::split_trailing_footer(&output) {
            let footer = footer.trim();
            let footer_tokens = estimate_tokens(footer.len() as u64 + 2);
            if !body.trim_end().is_empty() && !footer.is_empty() && footer_tokens < budget {
                let body_budget = budget.saturating_sub(footer_tokens);
                let body = apply_cli_token_budget_inner(
                    body.trim_end().to_string(),
                    Some(body_budget),
                    false,
                );
                let rendered = format!("{}\n\n{}", body.trim_end(), footer);
                if estimate_tokens(rendered.len() as u64) <= budget {
                    return rendered;
                }
            }
        }
    }

    let marker = "\n\n... truncated to fit --budget; narrow targets or raise --budget.";
    let max_bytes = usize::try_from(budget.saturating_mul(4)).unwrap_or(usize::MAX);
    if max_bytes == 0 {
        return String::new();
    }
    if marker.len() >= max_bytes {
        return marker[..marker.floor_char_boundary(max_bytes)].to_string();
    }

    truncate_utf8(&mut output, max_bytes - marker.len());
    output.push_str(marker);
    output
}

fn estimate_tokens(byte_len: u64) -> u64 {
    byte_len.div_ceil(4)
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let end = value.floor_char_boundary(max_bytes);
    let line_end = value[..end]
        .rfind('\n')
        .filter(|cut| *cut >= max_bytes / 2)
        .unwrap_or(end);
    value.truncate(line_end);
}

fn is_shell_safe_show_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '-' | '.' | '/' | ':')
        || cfg!(windows) && c == '\\'
}

fn shell_quote_show_path(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    if value.chars().all(is_shell_safe_show_path_char) {
        return Some(value.to_string());
    }

    #[cfg(windows)]
    {
        Some(format!("'{}'", value.replace('\'', "''")))
    }
    #[cfg(not(windows))]
    {
        Some(format!("'{}'", value.replace('\'', "'\\''")))
    }
}

fn is_line_or_range_token(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if let Ok(line) = value.parse::<usize>() {
        return line > 0;
    }
    let Some((start, end)) = value.split_once('-') else {
        return false;
    };
    let Ok(start) = start.trim().parse::<usize>() else {
        return false;
    };
    let Ok(end) = end.trim().parse::<usize>() else {
        return false;
    };
    start > 0 && end >= start
}

struct SameFileRangeShorthand<'a> {
    path: &'a str,
    selector: String,
}

enum SameFileRangeShorthandRoute<'a> {
    Clean(SameFileRangeShorthand<'a>),
    Ambiguous(String),
}

fn same_file_range_shorthand_route<'a>(
    targets: &'a [&'a str],
) -> Option<SameFileRangeShorthandRoute<'a>> {
    let first = targets.first()?.trim();
    let (path, first_range) = first.rsplit_once(':')?;
    if path.is_empty() || !is_line_or_range_token(first_range) {
        return None;
    }

    let mut ranges = vec![first_range.trim()];
    let mut saw_bare_range = false;
    for target in targets.iter().skip(1).map(|target| target.trim()) {
        if is_line_or_range_token(target) {
            saw_bare_range = true;
            ranges.push(target);
            continue;
        }
        if saw_bare_range {
            let selector = ranges.join(",");
            let path = shell_quote_show_path(path).unwrap_or_else(|| "<path>".to_string());
            return Some(SameFileRangeShorthandRoute::Ambiguous(format!(
                "ambiguous same-file comma range shorthand; use `srcwalk show {path} --section {selector}` for same-file ranges, or repeat the file path for each location"
            )));
        }
        return None;
    }

    saw_bare_range.then(|| {
        SameFileRangeShorthandRoute::Clean(SameFileRangeShorthand {
            path,
            selector: ranges.join(","),
        })
    })
}

fn assemble_labeled_batch(
    header: &str,
    separator: &str,
    target_headers: &[String],
    outputs: &[String],
) -> String {
    let targets = target_headers
        .iter()
        .zip(outputs)
        .map(|(target_header, output)| format!("{target_header}{output}"))
        .collect::<Vec<_>>();
    format!("{header}{}", targets.join(separator))
}

fn allocate_batch_target_budgets(wants: &[u64], available: u64) -> Vec<u64> {
    if wants.is_empty() {
        return Vec::new();
    }

    if available == 0 {
        return vec![0; wants.len()];
    }

    let mut budgets = vec![0; wants.len()];
    let mut remaining = (0..wants.len()).collect::<Vec<_>>();
    let mut remaining_budget = available;

    while !remaining.is_empty() {
        if remaining_budget == 0 {
            break;
        }

        let share = (remaining_budget / remaining.len() as u64).max(1);
        let mut settled_any = false;
        let mut still_truncated = Vec::new();

        for index in remaining {
            if wants[index] <= share {
                budgets[index] = wants[index];
                remaining_budget = remaining_budget.saturating_sub(wants[index]);
                settled_any = true;
            } else {
                still_truncated.push(index);
            }
        }

        if !settled_any {
            let count = still_truncated.len() as u64;
            let base = remaining_budget / count;
            let mut extra = remaining_budget % count;
            for index in still_truncated {
                budgets[index] = base;
                if extra > 0 {
                    budgets[index] += 1;
                    extra -= 1;
                }
            }
            break;
        }

        remaining = still_truncated;
    }

    budgets
}

#[allow(clippy::too_many_arguments)]
fn run_show(
    target: &str,
    scope: &Path,
    section: Option<&str>,
    budget: Option<u64>,
    full: bool,
    artifact: bool,
    context_lines: Option<usize>,
    cache: &srcwalk::cache::OutlineCache,
) -> Result<String, srcwalk::error::SrcwalkError> {
    if !target.contains(',') {
        return srcwalk::run_path_exact_with_artifact_and_context(
            target,
            scope,
            section,
            budget,
            full,
            artifact,
            context_lines,
            cache,
        );
    }

    if section.is_some() {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "--section applies to one show target; use comma-separated path:section targets instead"
                .to_string(),
        });
    }

    let targets: Vec<&str> = target
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if targets.is_empty() {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "empty show target list".to_string(),
        });
    }
    if let Some(route) = same_file_range_shorthand_route(&targets) {
        match route {
            SameFileRangeShorthandRoute::Clean(shorthand) => {
                return srcwalk::run_path_exact_with_artifact_and_context(
                    shorthand.path,
                    scope,
                    Some(&shorthand.selector),
                    budget,
                    full,
                    artifact,
                    context_lines,
                    cache,
                );
            }
            SameFileRangeShorthandRoute::Ambiguous(reason) => {
                return Err(srcwalk::error::SrcwalkError::InvalidQuery {
                    query: target.to_string(),
                    reason,
                });
            }
        }
    }
    if targets.len() > MAX_SHOW_TARGETS {
        return Err(srcwalk::error::SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!("show accepts at most {MAX_SHOW_TARGETS} comma-separated locations"),
        });
    }

    let header = format!("# Show: {} locations\n\n", targets.len());
    let separator = "\n\n---\n\n";
    let target_headers: Vec<String> = targets
        .iter()
        .map(|target| format!("## Target: {target}\n"))
        .collect();
    let wrapper_bytes = header.len()
        + separator.len() * targets.len().saturating_sub(1)
        + target_headers.iter().map(String::len).sum::<usize>();
    let wrapper_tokens = estimate_tokens(wrapper_bytes as u64);
    let available_target_budget = budget.map(|cap| cap.saturating_sub(wrapper_tokens));
    let probe_budget = available_target_budget.map(|cap| cap.max(1));
    let per_target_context = context_lines.map(|count| count.min(MAX_MULTI_CONTEXT_LINES));

    let mut probed_outputs = Vec::with_capacity(targets.len());
    for target in &targets {
        probed_outputs.push(srcwalk::run_path_exact_with_artifact_and_context(
            target,
            scope,
            None,
            probe_budget,
            full,
            artifact,
            per_target_context,
            cache,
        )?);
    }

    let output = assemble_labeled_batch(&header, separator, &target_headers, &probed_outputs);
    if estimate_tokens(output.len() as u64) <= budget.unwrap_or(u64::MAX) {
        return Ok(output);
    }

    let Some(available_target_budget) = available_target_budget else {
        return Ok(output);
    };
    let wanted_budgets: Vec<u64> = probed_outputs
        .iter()
        .map(|output| estimate_tokens(output.len() as u64))
        .collect();
    let allocated_budgets = allocate_batch_target_budgets(&wanted_budgets, available_target_budget);

    let mut outputs = Vec::with_capacity(targets.len());
    for (target, target_budget) in targets.iter().zip(allocated_budgets) {
        let output = srcwalk::run_path_exact_with_artifact_and_context(
            target,
            scope,
            None,
            Some(target_budget),
            full,
            artifact,
            per_target_context,
            cache,
        )?;
        outputs.push(apply_cli_token_budget_preserving_footer(
            output,
            Some(target_budget),
        ));
    }

    let output = assemble_labeled_batch(&header, separator, &target_headers, &outputs);
    Ok(apply_cli_token_budget_preserving_footer(output, budget))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_cli_token_budget_preserving_footer, resolve_output_budget,
        same_file_range_shorthand_route, SameFileRangeShorthandRoute,
    };
    use crate::cli::DEFAULT_OUTPUT_BUDGET;

    #[test]
    fn cli_footer_preservation_handles_trailing_newline() {
        let body = (0..200)
            .map(|i| format!("line {i}: lots of generated content"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!("# Header\n{body}\n\n> Caveat: partial\n> Next: use --expand next\n");

        let rendered = apply_cli_token_budget_preserving_footer(output, Some(80));

        assert!(rendered.contains("truncated to fit --budget"), "{rendered}");
        assert!(
            rendered.ends_with("> Caveat: partial\n> Next: use --expand next"),
            "{rendered}"
        );
    }
    #[test]
    fn same_file_range_shorthand_parses_windows_drive_path() {
        let targets = [r"C:\repo\src\lib.rs:1", "3-4"];
        let Some(SameFileRangeShorthandRoute::Clean(route)) =
            same_file_range_shorthand_route(&targets)
        else {
            panic!("expected clean same-file shorthand route");
        };

        assert_eq!(route.path, r"C:\repo\src\lib.rs");
        assert_eq!(route.selector, "1,3-4");
    }

    #[test]
    fn ambiguous_same_file_shorthand_uses_placeholder_when_path_cannot_be_quoted() {
        let targets = ["bad\npath.rs:1", "3", "other.rs:1"];
        let Some(SameFileRangeShorthandRoute::Ambiguous(reason)) =
            same_file_range_shorthand_route(&targets)
        else {
            panic!("expected ambiguous same-file shorthand route");
        };

        assert!(reason.contains("srcwalk show <path> --section 1,3"));
        assert!(!reason.contains("bad\npath.rs"), "{reason:?}");
    }
    #[test]
    fn output_budget_resolution_preserves_default_and_overrides() {
        assert_eq!(DEFAULT_OUTPUT_BUDGET, 6_000);
        assert_eq!(resolve_output_budget(None, false), Some(6_000));
        assert_eq!(resolve_output_budget(Some(1_234), false), Some(1_234));
        assert_eq!(resolve_output_budget(None, true), None);
        assert_eq!(resolve_output_budget(Some(1_234), true), None);
    }
}
