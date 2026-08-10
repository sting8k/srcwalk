//! Same-line ordered co-occurrence search (US-059).
//!
//! Translates an rg-style `a.*b` pattern into a bounded same-line search where
//! `a` must appear strictly before `b` on the same line. This is plain
//! line-by-line text scanning — no regex engine is executed.

use std::path::Path;
use std::sync::Mutex;

use crate::error::SrcwalkError;
use crate::search::rank;
use crate::types::{Match, SearchResult};

/// Maximum per-file size we scan for co-occurrence (same as content search).
const MAX_SEARCH_FILE_SIZE: u64 = 500_000;

/// Search lines where `term1` appears strictly before `term2` on the same line.
pub fn search_same_line_ordered(
    term1: &str,
    term2: &str,
    scope: &Path,
    glob: Option<&str>,
    artifact: crate::ArtifactMode,
) -> Result<SearchResult, SrcwalkError> {
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    let walker = if artifact.enabled() {
        super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        super::walker(scope, glob)?
    };

    walker.run(|| {
        let term1 = &term1;
        let term2 = &term2;
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
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > MAX_SEARCH_FILE_SIZE {
                    return ignore::WalkState::Continue;
                }
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return ignore::WalkState::Continue;
            };
            let (file_lines, mtime) = super::io::file_metadata(path);

            let mut file_matches = Vec::new();
            for (idx, line) in content.lines().enumerate() {
                let line_num = idx as u32 + 1;
                if let Some(pos) = line.find(term1) {
                    let after = pos + term1.len();
                    if after <= line.len() && line[after..].find(term2).is_some() {
                        file_matches.push(Match {
                            path: path.to_path_buf(),
                            line: line_num,
                            text: crate::search::truncate::compact_match_line(
                                line.trim_end(),
                                // Center on the first-term hit for long lines.
                                &format!("{term1}.*{term2}"),
                                true,
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
                    }
                }
            }

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

    rank::sort(&mut all_matches, &format!("{term1}.{term2}"), scope, None);

    let total = all_matches.len();
    Ok(SearchResult {
        query: format!("{term1}.*{term2}"),
        scope: scope.to_path_buf(),
        matches: all_matches,
        total_found: total,
        definition_candidates: 0,
        name_occurrence_candidates: 0,
        definitions: 0,
        usages: total,
        comments: 0,
        has_more: false,
        offset: 0,
    })
}
