use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::SrcwalkError;
use crate::lang::detect_file_type;
use crate::lang::outline::get_outline_entries;
use crate::search::rank;
use crate::types::{FileType, Match, OutlineEntry, SearchResult};
use crate::ArtifactMode;
use globset::Glob;

use super::super::{file_metadata, read_file_bytes, walker};
use super::definitions::outline_def_weight;

pub(super) fn search_name_glob(
    pattern: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_name_glob_with_artifact(pattern, scope, cache, context, glob, ArtifactMode::Source)
}

pub(super) fn search_name_glob_with_artifact(
    pattern: &str,
    scope: &Path,
    _cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    let matcher = Glob::new(pattern).map_err(|e| SrcwalkError::InvalidQuery {
        query: pattern.to_string(),
        reason: e.to_string(),
    })?;
    let matcher = matcher.compile_matcher();
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());

    let walker = if artifact.enabled() {
        super::super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        walker(scope, glob)?
    };
    walker.run(|| {
        let matches = &matches;
        let matcher = &matcher;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            let file_size = match std::fs::metadata(path) {
                Ok(meta) => {
                    if meta.len() > 500_000 {
                        return ignore::WalkState::Continue;
                    }
                    meta.len()
                }
                Err(_) => return ignore::WalkState::Continue,
            };
            let is_artifact = artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path);
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }
            if super::super::io::is_minified_filename(path) && !is_artifact {
                return ignore::WalkState::Continue;
            }

            let skip_bloom = false;
            let Some(bytes) = read_file_bytes(path, file_size) else {
                return ignore::WalkState::Continue;
            };
            if !skip_bloom
                && !is_artifact
                && file_size >= super::super::io::MINIFIED_CHECK_THRESHOLD
                && super::super::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };
            let FileType::Code(lang) = detect_file_type(path) else {
                return ignore::WalkState::Continue;
            };

            let (file_lines, mtime) = file_metadata(path);
            let entries = get_outline_entries(content, lang);
            let lines: Vec<&str> = content.lines().collect();
            let mut file_matches = Vec::new();
            collect_name_glob_matches(
                path,
                &lines,
                file_lines,
                mtime,
                &entries,
                matcher,
                &mut file_matches,
            );
            if is_artifact {
                collect_artifact_anchor_glob_matches(
                    path,
                    file_lines,
                    mtime,
                    content,
                    matcher,
                    &mut file_matches,
                );
            }

            if !file_matches.is_empty() {
                matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(file_matches);
            }
            ignore::WalkState::Continue
        })
    });

    let mut merged = matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rank::sort(&mut merged, pattern, scope, context);
    let total = merged.len();
    Ok(SearchResult {
        query: pattern.to_string(),
        scope: scope.to_path_buf(),
        matches: merged,
        total_found: total,
        definitions: total,
        usages: 0,
        comments: 0,
        has_more: false,
        offset: 0,
    })
}

fn collect_name_glob_matches(
    path: &Path,
    lines: &[&str],
    file_lines: u32,
    mtime: SystemTime,
    entries: &[OutlineEntry],
    matcher: &globset::GlobMatcher,
    out: &mut Vec<Match>,
) {
    for entry in entries {
        if matcher.is_match(&entry.name) {
            let line_idx = entry.start_line.saturating_sub(1) as usize;
            let line_text = lines.get(line_idx).unwrap_or(&"").trim_end();
            out.push(Match {
                path: path.to_path_buf(),
                line: entry.start_line,
                text: line_text.to_string(),
                is_definition: true,
                exact: false,
                file_lines,
                mtime,
                def_range: Some((entry.start_line, entry.end_line)),
                def_name: Some(entry.name.clone()),
                def_weight: outline_def_weight(entry.kind),
                impl_target: None,
                base_target: None,
                in_comment: false,
            });
        }
        collect_name_glob_matches(
            path,
            lines,
            file_lines,
            mtime,
            &entry.children,
            matcher,
            out,
        );
    }
}

fn collect_artifact_anchor_glob_matches(
    path: &Path,
    file_lines: u32,
    mtime: SystemTime,
    content: &str,
    matcher: &globset::GlobMatcher,
    out: &mut Vec<Match>,
) {
    for anchor in crate::artifact::capped_anchors(content, usize::MAX).0 {
        let qualified = format!("{} {}", anchor.kind, anchor.name);
        if matcher.is_match(&anchor.name) || matcher.is_match(&qualified) {
            out.push(Match {
                path: path.to_path_buf(),
                line: anchor.line,
                text: format!("artifact anchor {qualified}"),
                is_definition: true,
                exact: false,
                file_lines,
                mtime,
                def_range: None,
                def_name: Some(qualified),
                def_weight: 95,
                impl_target: None,
                base_target: None,
                in_comment: false,
            });
        }
    }
}
