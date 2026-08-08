use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "srcwalk-map-det-{name}-{}-{unique}",
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
