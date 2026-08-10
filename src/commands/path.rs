use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::{with_artifact_note, with_artifact_read_label, ArtifactMode};
use crate::error::SrcwalkError;
use crate::{artifact, budget, read};

fn is_path_section_suffix(section: &str) -> bool {
    if let Ok(line) = section.parse::<usize>() {
        return line > 0;
    }

    let Some((start, end)) = section.split_once('-') else {
        return false;
    };
    let Ok(start) = start.parse::<usize>() else {
        return false;
    };
    let Ok(end) = end.parse::<usize>() else {
        return false;
    };
    start > 0 && end >= start
}

fn split_inline_section(
    query: &str,
    scope: &Path,
    section: Option<&str>,
) -> (String, Option<String>) {
    if let Some(section) = section {
        return (query.to_string(), Some(section.to_string()));
    }

    let Some((path_part, inline_section)) = query.rsplit_once(':') else {
        return (query.to_string(), None);
    };
    if path_part.is_empty() || !is_path_section_suffix(inline_section) {
        return (query.to_string(), None);
    }

    if resolve_exact_path(path_part, scope).is_ok() {
        (path_part.to_string(), Some(inline_section.to_string()))
    } else {
        (query.to_string(), None)
    }
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
        guidance: Some(positional_read_recovery_hint(query, scope)),
    })
}

/// Recovery hint for the positional-read not-found branch (US-059b).
/// A bare-symbol or missing-path QUERY is a silent dead-end for agents used to
/// search-first workflows; point them at `discover` while keeping the exact-path
/// read contract explicit. Single-line, quotes the query, carries the scope.
fn positional_read_recovery_hint(query: &str, scope: &Path) -> String {
    let q = query.replace('\'', "'\\''");
    format!(
        "Try: srcwalk discover '{q}' --scope {}   (positional QUERY reads exact paths; `discover` searches)",
        crate::format::display_path(scope)
    )
}

pub(crate) fn run_path_exact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_path_exact_with_artifact(query, scope, section, budget_tokens, full, false, cache)
}

pub(crate) fn run_path_exact_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_path_exact_with_artifact_and_context(
        query,
        scope,
        section,
        budget_tokens,
        full,
        artifact,
        None,
        cache,
    )
}

pub(crate) fn run_path_exact_with_artifact_and_context(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    artifact: bool,
    context_lines: Option<usize>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    if section.is_none() {
        if let Some(target) = read::resolve_path_symbol_target(query, scope) {
            let suggestion = target.range.map(|(start, end)| {
                format!(
                    "{}:{start}-{end}",
                    crate::format::rel_nonempty(&target.path, scope)
                )
            });
            return Err(SrcwalkError::NotFound {
                path: scope.join(query),
                suggestion,
                guidance: Some(
                    "`show` takes line ranges; `path:symbol` is `context` grammar.".to_string(),
                ),
            });
        }
    }

    let (query, inline_section) = split_inline_section(query, scope, section);
    let section = inline_section.as_deref();
    let path = resolve_exact_path(&query, scope)?;
    let artifact_mode = ArtifactMode::from(artifact || artifact::should_auto_artifact_file(&path));
    let output = if artifact_mode.enabled() && context_lines.is_none() {
        if let Some(symbol) = section {
            if let Some(result) = artifact::read_js_ts_symbol_section(&path, symbol, budget_tokens)
            {
                result?
            } else {
                read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
            }
        } else {
            read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
        }
    } else {
        read::read_file_with_budget_and_context(
            &path,
            section,
            full,
            budget_tokens,
            cache,
            context_lines,
        )?
    };
    let output = with_artifact_read_label(output, artifact_mode);
    let output = if section.is_none() && !full {
        artifact::add_anchors(output, &path, artifact_mode)
    } else {
        output
    };
    let output = if section.is_none()
        && !full
        && !artifact_mode.enabled()
        && read::would_outline(&path)
        && !crate::capabilities::is_binary_artifact_path(&path)
    {
        let related = std::fs::read_to_string(&path)
            .ok()
            .map(|content| {
                read::imports::resolve_related_files_with_content_and_scope(
                    &path,
                    &content,
                    scope,
                    &crate::lang::tsconfig::ConfigCache::new(),
                    Some(8),
                )
            })
            .unwrap_or_default();
        if related.is_empty() {
            output
        } else {
            let hints = related
                .iter()
                .map(|related| crate::format::rel_nonempty(related, scope))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{output}\n\n> Related: {hints}")
        }
    } else {
        output
    };
    let output = with_artifact_note(output, artifact_mode);
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}
