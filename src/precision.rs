//! Shared offer-precision collapse helpers (US-060).
//!
//! One mechanism every command routes its same-relation list and evidence-range
//! rendering through, so collapse is consistent and every collapsed group stays
//! reachable via exactly one printed command. This never reduces reachability —
//! it only reduces *printed* lines: groups ≤ `K` items render byte-identical.

use std::collections::BTreeMap;
use std::path::Path;

use crate::format::{display_path, shell_quote_arg};

/// Same-relation list collapse threshold: show this many full items, then a
/// `+N more → <command>` pointer.
pub const K: usize = 3;
/// Offered range width threshold (lines): wider ranges render as an anchor
/// (`path:line (symbol)`) plus an `expand: srcwalk show path:start-end`.
pub const W: usize = 40;
/// Maximum offered source lines per packet. Collapse keeps every collapsed
/// group reachable via a printed command, so this bound never hides evidence.
pub const CAP: usize = 400;

/// A pointer command for a same-relation list, using existing path-display rules.
pub fn list_pointer(
    action: &str,
    target: &str,
    scope: &Path,
    filter: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> String {
    let mut cmd = format!("srcwalk {action} {target} --scope {}", display_path(scope));
    // Preserve the caller's query constraints in the pointer command so the
    // rerun reproduces the same result set (filter first, then limit/offset in
    // the CLI's stable flag order).
    if let Some(f) = filter {
        // Paste-safe quoting, matching the pointers callees.rs emits: a
        // multi-qualifier filter carries spaces that need shell quoting.
        let quoted = shell_quote_arg(f).unwrap_or_else(|| "<filter>".to_string());
        let _ = std::fmt::Write::write_fmt(&mut cmd, format_args!(" --filter {quoted}"));
    }
    if let Some(l) = limit {
        let _ = std::fmt::Write::write_fmt(&mut cmd, format_args!(" --limit {l}"));
    }
    if let Some(o) = offset {
        let _ = std::fmt::Write::write_fmt(&mut cmd, format_args!(" --offset {o}"));
    }
    cmd
}

/// True when a same-relation list of `shown` items should be collapsed.
pub fn should_collapse_list(shown: usize) -> bool {
    shown > K
}

/// Anchor for a wide offered range: `path:start-end (label)` plus an
/// `expand:` command, so a >W-line range is never printed in full but stays
/// one command away.
pub fn anchor_range(path: &Path, start: usize, end: usize, label: &str) -> String {
    let rel = display_path(path);
    format!("{rel}:{start}-{end} ({label})\nexpand: srcwalk show {rel}:{start}-{end}")
}

/// True when an offered range of `width` lines should be anchored instead of
/// printed in full.
pub fn should_anchor_range(width: usize) -> bool {
    width > W
}

/// True when a packet's total offered lines exceed the per-packet cap.
pub fn exceeded_line_cap(offered_lines: usize) -> bool {
    offered_lines > CAP
}

/// Phase A: per-packet offer accounting (debug/env-gated; no user-facing noise).
///
/// Counts how many offers a packet makes and how many source lines it offers,
/// broken down by kind. Emitted only when a debug env var is set, so the
/// default output is byte-identical to before.
#[derive(Default)]
pub struct PacketStats {
    pub offers: usize,
    pub offered_lines: usize,
    pub by_kind: BTreeMap<&'static str, usize>,
}

impl PacketStats {
    /// Record one offer of `lines` source lines under `kind`.
    pub fn record(&mut self, kind: &'static str, lines: usize) {
        self.offers += 1;
        self.offered_lines += lines;
        *self.by_kind.entry(kind).or_insert(0) += 1;
    }

    /// Emit the counters to stderr only when the debug env var is set.
    pub fn maybe_emit(&self, packet: &str) {
        if std::env::var("SRCWALK_STATS").is_ok() {
            eprintln!(
                "[precision] {packet}: offers={} offered_lines={} by_kind={:?}",
                self.offers, self.offered_lines, self.by_kind
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_thresholds() {
        assert!(!should_collapse_list(3), "groups of 3 stay byte-identical");
        assert!(should_collapse_list(4));
        assert_eq!(K, 3);
        assert_eq!(W, 40);
        assert_eq!(CAP, 400);
    }

    #[test]
    fn packet_stats_account_by_kind() {
        let mut stats = PacketStats::default();
        stats.record("caller", 5);
        stats.record("caller", 12);
        stats.record("evidence", 30);
        assert_eq!(stats.offers, 3);
        assert_eq!(stats.offered_lines, 47);
        assert_eq!(stats.by_kind.get("caller"), Some(&2));
        assert_eq!(stats.by_kind.get("evidence"), Some(&1));
    }

    #[test]
    fn list_pointer_uses_display_path() {
        let scope = Path::new("src");
        let p = list_pointer("callers", "parseGitUrl", scope, None, None, None);
        assert!(
            p.starts_with("srcwalk callers parseGitUrl --scope src"),
            "{p}"
        );
        let page = list_pointer("callers", "parseGitUrl", scope, None, None, Some(3));
        assert!(page.contains("--offset 3"), "{page}");
        // Constraint preservation keeps the pointer rerun's result set stable.
        let constrained = list_pointer(
            "callers",
            "parseGitUrl",
            scope,
            Some("caller:foo"),
            Some(5),
            Some(3),
        );
        assert!(constrained.contains("--filter caller:foo"), "{constrained}");
        assert!(constrained.contains("--limit 5"), "{constrained}");
        assert!(constrained.contains("--offset 3"), "{constrained}");
        let scope_i = constrained.find("--scope").unwrap();
        let filter_i = constrained.find("--filter").unwrap();
        let limit_i = constrained.find("--limit").unwrap();
        let offset_i = constrained.find("--offset").unwrap();
        assert!(scope_i < filter_i && filter_i < limit_i && limit_i < offset_i);
        // Multi-qualifier filter (spaces) must be shell-quoted, paste-safe.
        let spaced = list_pointer(
            "callers",
            "parseGitUrl",
            scope,
            Some("args:1 caller:foo"),
            None,
            None,
        );
        assert!(
            spaced.contains("--filter 'args:1 caller:foo'"),
            "multi-qualifier filter must be quoted, got: {spaced}"
        );
    }
}
