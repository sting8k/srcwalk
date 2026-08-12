use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Content search. `eligible_files` is always 0 here: counting eligibility is
/// opt-in and only the explicit literal-text routes request it, so out-of-scope
/// regex/co-occurrence/expanded/inferred routes incur no per-file counter.
pub fn search_with_artifact(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    search_with_artifact_impl(pattern, scope, is_regex, context, glob, artifact, false)
}

/// Explicit literal-text search variant that also counts eligible files in the
/// same walker pass via an atomic counter. Only single-scope `--as text` Search
/// and explicit Text OR call this; every other content caller stays on
/// `search_with_artifact` with `eligible_files = 0` and no per-file lock.
pub fn search_with_artifact_counting(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    search_with_artifact_impl(pattern, scope, is_regex, context, glob, artifact, true)
}

fn search_with_artifact_impl(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    glob: Option<&str>,
    artifact: ArtifactMode,
    count_eligible: bool,
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
    let eligible_files = AtomicUsize::new(0);
    let walker = if artifact.enabled() {
        super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        super::walker(scope, glob)?
    };

    walker.run(|| {
        let matcher = &matcher;
        let matches = &matches;
        let eligible_files = &eligible_files;

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

            // On the opt-in counting variant, tally eligibility in this existing
            // content-search pass, after every walker/glob/artifact/binary/size
            // guard and before I/O. Non-counting callers skip the atomic touch.
            if count_eligible {
                eligible_files.fetch_add(1, Ordering::Relaxed);
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
    let eligible_files = if count_eligible {
        eligible_files.load(Ordering::Relaxed)
    } else {
        0
    };

    rank::sort(&mut all_matches, pattern, scope, context);

    let total = all_matches.len();

    Ok(SearchResult {
        query: pattern.to_string(),
        scope: scope.to_path_buf(),
        matches: all_matches,
        total_found: total,
        eligible_files,
        definition_candidates: 0,
        name_occurrence_candidates: 0,
        definitions: 0,
        usages: total,
        comments: 0,
        has_more: false,
        offset: 0,
    })
}

#[cfg(test)]
mod tests {
    use crate::ArtifactMode;

    use super::{search_with_artifact, search_with_artifact_counting};

    #[test]
    fn eligible_counting_is_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn one() { frobnicate }\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn two() { frobnicate }\n").unwrap();

        // Non-opt-in content search must not collect an eligible denominator.
        let plain = search_with_artifact(
            "frobnicate",
            dir.path(),
            false,
            None,
            None,
            ArtifactMode::Source,
        )
        .unwrap();
        assert_eq!(plain.eligible_files, 0, "non-opt-in route abstains");
        assert_eq!(plain.total_found, 2);

        // The opt-in literal-text variant reports the eligible-file count.
        let counted = search_with_artifact_counting(
            "frobnicate",
            dir.path(),
            false,
            None,
            None,
            ArtifactMode::Source,
        )
        .unwrap();
        assert_eq!(counted.eligible_files, 2, "opt-in route counts eligibility");
        assert_eq!(counted.total_found, 2);
    }
}
