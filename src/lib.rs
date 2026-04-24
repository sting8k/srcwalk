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

pub(crate) mod budget;
pub mod cache;
pub(crate) mod classify;
pub mod error;
pub(crate) mod format;
pub mod index;
pub(crate) mod lang;
pub mod map;
pub mod overview;
pub(crate) mod read;
pub(crate) mod search;
pub(crate) mod session;
pub(crate) mod types;

use std::path::Path;

use cache::OutlineCache;
use classify::classify;
use error::SrcwalkError;
use types::QueryType;

/// Holds expanded search dependencies, allocated once.
/// Avoids scattered `Option<T>` + `unwrap()` throughout dispatch.
struct ExpandedCtx {
    session: session::Session,
    sym_index: index::SymbolIndex,
    bloom: index::bloom::BloomFilterCache,
    expand: usize,
}

fn resolve_exact_path(query: &str, scope: &Path) -> Result<std::path::PathBuf, SrcwalkError> {
    let candidates = if Path::new(query).is_absolute() {
        vec![std::path::PathBuf::from(query)]
    } else {
        let mut paths = vec![scope.join(query)];
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_path = cwd.join(query);
            if paths.first() != Some(&cwd_path) {
                paths.push(cwd_path);
            }
        }
        paths
    };

    for path in &candidates {
        if path.try_exists().unwrap_or(false) {
            return Ok(path.clone());
        }
    }

    Err(SrcwalkError::NotFound {
        path: candidates
            .first()
            .cloned()
            .unwrap_or_else(|| scope.join(query)),
        suggestion: None,
    })
}

/// The single public API. Everything flows through here:
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
        cache,
    )
}

