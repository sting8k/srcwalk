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
    result.offset = offset;
}
