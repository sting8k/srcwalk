// Graphwalk-style BFS over the call graph.
// Reuses `find_callers_batch` from the single-hop module — one walk per hop,
// Aho-Corasick multi-pattern.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;

use super::single::{find_callers_batch, TOP_LEVEL};

// ═══════════════════════════════════════════════════════════════════════════
// Graphwalk-style BFS over call graph (v1 — safe, reuses find_callers_batch)
// ═══════════════════════════════════════════════════════════════════════════

/// A single directed edge in the caller graph: `from` is the enclosing
/// function that contains a call to `to` at `from_loc`.
#[derive(Debug, Clone)]
pub struct BfsEdge {
    pub hop: usize,
    pub from: String,
    pub from_file: PathBuf,
    pub from_line: u32,
    pub to: String,
    /// Call-site source text (single line). Matches legacy `--callers` output:
    /// for a call spanning multiple lines, the first line is kept. Empty only
    /// when the underlying `CallerMatch.call_text` was empty.
    pub call_text: String,
}

/// Aggregated BFS result.
#[derive(Debug, Default)]
pub struct BfsStats {
    pub depth_reached: usize,
    pub edges_total: usize,
    pub frontier_cut_hops: Vec<(usize, usize)>, // (hop, frontier_size_before_cut)
    pub edges_cut_at_hop: Option<usize>,
    pub top_level_terminal: usize,
    pub unresolved_symbols: usize, // frontier symbols that produced zero callers
    pub hubs_skipped: Vec<String>, // hub symbols dropped from frontier (user override)
    pub auto_hubs_skipped: Vec<String>, // auto-promoted hub drops on later hops
    pub auto_hubs_promoted: Vec<(String, usize)>, // (symbol, edge_count) promoted this run
    pub per_hop: Vec<HopStats>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct HopStats {
    pub hop: usize,
    pub frontier_size: usize, // after hub-skip + cap
    pub edges: usize,
}

/// Hops where most edges land outside directories seen in the previous hop —
/// strong signal of cross-package name collision (e.g. hop 1 finds `pool.New`,
/// hop 2 then matches every unrelated `errors.New` / `bytes.NewBuffer` / ...).
/// Lang-agnostic: uses filesystem path proximity, no package concept.
#[derive(Debug, Clone)]
pub struct SuspicionInfo {
    pub hop: usize,
    pub total_edges: usize,
    pub related_edges: usize, // share parent dir with any hop N-1 file
}

/// Threshold above which low related-ratio is flagged. Small hops are ignored —
/// 5 spurious edges out of 5 is noise, 490 out of 500 is a real collision.
const SUSPICION_MIN_EDGES: usize = 50;
/// Related-ratio cutoff: if fewer than 1 in 5 edges share a dir with the
/// previous hop, flag the hop as suspect.
const SUSPICION_RELATED_NUM: usize = 1;
const SUSPICION_RELATED_DEN: usize = 5;

/// Scan completed BFS edges for hops whose matches are mostly "far" from the
/// previous hop's file paths. Pure function — unit-testable in isolation.
pub fn compute_suspicious_hops(edges: &[BfsEdge]) -> Vec<SuspicionInfo> {
    use std::collections::{BTreeMap, HashSet};
    let mut by_hop: BTreeMap<usize, Vec<&BfsEdge>> = BTreeMap::new();
    for e in edges {
        by_hop.entry(e.hop).or_default().push(e);
    }
    let mut out = Vec::new();
    for (&hop, list) in &by_hop {
        if hop < 2 {
            continue;
        }
        let total = list.len();
        if total < SUSPICION_MIN_EDGES {
            continue;
        }
        let prev = match by_hop.get(&(hop - 1)) {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let prev_dirs: HashSet<&Path> = prev.iter().filter_map(|e| e.from_file.parent()).collect();
        if prev_dirs.is_empty() {
            continue;
        }
        let related = list
            .iter()
            .filter(|e| e.from_file.parent().is_some_and(|p| prev_dirs.contains(p)))
            .count();
        // related / total < NUM / DEN  ⇔  related * DEN < total * NUM
        if related * SUSPICION_RELATED_DEN < total * SUSPICION_RELATED_NUM {
            out.push(SuspicionInfo {
                hop,
                total_edges: total,
                related_edges: related,
            });
        }
    }
    out
}

/// Threshold for auto-hub promotion: if a single symbol produces this many
/// edges in one hop, treat it as a hub and drop from the next frontier.
/// Data-driven, language-agnostic — no hard-coded hub list.
const AUTO_HUB_THRESHOLD: usize = 200;

/// Parse user-provided hub overrides. Returns empty set by default;
/// `--skip-hubs "A,B,C"` adds explicit skips on top of auto-promotion.
fn parse_hubs(skip_hubs: Option<&str>) -> HashSet<String> {
    match skip_hubs {
        None | Some("") => HashSet::new(),
        Some(csv) => csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
    }
}

/// BFS backward over callers up to `max_depth` hops.
/// Reuses `find_callers_batch` per hop — one walk per hop, Aho-Corasick multi-pattern.
///
/// Guards:
/// - `skip_hubs`: explicit user override for symbols to drop from frontier.
///   Auto-hub promotion (>= `AUTO_HUB_THRESHOLD` edges/hop) is always on.
/// - `max_frontier`: cap symbols explored per hop (drop lowest-priority excess).
/// - `max_edges`: hard stop on total edges.
/// - Visited set on `(symbol, path)` avoids revisits and loops.
/// - Unsupported-lang files become leaf nodes (inherited from `find_callers_batch`).
#[allow(clippy::too_many_arguments)]
pub fn search_callers_bfs(
    target: &str,
    scope: &Path,
    cache: &OutlineCache,
    bloom: &crate::index::bloom::BloomFilterCache,
    max_depth: usize,
    max_frontier: usize,
    max_edges: usize,
    glob: Option<&str>,
    skip_hubs: Option<&str>,
    json: bool,
    budget_tokens: Option<usize>,
) -> Result<String, SrcwalkError> {
    let t_start = std::time::Instant::now();
    let user_hubs = parse_hubs(skip_hubs);
    // Auto-promoted hubs: symbols producing >= AUTO_HUB_THRESHOLD edges per hop.
    // Dropped from frontier in subsequent hops. Data-driven, language-agnostic.
    let mut auto_hubs: HashSet<String> = HashSet::new();

    let mut edges: Vec<BfsEdge> = Vec::new();
    let mut stats = BfsStats::default();
    // Visited: (calling_function, path) — a function body in a given file
    // already enumerated as source of edges at some hop.
    let mut visited: HashSet<(String, PathBuf)> = HashSet::new();

    // Frontier at hop k: set of symbol names whose callers we want next.
    let mut frontier: HashSet<String> = HashSet::from([target.to_string()]);

    'outer: for hop in 1..=max_depth {
        if frontier.is_empty() {
            break;
        }

        // Drop hub symbols from frontier BEFORE the cap — they dominate fan-out.
        // Two sources: explicit user override (`--skip-hubs`) and auto-promoted
        // hubs detected in previous hops (>= AUTO_HUB_THRESHOLD edges/hop).
        // Root symbol is always explored even if it's a hub.
        let mut frontier_vec: Vec<String> = frontier
            .iter()
            .filter(|s| {
                if hop == 1 {
                    true
                } else if user_hubs.contains(s.as_str()) {
                    stats.hubs_skipped.push((*s).clone());
                    false
                } else if auto_hubs.contains(s.as_str()) {
                    stats.auto_hubs_skipped.push((*s).clone());
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        frontier_vec.sort();
        if frontier_vec.len() > max_frontier {
            stats.frontier_cut_hops.push((hop, frontier_vec.len()));
            frontier_vec.truncate(max_frontier);
        }
        let frontier_this_hop: HashSet<String> = frontier_vec.into_iter().collect();
        let frontier_size = frontier_this_hop.len();

        if frontier_this_hop.is_empty() {
            break;
        }

        let mut hop_matches =
            find_callers_batch(&frontier_this_hop, scope, bloom, glob, Some(cache), None)?;
        // Parallel walker returns matches in thread-scheduling order. Sort
        // deterministically so `--max-edges` truncation is reproducible across runs.
        // Key matches the final edge sort below: (from_file, from_line, callee, caller).
        hop_matches.sort_by(|(a_to, a), (b_to, b)| {
            a.path
                .cmp(&b.path)
                .then(a.line.cmp(&b.line))
                .then(a_to.cmp(b_to))
                .then(a.calling_function.cmp(&b.calling_function))
        });

        let mut next_frontier: HashSet<String> = HashSet::new();
        let mut hit_targets_this_hop: HashSet<String> = HashSet::new();
        let edges_before_hop = edges.len();
        // Count edges per callee in this hop — used for auto-hub promotion.
        let mut per_callee_count: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for (callee, m) in hop_matches {
            hit_targets_this_hop.insert(callee.clone());
            *per_callee_count.entry(callee.clone()).or_insert(0) += 1;

            let key = (m.calling_function.clone(), m.path.clone());

            if m.calling_function == TOP_LEVEL {
                stats.top_level_terminal += 1;
            } else if visited.insert(key) {
                // calling_function is qualified ("Class.method" / "mod::func").
                // Frontier needs the bare name since find_callers_batch matches
                // by-name call sites, not qualified references.
                let bare = m
                    .calling_function
                    .rsplit_once('.')
                    .or_else(|| m.calling_function.rsplit_once("::"))
                    .map_or(m.calling_function.as_str(), |(_, name)| name);
                next_frontier.insert(bare.to_string());
            }

            // Reduce call_text to a single line — multi-line calls collapse to
            // their first line. Mirrors legacy --callers convention; bounds
            // per-edge token cost without truncating mid-token.
            let call_text = m.call_text.lines().next().unwrap_or("").trim().to_string();

            edges.push(BfsEdge {
                hop,
                from: m.calling_function,
                from_file: m.path,
                from_line: m.line,
                to: callee,
                call_text,
            });

            if edges.len() >= max_edges {
                stats.edges_cut_at_hop = Some(hop);
                stats.depth_reached = hop;
                stats.per_hop.push(HopStats {
                    hop,
                    frontier_size,
                    edges: edges.len() - edges_before_hop,
                });
                break 'outer;
            }
        }

        // Promote high-fan-out symbols to auto-hubs for subsequent hops.
        for (sym, count) in &per_callee_count {
            if *count >= AUTO_HUB_THRESHOLD && auto_hubs.insert(sym.clone()) {
                stats.auto_hubs_promoted.push((sym.clone(), *count));
            }
        }

        // Unresolved = frontier symbols nobody matched in the repo (by-name miss,
        // unsupported-lang file-only, indirect dispatch, etc).
        for s in &frontier_this_hop {
            if !hit_targets_this_hop.contains(s) {
                stats.unresolved_symbols += 1;
            }
        }

        stats.per_hop.push(HopStats {
            hop,
            frontier_size,
            edges: edges.len() - edges_before_hop,
        });
        stats.depth_reached = hop;
        frontier = next_frontier;
    }

    stats.edges_total = edges.len();
    stats.elapsed_ms = t_start.elapsed().as_millis();

    // Deterministic sort across ALL edges (stable for both text + JSON).
    edges.sort_by(|a, b| {
        a.hop
            .cmp(&b.hop)
            .then_with(|| a.from_file.cmp(&b.from_file))
            .then_with(|| a.from_line.cmp(&b.from_line))
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.from.cmp(&b.from))
    });

    if json {
        Ok(format_bfs_json(
            target,
            scope,
            &edges,
            &stats,
            max_depth,
            budget_tokens,
        ))
    } else {
        Ok(format_bfs(target, scope, &edges, &stats, max_depth))
    }
}

/// Deterministic sort + pretty-print BFS edges grouped by hop.
fn format_bfs(
    target: &str,
    scope: &Path,
    edges: &[BfsEdge],
    stats: &BfsStats,
    max_depth: usize,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# BFS callers of \"{}\" in {} — depth={}/{}, {} edge{}, {} ms",
        target,
        scope.display(),
        stats.depth_reached,
        max_depth,
        stats.edges_total,
        if stats.edges_total == 1 { "" } else { "s" },
        stats.elapsed_ms
    );

    if edges.is_empty() {
        let _ = writeln!(
            out,
            "\nNo call sites found. Symbol may be invoked via indirect dispatch \
             (trait objects, interfaces, callbacks, reflection, macros) — srcwalk \
             only sees direct by-name calls."
        );
        return out;
    }

    // Group edges by hop, sort deterministically within each group.
    let mut by_hop: std::collections::BTreeMap<usize, Vec<&BfsEdge>> =
        std::collections::BTreeMap::new();
    for e in edges {
        by_hop.entry(e.hop).or_default().push(e);
    }

    for (hop, mut list) in by_hop {
        list.sort_by(|a, b| {
            a.from_file
                .cmp(&b.from_file)
                .then_with(|| a.from_line.cmp(&b.from_line))
                .then_with(|| a.to.cmp(&b.to))
                .then_with(|| a.from.cmp(&b.from))
        });
        let _ = writeln!(
            out,
            "\n── hop {} ({} edge{}) ──",
            hop,
            list.len(),
            if list.len() == 1 { "" } else { "s" }
        );
        for e in list {
            let rel = e.from_file.strip_prefix(scope).unwrap_or(&e.from_file);
            // Match legacy `--callers` convention: payload is the call-site
            // source text, not the bare callee symbol. Fall back to the
            // symbol only when call_text is unavailable.
            let payload = if e.call_text.is_empty() {
                e.to.as_str()
            } else {
                e.call_text.as_str()
            };
            let _ = writeln!(
                out,
                "  {:<28} {}:{}  → {}",
                e.from,
                rel.display(),
                e.from_line,
                payload
            );
        }
    }

    // Banner — truthfulness about what was cut.
    let mut notes: Vec<String> = Vec::new();
    if let Some(h) = stats.edges_cut_at_hop {
        notes.push(format!("edges capped at hop {h}"));
    }
    for (h, n) in &stats.frontier_cut_hops {
        notes.push(format!("frontier at hop {h} truncated from {n}"));
    }
    if !stats.auto_hubs_promoted.is_empty() {
        let mut parts: Vec<String> = stats
            .auto_hubs_promoted
            .iter()
            .map(|(s, c)| format!("{s}({c})"))
            .collect();
        parts.sort();
        let preview: Vec<String> = parts.iter().take(6).cloned().collect();
        let more = parts.len().saturating_sub(preview.len());
        let suffix = if more > 0 {
            format!(", +{more} more")
        } else {
            String::new()
        };
        notes.push(format!(
            "{} symbol(s) auto-promoted to hub (≥{} edges/hop): {}{}",
            parts.len(),
            AUTO_HUB_THRESHOLD,
            preview.join(", "),
            suffix
        ));
    }
    if !stats.hubs_skipped.is_empty() {
        let mut uniq: Vec<&String> = stats.hubs_skipped.iter().collect();
        uniq.sort();
        uniq.dedup();
        let preview: Vec<String> = uniq.iter().take(6).map(|s| (*s).clone()).collect();
        let more = uniq.len().saturating_sub(preview.len());
        let suffix = if more > 0 {
            format!(", +{more} more")
        } else {
            String::new()
        };
        notes.push(format!(
            "{} hub symbol(s) skipped: {}{}",
            uniq.len(),
            preview.join(","),
            suffix
        ));
    }
    if stats.top_level_terminal > 0 {
        notes.push(format!(
            "{} top-level terminal edge(s)",
            stats.top_level_terminal
        ));
    }
    if stats.unresolved_symbols > 0 {
        notes.push(format!(
            "{} frontier symbol(s) unresolved (unsupported lang / indirect / orphan)",
            stats.unresolved_symbols
        ));
    }
    let suspicious = compute_suspicious_hops(edges);
    for s in &suspicious {
        notes.push(format!(
            "⚠ hop {}: {} edges, only {} share a directory with hop {} — likely cross-package name collision (try qualifying the target, e.g. `pkg::{}`)",
            s.hop,
            s.total_edges,
            s.related_edges,
            s.hop - 1,
            target
        ));
    }
    let has_budget_notes = !notes.is_empty();
    if has_budget_notes {
        let _ = writeln!(out, "\n── budget ──");
        for n in notes {
            let _ = writeln!(out, "  • {n}");
        }
    }
    let _ = writeln!(
        out,
        "\nStatic by-name call graph only. May miss indirect dispatch, reflection, macros, \
         and calls from files > 500KB or from languages without a tree-sitter call query."
    );
    if has_budget_notes {
        let _ = writeln!(
            out,
            "\n> Tip: graph was capped. Retry with --max-edges <N> --max-frontier <N>, narrow with --scope <dir>, or use --skip-hubs \"\" to include hubs."
        );
    }

    let tokens = crate::types::estimate_tokens(out.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(out, "\n({token_str} tokens)");
    out
}

/// Emit the BFS result as JSON (edge-list schema compatible with GraphWalks-style consumers).
fn format_bfs_json(
    target: &str,
    scope: &Path,
    edges: &[BfsEdge],
    stats: &BfsStats,
    max_depth: usize,
    budget_tokens: Option<usize>,
) -> String {
    let all_edges_json: Vec<serde_json::Value> = edges
        .iter()
        .map(|e| {
            let rel = e
                .from_file
                .strip_prefix(scope)
                .unwrap_or(&e.from_file)
                .display()
                .to_string();
            serde_json::json!({
                "hop": e.hop,
                "from": e.from,
                "from_file": rel,
                "from_line": e.from_line,
                "to": e.to,
                "call_text": e.call_text,
            })
        })
        .collect();

    // Budget enforcement: estimate ~30 tokens per edge (JSON field names + values).
    // If budget given, cap the edges array and note how many were elided.
    let tokens_per_edge: usize = 30;
    let overhead_tokens: usize = 200; // stats, metadata, etc.
    let (edges_json, edges_budget_elided) = match budget_tokens {
        Some(b) if b > overhead_tokens => {
            let max_edges_by_budget = (b - overhead_tokens) / tokens_per_edge;
            if all_edges_json.len() > max_edges_by_budget {
                let elided = all_edges_json.len() - max_edges_by_budget;
                (all_edges_json[..max_edges_by_budget].to_vec(), Some(elided))
            } else {
                (all_edges_json, None)
            }
        }
        _ => (all_edges_json, None),
    };

    let per_hop: Vec<serde_json::Value> = stats
        .per_hop
        .iter()
        .map(|h| {
            serde_json::json!({
                "hop": h.hop,
                "frontier_size": h.frontier_size,
                "edges": h.edges,
            })
        })
        .collect();

    let frontier_cuts: Vec<serde_json::Value> = stats
        .frontier_cut_hops
        .iter()
        .map(|(h, n)| serde_json::json!({"hop": h, "frontier_size_before_cut": n}))
        .collect();

    let mut hubs_sorted: Vec<String> = stats.hubs_skipped.clone();
    hubs_sorted.sort();
    hubs_sorted.dedup();

    let auto_hubs_json: Vec<serde_json::Value> = stats
        .auto_hubs_promoted
        .iter()
        .map(|(s, c)| serde_json::json!({"symbol": s, "edges": c}))
        .collect();

    let suspicious_json: Vec<serde_json::Value> = compute_suspicious_hops(edges)
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "hop": s.hop,
                "total_edges": s.total_edges,
                "related_edges": s.related_edges,
                "reason": "name_collision",
            })
        })
        .collect();

    let payload = serde_json::json!({
        "query": format!("callers {} --depth {}", target, max_depth),
        "root": target,
        "scope": scope.display().to_string(),
        "max_depth": max_depth,
        "depth_reached": stats.depth_reached,
        "edges_total": stats.edges_total,
        "edges_returned": edges_json.len(),
        "elapsed_ms": stats.elapsed_ms,
        "edges": edges_json,
        "stats": {
            "per_hop": per_hop,
            "top_level_terminal": stats.top_level_terminal,
            "unresolved_symbols": stats.unresolved_symbols,
            "suspicious_hops": suspicious_json,
        },
        "elided": {
            "edges_cut_at_hop": stats.edges_cut_at_hop,
            "edges_budget_elided": edges_budget_elided,
            "frontier_cuts": frontier_cuts,
            "hubs_skipped": hubs_sorted,
            "auto_hubs_promoted": auto_hubs_json,
            "auto_hub_threshold": AUTO_HUB_THRESHOLD,
        },
        "disclaimer": "Static by-name call graph only. May miss indirect dispatch, reflection, macros, and calls from files > 500KB or from languages without a tree-sitter call query.",
    });

    serde_json::to_string_pretty(&payload).expect("serde_json::Value is always serializable")
}

