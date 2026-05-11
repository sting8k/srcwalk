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
fn files_rejects_huge_srcwalk_threads() {
    let dir = temp_repo("files_threads_guard");
    write_file(&dir.join("src/lib.rs"), "pub fn alpha() {}\n");

    let out = srcwalk()
        .env("SRCWALK_THREADS", "50000")
        .args(["files", "*.rs", "--scope", "src"])
        .current_dir(&dir)
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected huge thread count to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("SRCWALK_THREADS") && stderr.contains("1..=24"),
        "expected clear thread range error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn files_action_lists_file_globs() {
    let dir = temp_repo("files_action");
    write_file(
        &dir.join("controllers/front/CartController.php"),
        "<?php\nclass CartController {}\n",
    );
    write_file(
        &dir.join("controllers/front/ProductController.php"),
        "<?php\nclass ProductController {}\n",
    );
    write_file(
        &dir.join("controllers/admin/AdminController.php"),
        "<?php\nclass AdminController {}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["files", "*.php", "--scope", "controllers/front"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Files: \"*.php\" in controllers/front"),
        "bad header:\n{stdout}"
    );
    assert!(
        normalized.contains("controllers/front/ (2)"),
        "missing grouped directory:\n{stdout}"
    );
    assert!(
        stdout.contains("  CartController.php"),
        "missing cart:\n{stdout}"
    );
    assert!(
        stdout.contains("  ProductController.php"),
        "missing product:\n{stdout}"
    );
    assert!(
        !normalized.contains("controllers/admin/AdminController.php"),
        "scope leaked:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_file_globs_tell_user_to_use_files() {
    let dir = temp_repo("find_file_glob_error");
    write_file(&dir.join("src/lib.rs"), "fn alpha() {}\n");

    for args in [["find", "*.rs"].as_slice(), ["*.rs"].as_slice()] {
        let out = srcwalk()
            .current_dir(&dir)
            .args(args)
            .arg("--scope")
            .arg("src")
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "file glob through find/legacy should fail"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unsupported syntax for `srcwalk find`")
                && stderr.contains("srcwalk files '<glob>' --scope <dir>"),
            "expected supported files syntax, got:\n{stderr}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_unsupported_syntax_lists_supported_forms() {
    let dir = temp_repo("find_unsupported_syntax_error");
    write_file(
        &dir.join("src/lib.rs"),
        "pub struct BaseInfo;\npub struct DomainInfo;\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "BaseInfo\\|DomainInfo", "--scope", "src"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected invalid query to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported syntax for `srcwalk find`"),
        "expected generic syntax diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk find \"BaseInfo, DomainInfo\" --scope <dir>"),
        "expected supported syntax, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk find <query> --scope <dir>"),
        "expected single-query syntax, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk files '<glob>' --scope <dir>"),
        "expected filename syntax, got:\n{stderr}"
    );
    assert!(
        stderr.contains("rg '<regex>' <dir>"),
        "expected rg fallback, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("looks like a file path"),
        "expected no path-like error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_symbol_name_glob_supports_repeated_scopes() {
    let dir = temp_repo("symbol_name_glob_multi_scope");
    write_file(&dir.join("src/lib.rs"), "pub fn display_ajax_src() {}\n");
    write_file(&dir.join("tests/lib.rs"), "pub fn display_ajax_test() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "display_ajax_*",
            "--scope",
            "src",
            "--scope",
            "tests",
            "--filter",
            "kind:fn",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Search: \"display_ajax_*\" in 2 scopes"),
        "bad header:\n{stdout}"
    );
    assert!(
        normalized.contains("src/lib.rs:1-1"),
        "missing src match:\n{stdout}"
    );
    assert!(
        normalized.contains("tests/lib.rs:1-1"),
        "missing tests match:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_symbol_name_glob_expands_definition_source() {
    let dir = temp_repo("symbol_name_glob_expand");
    write_file(
        &dir.join("src/lib.rs"),
        "pub fn display_ajax_update() {\n    let value = 1;\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "display_ajax_*",
            "--scope",
            "src",
            "--filter",
            "kind:fn",
            "--expand",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("display_ajax_update"),
        "missing match:\n{stdout}"
    );
    assert!(
        stdout.contains("let value = 1"),
        "missing expanded body:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_symbol_name_glob_matches_definitions_only() {
    let dir = temp_repo("symbol_name_glob");
    write_file(
        &dir.join("controllers/front/CartController.php"),
        "<?php\nclass CartController {\n  public function displayAjaxUpdate() {}\n  public function displayAjaxRefresh() {}\n  public function other() { displayAjaxUpdate(); }\n}\n",
    );
    write_file(
        &dir.join("controllers/front/ProductController.php"),
        "<?php\nclass ProductController {\n  public function displayAjaxProductRefresh() {}\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "displayAjax*",
            "--scope",
            "controllers/front",
            "--filter",
            "kind:fn",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Search names: \"displayAjax*\" in controllers/front"),
        "bad header:\n{stdout}"
    );
    assert!(
        stdout.contains("displayAjaxUpdate"),
        "missing update:\n{stdout}"
    );
    assert!(
        stdout.contains("displayAjaxRefresh"),
        "missing refresh:\n{stdout}"
    );
    assert!(
        stdout.contains("displayAjaxProductRefresh"),
        "missing product refresh:\n{stdout}"
    );
    assert!(
        !stdout.contains("other"),
        "should not include non-matching definition:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_kind_zero_match_hints_when_definition_may_be_outside_scope() {
    let dir = temp_repo("find_kind_scope_hint");
    write_file(
        &dir.join("sdk/translator/registry.go"),
        "package translator\n\nfunc TranslateRequest() {}\n",
    );
    write_file(
        &dir.join("internal/executor/caller.go"),
        r#"package executor

import sdktranslator "example.com/sdk/translator"

func translateRequest() {
    sdktranslator.TranslateRequest()
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "find",
            "TranslateRequest",
            "--scope",
            "internal/executor",
            "--filter",
            "kind:fn",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 matches"), "{stdout}");
    assert!(
        stdout.contains("Did you mean: translateRequest"),
        "expected case suggestion, got:\n{stdout}"
    );
    assert!(
        stdout.contains("kind filters only match symbols inside --scope"),
        "expected scope hint, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
