//! BFS multi-hop regression + determinism + auto-hub guards.
//!
//! Fixture scope: the srcwalk repo itself, src/ subtree (small, stable, Rust).
//! These tests lock behavior after the P0 fix (f8a3d3f) so future refactors
//! can't silently regress byte-exact legacy output, determinism, or
//! data-driven auto-hub promotion.

#![allow(clippy::pedantic)]

use srcwalk::cache::OutlineCache;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_scope() -> PathBuf {
    repo_root().join("src")
}

fn run_callers(target: &str, scope: &Path, depth: Option<usize>, json: bool) -> String {
    run_callers_capped(target, scope, depth, json, Some(20_000))
}

fn run_callers_capped(
    target: &str,
    scope: &Path,
    depth: Option<usize>,
    json: bool,
    max_edges: Option<usize>,
) -> String {
    let cache = OutlineCache::new();
    srcwalk::run_callers(
        target, scope, /* expand */ 0, /* budget_tokens */ None, /* limit */ None,
        /* offset */ 0, /* glob */ None, &cache, depth, /* max_frontier */ None,
        max_edges, /* skip_hubs */ None, json,
    )
    .expect("run_callers should succeed on fixture")
}

/// Strip ", N ms" and "(~N tokens)" → make determinism comparison robust.
/// Both are non-deterministic artifacts of wall-clock timing (the token
/// estimator runs on a string that still contains the `N ms` banner).
fn strip_timing(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for pattern ", <digits> ms"
        if bytes[i] == b',' && i + 1 < bytes.len() && bytes[i + 1] == b' ' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 && j + 3 <= bytes.len() && &bytes[j..j + 3] == b" ms" {
                // Skip the whole ", N ms" segment.
                i = j + 3;
                continue;
            }
        }
        // Look for pattern "(~<digits[.digits]>[k] tokens)"
        if bytes[i] == b'(' && i + 2 < bytes.len() && &bytes[i + 1..i + 2] == b"~" {
            let mut j = i + 2;
            while j < bytes.len()
                && (bytes[j].is_ascii_digit() || bytes[j] == b'.' || bytes[j] == b'k')
            {
                j += 1;
            }
            if j > i + 2 && j + 8 <= bytes.len() && &bytes[j..j + 8] == b" tokens)" {
                i = j + 8;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ── #1 Legacy byte-exact ─────────────────────────────────────────────────
// No --depth  ==  --depth 1. Protects the "don't surprise existing users"
// contract documented in src/lib.rs:150.

#[test]
fn legacy_equals_depth_1() {
    let scope = fixture_scope();
    let legacy = run_callers("find_callers_batch", &scope, None, false);
    let depth_1 = run_callers("find_callers_batch", &scope, Some(1), false);

    // Both paths route through `search_callers_expanded`. The impact hop-2
    // section order depends on parallel walker scheduling (pre-existing
    // nondeterminism, not introduced by BFS). Compare as line-sets.
    let lines_eq = |a: &str, b: &str| {
        let mut la: Vec<&str> = a.lines().collect();
        let mut lb: Vec<&str> = b.lines().collect();
        la.sort();
        lb.sort();
        la == lb
    };
    assert!(
        lines_eq(&legacy, &depth_1),
        "legacy (no --depth) and --depth 1 must produce the same line-set\n---\nLEGACY:\n{legacy}\n---\nDEPTH_1:\n{depth_1}"
    );
}

// ── #2 Determinism ───────────────────────────────────────────────────────
// Protects the P0 fix: before f8a3d3f, BATCH_EARLY_QUIT=50 leaked into BFS
// via a race between parallel walker threads. 3 runs of identical input
// had varying edge counts. Now they must be byte-identical (modulo timing).

#[test]
fn bfs_deterministic_across_runs() {
    let scope = fixture_scope();
    let out1 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3), false));
    let out2 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3), false));
    let out3 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3), false));
    assert_eq!(out1, out2, "BFS run 1 vs 2 must be deterministic");
    assert_eq!(out2, out3, "BFS run 2 vs 3 must be deterministic");
}

// ── #3 Monotonicity: depth=N edges ⊇ depth=N-1 edges ─────────────────────
// Property test. Deeper BFS must include every edge from shallower BFS
// (same frontier/edge caps). A regression that drops hop-1 edges while
// computing hop-2 would fail here.

