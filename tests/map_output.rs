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
fn map_default_is_compact_without_symbols() {
    let dir = temp_repo("map_compact");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("sizes ~= tokens"),
        "expected units in header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("lib.rs  ~") && stdout.contains("src/  ~"),
        "expected compact file/dir sizes, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("lib.rs: alpha") && !stdout.contains("~9 tokens"),
        "default map should not include symbols or repeated token units, got:\n{stdout}"
    );
    assert!(
        stdout.contains("add --symbols") && stdout.contains("--scope <dir>"),
        "expected compact map tip, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_budget_truncation_keeps_footer_tip() {
    let dir = temp_repo("map_budget_tip");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..40 {
        fs::write(
            dir.join("src").join(format!("file_{i}.rs")),
            "pub fn alpha() {}\n",
        )
        .unwrap();
    }

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--budget")
        .arg("80")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("truncated") && stdout.contains("add --symbols"),
        "expected truncated map to keep footer tip, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_symbols_includes_symbol_names() {
    let dir = temp_repo("map_symbols");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--symbols")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --symbols to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lib.rs: alpha, beta"),
        "expected symbol names with --symbols, got:\n{stdout}"
    );
    assert!(
        stdout.contains("narrow with --scope <dir>"),
        "expected symbols map tip, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
