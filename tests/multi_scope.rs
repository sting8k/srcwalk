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

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn norm_path_separators(s: &str) -> String {
    s.replace('\\', "/")
}

#[test]
fn find_accepts_repeated_scopes_and_outputs_pwd_relative_hits() {
    let dir = temp_repo("multi_scope_find");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");
    write_file(&dir.join("tests/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "shared_target",
            "--scope",
            "src",
            "--scope",
            "tests",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected multi-scope find to succeed, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Search: \"shared_target\" in 2 scopes"),
        "expected multi-scope header, got:\n{stdout}"
    );
    assert!(
        normalized.contains("Scopes: src (1), tests (1)")
            && normalized.contains("src/lib.rs:1-1")
            && normalized.contains("tests/lib.rs:1-1"),
        "expected pwd-relative hits and per-scope counts, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn paginated_multi_scope_find_labels_scope_counts_as_page_counts() {
    let dir = temp_repo("multi_scope_find_page_counts");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");
    write_file(&dir.join("tests/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "shared_target",
            "--scope",
            "src",
            "--scope",
            "tests",
            "--limit",
            "1",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected paginated multi-scope find to succeed, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Scopes on this page:"),
        "paginated scope counts should be labeled as page counts, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("\nScopes: src"),
        "paginated output should not label page counts as total scope counts, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_accepts_repeated_scopes_with_multi_symbol_query() {
    let dir = temp_repo("multi_scope_multi_symbol_find");
    write_file(
        &dir.join("classes/cart.php"),
        "<?php function alpha_symbol() {} customized_data();\n",
    );
    write_file(
        &dir.join("controllers/upload.php"),
        "<?php function beta_symbol() {} customized_data();\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "alpha_symbol, beta_symbol, customized_data",
            "--scope",
            "classes",
            "--scope",
            "controllers",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected multi-scope multi-symbol find to succeed, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.contains("# Search: \"alpha_symbol\" in 2 scopes")
            && stdout.contains("# Search: \"beta_symbol\" in 2 scopes")
            && stdout.contains("# Search: \"customized_data\" in 2 scopes"),
        "expected one section per query, got:\n{stdout}"
    );
    assert!(
        normalized.contains("classes/cart.php") && normalized.contains("controllers/upload.php"),
        "expected hits from both scopes, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_repeated_scopes_fail_fast_when_any_scope_is_invalid() {
    let dir = temp_repo("multi_scope_invalid");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "shared_target",
            "--scope",
            "src",
            "--scope",
            "missing",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "invalid scope should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid scope") && stderr.contains("missing"),
        "expected invalid scope error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_repeated_scopes_rejects_expand_with_minimal_hint() {
    let dir = temp_repo("multi_scope_expand");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");
    write_file(&dir.join("tests/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "shared_target",
            "--scope",
            "src",
            "--scope",
            "tests",
            "--expand",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "multi-scope expand should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not support --expand yet") && stderr.contains("try one --scope"),
        "expected short expand hint, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn repeated_scopes_are_rejected_for_non_find_commands() {
    let dir = temp_repo("multi_scope_reject_non_find");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");
    write_file(&dir.join("tests/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "callers",
            "shared_target",
            "--scope",
            "src",
            "--scope",
            "tests",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "non-find multi-scope should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("supported only by `srcwalk find`"),
        "expected unsupported multi-scope error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn overlapping_scopes_are_deduped_before_pagination() {
    let dir = temp_repo("multi_scope_overlap");
    write_file(&dir.join("src/lib.rs"), "pub fn shared_target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "shared_target",
            "--scope",
            ".",
            "--scope",
            "src",
            "--limit",
            "10",
        ])
        .output()
        .unwrap();

    assert!(out.status.success(), "expected overlap search to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert_eq!(
        normalized.matches("src/lib.rs:1-1").count(),
        1,
        "overlapping scopes should dedupe duplicate hits, got:\n{stdout}"
    );
    assert!(
        normalized.contains("overlapping scopes were deduplicated"),
        "expected overlap note, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
