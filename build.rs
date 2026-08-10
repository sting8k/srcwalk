//! Build-time provenance metadata (US-061): embed the git short-SHA + dirty
//! flag + UTC build date into the binary via `cargo:rustc-env`.
//!
//! Cross-platform and fail-soft: when `.git` or the `git` binary is absent
//! (tarball/npm build), the label falls back to `unknown` and the build never
//! fails. `rerun-if-changed` on `.git/HEAD`/refs keeps incremental rebuilds
//! from re-invoking git on every source compile.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Avoid rebuild churn: only re-run git when the checked-out commit or its
    // refs move. These paths are no-ops when `.git` is absent.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let sha = run_git(&["rev-parse", "--short", "HEAD"]);
    // `git status` is the dirty check; it fails cleanly when git is absent
    // (label then stays a bare SHA). Works for normal repos and worktrees.
    let dirty = run_git(&["status", "--porcelain"]).map(|s| !s.trim().is_empty());

    let label = match (sha, dirty) {
        (Some(sha), Some(true)) => format!("{sha}-dirty"),
        (Some(sha), _) => sha,
        _ => String::from("unknown"),
    };

    println!("cargo:rustc-env=SRCWALK_GIT_LABEL={label}");
    println!("cargo:rustc-env=SRCWALK_BUILD_DATE={}", build_date_utc());
}

fn run_git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_string())
}

/// Current UTC calendar date as `YYYY-MM-DD`, computed from the system clock
/// (no chrono dependency). Falls back to the epoch date on clock error.
fn build_date_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's civil_from_days: days since 1970-01-01 -> (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}