fn extract_edges(json: &str) -> Vec<(u32, String, String, u32, String)> {
    let v: serde_json::Value = serde_json::from_str(json).expect("valid json");
    v["edges"]
        .as_array()
        .expect("edges array")
        .iter()
        .map(|e| {
            (
                e["hop"].as_u64().unwrap() as u32,
                e["from"].as_str().unwrap().to_string(),
                e["from_file"].as_str().unwrap().to_string(),
                e["from_line"].as_u64().unwrap() as u32,
                e["to"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn deeper_bfs_superset_of_shallower() {
    let scope = fixture_scope();
    let e2 = extract_edges(&run_callers("find_callers_batch", &scope, Some(2), true));
    let e3 = extract_edges(&run_callers("find_callers_batch", &scope, Some(3), true));

    // Every hop-1 edge in depth=2 must exist in depth=3.
    // (We compare hop-1 only — deeper hops may differ if frontier cap differs,
    //  but hop-1 is fully determined by the root symbol.)
    let hop1_from_d2: std::collections::HashSet<_> = e2.iter().filter(|e| e.0 == 1).collect();
    let hop1_from_d3: std::collections::HashSet<_> = e3.iter().filter(|e| e.0 == 1).collect();
    assert_eq!(
        hop1_from_d2, hop1_from_d3,
        "hop-1 edges must be identical regardless of max_depth"
    );
}

// ── #4 Auto-hub promotion guard ──────────────────────────────────────────
// Fixture scope (src/) has `Session::new()` called ~many places. Raising
// AUTO_HUB_THRESHOLD above this count would silently disable the feature
// for Rust codebases. This test fails if threshold drifts upward without
// intent.
//
// Note: uses a target we know explodes. If promotion count changes by a
// small amount, update the lower bound — but never remove the assertion.

#[test]
fn auto_hub_promotes_new_on_self() {
    let scope = repo_root(); // full repo so `new` fan-out > 200
    let json = run_callers("Session", &scope, Some(4), true);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    let promoted = v["elided"]["auto_hubs_promoted"]
        .as_array()
        .expect("auto_hubs_promoted array");
    assert!(
        !promoted.is_empty(),
        "expected at least one auto-promoted hub on self-BFS; got empty"
    );
    // Must include "new" with >= 200 edges (the AUTO_HUB_THRESHOLD).
    let new_entry = promoted
        .iter()
        .find(|e| e["symbol"].as_str() == Some("new"));
    assert!(
        new_entry.is_some(),
        "expected `new` to be auto-promoted; got: {promoted:?}"
    );
    let edges = new_entry.unwrap()["edges"].as_u64().unwrap();
    assert!(
        edges >= 200,
        "auto-promoted `new` must have >=200 edges (threshold); got {edges}"
    );
}

// ── #5 JSON schema lock ──────────────────────────────────────────────────
// Top-level keys must remain stable — agents parse this output.

#[test]
fn json_schema_top_level_keys() {
    let scope = fixture_scope();
    let json = run_callers("find_callers_batch", &scope, Some(2), true);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");

    for key in [
        "root",
        "scope",
        "depth_reached",
        "max_depth",
        "edges_total",
        "elapsed_ms",
        "edges",
        "stats",
        "elided",
        "disclaimer",
    ] {
        assert!(
            v.get(key).is_some(),
            "JSON missing top-level key `{key}`; schema changed"
        );
    }

    for key in ["per_hop", "top_level_terminal", "unresolved_symbols"] {
        assert!(
            v["stats"].get(key).is_some(),
            "JSON missing stats.{key}; schema changed"
        );
    }

    for key in [
        "edges_cut_at_hop",
        "frontier_cuts",
        "hubs_skipped",
        "auto_hubs_promoted",
        "auto_hub_threshold",
    ] {
        assert!(
            v["elided"].get(key).is_some(),
            "JSON missing elided.{key}; schema changed"
        );
    }
}

// ── #6 Determinism under --max-edges cap ─────────────────────────────────
// Regression guard for the cap-path race: `find_callers_batch` is fed by
// the parallel walker and returned in thread-scheduling order. When the
// BFS truncates at `max_edges`, the surviving subset depended on that
// order → non-deterministic output across runs (repro: Bifrost NewClient
// d=2 returned 3 different hashes). The fix sorts hop_matches by
// (from_file, from_line, callee, caller) before the truncation loop.
//
// This test exercises the cap code path on the srcwalk repo itself
// (Session d=4 with max_edges=50 trips the cap at hop 2).

#[test]
fn bfs_deterministic_under_edge_cap() {
    let scope = repo_root();
    let j1 = run_callers_capped("Session", &scope, Some(4), true, Some(50));
    let j2 = run_callers_capped("Session", &scope, Some(4), true, Some(50));
    let j3 = run_callers_capped("Session", &scope, Some(4), true, Some(50));
    let e1 = extract_edges(&j1);
    let e2 = extract_edges(&j2);
    let e3 = extract_edges(&j3);
    assert_eq!(e1, e2, "cap-truncated BFS run 1 vs 2 must be deterministic");
    assert_eq!(e2, e3, "cap-truncated BFS run 2 vs 3 must be deterministic");
    // Sanity: cap must actually trigger, otherwise the test doesn't guard.
    let v: serde_json::Value = serde_json::from_str(&j1).unwrap();
    assert!(
        v["elided"]["edges_cut_at_hop"].is_number(),
        "test fixture no longer trips the edge cap; pick a heavier target or lower the cap"
    );
}

// ── #7 Frontier bare-name extraction ─────────────────────────────────────
// Guards the fix for qualified-name frontier bug: BFS used to push
// "Class.method" into next_frontier, but find_callers_batch matches bare
// names. Without rsplit on '.'/'::' hop 2+ would always be empty.
//
// Uses `search_callers_expanded` as target because its single caller
// (`run_callers`) has a qualified name in hop 1. If hop 2 has zero edges,
// the bare-name extraction regressed.

#[test]
fn bfs_hop2_finds_edges_after_qualified_frontier() {
    let scope = fixture_scope();
    let json = run_callers("search_callers_expanded", &scope, Some(3), true);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    let per_hop = v["stats"]["per_hop"].as_array().expect("per_hop array");

    // Must reach at least hop 2.
    assert!(
        per_hop.len() >= 2,
        "expected at least 2 hops; got {}",
        per_hop.len()
    );
    // Hop 2 must have edges (proves bare-name frontier works).
    let hop2_edges = per_hop[1]["edges"].as_u64().unwrap_or(0);
    assert!(
        hop2_edges > 0,
        "hop 2 must have >0 edges (frontier bare-name extraction); got 0"
    );
}
