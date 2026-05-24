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

fn run_callers(target: &str, scope: &Path, depth: Option<usize>) -> String {
    run_callers_capped(target, scope, depth, Some(20_000))
}

fn run_callers_capped(
    target: &str,
    scope: &Path,
    depth: Option<usize>,
    max_edges: Option<usize>,
) -> String {
    let cache = OutlineCache::new();
    srcwalk::run_callers(
        target, scope, /* expand */ 0, /* budget_tokens */ None, /* limit */ None,
        /* offset */ 0, /* glob */ None, &cache, depth, /* max_frontier */ None,
        max_edges, /* skip_hubs */ None, /* filter */ None, /* count_by */ None,
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

fn auto_hub_count(output: &str, symbol: &str) -> Option<usize> {
    let needle = format!("{symbol}(");
    let line = output
        .lines()
        .find(|line| line.contains("auto-promoted to hub") && line.contains(&needle))?;
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find(')')?;
    rest[..end].parse().ok()
}

// ── #1 Legacy byte-exact ─────────────────────────────────────────────────
// No --depth  ==  --depth 1. Protects the "don't surprise existing users"
// contract documented in src/lib.rs:150.

#[test]
fn legacy_equals_depth_1() {
    let scope = fixture_scope();
    let legacy = run_callers("find_callers_batch", &scope, None);
    let depth_1 = run_callers("find_callers_batch", &scope, Some(1));

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
    let out1 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3)));
    let out2 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3)));
    let out3 = strip_timing(&run_callers("find_callers_batch", &scope, Some(3)));
    assert_eq!(out1, out2, "BFS run 1 vs 2 must be deterministic");
    assert_eq!(out2, out3, "BFS run 2 vs 3 must be deterministic");
}

// ── #3 Monotonicity: depth=N edges ⊇ depth=N-1 edges ─────────────────────
// Property test. Deeper BFS must include every edge from shallower BFS
// (same frontier/edge caps). A regression that drops hop-1 edges while
// computing hop-2 would fail here.

fn hop_lines(output: &str, hop: usize) -> Vec<String> {
    let mut in_hop = false;
    let marker = format!("── hop {hop} ");
    let mut lines = Vec::new();
    for line in output.lines() {
        if line.starts_with("── hop ") {
            in_hop = line.starts_with(&marker);
            continue;
        }
        if in_hop && line.starts_with("  ") && line.contains("  → ") {
            lines.push(line.trim().to_string());
        }
    }
    lines
}

#[test]
fn deeper_bfs_superset_of_shallower() {
    let scope = fixture_scope();
    let d2 = run_callers("find_callers_batch", &scope, Some(2));
    let d3 = run_callers("find_callers_batch", &scope, Some(3));

    // Every hop-1 edge in depth=2 must exist in depth=3.
    // (We compare hop-1 only — deeper hops may differ if frontier cap differs,
    //  but hop-1 is fully determined by the root symbol.)
    let hop1_from_d2: std::collections::HashSet<_> = hop_lines(&d2, 1).into_iter().collect();
    let hop1_from_d3: std::collections::HashSet<_> = hop_lines(&d3, 1).into_iter().collect();
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
    let output = run_callers("Session", &scope, Some(4));
    let new_edges = auto_hub_count(&output, "new")
        .unwrap_or_else(|| panic!("expected `new` to be auto-promoted; output:\n{output}"));
    assert!(
        new_edges >= 200,
        "auto-promoted `new` must have >=200 edges (threshold); got {new_edges}\n{output}"
    );
}

// ── #5 Text shape lock ───────────────────────────────────────────────────
// Multi-hop callers now has a text-only public surface. Keep the core sections
// stable enough for agents to navigate without relying on machine output.

#[test]
fn bfs_text_includes_core_sections() {
    let scope = fixture_scope();
    let output = run_callers("find_callers_batch", &scope, Some(2));

    assert!(output.contains("# BFS callers of \"find_callers_batch\""));
    assert!(output.contains("── hop 1 ("));
    assert!(output.contains("Static by-name call graph only."));
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
    let out1 = strip_timing(&run_callers_capped("Session", &scope, Some(4), Some(50)));
    let out2 = strip_timing(&run_callers_capped("Session", &scope, Some(4), Some(50)));
    let out3 = strip_timing(&run_callers_capped("Session", &scope, Some(4), Some(50)));
    assert_eq!(
        out1, out2,
        "cap-truncated BFS run 1 vs 2 must be deterministic"
    );
    assert_eq!(
        out2, out3,
        "cap-truncated BFS run 2 vs 3 must be deterministic"
    );
    // Sanity: cap must actually trigger, otherwise the test doesn't guard.
    assert!(
        out1.contains("edges capped at hop"),
        "test fixture no longer trips the edge cap; pick a heavier target or lower the cap"
    );
}

// ── #7 Frontier bare-name extraction ─────────────────────────────────────
// Guards the fix for qualified-name frontier bug: BFS used to push
// "Class.method" into next_frontier, but find_callers_batch matches bare
// names. Without rsplit on '.'/'::' hop 2+ would always be empty.
//
// Uses `find_callers_batch` as target because hop 1 feeds several callers into
// the next frontier, and hop 2 must find callers of those hop-1 functions. If
// hop 2 has zero edges, frontier propagation regressed.

#[test]
fn bfs_hop2_finds_edges_after_frontier_propagation() {
    let scope = fixture_scope();
    let output = run_callers("find_callers_batch", &scope, Some(3));
    let hop2_edges = hop_lines(&output, 2);

    // Hop 2 must have edges (proves bare-name frontier works).
    assert!(
        !hop2_edges.is_empty(),
        "hop 2 must have >0 edges (frontier propagation); output:
{output}"
    );
}
