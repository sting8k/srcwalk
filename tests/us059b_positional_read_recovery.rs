//! US-059b: positional-read not-found branch adds a discover recovery hint so a
//! bare-symbol or missing-path QUERY is never a silent dead-end. Exit code and
//! routing are unchanged — only the `not found:` error gains a `> Try:` line.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_us059b_{name}_{}_{}",
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
fn bare_symbol_miss_gets_discover_recovery_hint() {
    let dir = temp_repo("bare_symbol_miss");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/utils.ts"), "export const loadConfig = 1;\n").unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["loadConfig", "--scope", "src"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "exit code must stay 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found: src/loadConfig"),
        "expected not found, got:\n{stderr}"
    );
    assert!(
        stderr.contains("> Try: srcwalk discover 'loadConfig' --scope src"),
        "missing discover recovery hint, got:\n{stderr}"
    );
    assert!(
        stderr.contains("positional QUERY reads exact paths"),
        "missing read-contract note, got:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_miss_gets_discover_recovery_hint() {
    let dir = temp_repo("path_miss");
    fs::create_dir_all(dir.join("src")).unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["src/utils.ts", "--scope", "."])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "exit code must stay 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found: src/utils.ts"),
        "expected not found, got:\n{stderr}"
    );
    assert!(
        stderr.contains("> Try: srcwalk discover 'src/utils.ts' --scope"),
        "missing discover recovery hint, got:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn existing_path_read_is_unchanged() {
    let dir = temp_repo("existing_path_read");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/utils.ts"), "export const loadConfig = 1;\n").unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["src/utils.ts", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "existing path read should succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("loadConfig"),
        "existing path read should render content, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