/// Run with expanded search — inline source for top N matches.
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
    let path = resolve_exact_path(query, scope)?;
    let output = read::read_file_with_budget(&path, section, full, budget_tokens, cache)?;
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
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
    json: bool,
) -> Result<String, SrcwalkError> {
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
        _ => {
            let mut callers_out = search::callers::search_callers_expanded(
                target, scope, cache, &session, &bloom, expand, None, limit, offset, glob,
            )?;
            callers_out.push_str("\n\n> Tip: use --depth N for transitive callers (max 5)");
            callers_out
        }
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

/// Show what a symbol calls (forward call graph).
pub fn run_callees(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
    depth: Option<usize>,
    detailed: bool,
) -> Result<String, SrcwalkError> {
    use std::fmt::Write;
    let bloom = index::bloom::BloomFilterCache::new();

    // Find definition of target symbol
    let raw = search::search_symbol_raw(target, scope, None)?;
    let def_match = raw
        .matches
        .iter()
        .find(|m| m.is_definition && m.def_range.is_some())
        .ok_or_else(|| SrcwalkError::NoMatches {
            query: target.to_string(),
            scope: scope.to_path_buf(),
            suggestion: symbol_or_file_suggestion(scope, target, None),
        })?;

    let content = std::fs::read_to_string(&def_match.path).map_err(|e| SrcwalkError::IoError {
        path: def_match.path.clone(),
        source: e,
    })?;

    let file_type = lang::detect_file_type(&def_match.path);
    let types::FileType::Code(lang) = file_type else {
        return Ok(format!("# Callees: {target}\n\n(not a code file)"));
    };

    let rel = format::rel_nonempty(&def_match.path, scope);

    // Detailed mode: ordered call sites with args + assignment context.
    if detailed {
        let sites = search::callees::extract_call_sites(&content, lang, def_match.def_range);
        if sites.is_empty() {
            return Ok(format!("# Callees: {target} ({rel})\n\n(no calls found)"));
        }
        let mut out = format!("# Callees: {target} ({rel})\n");
        for s in &sites {
            let prefix = if s.is_return { "->ret " } else { "" };
            match &s.return_var {
                Some(var) => {
                    let _ = write!(out, "\nL{} {}{} = {}", s.line, prefix, var, s.call_text);
                }
                None => {
                    let _ = write!(out, "\nL{} {}{}", s.line, prefix, s.call_text);
                }
            }
        }
        out.push_str("\n\n> Tip: detailed call sites can be long. Retry with --budget <N>, or omit --detailed for resolved callee summaries.");
        let output = match budget_tokens {
            Some(b) => budget::apply_preserving_footer(&out, b),
            None => out,
        };
        return Ok(output);
    }

    // Default mode: resolved callees with transitive expansion.
    let callee_names = search::callees::extract_callee_names(&content, lang, def_match.def_range);
    if callee_names.is_empty() {
        return Ok(format!(
            "# Callees: {target} (in {rel})\n\n(no calls found)"
        ));
    }

    let depth_limit = depth.map_or(1, |d| d.min(5) as u32);
    let nodes = search::callees::resolve_callees_transitive(
        &callee_names,
        &def_match.path,
        &content,
        cache,
        &bloom,
        depth_limit,
        50,
    );

    let mut out = format!("# Callees: {target} (in {rel})\n");

    // Unresolved callees
    let resolved_names: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.callee.name.as_str()).collect();
    let unresolved: Vec<&String> = callee_names
        .iter()
        .filter(|n| !resolved_names.contains(n.as_str()))
        .collect();

    for node in &nodes {
        let c = &node.callee;
        let rel_c = format::rel_nonempty(&c.file, scope);
        let sig = c.signature.as_deref().unwrap_or("");
        let _ = write!(
            out,
            "\n  {:<30} {}:{}-{}",
            c.name, rel_c, c.start_line, c.end_line
        );
        if !sig.is_empty() {
            let _ = write!(out, "  {sig}");
        }
        for child in &node.children {
            let rel_ch = format::rel_nonempty(&child.file, scope);
            let _ = write!(
                out,
                "\n    {:<28} {}:{}-{}",
                child.name, rel_ch, child.start_line, child.end_line
            );
            if let Some(ref s) = child.signature {
                let _ = write!(out, "  {s}");
            }
        }
    }

    if !unresolved.is_empty() {
        out.push_str("\n\n  (unresolved): ");
        out.push_str(
            &unresolved
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    out.push_str("\n\n> Tip: use --detailed for ordered call sites with args and assignments");

    let output = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&out, b),
        None => out,
    };
    Ok(output)
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

/// Test/vendor/build directories that we de-prioritize when picking a single
/// file for a bare-filename + `--section` request.
const NON_PROD_DIR_SEGMENTS: &[&str] = &[
    "tests",
    "test",
    "spec",
    "specs",
    "__tests__",
    "vendor",
    "node_modules",
    "override",
    "overrides",
    "fixtures",
    "examples",
    "docs",
    "build",
    "dist",
    "target",
];

fn is_non_prod(path: &Path, scope: &Path) -> bool {
    let rel = path.strip_prefix(scope).unwrap_or(path);
    rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| NON_PROD_DIR_SEGMENTS.contains(&s))
    })
}

/// Build a set of files visible to a .gitignore-respecting walk of `scope`.
/// Anything NOT in this set (e.g. build artifacts, benchmark fixtures, caches,
/// egg-info, venvs) is treated as non-primary — this lets us avoid hardcoding
/// every repo's ignore patterns and naturally adapts to whatever conventions
/// a project uses (`.gitignore` + `.ignore` + `.git/info/exclude`).
fn build_visible_set(scope: &Path) -> std::collections::HashSet<std::path::PathBuf> {
    let walker = ignore::WalkBuilder::new(scope)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .build();
    let mut out = std::collections::HashSet::new();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            out.insert(entry.path().to_path_buf());
        }
    }
    out
}

