use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::error::SrcwalkError;
use crate::types::Match;
use crate::ArtifactMode;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;

use super::super::{file_metadata, walker};
use super::MAX_ARTIFACT_FILE_SIZE;

/// Find all usages via ripgrep (word-boundary matching).
/// Collects per-file, locks once per file (not per line).
/// Early termination once enough usages found.
pub(super) fn find_usages_with_artifact(
    query: &str,
    matcher: &RegexMatcher,
    scope: &Path,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<Vec<Match>, SrcwalkError> {
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    // Relaxed: same reasoning as find_definitions — approximate early-quit, joined before read
    let found_count = AtomicUsize::new(0);

    let walker = if artifact.enabled() {
        super::super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        walker(scope, glob)?
    };

    walker.run(|| {
        let matches = &matches;
        let found_count = &found_count;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            let is_artifact = artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path);
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }
            if super::super::io::is_minified_filename(path) && !is_artifact {
                return ignore::WalkState::Continue;
            }

            // Skip oversized files
            if let Ok(meta) = std::fs::metadata(path) {
                let is_smali_or_asm = false;
                if !is_smali_or_asm
                    && meta.len()
                        > if is_artifact {
                            MAX_ARTIFACT_FILE_SIZE
                        } else {
                            500_000
                        }
                {
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
                        text: line.trim_end().to_string(),
                        is_definition: false,
                        exact: line.contains(query),
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
                found_count.fetch_add(file_matches.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_matches);
            }

            ignore::WalkState::Continue
        })
    });

    Ok(matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

pub(super) fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
