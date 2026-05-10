use std::path::Path;

use crate::cache::OutlineCache;
use crate::commands::context::{with_artifact_note, with_artifact_read_label, ArtifactMode};
use crate::error::SrcwalkError;
use crate::{artifact, budget, read};

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
    let path = resolve_exact_path(query, scope)?;
    let artifact_mode = ArtifactMode::from(artifact);
    let output = if artifact_mode.enabled() {
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
        read::read_file_with_budget(&path, section, full, budget_tokens, cache)?
    };
    let output = with_artifact_read_label(output, artifact_mode);
    let output = if section.is_none() && !full {
        artifact::add_anchors(output, &path, artifact_mode)
    } else {
        output
    };
    let output = with_artifact_note(output, artifact_mode);
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}
