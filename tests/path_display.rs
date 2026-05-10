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

fn write_fixture(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn target_fn() {\n    helper();\n}\n\nfn helper() {}\n",
    )
    .unwrap();
}

#[test]
fn read_header_prefers_pwd_relative_path() {
    let dir = temp_repo("path_display_read");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .arg("src/lib.rs")
        .arg("--section")
        .arg("1")
        .arg("--full")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected read to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# src/lib.rs "),
        "expected pwd-relative file header, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn search_header_and_hits_are_pwd_relative_and_drillable() {
    let dir = temp_repo("path_display_search");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "target_fn", "--scope", "src"])
        .output()
        .unwrap();

    assert!(out.status.success(), "expected search to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# Search: \"target_fn\" in src —"),
        "expected pwd-relative search header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("src/lib.rs:1-3"),
        "expected pwd-relative copy-pasteable hit path, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let drill = srcwalk()
        .current_dir(&dir)
        .arg("src/lib.rs:1")
        .output()
        .unwrap();
    assert!(
        drill.status.success(),
        "displayed hit path should be drillable; stderr:\n{}",
        String::from_utf8_lossy(&drill.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_long_line_content_hit_is_centered_and_capped() {
    let dir = temp_repo("path_display_long_line_cap");
    let long_line = format!("{} NEEDLE {}", "a".repeat(500), "b".repeat(500));
    fs::write(dir.join("data.txt"), format!("{long_line}\n")).unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "NEEDLE", "--scope", "."])
        .output()
        .unwrap();
    assert!(out.status.success(), "expected find to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("NEEDLE"), "{stdout}");
    assert!(
        stdout.contains('…'),
        "long line should be compacted: {stdout}"
    );
    assert!(
        !stdout.contains(&"a".repeat(400)) && !stdout.contains(&"b".repeat(400)),
        "long line should not be dumped in full:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_slash_route_query_searches_content_not_missing_path() {
    let dir = temp_repo("path_display_route_find");
    fs::write(
        dir.join("server.js"),
        "if (req.method === 'GET' && pathname === '/api/gold') return handleGold(req, res);\n",
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "api/gold", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "route-like slash query should search content; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("server.js:1"), "{stdout}");
    assert!(stdout.contains("/api/gold"), "{stdout}");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "/api/gold", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "absolute route-like slash query should search content; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_explicit_relative_missing_path_still_fails_as_path() {
    let dir = temp_repo("path_display_missing_explicit_path");
    fs::write(dir.join("server.js"), "const route = '/api/gold';\n").unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "./missing.js", "--scope", "."])
        .output()
        .unwrap();
    assert!(!out.status.success(), "missing explicit path should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found:"), "{stderr}");
    assert!(stderr.contains("looks like a file path"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compact_usage_facets_group_repeated_hits_by_path() {
    let dir = temp_repo("path_display_grouped_find");
    fs::create_dir_all(dir.join("tool-tags")).unwrap();
    fs::write(
        dir.join("index.ts"),
        r#"pi.on("message_start", () => {});
pi.on("message_update", () => {});
pi.on("message_end", () => {});
pi.on("session_start", () => {});
"#,
    )
    .unwrap();
    fs::write(
        dir.join("tool-tags/read.ts"),
        r#"pi.registerTool({ name: "read" });
pi.registerTool({ name: "read_more" });
"#,
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args(["find", "pi.", "--scope", "."])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected find to succeed, stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("  index.ts [4 usages]"), "{stdout}");
    assert!(stdout.contains("    [usage] :1 | pi.on"), "{stdout}");
    assert!(
        stdout.contains("  tool-tags/read.ts [2 usages]"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("  [usage] index.ts:1 |"),
        "repeated path hits should be grouped, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_header_prefers_pwd_relative_scope() {
    let dir = temp_repo("path_display_map");
    write_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["map", "--scope", "src", "--depth", "1"])
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# Map: src (depth 1"),
        "expected pwd-relative map header, got:\n{stdout}"
    );
    assert!(
        !stdout.contains(dir.to_string_lossy().as_ref()),
        "output should not include absolute temp root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
