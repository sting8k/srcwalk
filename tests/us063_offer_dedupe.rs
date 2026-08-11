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

#[test]
fn discover_suppresses_offer_for_fully_rendered_target() {
    let dir = fixture("suppressed");
    // `--expand` renders the definition's code block verbatim.
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
        !stdout.contains("> Next: srcwalk show lib.js:1-6"),
        "fully-rendered target must not be offered again:\n{stdout}"
    );
    assert!(
        stdout.contains("already shown in full above"),
        "expected the already-shown caveat:\n{stdout}"
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
        stdout.contains("> Next: srcwalk show lib.js --section target"),
        "unrendered target should still be offered:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
