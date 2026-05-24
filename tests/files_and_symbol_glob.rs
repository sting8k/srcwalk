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
    s.replace("\r\n", "\n").replace('\\', "/")
}

#[test]
fn discover_outputs_per_hit_provenance_for_structural_usage_and_text_hits() {
    let dir = temp_repo("hit_provenance");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn alpha(input: &str) -> String {
    let value = input.trim();
    value.to_string()
}

pub fn beta() {
    let _ = alpha("x");
}
"#,
    );
    write_file(&dir.join("src/readme.txt"), "alpha text only\n");

    let symbol_out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "alpha", "--scope", "src", "--limit", "5"])
        .output()
        .unwrap();
    assert!(
        symbol_out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&symbol_out.stderr)
    );
    let symbol_stdout = norm_path_separators(&String::from_utf8_lossy(&symbol_out.stdout));
    assert!(
        symbol_stdout.contains("[fn] alpha src/lib.rs:1-4\n  source: ast · kind: definition · confidence: structural syntax"),
        "definition provenance should identify AST-backed structural evidence:\n{symbol_stdout}"
    );
    assert!(
        symbol_stdout.contains(
            "## src/lib.rs:7 [usage]\nsource: text · kind: usage · confidence: text evidence"
        ),
        "usage provenance should identify text-backed usage evidence:\n{symbol_stdout}"
    );
    assert!(
        symbol_stdout.contains(
            "## src/readme.txt:1 [text]\nsource: text · kind: text · confidence: text evidence"
        ),
        "text-file provenance should not overclaim as usage evidence:\n{symbol_stdout}"
    );

    let text_out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "alpha", "--as", "text", "--scope", "src", "--limit", "5",
        ])
        .output()
        .unwrap();
    assert!(
        text_out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&text_out.stderr)
    );
    let text_stdout = norm_path_separators(&String::from_utf8_lossy(&text_out.stdout));
    assert!(
        text_stdout.contains(
            "## src/lib.rs [2 usages]\nsource: text · kind: usage · confidence: text evidence"
        ),
        "grouped usage provenance should stay visible at the group header:\n{text_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn exact_symbol_miss_suggests_wildcard_file_and_text_routes() {
    let dir = temp_repo("symbol_prefix_guidance");
    write_file(
        &dir.join("src/http/modules/ngx_http_secure_link_module.c"),
        "void ngx_http_secure_link_variable(void) {}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "ngx_http_secure_link",
            "--scope",
            "src/http/modules",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "exact zero-result symbol search should remain a successful search"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 matches"),
        "expected zero-result output:\n{stdout}"
    );
    assert!(
        stdout.contains("No exact symbol named `ngx_http_secure_link`"),
        "expected exact-symbol explanation:\n{stdout}"
    );
    assert!(
        stdout.contains("ngx_http_secure_link*")
            && stdout.contains("--as file")
            && stdout.contains("--as text"),
        "expected wildcard/file/text guidance:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn filter_zero_result_error_names_the_filter() {
    let dir = temp_repo("filter_zero_guidance");
    write_file(
        &dir.join("src/http/module.c"),
        "static const char *alias = \"/tmp\";\nvoid handler(void) { alias; }\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "alias", "--filter", "kind:fn", "--scope", "src/http",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "filter should remove all matches");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no matches after --filter kind:fn"),
        "expected filter-specific no-match guidance:\n{stderr}"
    );
    assert!(
        stderr.contains("kind filters match result row kinds")
            && stderr.contains("--as symbol")
            && stderr.contains("--as text"),
        "expected kind/action hints:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn files_rejects_huge_srcwalk_threads() {
    let dir = temp_repo("files_threads_guard");
    write_file(&dir.join("src/lib.rs"), "pub fn alpha() {}\n");

    let out = srcwalk()
        .env("SRCWALK_THREADS", "50000")
        .args(["discover", "*.rs", "--as", "file", "--scope", "src"])
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
        .args([
            "discover",
            "*.php",
            "--as",
            "file",
            "--scope",
            "controllers/front",
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
fn files_action_accepts_glob_scope() {
    let dir = temp_repo("files_glob_scope");
    write_file(&dir.join("src/http/a.c"), "void a() {}\n");
    write_file(&dir.join("src/http/b.h"), "void b();\n");
    write_file(&dir.join("src/http/nested/c.c"), "void c() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "*.c", "--as", "file", "--scope", "src/http/*.c"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(normalized.contains("src/http/ (1)"), "{stdout}");
    assert!(normalized.contains("  a.c"), "{stdout}");
    assert!(
        !normalized.contains("src/http/b.h"),
        "glob scope should filter non-matching extensions:\n{stdout}"
    );
    assert!(
        !normalized.contains("nested/c.c"),
        "src/http/*.c should not include nested files:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_accepts_exact_file_range_scope() {
    let dir = temp_repo("text_file_range_scope");
    write_file(
        &dir.join("src/lib.rs"),
        "pub fn before() {\n    let target = 1;\n}\npub fn inside() {\n    let target = 2;\n}\npub fn after() {\n    let target = 3;\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "target",
            "--as",
            "text",
            "--scope",
            "src/lib.rs:4-6",
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
        normalized.starts_with("# Search: \"target\" in src/lib.rs — 1 matches"),
        "range scope should narrow text hits before counts:\n{stdout}"
    );
    assert!(stdout.contains(":5"), "{stdout}");
    assert!(stdout.contains("let target = 2;"), "{stdout}");
    assert!(!stdout.contains("let target = 1;"), "{stdout}");
    assert!(!stdout.contains("let target = 3;"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_range_scope_rejects_invalid_line_range_before_path_lookup() {
    let dir = temp_repo("invalid_range_scope");
    write_file(&dir.join("src/lib.rs"), "pub fn target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "target",
            "--as",
            "text",
            "--scope",
            "src/lib.rs:4-2",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "invalid range should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid scope line range: 4-2"),
        "expected clear range error, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("No such file or directory"),
        "range parser should not report the whole file:range as a missing path:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_range_scope_rejects_explicit_line_filter() {
    let dir = temp_repo("range_scope_line_filter");
    write_file(
        &dir.join("src/lib.rs"),
        "pub fn target() {\n    target();\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "target",
            "--scope",
            "src/lib.rs:1-2",
            "--filter",
            "line:1-1",
        ])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "ambiguous double line range should fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be combined with --filter line"),
        "expected clear line-filter conflict, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_glob_scope_rejects_extra_glob_filter() {
    let dir = temp_repo("scope_glob_plus_glob");
    write_file(&dir.join("src/a.rs"), "pub fn target() {}\n");
    write_file(&dir.join("src/a.ts"), "function target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "target", "--scope", "src/*.rs", "--glob", "*.ts",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "scope glob plus --glob should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("glob scope cannot be combined with --glob"),
        "expected clear glob conflict, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_range_scope_reports_missing_file_without_range_suffix() {
    let dir = temp_repo("missing_file_range_scope");
    write_file(&dir.join("src/lib.rs"), "pub fn target() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "target",
            "--as",
            "text",
            "--scope",
            "src/missing.rs:1-2",
        ])
        .output()
        .unwrap();

    assert!(!out.status.success(), "missing file range should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid scope: src/missing.rs"),
        "expected missing base file path, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("src/missing.rs:1-2"),
        "range suffix should not be reported as part of missing path:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_all_accepts_exact_file_and_glob_scopes() {
    let dir = temp_repo("match_all_file_glob_scope");
    write_file(
        &dir.join("src/one.rs"),
        "pub fn target() {\n    let alpha = beta;\n}\n",
    );
    write_file(&dir.join("src/two.py"), "def target():\n    alpha = beta\n");

    let file_out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "all",
            "--as",
            "text",
            "--scope",
            "src/one.rs",
        ])
        .output()
        .unwrap();
    assert!(
        file_out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&file_out.stderr)
    );
    let file_stdout = String::from_utf8_lossy(&file_out.stdout);
    assert!(file_stdout.contains("src/one.rs"), "{file_stdout}");
    assert!(!file_stdout.contains("two.py"), "{file_stdout}");

    let glob_out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "all",
            "--as",
            "text",
            "--scope",
            "src/*.rs",
        ])
        .output()
        .unwrap();
    assert!(
        glob_out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&glob_out.stderr)
    );
    let glob_stdout = String::from_utf8_lossy(&glob_out.stdout);
    assert!(glob_stdout.contains("src/one.rs"), "{glob_stdout}");
    assert!(!glob_stdout.contains("two.py"), "{glob_stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_file_globs_infer_file_discovery() {
    let dir = temp_repo("find_file_glob_inferred");
    write_file(&dir.join("src/lib.rs"), "fn alpha() {}\n");
    write_file(&dir.join("src/two.py"), "def alpha(): pass\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "*.rs", "--scope", "src"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "file glob through discover should infer file mode:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Files:"), "{stdout}");
    assert!(stdout.contains("src/ (1)"), "{stdout}");
    assert!(stdout.contains("lib.rs"), "{stdout}");
    assert!(!stdout.contains("two.py"), "{stdout}");

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
        .args(["discover", "BaseInfo\\|DomainInfo", "--scope", "src"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected invalid query to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported syntax for `srcwalk discover`"),
        "expected generic syntax diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk discover \"BaseInfo, DomainInfo\" --scope <dir>"),
        "expected supported syntax, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk discover <query> --scope <dir>"),
        "expected single-query syntax, got:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk discover '<glob>' --as file --scope <dir>"),
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
            "discover",
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
            "discover",
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
            "discover",
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
            "discover",
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
