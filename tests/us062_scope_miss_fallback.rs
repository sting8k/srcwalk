//! US-062: discover file-target queries (bare filename, file glob, path
//! fragment) widen to the repo root only on a zero in-scope match and emit a
//! labeled outside-scope hint + corrected `> Try:` scope. Text/symbol routes
//! never widen; in-scope matches and no-match-anywhere stay byte-identical.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "us062_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(dir.join("packages/coding-agent/src")).unwrap();
    fs::create_dir_all(dir.join("packages/ai")).unwrap();
    fs::create_dir_all(dir.join("config")).unwrap();
    // A real repo root so the widening targets this temp dir, not the FS root.
    let init = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&dir)
        .output();
    assert!(
        init.is_ok() && init.unwrap().status.success(),
        "git init failed"
    );
    fs::write(dir.join("packages/ai/models.json"), b"{}\n").unwrap();
    fs::write(dir.join("config/models.json"), b"{}\n").unwrap();
    fs::write(
        dir.join("packages/coding-agent/src/keep.js"),
        b"export const keep = 1;\n",
    )
    .unwrap();
    dir
}

const NARROW: &str = "packages/coding-agent/src";

/// Returns the `> Found outside scope` line if present.
fn outside_scope_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find(|l| l.starts_with("> Found outside scope"))
        .map(|l| l.to_string())
}

#[test]
fn bare_filename_outside_scope_emits_hint_and_try_round_trips() {
    let dir = fixture("bare");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "models.json", "--as", "file", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hint = outside_scope_line(&stdout).expect("outside-scope hint missing");
    assert!(stdout.contains("0 of 0 files"), "{stdout}");
    assert!(hint.contains("(2)"), "{hint}");
    assert!(
        hint.contains("config/models.json") && hint.contains("packages/ai/models.json"),
        "{hint}"
    );
    // The Try command names a scope that finds the file.
    assert!(
        stdout.contains("> Try: `srcwalk discover 'models.json' --scope"),
        "{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_glob_outside_scope_emits_hint() {
    let dir = fixture("glob");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "*.json", "--as", "file", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hint = outside_scope_line(&stdout).expect("file-glob outside-scope hint missing");
    assert!(hint.contains("(2)"), "{hint}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_fragment_outside_scope_emits_hint() {
    let dir = fixture("fragment");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "packages/ai", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hint = outside_scope_line(&stdout).expect("path-fragment outside-scope hint missing");
    assert!(hint.contains("packages/ai/models.json"), "{hint}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn in_scope_match_stays_byte_identical_with_no_hint() {
    let dir = fixture("inscope");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "keep.js", "--as", "file", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        outside_scope_line(&stdout).is_none(),
        "should not widen on in-scope match:\n{stdout}"
    );
    assert!(stdout.contains("keep.js"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn no_match_anywhere_stays_unwidened() {
    let dir = fixture("nomatch");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "nope.zzz", "--as", "file", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        outside_scope_line(&stdout).is_none(),
        "no second pass when nothing matches:\n{stdout}"
    );
    assert!(stdout.contains("0 of 0 files"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scope_equal_to_repo_root_does_not_repass() {
    let dir = fixture("rootscope");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "models.json", "--as", "file", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        outside_scope_line(&stdout).is_none(),
        "no repass when scope is the root:\n{stdout}"
    );
    assert!(stdout.contains("2 of 2 files"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn symbol_routes_never_widen() {
    let dir = fixture("symbol");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "keep", "--scope", NARROW])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        outside_scope_line(&stdout).is_none(),
        "text/symbol route must not widen:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
