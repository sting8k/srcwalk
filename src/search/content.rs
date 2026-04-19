use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::file_metadata;

use crate::error::TilthError;
use crate::search::rank;
use crate::types::{Match, SearchResult};
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;

const MAX_SEARCH_FILE_SIZE: u64 = 500_000;

/// Content search using ripgrep crates. Literal by default, regex if `is_regex`.
pub fn search(
    pattern: &str,
    scope: &Path,
    is_regex: bool,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
) -> Result<SearchResult, TilthError> {
    let matcher = if is_regex {
        RegexMatcher::new(pattern)
    } else {
        RegexMatcher::new(&regex_syntax::escape(pattern))
    }
    .map_err(|e| TilthError::InvalidQuery {
        query: pattern.to_string(),
        reason: e.to_string(),
    })?;

    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    // Relaxed is correct: walker.run() joins all threads before we read the final value.
    // Early-quit checks are approximate by design — one extra iteration is harmless.
    let total_found = AtomicUsize::new(0);

    let walker = super::walker(scope, glob)?;

    walker.run(|| {
        let matcher = &matcher;
        let matches = &matches;
        let total_found = &total_found;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();

            // Skip oversized files — tree-sitter and ripgrep shouldn't spend time on minified bundles
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > MAX_SEARCH_FILE_SIZE {
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
                        exact: false,
                        file_lines,
                        mtime,
                        def_range: None,
                        def_name: None,
                        def_weight: 0,
                        impl_target: None,
                    });
                    Ok(true)
                }),
            );

            if !file_matches.is_empty() {
                total_found.fetch_add(file_matches.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_matches);
            }

            ignore::WalkState::Continue
        })
    });

    let _total_raw = total_found.load(Ordering::Relaxed);
    let mut all_matches = matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    rank::sort(&mut all_matches, pattern, scope, context);

    let total = all_matches.len();
    let display_limit = limit.unwrap_or(usize::MAX);

    // Apply offset + limit pagination
    if offset > 0 && offset < all_matches.len() {
        all_matches = all_matches.split_off(offset);
    } else if offset >= all_matches.len() {
        all_matches.clear();
    }
    all_matches.truncate(display_limit);
    let has_more = total > offset + display_limit;

    Ok(SearchResult {
        query: pattern.to_string(),
        scope: scope.to_path_buf(),
        matches: all_matches,
        total_found: total,
        definitions: 0,
        usages: total,
        has_more,
        offset,
    })
}
