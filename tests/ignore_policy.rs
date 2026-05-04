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
fn root_search_skips_gitignored_subtree_but_explicit_scope_can_search_it() {
    let dir = temp_repo("ignore_explicit_scope");
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::create_dir_all(dir.join("ignored_dir")).unwrap();
    fs::write(dir.join(".gitignore"), "ignored_dir/\n").unwrap();
    fs::write(dir.join("visible.rs"), "fn visible_symbol() {}\n").unwrap();
    fs::write(dir.join("ignored_dir/hidden.rs"), "fn hidden_symbol() {}\n").unwrap();

    let root_out = srcwalk()
        .arg("hidden_symbol")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        root_out.status.success(),
        "root search command should complete"
    );
    let root_stdout = String::from_utf8_lossy(&root_out.stdout);
    assert!(
        root_stdout.contains("0 matches") && !root_stdout.contains("hidden.rs"),
        "expected root search to skip ignored subtree, got:\n{root_stdout}"
    );

    let explicit_out = srcwalk()
        .arg("hidden_symbol")
        .arg("--scope")
        .arg(dir.join("ignored_dir"))
        .output()
        .unwrap();
    assert!(
        explicit_out.status.success(),
        "explicit ignored subtree scope should be searchable"
    );
    let explicit_stdout = String::from_utf8_lossy(&explicit_out.stdout);
    assert!(
        explicit_stdout.contains("hidden.rs") && explicit_stdout.contains("hidden_symbol"),
        "expected explicit subtree hit, got:\n{explicit_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn explicit_path_read_can_open_gitignored_file() {
    let dir = temp_repo("ignore_explicit_path");
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::create_dir_all(dir.join("ignored_dir")).unwrap();
    fs::write(dir.join(".gitignore"), "ignored_dir/\n").unwrap();
    fs::write(dir.join("ignored_dir/hidden.rs"), "fn hidden_symbol() {}\n").unwrap();

    let out = srcwalk()
        .arg(dir.join("ignored_dir/hidden.rs"))
        .arg("--full")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "explicit ignored file read should succeed"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hidden.rs") && stdout.contains("fn hidden_symbol"),
        "expected explicit ignored file contents, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
