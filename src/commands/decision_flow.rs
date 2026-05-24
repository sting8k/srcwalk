use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::commands::find::symbol_or_file_suggestion;
use crate::error::SrcwalkError;
use crate::lang::{self, decision_flow, decision_flow::FlowTarget, decision_flow::TargetSelector};
use crate::search;
use crate::types::{self, FileType};

pub(crate) fn run_decision_flow(
    target: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    _cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let resolved = resolve_decision_flow_target(target, scope)?;
    let content =
        std::fs::read_to_string(&resolved.path).map_err(|source| SrcwalkError::IoError {
            path: resolved.path.clone(),
            source,
        })?;
    let FileType::Code(lang) = lang::detect_file_type(&resolved.path) else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "decision-flow requires a source code file".to_string(),
        });
    };

    decision_flow::render_decision_flow(&resolved, &content, lang, budget_tokens)
}

pub(crate) fn resolve_decision_flow_target(
    target: &str,
    scope: &Path,
) -> Result<FlowTarget, SrcwalkError> {
    if let Some(target) = resolve_path_target(target, scope)? {
        return Ok(target);
    }

    let def_match = find_primary_definition(target, scope)?;
    let Some((start, end)) = def_match.def_range else {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: "symbol target did not provide a definition range".to_string(),
        });
    };
    Ok(FlowTarget {
        path: def_match.path,
        display_target: target.to_string(),
        selector: TargetSelector::LineRange { start, end },
    })
}

fn resolve_path_target(target: &str, scope: &Path) -> Result<Option<FlowTarget>, SrcwalkError> {
    if let Some((path_part, selector)) = target.rsplit_once(':') {
        if path_part.is_empty() {
            return Ok(None);
        }
        if let Some(path) = resolve_existing_file(path_part, scope) {
            return Ok(Some(FlowTarget {
                path,
                display_target: target.to_string(),
                selector: parse_selector(selector),
            }));
        }
        if looks_like_path(path_part) {
            return Err(SrcwalkError::NotFound {
                path: scope.join(path_part),
                suggestion: None,
            });
        }
    }

    if let Some(path) = resolve_existing_file(target, scope) {
        return Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!(
                "target needs a symbol, line, or range; read the file with `srcwalk {}` or try {}:<symbol> / {}:<line>",
                crate::format::display_path(&path),
                crate::format::display_path(&path),
                crate::format::display_path(&path)
            ),
        });
    }

    Ok(None)
}

fn parse_selector(selector: &str) -> TargetSelector {
    parse_line_range(selector).map_or_else(
        || TargetSelector::Symbol(selector.to_string()),
        |(start, end)| TargetSelector::FocusedLineRange { start, end },
    )
}

fn parse_line_range(section: &str) -> Option<(u32, u32)> {
    if let Ok(line) = section.parse::<u32>() {
        return (line > 0).then_some((line, line));
    }
    let (start, end) = section.split_once('-')?;
    let start = start.parse::<u32>().ok()?;
    let end = end.parse::<u32>().ok()?;
    (start > 0 && end >= start).then_some((start, end))
}

fn resolve_existing_file(raw: &str, scope: &Path) -> Option<PathBuf> {
    let path = Path::new(raw);
    let mut candidates = Vec::new();
    if path.is_absolute() {
        candidates.push(path.to_path_buf());
    } else {
        candidates.push(scope.join(path));
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_path = cwd.join(path);
            if candidates.first() != Some(&cwd_path) {
                candidates.push(cwd_path);
            }
        }
    }

    candidates.into_iter().find(|candidate| {
        std::fs::metadata(candidate)
            .ok()
            .is_some_and(|meta| meta.is_file())
    })
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/') || value.contains('\\') || Path::new(value).extension().is_some()
}

fn find_primary_definition(target: &str, scope: &Path) -> Result<types::Match, SrcwalkError> {
    let raw = search::search_symbol_raw(target, scope, None)?;
    let definitions: Vec<_> = raw
        .matches
        .into_iter()
        .filter(|m| m.is_definition && m.def_range.is_some())
        .collect();

    match definitions.as_slice() {
        [] => Err(SrcwalkError::NoMatches {
            query: target.to_string(),
            scope: scope.to_path_buf(),
            suggestion: symbol_or_file_suggestion(scope, target, None),
            guidance: None,
        }),
        [definition] => Ok(definition.clone()),
        _ => Err(SrcwalkError::InvalidQuery {
            query: target.to_string(),
            reason: format!(
                "ambiguous symbol target matched multiple definitions; use file:symbol or file:line. Candidates: {}",
                definitions
                    .iter()
                    .take(5)
                    .map(|m| {
                        let range = m
                            .def_range
                            .map(|(start, end)| format!(":{start}-{end}"))
                            .unwrap_or_default();
                        format!("{}{}", crate::format::display_path(&m.path), range)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_selector_accepts_line_ranges_and_symbols() {
        assert!(matches!(
            parse_selector("10-12"),
            TargetSelector::FocusedLineRange { start: 10, end: 12 }
        ));
        assert!(matches!(
            parse_selector("route"),
            TargetSelector::Symbol(symbol) if symbol == "route"
        ));
    }
}
