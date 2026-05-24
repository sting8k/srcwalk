//! Caller graph: who calls a target symbol.
//!
//! - [`single`] — direct call sites (one-hop).
//! - [`bfs`] — transitive caller graph (multi-hop, `--depth N`).
//!
//! Re-export stable `crate::search::callers::*` paths while implementation
//! modules stay internal.

// Implementation modules stay internal; callers use the re-exports below.
mod bfs;
mod single;

#[allow(unused_imports)]
pub use bfs::{compute_suspicious_hops, search_callers_bfs, BfsEdge, BfsStats, SuspicionInfo};
pub(crate) use single::find_callers_batch;
#[allow(unused_imports)]
pub use single::{
    find_callers, find_callers_with_artifact, search_callers_expanded,
    search_callers_expanded_with_artifact, CallerMatch,
};
