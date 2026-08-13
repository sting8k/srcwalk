//! US-063: in-packet offer dedupe — a structural-targets `> Next: srcwalk show
//! path:A-B` offer is suppressed when the packet already rendered that exact
//! range verbatim in a code block. Partial or no overlap keeps the offer.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "us063_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lib.js"),
        "export function target() {\n  const a = 1;\n  const b = 2;\n  const c = 3;\n  return a + b + c;\n}\n",
    )
    .unwrap();
    dir
}

/// A single file with two same-name top-level functions, so the bare selector is
/// ambiguous for both bodies and neither is symbol-backed (numeric fallback).
fn ambiguous_fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "us063_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lib.rs"),
        "fn helper() -> i32 { 1 }\nfn other() -> i32 { 2 }\nfn helper() -> i32 { 3 }\n",
    )
    .unwrap();
    dir
}

#[test]
fn discover_suppresses_offer_for_fully_rendered_target() {
    let dir = ambiguous_fixture("suppressed");
    // Two same-name top-level functions make the selector ambiguous, so the
    // target is a NUMERIC fallback (never canonical). `--expand` renders the
    // bodies verbatim, so the redundant numeric read stays suppressed.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "helper", "--expand=6"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("> Next: srcwalk show lib.rs:1-1"),
        "fully-rendered numeric target must not be offered again:\n{stdout}"
    );
    assert!(
        !stdout.contains("> Next: srcwalk show lib.rs:3-3"),
        "fully-rendered numeric target must not be offered again:\n{stdout}"
    );
    assert!(
        stdout.contains("already shown in full above"),
        "expected the already-shown caveat:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_keeps_canonical_offer_for_fully_rendered_target() {
    let dir = fixture("canonical");
    // A unique top-level function is canonical. Even when `--expand` renders
    // its body inline, the same <path>:<symbol> stays reusable for context,
    // callers, and callees, so the canonical action is kept.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--expand=6"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("> Next: srcwalk show lib.js:target"),
        "fully-rendered canonical target must stay reusable:\n{stdout}"
    );
    assert!(
        !stdout.contains("> Next: srcwalk show lib.js:1-6"),
        "the numeric read must not replace the canonical target:\n{stdout}"
    );
    assert!(
        !stdout.contains("already shown in full above"),
        "a kept canonical target must not claim it was suppressed:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_keeps_offer_when_source_not_rendered() {
    let dir = fixture("kept");
    // No expand: the definition source is not rendered, so the offer stays.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("> Next: srcwalk show lib.js:target"),
        "unrendered target should still be offered:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
