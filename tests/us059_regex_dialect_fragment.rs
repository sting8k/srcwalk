//! US-059: `discover` handles regex-dialect and path-fragment queries without
//! dead ends. These integration tests exercise the acceptance criteria against
//! a fixture tree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_us059_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture_repo(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    // `packages/ai` is NOT created as a real directory — only `packages/ai-client`
    // exists, so `discover packages/ai` is an unresolvable path fragment that
    // still matches `packages/ai-client/...` relative paths by substring.
    fs::create_dir_all(dir.join("packages/ai-client")).unwrap();
    fs::create_dir_all(dir.join("packages/share")).unwrap();
    fs::write(
        dir.join("packages/ai-client/worker.js"),
        "function parseGitUrl(raw) { return parseGitUrl(raw).href; }\nexport function parseGitUrl(url) { return new URL(url); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("packages/share/index.js"),
        "function f() { const budget = 5; return truncate(budget); }\n",
    )
    .unwrap();
    fs::write(dir.join("models.json"), "{\"name\": \"fixture\"}\n").unwrap();
    fs::write(
        dir.join("packages/ai-client/usage.js"),
        "import { parseGitUrl } from './worker.js';\nconst m = parseGitUrl('https://x');\n",
    )
    .unwrap();
    dir
}

#[test]
fn regex_escape_query_returns_symbol_and_text_with_interpreted_as_header() {
    let dir = fixture_repo("regex_escape");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", r"parseGitUrl\(", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "regex-escape discover failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("interpreted as"),
        "expected `interpreted as` header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("## Symbol: parseGitUrl"),
        "expected a symbol section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("## Text: parseGitUrl("),
        "expected a text section, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_escape_equals_plain_filename_glob() {
    let dir = fixture_repo("bare_filename_escape");
    let escaped = srcwalk()
        .current_dir(&dir)
        .args(["discover", r"models\.json", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        escaped.status.success(),
        "escaped filename discover failed:\n{}",
        String::from_utf8_lossy(&escaped.stderr)
    );
    let escaped_out = String::from_utf8_lossy(&escaped.stdout);
    let plain = srcwalk()
        .current_dir(&dir)
        .args(["discover", "models.json", "--scope", "."])
        .output()
        .unwrap();
    let plain_out = String::from_utf8_lossy(&plain.stdout);
    // Both must surface the file; the escaped form additionally labels itself.
    assert!(
        escaped_out.contains("models.json"),
        "escaped filename did not resolve the file:\n{escaped_out}"
    );
    assert!(
        escaped_out.contains("interpreted as"),
        "escaped form should carry the label:\n{escaped_out}"
    );
    assert!(
        plain_out.contains("models.json"),
        "plain filename did not resolve the file:\n{plain_out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cooccurrence_pattern_returns_ordered_same_line_matches() {
    let dir = fixture_repo("cooccurrence");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "budget.*truncate", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "co-occurrence discover failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("budget") && stdout.contains("truncate"),
        "expected both terms in co-occurrence result, got:\n{stdout}"
    );
    assert!(
        stdout.contains("interpreted as same-line"),
        "expected co-occurrence label, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unresolvable_path_fragment_returns_path_rows_not_error() {
    let dir = fixture_repo("path_fragment");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "packages/ai", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "path-fragment discover should not error:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("worker.js") && stdout.contains("usage.js"),
        "expected path-fragment rows, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Path fragments"),
        "expected path-fragment header, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn zero_match_glob_route_emits_try_line() {
    let dir = temp_repo("zero_match_glob");
    fs::write(dir.join("a.rs"), "fn alpha() {}\n").unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "*.zzz", "--as", "file", "--scope", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let all = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        all.contains("> Try:"),
        "zero-match glob route should emit `> Try:`, got:\n{all}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn zero_match_cooccurrence_emits_try_line() {
    let dir = temp_repo("zero_match_cooccurrence");
    fs::write(dir.join("a.rs"), "fn alpha() {}\n").unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "zzz.*qqq", "--scope", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("> Try:"),
        "zero-match co-occurrence should emit `> Try:`, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn route_like_slash_query_still_searches_content() {
    // Regression guard: `api/gold` is a route literal, not a path fragment.
    let dir = temp_repo("route_like_slash");
    fs::write(
        dir.join("server.js"),
        "if (pathname === '/api/gold') return handleGold(req, res);\n",
    )
    .unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "api/gold", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "route query failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("server.js:1") && stdout.contains("/api/gold"),
        "route query should search content, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn existing_directory_returns_listing_not_error() {
    let dir = temp_repo("existing_directory_listing");
    fs::create_dir_all(dir.join("packages/ai")).unwrap();
    fs::write(
        dir.join("packages/ai/worker.js"),
        "function parseGitUrl() {}\n",
    )
    .unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "packages/ai", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "existing directory should return a listing, not error:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("worker.js"),
        "expected directory listing, got:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_fragment_output_capped_at_twenty() {
    let dir = temp_repo("path_fragment_cap");
    for i in 0..=30 {
        let sub = dir.join(format!("packages/ai/f{i}"));
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("mod.js"), "export const x = 1;\n").unwrap();
    }
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "packages/ai", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "path fragment discover failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let rows = stdout.matches("mod.js").count();
    assert!(
        rows <= 20,
        "path-fragment output should be capped at 20 rows, got {rows}:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn explicit_relative_missing_path_still_fails() {
    let dir = temp_repo("explicit_relative_missing");
    fs::write(dir.join("server.js"), "const route = '/api/gold';\n").unwrap();
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "./missing.js", "--scope", "."])
        .output()
        .unwrap();
    assert!(!out.status.success(), "explicit missing path should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found:"),
        "expected explicit missing path error, got:\n{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_with_filter_rejects_like_plain_file_glob() {
    // US-059 review P1-3: `models\.json --filter ...` is a file/glob read and
    // must reject --filter with the same message as `discover models.json`,
    // not silently ignore it.
    let dir = fixture_repo("bare_filename_filter");
    let escaped = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            r"models\.json",
            "--scope",
            ".",
            "--filter",
            "path:whatever",
        ])
        .output()
        .unwrap();
    assert!(
        !escaped.status.success(),
        "bare-filename + --filter must fail (exit {}):\nstdout={}\nstderr={}",
        escaped.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&escaped.stdout),
        String::from_utf8_lossy(&escaped.stderr)
    );
    let stderr = String::from_utf8_lossy(&escaped.stderr);
    assert!(
        stderr.contains("invalid query"),
        "expected invalid query diagnostic, got stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "--filter applies to discover results and direct trace callers, not file/glob reads"
        ),
        "expected the shared file/glob reject message, got stderr:\n{stderr}"
    );

    // The plain filename route must reject with the identical message
    // (consistency contract).
    let plain = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "models.json",
            "--scope",
            ".",
            "--filter",
            "path:whatever",
        ])
        .output()
        .unwrap();
    assert!(!plain.status.success(), "plain file/glob must also reject");
    let plain_stderr = String::from_utf8_lossy(&plain.stderr);
    let reason =
        "--filter applies to discover results and direct trace callers, not file/glob reads";
    assert!(
        plain_stderr.contains(reason),
        "plain file/glob must reject with the shared reason, got:
{plain_stderr}"
    );
    assert!(
        stderr.contains(reason) && plain_stderr.contains(reason),
        "both routes must share the same reject reason"
    );
    assert!(
        stderr.contains(r"models\.json") && plain_stderr.contains("models.json"),
        "each error quotes its own typed query (escaped vs bare)"
    );
    let _ = fs::remove_dir_all(&dir);
}