/// Rank by path-depth from scope (shallower = more primary). Used as a
/// tiebreaker when gitignore + hardcoded filters still leave >1 candidate:
/// an `index.ts` or `Program.cs` at the workspace root is almost always the
/// one the agent wants, vs. nested test harness copies.
fn depth_from_scope(path: &Path, scope: &Path) -> usize {
    path.strip_prefix(scope)
        .unwrap_or(path)
        .components()
        .count()
}

/// Resolve a glob pattern produced from a bare filename to a single file when
/// `--section` is supplied. Returns:
/// - `Some((picked, Some(note)))` when exactly one prod-path candidate exists
///   and other candidates were skipped.
/// - `Some((picked, None))` when there's a single match overall.
/// - Returns an `Err(InvalidQuery)` listing candidates when the choice is
///   ambiguous (>1 prod paths or >1 total with no prod/non-prod split).
/// - `Ok(None)` when the glob matched nothing — caller falls back to the
///   normal Glob handler so existing 0-match UX is preserved.
fn disambiguate_glob_for_section(
    pattern: &str,
    scope: &Path,
    original_query: &str,
) -> Result<Option<(std::path::PathBuf, Option<String>)>, SrcwalkError> {
    let result = search::glob::search(pattern, scope, Some(200), 0)?;
    if result.files.is_empty() {
        return Ok(None);
    }

    let total = result.files.len();
    if total == 1 {
        return Ok(Some((result.files[0].path.clone(), None)));
    }

    // .gitignore-aware "primary" set — a file is primary iff it is visible
    // to a standard gitignore-respecting walk AND not inside one of the
    // hardcoded test/vendor segments (which stay around even in repos
    // without a .gitignore).
    let visible = build_visible_set(scope);
    let primary: Vec<&std::path::PathBuf> = result
        .files
        .iter()
        .map(|e| &e.path)
        .filter(|p| visible.contains(*p) && !is_non_prod(p, scope))
        .collect();

    // Picker: single primary → done. Multiple primary → break tie by
    // min depth-from-scope if unique, otherwise fail loud.
    let picked_opt: Option<std::path::PathBuf> = match primary.len().cmp(&1) {
        std::cmp::Ordering::Equal => Some(primary[0].clone()),
        std::cmp::Ordering::Greater => {
            let min_depth = primary
                .iter()
                .map(|p| depth_from_scope(p, scope))
                .min()
                .unwrap_or(0);
            let shallowest: Vec<&std::path::PathBuf> = primary
                .iter()
                .copied()
                .filter(|p| depth_from_scope(p, scope) == min_depth)
                .collect();
            if shallowest.len() == 1 {
                Some(shallowest[0].clone())
            } else {
                None
            }
        }
        std::cmp::Ordering::Less => None,
    };

    if let Some(picked) = picked_opt {
        let skipped_count = total - 1;
        // Preview up to 3 of the skipped non-primary paths so the agent
        // knows what got filtered (helps when the pick is wrong).
        let skipped_preview: Vec<String> = result
            .files
            .iter()
            .map(|e| &e.path)
            .filter(|p| **p != picked)
            .take(3)
            .map(|p| p.strip_prefix(scope).unwrap_or(p).display().to_string())
            .collect();
        let skipped_str = if skipped_preview.is_empty() {
            String::new()
        } else {
            let joined = skipped_preview.join(", ");
            let more = if skipped_count > skipped_preview.len() {
                format!(", +{} more", skipped_count - skipped_preview.len())
            } else {
                String::new()
            };
            format!(" [{joined}{more}]")
        };
        let note = format!(
            "Resolved '{original_query}' → {} (skipped {skipped_count} non-primary {}{skipped_str}). Pass full path to override.",
            picked.strip_prefix(scope).unwrap_or(&picked).display(),
            if skipped_count == 1 { "copy" } else { "copies" },
        );
        return Ok(Some((picked, Some(note))));
    }

    // Ambiguous — fail loud with top-5 candidates (prefer primary set).
    let candidates: Vec<&std::path::PathBuf> = if primary.is_empty() {
        result.files.iter().take(5).map(|e| &e.path).collect()
    } else {
        primary
    };
    let listing = candidates
        .iter()
        .take(5)
        .map(|p| format!("  - {}", p.strip_prefix(scope).unwrap_or(p).display()))
        .collect::<Vec<_>>()
        .join("\n");
    let more = if candidates.len() > 5 {
        format!("\n  ... and {} more", candidates.len() - 5)
    } else {
        String::new()
    };
    Err(SrcwalkError::InvalidQuery {
        query: original_query.to_string(),
        reason: format!(
            "matches {total} files; --section needs exactly one. Candidates:\n{listing}{more}\nPass full path or narrow --scope."
        ),
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
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
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

    if resolution_note.is_none()
        && classify::looks_like_path_query(query)
        && !matches!(query_type, QueryType::FilePath(_))
    {
        let mode = if matches!(query_type, QueryType::Glob(_)) {
            "glob"
        } else {
            "search"
        };
        resolution_note = Some(format!(
            "> Note: query looks like a path but was not found under {}; interpreting as {mode}.\n> Tip: pass --scope <repo>, use an absolute path, or use --path-exact to fail fast.",
            scope.display()
        ));
    }

    let use_expanded =
        expand > 0 && !matches!(query_type, QueryType::FilePath(_) | QueryType::Glob(_));

    // Multi-symbol: comma-separated identifiers, 2..=5 items
    // Check before main dispatch. Only activate when all parts look like identifiers
    // to avoid hijacking regex (/foo,bar/) or glob (*.{rs,ts}) queries.
    if query.contains(',')
        && !matches!(
            query_type,
            QueryType::Regex(_) | QueryType::Glob(_) | QueryType::FilePath(_)
        )
    {
        let parts: Vec<&str> = query
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let all_identifiers = parts.iter().all(|p| classify::is_identifier(p));
        if parts.len() > 5 && all_identifiers {
            return Err(SrcwalkError::InvalidQuery {
                query: query.to_string(),
                reason: "multi-symbol search supports 2-5 symbols".to_string(),
            });
        }
        if parts.len() >= 2 && parts.len() <= 5 && all_identifiers {
            let session = session::Session::new();
            let sym_index = index::SymbolIndex::new();
            let bloom = index::bloom::BloomFilterCache::new();
            let expand = if expand > 0 { expand } else { 2 };
            let output = search::search_multi_symbol_expanded(
                &parts, scope, cache, &session, &sym_index, &bloom, expand, None, limit, offset,
                glob,
            )?;
            return match budget_tokens {
                Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
                None => Ok(output),
            };
        }
    }

    // FilePath and Glob are read operations, not search — handle before expanded dispatch
    let output_result = match query_type {
        QueryType::FilePath(path) => {
            let mut out = read::read_file_with_budget(&path, section, full, budget_tokens, cache)?;
            if section.is_none() && !full && read::would_outline(&path) {
                let related = read::imports::resolve_related_files(&path);
                if !related.is_empty() {
                    let hints: Vec<String> = related
                        .iter()
                        .filter_map(|p| p.strip_prefix(scope).ok().or(Some(p.as_path())))
                        .map(|p| p.display().to_string())
                        .collect();
                    out.push_str("\n\n> Related: ");
                    out.push_str(&hints.join(", "));
                }
                out.push_str("\n> Tip: use --deps to see imports and dependents (blast radius)");
            }
            Ok(out)
        }
        QueryType::Glob(pattern) => search::search_glob(&pattern, scope, cache, limit, offset),
        _ if use_expanded => {
            let ctx = ExpandedCtx {
                session: session::Session::new(),
                sym_index: index::SymbolIndex::new(),
                bloom: index::bloom::BloomFilterCache::new(),
                expand,
            };
            run_query_expanded(&query_type, scope, cache, &ctx, limit, offset, glob)
        }
        _ => run_query_basic(&query_type, scope, cache, limit, offset, glob),
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

    let final_out = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    };
    Ok(match resolution_note {
        Some(note) => format!("{note}\n\n{final_out}"),
        None => final_out,
    })
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
        ),
        QueryType::Regex(pattern) => search::search_regex_expanded(
            pattern,
            scope,
            cache,
            &ctx.session,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
        ),
        // FilePath/Glob never reach here (gated by use_expanded)
        QueryType::FilePath(_) | QueryType::Glob(_) => {
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
) -> Result<String, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) => search::search_symbol(name, scope, cache, limit, offset, glob),
        QueryType::Concept(text) if text.contains(' ') => {
            multi_word_concept_search(text, scope, cache, limit, offset, glob)
        }
        QueryType::Concept(text) => {
            single_query_search(text, scope, cache, true, limit, offset, glob)
        }
        QueryType::Regex(pattern) => {
            search::search_regex(pattern, scope, cache, limit, offset, glob)
        }
        QueryType::Fallthrough(text) => {
            single_query_search(text, scope, cache, false, limit, offset, glob)
        }
        QueryType::FilePath(_) | QueryType::Glob(_) => {
            unreachable!("non-search query type in basic path")
        }
    }
}

