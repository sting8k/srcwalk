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
fn path_like_query_searches_content_before_missing_path_error() {
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

    assert!(
        out.status.success(),
        "path-like query with content match should search before missing-path error"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("notes.txt:1") && stdout.contains("internal/missing.go appears in a note"),
        "expected content hit for path-like text, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_like_missing_nested_package_suggests_basename_lookup() {
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
            && stderr.contains("node_modules/pkg/dist/interactive/interactive-mode.js")
            && stderr.contains("Next: check the path or scope"),
        "expected basename locate hint, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_exact_reads_file_without_fallback() {
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
        .arg("--path-exact")
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
        "--path-exact should not fallback to search/glob, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_exact_missing_fails_fast() {
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
        .arg("--path-exact")
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected --path-exact to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found") && stderr.contains("internal/missing.go"),
        "expected not found error, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty(),
        "--path-exact should not emit fallback search output, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
