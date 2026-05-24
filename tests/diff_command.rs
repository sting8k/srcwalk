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
    init_git(&dir);
    dir
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git(dir: &Path) {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "srcwalk@example.test"]);
    git(dir, &["config", "user.name", "Srcwalk Test"]);
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
}

fn norm_path_separators(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\\', "/")
}

#[test]
fn diff_committed_range_maps_hunks_to_enclosing_symbols() {
    let dir = temp_repo("diff_range_symbols");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn compute(flag: bool) -> usize {
    if flag { 1 } else { 0 }
}
"#,
    );
    commit_all(&dir, "base");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn compute(flag: bool) -> usize {
    let size = if flag { 2 } else { 1 };
    size
}
"#,
    );
    commit_all(&dir, "change compute");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert!(normalized.starts_with("# Diff: HEAD~1..HEAD"), "{stdout}");
    assert!(stdout.contains("confidence: structural syntax"), "{stdout}");
    assert!(
        stdout.contains("diff-to-evidence navigation only; not risk, runtime, or security proof"),
        "{stdout}"
    );
    assert!(normalized.contains("## src/lib.rs"), "{stdout}");
    assert!(stdout.contains("inside compute"), "{stdout}");
    assert!(stdout.contains("changed symbols:"), "{stdout}");
    assert!(stdout.contains("compute :1-4"), "{stdout}");
    assert!(
        normalized.contains("> Next: srcwalk show src/lib.rs:")
            && normalized.contains("> Next: srcwalk review src/lib.rs:compute"),
        "{stdout}"
    );
    assert_eq!(
        normalized
            .matches("> Next: srcwalk review src/lib.rs:compute")
            .count(),
        1,
        "review next action should be deduplicated:\n{normalized}"
    );
    assert_eq!(
        normalized
            .matches("> Next: srcwalk deps src/lib.rs")
            .count(),
        1,
        "deps next action should be deduplicated:\n{normalized}"
    );
    assert!(!stdout.contains("vulnerability"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_deletion_only_hunk_maps_to_enclosing_symbol() {
    let dir = temp_repo("diff_delete_inside_symbol");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn compute() -> u8 {
    let _debug = 1;
    2
}
"#,
    );
    commit_all(&dir, "base");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn compute() -> u8 {
    2
}
"#,
    );
    commit_all(&dir, "delete debug line");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("old:2 inside compute"), "{stdout}");
    assert!(
        stdout.contains("compute :1-3 modified lines old:2"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_import_only_change_stays_file_level() {
    let dir = temp_repo("diff_import_only");
    write_file(
        &dir.join("src/lib.rs"),
        r#"use std::fmt;

pub fn value() -> u8 {
    1
}
"#,
    );
    commit_all(&dir, "base");
    write_file(
        &dir.join("src/lib.rs"),
        r#"use std::fmt;
use std::io;

pub fn value() -> u8 {
    1
}
"#,
    );
    commit_all(&dir, "change import");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("file-level"), "{stdout}");
    assert!(
        !stdout.contains("changed symbols:") && !stdout.contains("inside use std"),
        "imports should not be rendered as changed symbols:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_staged_reports_staged_changes_only() {
    let dir = temp_repo("diff_staged");
    write_file(&dir.join("src/lib.rs"), "pub fn flag() -> bool { false }\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/lib.rs"), "pub fn flag() -> bool { true }\n");
    git(&dir, &["add", "src/lib.rs"]);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "--staged", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(stdout.starts_with("# Diff: staged"), "{stdout}");
    assert!(normalized.contains("## src/lib.rs"), "{stdout}");
    assert!(stdout.contains("status: modified"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_default_includes_untracked_non_ignored_files() {
    let dir = temp_repo("diff_untracked");
    write_file(&dir.join("README.md"), "base\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/new.rs"), "pub fn fresh() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(stdout.starts_with("# Diff: working tree"), "{stdout}");
    assert!(normalized.contains("## src/new.rs"), "{stdout}");
    assert!(stdout.contains("status: added"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_limit_reports_omitted_section_before_pagination_next_action() {
    let dir = temp_repo("diff_limit_omitted");
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 1 }\n");
    write_file(&dir.join("src/b.rs"), "pub fn b() -> u8 { 1 }\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 2 }\n");
    write_file(&dir.join("src/b.rs"), "pub fn b() -> u8 { 2 }\n");
    commit_all(&dir, "change both");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src", "--limit", "1"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("files: changed=2 shown=1"), "{stdout}");
    assert!(stdout.contains("## omitted\n- files: 1"), "{stdout}");
    assert!(
        stdout.contains("> Next: 1 more changed files: add --offset 1 --limit 1."),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: 1 more changed files: add --offset 1 --limit 1.")
            .count(),
        1,
        "pagination next action should be emitted once:\n{stdout}"
    );
    assert!(
        stdout.find("## omitted").unwrap() < stdout.find("(~").unwrap(),
        "omitted counts should be part of the packet body before token footer:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_scope_filters_changed_files() {
    let dir = temp_repo("diff_scope_filter");
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 1 }\n");
    write_file(&dir.join("tests/b.rs"), "pub fn b() -> u8 { 1 }\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 2 }\n");
    write_file(&dir.join("tests/b.rs"), "pub fn b() -> u8 { 2 }\n");
    commit_all(&dir, "change both");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(normalized.contains("## src/a.rs"), "{stdout}");
    assert!(!normalized.contains("tests/b.rs"), "{stdout}");

    let exact = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src/a.rs"])
        .output()
        .unwrap();
    assert!(
        exact.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    let exact_stdout = norm_path_separators(&String::from_utf8_lossy(&exact.stdout));
    assert!(exact_stdout.contains("## src/a.rs"), "{exact_stdout}");
    assert!(!exact_stdout.contains("tests/b.rs"), "{exact_stdout}");

    let glob = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD~1..HEAD", "--scope", "src/*.rs"])
        .output()
        .unwrap();
    assert!(
        glob.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&glob.stderr)
    );
    let glob_stdout = norm_path_separators(&String::from_utf8_lossy(&glob.stdout));
    assert!(glob_stdout.contains("## src/a.rs"), "{glob_stdout}");
    assert!(!glob_stdout.contains("tests/b.rs"), "{glob_stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_handles_changed_paths_with_spaces() {
    let dir = temp_repo("diff_path_spaces");
    write_file(&dir.join("src/a file.rs"), "pub fn spaced() -> u8 { 1 }\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/a file.rs"), "pub fn spaced() -> u8 { 2 }\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(normalized.contains("## src/a file.rs"), "{stdout}");
    assert!(stdout.contains("inside spaced"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn diff_rejects_single_revision_shorthand() {
    let dir = temp_repo("diff_reject_single_rev");
    write_file(&dir.join("src/lib.rs"), "pub fn x() {}\n");
    commit_all(&dir, "base");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["diff", "HEAD", "--scope", "."])
        .output()
        .unwrap();

    assert!(!out.status.success(), "single revision should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("diff revision range must use explicit A..B or A...B"),
        "{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
