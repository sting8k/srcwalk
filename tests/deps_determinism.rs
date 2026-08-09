//! Deterministic reverse-dependency evidence (US-052 Phase 2B).
//!
//! `deps` reverse search must be byte-identical across repeated runs and
//! across worker settings, independent of parallel walker completion order.
//!
//! Regression: `analyze_deps` passed `Some(50)` to `find_callers_batch`,
//! whose parallel walker early-quits via a relaxed-atomic callsite count. When
//! the reverse search exceeded the cap, the collected callsite subset was
//! scheduling-dependent, so the grouped dependent count varied across runs.
//! Reproduced on the srcwalk repo (`src/search/rank.rs` gave 19,16,16,13,19,18
//! dependents) and on a synthetic 140-caller fixture (41..49 with 8 workers;
//! 42/39/49 for 1/2/8 workers).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// Unique temp repo for parallel test isolation.
fn fixture(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "srcwalk-deps-det-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// Over-cap fixture: a hot exported symbol with far more than the
/// reverse-search callsite cap, spread across JS/TS/Python/Ruby caller files
/// so different completion orders change the grouped dependent count.
fn over_cap_fixture() -> PathBuf {
    let root = fixture("overcap");
    write(
        &root,
        "hot.ts",
        "export function computeTotal() { return 1; }\n",
    );
    for i in 1..=80 {
        let calls = "computeTotal();\n".repeat(if i % 14 == 0 { 5 } else { 1 });
        write(
            &root,
            &format!("callers/ts/c{i:04}.ts"),
            &format!(
                "import {{ computeTotal }} from '../../hot';\n\nfunction c{i:04}() {{\n{calls}}}\n"
            ),
        );
    }
    for i in 1..=30 {
        write(
            &root,
            &format!("callers/js/c{i:04}.js"),
            &format!(
                "import {{ computeTotal }} from '../../hot.js';\n\nfunction c{i:04}() {{\n  computeTotal();\n}}\n"
            ),
        );
    }
    for i in 1..=20 {
        write(
            &root,
            &format!("callers/py/c{i:04}.py"),
            &format!("def caller_{i:04}():\n    computeTotal()\n"),
        );
    }
    for i in 1..=10 {
        write(
            &root,
            &format!("callers/rb/c{i:04}.rb"),
            &format!("def caller_{i:04}\n  computeTotal()\nend\n"),
        );
    }
    root
}

fn deps_stdout(root: &Path, threads: &str) -> String {
    let out = srcwalk()
        .env("SRCWALK_THREADS", threads)
        .arg("deps")
        .arg(root.join("hot.ts"))
        .args(["--scope"])
        .arg(root)
        .output()
        .expect("run deps");
    assert!(
        out.status.success(),
        "deps failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).replace('\\', "/")
}

fn overview_stdout(scope: &Path, threads: &str) -> String {
    let out = srcwalk()
        .env("SRCWALK_THREADS", threads)
        .arg("overview")
        .arg("--scope")
        .arg(scope)
        .arg("--depth")
        .arg("2")
        .output()
        .expect("run overview");
    assert!(
        out.status.success(),
        "overview failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).replace('\\', "/")
}

/// The over-cap reverse search must be byte-identical across >=20 repeated
/// runs and across worker settings 1, 2, and 8, with deterministic path/line
/// ordering and a complete dependent count.
#[test]
fn deps_reverse_search_deterministic_across_runs_and_workers() {
    let root = over_cap_fixture();
    let mut outputs = Vec::new();
    // 20 repeated runs on the parallel path (the racy setting).
    for _ in 0..20 {
        outputs.push(deps_stdout(&root, "8"));
    }
    // Cross-check the supported worker settings.
    for threads in ["1", "2", "8"] {
        outputs.push(deps_stdout(&root, threads));
    }
    let first = outputs[0].clone();
    for (i, out) in outputs.iter().enumerate() {
        assert_eq!(*out, first, "output {i} must be byte-identical to run 0");
    }
    // Complete, deterministic count: all 140 caller files are dependents.
    assert!(
        first.contains("140 dependents"),
        "expected complete dependent count, got:\n{first}"
    );
    // Deterministic path ordering among the visible (first 15) rows. Rows
    // render as basenames under a directory-group header (`callers/js/`).
    let pos = |s: &str| {
        first
            .find(s)
            .unwrap_or_else(|| panic!("{s} missing:\n{first}"))
    };
    assert!(pos("  c0001.js:") < pos("  c0002.js:"), "{first}");
    assert!(pos("  c0002.js:") < pos("  c0003.js:"), "{first}");
}

/// Below-cap reverse search stays byte-identical across runs and worker
/// settings, with representative JS/TS + Python + Ruby reverse call evidence
/// in one scope, preserving deterministic path ordering.
#[test]
fn deps_reverse_search_below_cap_stable_and_preserved() {
    let root = fixture("belowcap");
    write(
        &root,
        "hot.ts",
        "export function computeTotal() { return 1; }\n",
    );
    write(
        &root,
        "c1.js",
        "import { computeTotal } from './hot.js';\nfunction a() { computeTotal(); }\n",
    );
    write(
        &root,
        "c2.ts",
        "import { computeTotal } from './hot';\nfunction b() { computeTotal(); }\n",
    );
    write(&root, "c3.py", "def caller():\n    computeTotal()\n");
    write(&root, "c4.rb", "def caller\n  computeTotal()\nend\n");

    let first = deps_stdout(&root, "8");
    for _ in 0..5 {
        assert_eq!(deps_stdout(&root, "8"), first);
        assert_eq!(deps_stdout(&root, "1"), first);
        assert_eq!(deps_stdout(&root, "2"), first);
    }
    assert!(first.contains("4 dependents"), "{first}");
    let pos = |s: &str| {
        first
            .find(s)
            .unwrap_or_else(|| panic!("{s} missing:\n{first}"))
    };
    assert!(pos("c1.js") < pos("c2.ts"), "{first}");
    assert!(pos("c2.ts") < pos("c3.py"), "{first}");
    assert!(pos("c3.py") < pos("c4.rb"), "{first}");
}

fn overview_relations_fixture() -> PathBuf {
    let root = fixture("overview-relations");
    for i in 0..64 {
        write(
            &root,
            &format!("packages/app{i:03}/src/main.ts"),
            &format!(
                "import {{ value }} from \"../../shared/value{i:03}\";\nexport const result = value;\n"
            ),
        );
        write(
            &root,
            &format!("packages/shared/value{i:03}.ts"),
            "export const value = 1;\n",
        );
    }
    root
}

fn overview_outbound_fixture() -> PathBuf {
    let root = fixture("overview-outbound");
    write(
        &root,
        "packages/app/main.ts",
        "import { value } from \"../shared/value\";\nexport const result = value;\n",
    );
    write(
        &root,
        "packages/shared/value.ts",
        "export const value = 1;\n",
    );
    root
}

fn assert_overview_deterministic(scope: &Path, expected_marker: &str) {
    let mut outputs = Vec::new();
    for threads in ["1", "2", "8"] {
        for _ in 0..20 {
            outputs.push(overview_stdout(scope, threads));
        }
    }
    let first = outputs.first().expect("at least one overview output");
    for (index, output) in outputs.iter().enumerate() {
        assert_eq!(
            output, first,
            "overview output {index} must be byte-identical to run 0"
        );
    }
    assert!(
        first.contains(expected_marker),
        "expected {expected_marker:?}, got:\n{first}"
    );
}

#[test]
fn overview_js_ts_relations_deterministic_across_runs_and_workers() {
    let root = overview_relations_fixture();
    assert_overview_deterministic(&root.join("packages"), "[relations]");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn overview_js_ts_outbound_deterministic_across_runs_and_workers() {
    let root = overview_outbound_fixture();
    assert_overview_deterministic(
        &root.join("packages/app"),
        "[outbound deps] 1 group (targets outside scope)",
    );
    let _ = fs::remove_dir_all(root);
}
