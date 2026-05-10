use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use super::file_metadata;

use crate::error::SrcwalkError;
use crate::lang::detection;
use crate::search::rank;
use crate::types::{Match, SearchResult};
use crate::ArtifactMode;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;

const MAX_SEARCH_FILE_SIZE: u64 = 500_000;
const MAX_ARTIFACT_TEXT_FILE_SIZE: u64 = 100_000_000;

fn is_binary_file(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut buf = [0u8; 512];
    let Ok(n) = file.read(&mut buf) else {
        return true;
    };
    detection::is_binary(&buf[..n])
}

/// Content search using ripgrep crates. Literal by default, regex if `is_regex`.
pub fn search(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    search_with_artifact(
        pattern,
        scope,
        is_regex,
        context,
        glob,
        ArtifactMode::Source,
    )
}

pub fn search_with_artifact(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    let matcher = if is_regex {
        RegexMatcher::new(pattern)
    } else {
        RegexMatcher::new(&regex_syntax::escape(pattern))
    }
    .map_err(|e| SrcwalkError::InvalidQuery {
        query: pattern.to_string(),
        reason: e.to_string(),
    })?;

    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    let walker = if artifact.enabled() {
        super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        super::walker(scope, glob)?
    };

    walker.run(|| {
        let matcher = &matcher;
        let matches = &matches;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            if super::io::is_minified_filename(path) && !artifact.enabled() {
                return ignore::WalkState::Continue;
            }
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }
            if artifact.enabled() && is_binary_file(path) {
                return ignore::WalkState::Continue;
            }

            if let Ok(meta) = std::fs::metadata(path) {
                let max_size = if artifact.enabled() {
                    MAX_ARTIFACT_TEXT_FILE_SIZE
                } else {
                    MAX_SEARCH_FILE_SIZE
                };
                if meta.len() > max_size {
                    return ignore::WalkState::Continue;
                }
            }

            let (file_lines, mtime) = file_metadata(path);

            let mut file_matches = Vec::new();
            let mut searcher = Searcher::new();

            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|line_num, line| {
                    file_matches.push(Match {
                        path: path.to_path_buf(),
                        line: line_num as u32,
                        text: crate::search::truncate::compact_match_line(
                            line.trim_end(),
                            pattern,
                            is_regex,
                        ),
                        is_definition: false,
                        exact: false,
                        file_lines,
                        mtime,
                        def_range: None,
                        def_name: None,
                        def_weight: 0,
                        impl_target: None,
                        base_target: None,
                        in_comment: false,
                    });
                    Ok(true)
                }),
            );

            if !file_matches.is_empty() {
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_matches);
            }

            ignore::WalkState::Continue
        })
    });

    let mut all_matches = matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    rank::sort(&mut all_matches, pattern, scope, context);

    let total = all_matches.len();

    Ok(SearchResult {
        query: pattern.to_string(),
        scope: scope.to_path_buf(),
        matches: all_matches,
        total_found: total,
        definitions: 0,
        usages: total,
        comments: 0,
        has_more: false,
        offset: 0,
    })
}