#[cfg(test)]
mod suspicion_tests {
    use super::*;

    fn edge(hop: usize, file: &str, line: u32, to: &str) -> BfsEdge {
        BfsEdge {
            hop,
            from: "f".into(),
            from_file: PathBuf::from(file),
            from_line: line,
            to: to.into(),
            call_text: String::new(),
        }
    }

    #[test]
    fn ignores_small_hops() {
        // 10 edges, all in distant dirs — below SUSPICION_MIN_EDGES.
        let mut edges = vec![edge(1, "a/root.rs", 1, "x")];
        for i in 0..10 {
            edges.push(edge(2, &format!("z/far{i}.rs"), i, "y"));
        }
        assert!(compute_suspicious_hops(&edges).is_empty());
    }

    #[test]
    fn flags_cross_package_collision() {
        // hop 1 rooted in pkg/a, hop 2 has 60 matches in unrelated dirs.
        let mut edges = vec![edge(1, "pkg/a/root.go", 1, "New")];
        for i in 0..60 {
            edges.push(edge(2, &format!("vendor/errors/err{i}.go"), i, "New"));
        }
        let sus = compute_suspicious_hops(&edges);
        assert_eq!(sus.len(), 1);
        assert_eq!(sus[0].hop, 2);
        assert_eq!(sus[0].total_edges, 60);
        assert_eq!(sus[0].related_edges, 0);
    }