/// Shared cascade for single-word queries: symbol → content → not found.
///
/// When `prefer_definitions` is true (Concept path), only accept symbol results
/// that contain actual definitions; fall back to content otherwise.
/// When false (Fallthrough path), accept any symbol match immediately.
fn single_query_search(
    text: &str,
    scope: &Path,
    cache: &cache::OutlineCache,
    prefer_definitions: bool,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
) -> Result<String, error::SrcwalkError> {
    let mut sym_result = search::search_symbol_raw(text, scope, glob)?;
    let accept_sym = if prefer_definitions {
        sym_result.definitions > 0
    } else {
        sym_result.total_found > 0
    };

    if accept_sym {
        search::pagination::paginate(&mut sym_result, limit, offset);
        return search::format_raw_result(&sym_result, cache);
    }

    let mut content_result = search::search_content_raw(text, scope, glob)?;
    if content_result.total_found > 0 {
        search::pagination::paginate(&mut content_result, limit, offset);
        return search::format_raw_result(&content_result, cache);
    }

    // For concept queries: if symbol had usages but no definitions, show those
    if prefer_definitions && sym_result.total_found > 0 {
        search::pagination::paginate(&mut sym_result, limit, offset);
        return search::format_raw_result(&sym_result, cache);
    }

    Err(error::SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, text, glob),
    })
}

/// Multi-word concept search: exact phrase first, then relaxed word proximity.
fn multi_word_concept_search(
    text: &str,
    scope: &Path,
    cache: &cache::OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
) -> Result<String, error::SrcwalkError> {
    // Try exact phrase match first
    let mut content_result = search::search_content_raw(text, scope, glob)?;
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

    let mut relaxed_result = search::search_regex_raw(&relaxed, scope, glob)?;
    relaxed_result.query = text.to_string();
    if relaxed_result.total_found > 0 {
        search::pagination::paginate(&mut relaxed_result, limit, offset);
        return search::format_raw_result(&relaxed_result, cache);
    }

    let first_word = words.first().copied().unwrap_or(text);
    Err(error::SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, first_word, glob),
    })
}

/// Cross-convention symbol suggest first (P1.3 infra), then file-name fallback.
/// Used by symbol→content miss paths so users get a useful "Did you mean: ...".
fn symbol_or_file_suggestion(scope: &Path, query: &str, glob: Option<&str>) -> Option<String> {
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
        let rel = path.strip_prefix(scope).unwrap_or(&path).display();
        return Some(format!("{name} ({rel}:{line})"));
    }
    read::suggest_similar_file(scope, query)
}
