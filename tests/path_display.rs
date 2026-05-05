use std::fs;
use std::path::{Path, PathBuf};
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

fn write_fixture(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn target_fn() {\n    helper();\n}\n\nfn helper() {}\n",
    )
    .unwrap();
}

#[test]
fn read_header_prefers_pwd_relative_path() {
    let dir = temp_repo("path_display_read");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .arg("src/lib.rs")
        .arg("--section")
        .arg("1")
        .arg("--full")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected read to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# src/lib.rs "),
        "expected pwd-relative file header, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_header_and_hits_are_pwd_relative_and_drillable() {
    let dir = temp_repo("path_display_search");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "target_fn", "--scope", "src"])
        .output()
        .unwrap();

    assert!(out.status.success(), "expected search to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# Search: \"target_fn\" in src —"),
        "expected pwd-relative search header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("src/lib.rs:1-3"),
        "expected pwd-relative copy-pasteable hit path, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let drill = srcwalk()
        .current_dir(&dir)
        .arg("src/lib.rs:1")
        .output()
        .unwrap();
    assert!(
        drill.status.success(),
        "displayed hit path should be drillable; stderr:\n{}",
        String::from_utf8_lossy(&drill.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_header_prefers_pwd_relative_scope() {
    let dir = temp_repo("path_display_map");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["map", "--scope", "src", "--depth", "1"])
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# Map: src (depth 1"),
        "expected pwd-relative map header, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