    #[test]
    fn no_flag_when_related_majority() {
        // hop 2 matches all live in same dir as hop 1 callers.
        let mut edges = vec![edge(1, "pkg/a/root.go", 1, "X")];
        for i in 0..60 {
            edges.push(edge(2, &format!("pkg/a/file{i}.go"), i, "Y"));
        }
        assert!(compute_suspicious_hops(&edges).is_empty());
    }

    #[test]
    fn threshold_boundary() {
        // 50 edges, 10 related = 20% = exactly at cutoff (1/5). Should NOT flag.
        let mut edges = vec![edge(1, "pkg/a/root.go", 1, "X")];
        for i in 0..10 {
            edges.push(edge(2, &format!("pkg/a/rel{i}.go"), i, "Y"));
        }
        for i in 0..40 {
            edges.push(edge(2, &format!("far/dir/far{i}.go"), i, "Y"));
        }
        // 10/50 = 0.2, 10*5 = 50 = 50*1 → not strictly less → no flag.
        assert!(compute_suspicious_hops(&edges).is_empty());
        // 9/50 → flag.
        let mut edges = vec![edge(1, "pkg/a/root.go", 1, "X")];
        for i in 0..9 {
            edges.push(edge(2, &format!("pkg/a/rel{i}.go"), i, "Y"));
        }
        for i in 0..41 {
            edges.push(edge(2, &format!("far/dir/far{i}.go"), i, "Y"));
        }
        assert_eq!(compute_suspicious_hops(&edges).len(), 1);
    }

    #[test]
    fn never_flags_hop_1() {
        let mut edges = Vec::new();
        for i in 0..100 {
            edges.push(edge(1, &format!("any/dir{i}/f.go"), i, "X"));
        }
        assert!(compute_suspicious_hops(&edges).is_empty());
    }
}
