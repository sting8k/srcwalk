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
fn map_honors_depth() {
    let dir = temp_repo("map_depth");
    fs::create_dir_all(dir.join("src/nested")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("src/nested/deep.rs"), "pub fn beta() {}\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --depth to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("depth 1"),
        "expected depth in header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("src/"),
        "expected depth-1 dir, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("deep.rs") && !stdout.contains("nested/"),
        "expected deeper entries to be excluded, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_honors_glob() {
    let dir = temp_repo("map_glob");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("src/app.ts"), "export function beta() {}\n").unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--glob")
        .arg("*.rs")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --glob to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lib.rs"),
        "expected rs file, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("app.ts") && !stdout.contains("README.md"),
        "expected glob to exclude non-rs files, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_rejects_filter_and_json_noops() {
    let dir = temp_repo("map_noops");
    fs::write(dir.join("lib.rs"), "pub fn alpha() {}\n").unwrap();

    let filter = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--filter")
        .arg("path:src")
        .output()
        .unwrap();
    assert!(!filter.status.success(), "expected --map --filter to fail");

    let json = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--json")
        .output()
        .unwrap();
    assert!(!json.status.success(), "expected --map --json to fail");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_sorts_dirs_and_files_by_size() {
    let dir = temp_repo("map_sort");
    fs::create_dir_all(dir.join("small_dir")).unwrap();
    fs::create_dir_all(dir.join("large_dir")).unwrap();
    fs::write(dir.join("small_dir/tiny.rs"), "x\n").unwrap();
    fs::write(dir.join("large_dir/big.rs"), "x\n".repeat(200)).unwrap();
    fs::write(dir.join("small_root.rs"), "x\n").unwrap();
    fs::write(dir.join("large_root.rs"), "x\n".repeat(100)).unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected sorted map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let large_dir = stdout.find("large_dir/").expect("large_dir missing");
    let small_dir = stdout.find("small_dir/").expect("small_dir missing");
    let large_file = stdout.find("large_root.rs").expect("large_root missing");
    let small_file = stdout.find("small_root.rs").expect("small_root missing");

    assert!(
        large_dir < small_dir,
        "expected larger dir before smaller dir, got:\n{stdout}"
    );
    assert!(
        small_dir < large_file,
        "expected dirs before root files, got:\n{stdout}"
    );
    assert!(
        large_file < small_file,
        "expected larger file before smaller file, got:\n{stdout}"
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
