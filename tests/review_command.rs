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
    s.replace('\\', "/")
}

fn assert_no_review_verdicts(stdout: &str) {
    let lower = stdout.to_ascii_lowercase();
    for forbidden in [
        "root cause",
        "vulnerab",
        "security",
        " risk",
        "risk:",
        "safe to",
        "unsafe",
        "correctness",
    ] {
        assert!(
            !lower.contains(forbidden),
            "review output should not emit verdict-like claim `{forbidden}`:\n{stdout}"
        );
    }
    for word in ["bug", "bugs"] {
        assert!(
            !contains_word(&lower, word),
            "review output should not emit verdict-like word `{word}`:\n{stdout}"
        );
    }
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|word| word == needle)
}

#[test]
fn review_no_verdict_helper_allows_debug_text() {
    assert_no_review_verdicts("debug output is still navigation evidence");
}

#[test]
fn review_local_function_emits_review_packet_with_flow_map() {
    let dir = temp_repo("review_local_flow_map");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn route(flag: bool) -> usize {
    if flag {
        return 1;
    }
    0
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.starts_with("# Review Packet: src/lib.rs:route"),
        "{stdout}"
    );
    assert!(stdout.contains("confidence: structural syntax"), "{stdout}");
    assert!(stdout.contains("## target"), "{stdout}");
    assert!(stdout.contains("src/lib.rs:1-6"), "{stdout}");
    assert!(stdout.contains("## flow map"), "{stdout}");
    assert!(stdout.contains("N1 entry"), "{stdout}");
    assert!(stdout.contains("decision"), "{stdout}");
    assert!(stdout.contains("true ->"), "{stdout}");
    assert!(stdout.contains("false ->"), "{stdout}");
    assert!(stdout.contains("## exits"), "{stdout}");
    assert!(stdout.contains("return 1"), "{stdout}");
    assert!(stdout.contains("> Next:"), "{stdout}");
    assert!(
        stdout.contains("srcwalk show src/lib.rs:1-6 -C 20"),
        "{stdout}"
    );
    assert!(!stdout.contains("srcwalk decision-flow"), "{stdout}");
    assert!(!stdout.contains("srcwalk diff"), "{stdout}");
    assert!(!stdout.contains("vulnerability"), "{stdout}");
    assert!(!stdout.contains("security"), "{stdout}");
    assert!(!stdout.contains("risk"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_flow_map_includes_structural_evidence_annotations() {
    let dir = temp_repo("review_flow_map_annotations");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub struct Engine { pub is_args: bool, pub quote: bool, pub size: usize }
pub fn route(e: &Engine) -> usize {
    if e.is_args || e.quote {
        allocate(e.size);
        return encode(e.is_args);
    }
    0
}
fn allocate(_: usize) {}
fn encode(_: bool) -> usize { 1 }
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:route", "--no-budget"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.contains("reads: e.is_args condition :3; e.quote condition :3"),
        "{stdout}"
    );
    assert!(stdout.contains("calls: allocate :4"), "{stdout}");
    assert!(stdout.contains("reads: e.size call_arg :4"), "{stdout}");
    assert!(stdout.contains("calls: encode :5"), "{stdout}");
    assert!(stdout.contains("reads: e.is_args call_arg :5"), "{stdout}");
    assert!(!stdout.contains("depends"), "{stdout}");
    assert!(!stdout.contains("affects"), "{stdout}");
    assert!(!stdout.contains("unsafe"), "{stdout}");
    assert!(!stdout.contains("mismatch"), "{stdout}");
    assert!(!stdout.contains("risk"), "{stdout}");
    assert!(!stdout.contains("bug"), "{stdout}");
    assert!(!stdout.contains("security"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_parent_relative_target_is_local_not_revision_range() {
    let dir = temp_repo("review_parent_relative_target");
    let subdir = dir.join("work");
    fs::create_dir_all(&subdir).unwrap();
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn route(flag: bool) -> usize {
    if flag {
        return 1;
    }
    0
}
"#,
    );

    let out = srcwalk()
        .current_dir(&subdir)
        .args(["review", "../src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.starts_with("# Review Packet: ../src/lib.rs:route"),
        "{stdout}"
    );
    assert!(stdout.contains("## flow map"), "{stdout}");
    assert!(stdout.contains("> Next:"), "{stdout}");
    assert!(
        !stdout.contains("diff revision range must use explicit"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_local_function_budget_preserves_exits_and_next_reads() {
    let dir = temp_repo("review_budget_footer");
    let mut body = String::from("pub fn big(value: usize) -> usize {\n");
    for i in 0..40 {
        body.push_str(&format!("    if value == {i} {{ return {i}; }}\n"));
    }
    body.push_str("    value\n}\n");
    write_file(&dir.join("src/lib.rs"), &body);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:big", "--budget", "120"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("... truncated"), "{stdout}");
    assert!(stdout.contains("## exits"), "{stdout}");
    assert!(stdout.contains("return 0"), "{stdout}");
    assert!(stdout.contains("> Next:"), "{stdout}");
    assert!(
        stdout.contains("srcwalk show src/lib.rs:1-43 -C 20"),
        "{stdout}"
    );
    assert!(!stdout.contains("srcwalk decision-flow"), "{stdout}");
    assert!(!stdout.contains("srcwalk diff"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_local_function_node_cap_omission_is_not_duplicated() {
    let dir = temp_repo("review_node_cap_notice");
    let mut body = String::from("pub fn capped(value: usize) -> usize {\n");
    for i in 0..90 {
        body.push_str(&format!("    if value == {i} {{ return {i}; }}\n"));
    }
    body.push_str("    value\n}\n");
    write_file(&dir.join("src/lib.rs"), &body);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:capped", "--no-budget"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("## flow map"), "{stdout}");
    let node_cap_notice = "omitted: node cap reached; narrow the target range.";
    assert_eq!(stdout.matches(node_cap_notice).count(), 1, "{stdout}");
    assert!(
        !stdout.contains("omitted: flow nodes were capped; narrow the target range."),
        "{stdout}"
    );
    assert!(stdout.contains("## exits"), "{stdout}");
    assert!(stdout.contains("> Next:"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_branch_function_summarizes_linear_action_chains() {
    let dir = temp_repo("review_branch_action_chain_summary");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn route(flag: bool) -> usize {
    prepare();
    encode();
    persist();
    if flag {
        return 1;
    }
    finish()
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("shape: 1 entry, 1 decision"), "{stdout}");
    assert!(
        stdout.contains("actions summarized :2-4 3 action nodes"),
        "{stdout}"
    );
    assert!(
        stdout.contains("calls: prepare :2; encode :3; persist :4"),
        "{stdout}"
    );
    assert!(!stdout.contains("N2 action :2 prepare"), "{stdout}");
    assert!(stdout.contains("decision"), "{stdout}");
    assert!(stdout.contains("## exits"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_linear_function_summarizes_actions_instead_of_dumping_nodes() {
    let dir = temp_repo("review_linear_action_summary");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn build() {
    prepare();
    encode();
    persist();
    notify();
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:build"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.contains(
            "shape: linear structural flow; no branch nodes detected by supported parser"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("actions summarized :2-5 4 action nodes"),
        "{stdout}"
    );
    assert!(!stdout.contains("N2 action"), "{stdout}");
    assert!(stdout.contains("## exits"), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk show src/lib.rs:1-6 -C 20"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_flow_map_shape_summary_is_language_agnostic() {
    let cases = [
        (
            "rust",
            "src/lib.rs",
            "src/lib.rs:route",
            r#"pub fn route(flag: bool) -> usize {
    if flag {
        return 1;
    }
    0
}
"#,
        ),
        (
            "ts",
            "src/router.ts",
            "src/router.ts:route",
            r#"export function route(flag: boolean): number {
  if (flag) {
    return 1;
  }
  return 0;
}
"#,
        ),
        (
            "python",
            "app.py",
            "app.py:route",
            r#"def route(flag):
    if flag:
        return 1
    return 0
"#,
        ),
        (
            "go",
            "src/router.go",
            "src/router.go:route",
            r#"package main
func route(flag bool) int {
    if flag {
        return 1
    }
    return 0
}
"#,
        ),
    ];

    for (name, path, target, source) in cases {
        let dir = temp_repo(&format!("review_shape_{name}"));
        write_file(&dir.join(path), source);

        let out = srcwalk()
            .current_dir(&dir)
            .args(["review", target])
            .output()
            .unwrap();

        assert!(
            out.status.success(),
            "case {name} stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
        assert!(
            stdout.contains("shape: 1 entry, 1 decision, 0 loops"),
            "case {name}:\n{stdout}"
        );
        assert!(stdout.contains("## exits"), "case {name}:\n{stdout}");
        assert!(stdout.contains("return 1"), "case {name}:\n{stdout}");

        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn review_linear_focus_preserves_summary_nodes() {
    let dir = temp_repo("review_linear_focus_summary");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn build() {
    prepare();
    encode();
    persist();
    notify();
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "src/lib.rs:4", "--no-budget"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.contains(
            "shape: linear structural flow; no branch nodes detected by supported parser"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("summary: N2 summary :2-3 pre-target statements x2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("action: N3 action :4 persist()"),
        "{stdout}"
    );
    assert!(stdout.contains("## exits"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_unsupported_line_target_degrades_to_file_level_packet() {
    let dir = temp_repo("review_unsupported_fallback");
    write_file(&dir.join("README.md"), "# Title\n\nBody\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "README.md:1"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.starts_with("# Review Packet: README.md:1"),
        "{stdout}"
    );
    assert!(stdout.contains("## flow map"), "{stdout}");
    assert!(
        stdout.contains("file-level evidence only; structural function map unavailable"),
        "{stdout}"
    );
    assert!(stdout.contains("srcwalk show README.md -C 20"), "{stdout}");
    assert!(!stdout.contains("security"), "{stdout}");
    assert!(!stdout.contains("risk"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_deleted_file_uses_colon_scoped_old_hunk_provenance() {
    let dir = temp_repo("review_deleted_file_provenance");
    init_git(&dir);
    write_file(
        &dir.join("src/dead.rs"),
        r#"pub fn dead() -> usize {
    1
}
"#,
    );
    commit_all(&dir, "base");
    fs::remove_file(dir.join("src/dead.rs")).unwrap();
    commit_all(&dir, "delete dead");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("source: diff metadata"), "{stdout}");
    assert!(stdout.contains("provenance: src/dead.rs:old:"), "{stdout}");
    assert_no_review_verdicts(&stdout);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn review_change_packet_reports_omitted_files_for_limit() {
    let dir = temp_repo("review_limit_omitted");
    init_git(&dir);
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 1 }\n");
    write_file(&dir.join("src/b.rs"), "pub fn b() -> u8 { 1 }\n");
    commit_all(&dir, "base");
    write_file(&dir.join("src/a.rs"), "pub fn a() -> u8 { 2 }\n");
    write_file(&dir.join("src/b.rs"), "pub fn b() -> u8 { 2 }\n");
    commit_all(&dir, "change both");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "HEAD~1..HEAD", "--scope", "src", "--limit", "1"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(stdout.contains("files: changed=2 shown=1"), "{stdout}");
    assert!(stdout.contains("## omitted"), "{stdout}");
    assert!(stdout.contains("- files: 1"), "{stdout}");
    assert!(
        stdout.contains("> Next: 1 more changed files. Continue with srcwalk review HEAD~1..HEAD --scope src --offset 1 --limit 1."),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: 1 more changed files. Continue with srcwalk review HEAD~1..HEAD --scope src --offset 1 --limit 1.")
            .count(),
        1,
        "pagination next action should be deduplicated:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
#[test]
fn review_committed_range_composes_diff_evidence_and_flow_maps() {
    let dir = temp_repo("review_range_flow_maps");
    init_git(&dir);
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
    if flag {
        return 2;
    }
    1
}
"#,
    );
    commit_all(&dir, "change compute");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["review", "HEAD~1..HEAD", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.starts_with("# Review Packet: HEAD~1..HEAD"),
        "{stdout}"
    );
    assert!(
        stdout.contains("confidence: structural syntax + diff metadata"),
        "{stdout}"
    );
    assert!(stdout.contains("## changed evidence"), "{stdout}");
    assert!(stdout.contains("### src/lib.rs"), "{stdout}");
    assert!(stdout.contains("inside compute"), "{stdout}");
    assert!(
        stdout.contains("| source: diff metadata | provenance: src/lib.rs:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("context: compute :1-6 | confidence: structural syntax"),
        "{stdout}"
    );
    assert!(stdout.contains("## changed symbols"), "{stdout}");
    assert!(stdout.contains("compute :1-6"), "{stdout}");
    assert!(stdout.contains("## flow maps"), "{stdout}");
    assert!(
        stdout.contains(
            "bounds: changed function targets; shown=1 omitted=0 cap=5; confidence: structural syntax"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("### src/lib.rs:compute"), "{stdout}");
    assert!(
        stdout.contains("provenance: post-change src/lib.rs:1-6 | confidence: structural syntax"),
        "{stdout}"
    );
    assert!(stdout.contains("> Next:"), "{stdout}");
    assert!(stdout.contains("srcwalk show src/lib.rs:"), "{stdout}");
    assert!(
        stdout.contains("srcwalk review src/lib.rs:compute"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("srcwalk review HEAD~1..HEAD --scope src\n"),
        "{stdout}"
    );
    assert!(!stdout.contains("srcwalk decision-flow"), "{stdout}");
    assert!(!stdout.contains("srcwalk diff"), "{stdout}");
    assert_no_review_verdicts(&stdout);

    let _ = fs::remove_dir_all(&dir);
}
