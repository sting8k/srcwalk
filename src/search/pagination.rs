//! Pagination for search results.
//!
//! Applied after ranking so each page is deterministic and stable across runs.

use crate::types::SearchResult;

/// Apply limit/offset pagination to a `SearchResult`.
pub(crate) fn paginate(result: &mut SearchResult, limit: Option<usize>, offset: usize) {
    let total = result.matches.len();
    if offset > 0 {
        if offset >= total {
            result.matches.clear();
        } else {
            result.matches = result.matches.split_off(offset);
        }
    }
    if let Some(cap) = limit {
        if result.matches.len() > cap {
            result.matches.truncate(cap);
            result.has_more = true;
        }
    }
    result.definitions = result.matches.iter().filter(|m| m.is_definition).count();
    result.comments = result.matches.iter().filter(|m| m.in_comment).count();
    result.usages = result
        .matches
        .len()
        .saturating_sub(result.definitions + result.comments);
    result.offset = offset;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use crate::types::{Match, SearchResult};

    use super::paginate;

    fn hit(line: u32, is_definition: bool, in_comment: bool) -> Match {
        Match {
            path: PathBuf::from("lib.rs"),
            line,
            text: "hit".to_string(),
            is_definition,
            exact: true,
            file_lines: 3,
            mtime: SystemTime::UNIX_EPOCH,
            def_range: None,
            def_name: None,
            def_weight: 0,
            impl_target: None,
            base_target: None,
            in_comment,
        }
    }

    #[test]
    fn paginate_recomputes_counts_for_returned_page() {
        let mut result = SearchResult {
            query: "hit".to_string(),
            scope: PathBuf::from("."),
            matches: vec![
                hit(1, true, false),
                hit(2, false, true),
                hit(3, false, false),
            ],
            total_found: 3,
            definitions: 1,
            usages: 1,
            comments: 1,
            has_more: false,
            offset: 0,
        };

        paginate(&mut result, Some(1), 1);

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.total_found, 3);
        assert_eq!(result.definitions, 0);
        assert_eq!(result.usages, 0);
        assert_eq!(result.comments, 1);
    }
}
