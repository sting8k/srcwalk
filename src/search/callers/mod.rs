//! Caller graph: who calls a target symbol.
//!
//! - [`single`] — direct call sites (one-hop).
//! - [`bfs`] — transitive caller graph (multi-hop, `--depth N`).
//!
//! Public surface is re-exported here for stable `crate::search::callers::*`
//! paths; both submodules are private.

// Submodules are crate-private; external paths use the re-exports below.
mod bfs;
mod single;

#[allow(unused_imports)]
pub use bfs::{
    compute_suspicious_hops, search_callers_bfs, BfsEdge, BfsStats, HopStats, SuspicionInfo,
};
pub(crate) use single::find_callers_batch;
#[allow(unused_imports)]
pub use single::{
    find_callers, search_callers_expanded, search_callers_expanded_with_artifact, CallerMatch,
};
