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
fn json_flag_is_removed_from_public_cli() {
    let help = srcwalk()
        .args(["trace", "callers", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(!stdout.contains("--json"), "{stdout}");

    let output = srcwalk()
        .args(["discover", "anything", "--json"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "discover --json unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--json'"), "{stderr}");
}

#[test]
fn show_context_lines_expands_focused_line_window() {
    let dir = temp_repo("show_context_lines");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\nfn fourth() {}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:3", "-C", "1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show -C failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 │ fn second() {}"), "{stdout}");
    assert!(stdout.contains("►    3 │ fn third() {}"), "{stdout}");
    assert!(stdout.contains("4 │ fn fourth() {}"), "{stdout}");
    assert!(
        !stdout.contains("1: fn first() {}"),
        "context should be exactly one line around focus:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_reads_strict_comma_separated_locations() {
    let dir = temp_repo("show_multi");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:1,lib.rs:3", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "multi-show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Show: 2 locations"), "{stdout}");
    assert!(stdout.contains("►    1 │ fn first() {}"), "{stdout}");
    assert!(stdout.contains("►    3 │ fn third() {}"), "{stdout}");
    assert!(stdout.contains("\n---\n"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_as_text_forces_content_search_for_path_like_query() {
    let dir = temp_repo("discover_text_path_like");
    fs::write(dir.join("notes.txt"), "docs/missing.md is mentioned here\n").unwrap();

    let output = srcwalk()
        .args(["discover", "docs/missing.md", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --as text failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("notes.txt:1"), "{stdout}");
    assert!(
        stdout.contains("docs/missing.md is mentioned here"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_accepts_file_and_glob_scope_for_search_modes() {
    let dir = temp_repo("discover_scope_specs");
    fs::create_dir_all(dir.join("src/nested")).unwrap();
    fs::write(dir.join("src/one.rs"), "fn target() {}\nfn helper() {}\n").unwrap();
    fs::write(dir.join("src/two.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("src/nested/three.rs"), "fn target() {}\n").unwrap();

    let file_output = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--scope", "src/one.rs"])
        .output()
        .unwrap();
    assert!(
        file_output.status.success(),
        "file-scope symbol search failed:\n{}",
        String::from_utf8_lossy(&file_output.stderr)
    );
    let file_stdout = String::from_utf8_lossy(&file_output.stdout);
    assert!(file_stdout.contains("src/one.rs:1-1"), "{file_stdout}");
    assert!(
        !file_stdout.contains("src/two.rs"),
        "file scope should not scan siblings:\n{file_stdout}"
    );

    let glob_output = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--as", "text", "--scope", "src/*.rs"])
        .output()
        .unwrap();
    assert!(
        glob_output.status.success(),
        "glob-scope text search failed:\n{}",
        String::from_utf8_lossy(&glob_output.stderr)
    );
    let glob_stdout = String::from_utf8_lossy(&glob_output.stdout);
    assert!(glob_stdout.contains("src/one.rs:1"), "{glob_stdout}");
    assert!(glob_stdout.contains("src/two.rs:1"), "{glob_stdout}");
    assert!(
        !glob_stdout.contains("nested/three.rs"),
        "src/*.rs should not include nested files:\n{glob_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_all_reports_same_file_cooccurrence_caveat() {
    let dir = temp_repo("discover_match_all");
    fs::write(dir.join("one.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
    fs::write(dir.join("two.rs"), "fn alpha() {}\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "all",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --match all failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Co-occurrence:"), "{stdout}");
    assert!(stdout.contains("same-file co-occurrence only"), "{stdout}");
    assert!(stdout.contains("one.rs"), "{stdout}");
    assert!(
        !stdout.contains("two.rs"),
        "files without all terms must be excluded:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_exclude_filters_discovery_evidence_by_file_pattern() {
    let dir = temp_repo("discover_exclude");
    fs::write(dir.join("keep.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("skip_test.rs"), "fn target() {}\n").unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--exclude", "*test.rs", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --exclude failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.rs"), "{stdout}");
    assert!(!stdout.contains("skip_test.rs"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_as_file_supports_exclude_without_legacy_glob_filter() {
    let dir = temp_repo("discover_file_exclude");
    fs::write(dir.join("keep.rs"), "fn keep() {}\n").unwrap();
    fs::write(dir.join("skip_test.rs"), "fn skip() {}\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "*.rs",
            "--as",
            "file",
            "--exclude",
            "*test.rs",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --as file --exclude failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.rs"), "{stdout}");
    assert!(!stdout.contains("skip_test.rs"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_all_rejects_file_and_access_interpretations() {
    for as_kind in ["file", "access"] {
        let output = srcwalk()
            .args(["discover", "alpha,beta", "--match", "all", "--as", as_kind])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "--match all should reject --as {as_kind}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("symbol/text co-occurrence"),
            "expected co-occurrence error, got:\n{stderr}"
        );
    }
}

#[test]
fn discover_infers_file_mode_for_path_like_glob() {
    let dir = temp_repo("discover_infer_file_glob");
    fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.join("notes.txt"), "main.rs is mentioned here\n").unwrap();

    let output = srcwalk()
        .args(["discover", "*.rs", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover inferred file glob failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Files:"), "{stdout}");
    assert!(stdout.contains("main.rs"), "{stdout}");
    assert!(!stdout.contains("notes.txt:1"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_comma_terms_are_literal_or() {
    let dir = temp_repo("discover_text_or");
    fs::write(dir.join("one.txt"), "alpha only\n").unwrap();
    fs::write(dir.join("two.txt"), "beta only\n").unwrap();
    fs::write(dir.join("literal.txt"), "alpha,beta exact phrase\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Text OR:"), "{stdout}");
    assert!(stdout.contains("## alpha"), "{stdout}");
    assert!(stdout.contains("## beta"), "{stdout}");
    assert!(stdout.contains("one.txt:1"), "{stdout}");
    assert!(stdout.contains("two.txt:1"), "{stdout}");
    assert!(stdout.contains("literal OR text evidence only"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_large_output_rolls_up_by_file() {
    let dir = temp_repo("discover_text_or_file_rollup");
    fs::write(
        dir.join("feature.go"),
        "alpha here\nbeta here\ngamma here\nalpha beta gamma\n",
    )
    .unwrap();
    fs::write(
        dir.join("feature_test.go"),
        "alpha test\nbeta test\ngamma test\n",
    )
    .unwrap();
    fs::write(dir.join("other.go"), "alpha only\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma,missing",
            "--match",
            "any",
            "--as",
            "text",
            "--limit",
            "2",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files_idx = stdout.find("## Files ranked by term coverage").unwrap();
    let terms_idx = stdout.find("## Terms").unwrap();
    assert!(
        files_idx < terms_idx,
        "file rollup must lead term summary:\n{stdout}"
    );
    assert!(stdout.contains("feature.go — 3 terms"), "{stdout}");
    assert!(stdout.contains("terms: alpha("), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk show feature.go:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("missing — 0/0 matches, 0 files"),
        "{stdout}"
    );
    assert!(stdout.contains("omitted by per-term limit"), "{stdout}");
    assert!(
        !stdout.contains("## alpha —"),
        "compact mode should not dump term-first hit blocks:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_does_not_treat_latest_as_test_path() {
    let dir = temp_repo("discover_text_or_latest_not_test");
    fs::create_dir_all(dir.join("latest")).unwrap();
    fs::write(dir.join("latest/prod.go"), "alpha\nbeta\ngamma\n").unwrap();
    fs::write(dir.join("a_test.go"), "alpha\nbeta\ngamma\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let prod_idx = stdout.find("latest/prod.go — 3 terms").unwrap();
    let test_idx = stdout.find("a_test.go — 3 terms").unwrap();
    assert!(
        prod_idx < test_idx,
        "production path under latest/ must rank before test file:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_omission_note_does_not_suggest_global_offset() {
    let dir = temp_repo("discover_text_or_omission_note");
    fs::write(dir.join("one.txt"), "alpha one\nalpha two\nbeta one\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--limit",
            "1",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## alpha — 1/2 matches"), "{stdout}");
    assert!(
        stdout.contains("more `alpha` matches omitted by per-term limit 1"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("Continue with --offset"),
        "text OR must not suggest a global offset that hides shorter terms:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_rejects_offset() {
    let dir = temp_repo("discover_text_or_rejects_offset");
    fs::write(dir.join("one.txt"), "alpha one\nbeta one\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--offset",
            "1",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(!output.status.success(), "offset should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not support --offset"),
        "expected offset diagnostic, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_bare_punctuation_list_infers_text_or() {
    let dir = temp_repo("discover_bare_text_or");
    fs::write(dir.join("one.rs"), "fn handler() { let url = req.body; }\n").unwrap();
    fs::write(dir.join("two.rs"), "fn proxy() { fetch(url); }\n").unwrap();

    let output = srcwalk()
        .args(["discover", "req.body,fetch", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "bare punctuation list should infer text OR:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Text OR:"), "{stdout}");
    assert!(stdout.contains("## req.body"), "{stdout}");
    assert!(stdout.contains("## fetch"), "{stdout}");
    assert!(stdout.contains("literal OR text evidence only"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_structural_matches_suggest_confirmed_context_targets() {
    let dir = temp_repo("discover_context_targets");
    fs::write(
        dir.join("lib.rs"),
        r#"fn target() -> i32 {
    helper()
}

fn helper() -> i32 { 1 }

fn caller() -> i32 {
    target()
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover target failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Confirmed next context targets"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk context lib.rs:1-3"),
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("> Next: srcwalk context lib.rs:1-3").count(),
        1,
        "confirmed context next action should be deduplicated:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_typescript_function_definition_suggests_confirmed_context_target() {
    let dir = temp_repo("discover_ts_context_target");
    fs::write(
        dir.join("server.ts"),
        r#"import { Server } from "@modelcontextprotocol/sdk/server/index.js";

function createGatewayServer(context: unknown): Server {
    const server = new Server({ name: "gateway", version: "1" });
    server.setRequestHandler("list", async () => ({ tools: [] }));
    server.setRequestHandler("call", async (request) => request);
    return server;
}

export function startServer() {
    return createGatewayServer({});
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "createGatewayServer", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover createGatewayServer failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Confirmed next context targets"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk context server.ts:3-8"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_unsupported_language_function_does_not_suggest_context_target() {
    let dir = temp_repo("discover_unsupported_context_target");
    fs::write(
        dir.join("App.swift"),
        r#"func didUpdatePermissionStatus(_ message: String) {
    print(message)
}

func callStatus() {
    didUpdatePermissionStatus("ok")
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "didUpdatePermissionStatus", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover didUpdatePermissionStatus failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[fn] didUpdatePermissionStatus"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("## Confirmed next context targets"),
        "unsupported context language must not suggest context targets:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_and_document_hits_do_not_guess_context_targets() {
    let dir = temp_repo("discover_no_context_guess");
    fs::write(dir.join("lib.rs"), "fn handler() { let token = 1; }\n").unwrap();
    fs::write(
        dir.join("README.md"),
        "# Getting Started\n\nUse token here.\n",
    )
    .unwrap();

    let text_output = srcwalk()
        .args(["discover", "token", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(text_output.status.success());
    let text_stdout = String::from_utf8_lossy(&text_output.stdout);
    assert!(
        !text_stdout.contains("## Confirmed next context targets"),
        "text evidence must not guess context targets:\n{text_stdout}"
    );

    let doc_output = srcwalk()
        .args(["discover", "Getting Started", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(doc_output.status.success());
    let doc_stdout = String::from_utf8_lossy(&doc_output.stdout);
    assert!(
        doc_stdout.contains("[section] Getting Started"),
        "{doc_stdout}"
    );
    assert!(
        !doc_stdout.contains("## Confirmed next context targets"),
        "document evidence must not suggest context targets:\n{doc_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_default_text_comma_stays_literal_and_hints_when_empty() {
    let dir = temp_repo("discover_text_literal_comma_hint");
    fs::write(dir.join("one.txt"), "alpha only\nbeta only\n").unwrap();

    let output = srcwalk()
        .args(["discover", "alpha,beta", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "literal comma text search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0 matches"), "{stdout}");
    assert!(
        stdout.contains("treated as one literal text query"),
        "{stdout}"
    );
    assert!(stdout.contains("--match any --as text"), "{stdout}");
    assert!(stdout.contains("--match all --as text"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn slash_delimited_text_query_is_literal_not_regex() {
    let dir = temp_repo("discover_regex_removed");
    fs::write(dir.join("one.txt"), "alpha only\nbeta only\n").unwrap();

    let output = srcwalk()
        .args(["discover", "/alpha|beta/", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "slash literal text search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Search: \"/alpha|beta/\""), "{stdout}");
    assert!(stdout.contains("0 matches"), "{stdout}");
    assert!(!stdout.contains("one.txt:1"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}
