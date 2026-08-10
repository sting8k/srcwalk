//! US-060: offer-precision collapse (phase A + B). Collapse reduces printed
//! lines but never reachability — every collapsed group stays one command away,
//! and groups ≤3 items render byte-identical (no collapse markers).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_us060_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn callers_fixture(name: &str, n: usize) -> PathBuf {
    let dir = temp_repo(name);
    for i in 0..n {
        fs::write(
            dir.join(format!("caller{i}.js")),
            format!("export function caller{i}() {{ return target(); }}\n"),
        )
        .unwrap();
    }
    fs::write(
        dir.join("target.js"),
        "export function target() { return 42; }\n",
    )
    .unwrap();
    dir
}

#[test]
fn more_than_three_callers_collapse_to_top_three_plus_pointer() {
    let dir = callers_fixture("callers_collapse", 5);
    let out = srcwalk()
        .current_dir(&dir)
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "trace callers failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Top 3 shown in full.
    assert!(
        stdout.contains("caller0.js"),
        "top caller missing:\n{stdout}"
    );
    assert!(stdout.contains("caller1.js"));
    assert!(stdout.contains("caller2.js"));
    // Pointer line collapses the remaining 2.
    assert!(
        stdout.contains("+2 more → srcwalk trace callers target"),
        "expected pointer line, got:\n{stdout}"
    );
    // The pointer command reproduces the remaining callers.
    let pointer = srcwalk()
        .current_dir(&dir)
        .args([
            "trace", "callers", "target", "--scope", ".", "--offset", "3",
        ])
        .output()
        .unwrap();
    let pointer_out = String::from_utf8_lossy(&pointer.stdout);
    assert!(
        pointer_out.contains("caller3.js") && pointer_out.contains("caller4.js"),
        "pointer command should list remaining callers:\n{pointer_out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn groups_of_three_stay_byte_identical_no_collapse_marker() {
    let dir = callers_fixture("callers_three", 3);
    let out = srcwalk()
        .current_dir(&dir)
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("more →"),
        "groups ≤3 must not collapse, got:\n{stdout}"
    );
    assert!(
        stdout.contains("caller0.js")
            && stdout.contains("caller1.js")
            && stdout.contains("caller2.js"),
        "all 3 callers should render in full:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn phase_a_counters_emitted_only_under_debug_env() {
    let dir = callers_fixture("phase_a_stats", 4);
    // Without the env var: no precision noise on stdout.
    let quiet = srcwalk()
        .current_dir(&dir)
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    let quiet_out = String::from_utf8_lossy(&quiet.stdout);
    assert!(
        !quiet_out.contains("[precision]"),
        "no precision counters without the debug env var:\n{quiet_out}"
    );
    // With the env var: counters appear on stderr.
    let verbose = srcwalk()
        .current_dir(&dir)
        .env("SRCWALK_STATS", "1")
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&verbose.stderr);
    assert!(
        stderr.contains("[precision]") && stderr.contains("offers="),
        "debug env should emit phase-A counters, got stderr:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn collapsed_pointer_preserves_filter_and_limit() {
    // US-060 review P1-2: the `+N more →` pointer must carry the caller's
    // --filter/--limit so a rerun reproduces the same result set.
    let dir = callers_fixture("callers_flags", 5);
    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "trace",
            "callers",
            "target",
            "--scope",
            ".",
            "--filter",
            "path:caller",
            "--limit",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "trace callers with filter/limit failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("+2 more → srcwalk trace callers target --scope . --filter path:caller --limit 5 --offset 3"),
        "pointer must preserve --filter and --limit, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn collapsed_pointer_without_constraints_stays_byte_identical() {
    // Unconstrained query: pointer format unchanged from the pre-fix output.
    let dir = callers_fixture("callers_plain", 5);
    let out = srcwalk()
        .current_dir(&dir)
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("+2 more → srcwalk trace callers target --scope . --offset 3"),
        "unconstrained pointer must stay byte-identical, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
