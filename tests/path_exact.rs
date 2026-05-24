use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn root_path_like_query_no_longer_falls_back_to_search() {
    let dir = temp_repo("path_note");
    fs::write(
        dir.join("notes.txt"),
        "internal/missing.go appears in a note\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("internal/missing.go")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(!out.status.success(), "root non-path fallback is removed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found") && stderr.contains("internal/missing.go"),
        "expected exact-read not found, got:\n{stderr}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).is_empty(),
        "missing exact path should not emit fallback search output"
    );

    let discover = srcwalk()
        .arg("discover")
        .arg("internal/missing.go")
        .arg("--as")
        .arg("text")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        discover.status.success(),
        "discover text query should search path-like text:\n{}",
        String::from_utf8_lossy(&discover.stderr)
    );
    let stdout = String::from_utf8_lossy(&discover.stdout);
    assert!(
        stdout.contains("notes.txt:1") && stdout.contains("internal/missing.go appears in a note"),
        "expected content hit for path-like text, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_like_missing_nested_package_fails_as_exact_read() {
    let dir = temp_repo("path_note_nested");
    fs::create_dir_all(dir.join("node_modules/pkg/dist/modes/interactive")).unwrap();
    fs::write(
        dir.join("node_modules/pkg/dist/modes/interactive/interactive-mode.js"),
        "export function interactive() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("node_modules/pkg/dist/interactive/interactive-mode.js")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "expected missing nested path to fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found")
            && stderr.contains("node_modules/pkg/dist/interactive/interactive-mode.js"),
        "expected exact-read not found, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn root_path_shortcut_reads_file_without_fallback() {
    let dir = temp_repo("path_exact_read");
    fs::create_dir_all(dir.join("internal")).unwrap();
    fs::write(
        dir.join("internal/target.go"),
        "package main\nfunc Target() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("internal/target.go")
        .arg("--scope")
        .arg(&dir)
        .arg("--full")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected exact path read to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("internal/target.go") && stdout.contains("func Target"),
        "expected exact file contents, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Search:") && !stdout.contains("Glob:"),
        "root path shortcut should not fallback to search/glob, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn root_missing_path_fails_fast() {
    let dir = temp_repo("path_exact_missing");
    fs::write(
        dir.join("notes.txt"),
        "internal/missing.go appears in a note\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("internal/missing.go")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected missing path to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found") && stderr.contains("internal/missing.go"),
        "expected not found error, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty(),
        "missing exact path should not emit fallback search output, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
