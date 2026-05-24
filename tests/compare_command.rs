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
fn compare_exact_function_targets_reports_shared_and_only_structural_evidence() {
    let dir = temp_repo("compare_exact_targets");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub struct Engine { pub is_args: bool, pub quote: bool, pub size: usize }
pub fn left(e: &mut Engine) -> usize {
    if e.is_args || e.quote {
        e.size = 0;
        allocate(e.size);
        return encode(e.is_args);
    }
    0
}
pub fn right(e: &mut Engine) -> usize {
    if e.is_args || e.quote {
        allocate(e.size);
        return encode(e.is_args);
    }
    1
}
fn allocate(_: usize) {}
fn encode(_: bool) -> usize { 1 }
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "compare",
            "src/lib.rs:left",
            "src/lib.rs:right",
            "--no-budget",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.starts_with("# Compare: src/lib.rs:left <> src/lib.rs:right"),
        "{stdout}"
    );
    assert!(stdout.contains("confidence: structural syntax"), "{stdout}");
    assert!(
        stdout.contains("caveat: structural comparison only; not equivalence"),
        "{stdout}"
    );
    assert!(stdout.contains("targets:"), "{stdout}");
    assert!(stdout.contains("A src/lib.rs:left :2-9"), "{stdout}");
    assert!(stdout.contains("B src/lib.rs:right :10-16"), "{stdout}");
    assert!(stdout.contains("metrics: A features="), "{stdout}");
    assert!(stdout.contains("shared field access:"), "{stdout}");
    assert!(stdout.contains("is_args read condition"), "{stdout}");
    assert!(stdout.contains("quote read condition"), "{stdout}");
    assert!(stdout.contains("size read call_arg"), "{stdout}");
    assert!(stdout.contains("shared calls:"), "{stdout}");
    assert!(stdout.contains("call allocate"), "{stdout}");
    assert!(stdout.contains("call encode"), "{stdout}");
    assert!(stdout.contains("only in A:"), "{stdout}");
    assert!(stdout.contains("size reset assignment_lhs"), "{stdout}");
    assert!(stdout.contains("only in B:\n- none"), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk context src/lib.rs:left"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk context src/lib.rs:right"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show src/lib.rs:2-9"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show src/lib.rs:10-16"),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk context src/lib.rs:left")
            .count(),
        1,
        "left context next action should be deduplicated:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk show src/lib.rs:2-9")
            .count(),
        1,
        "left show next action should be deduplicated:\n{stdout}"
    );
    assert!(!stdout.contains("decision-flow"), "{stdout}");
    assert!(!stdout.contains("diff"), "{stdout}");
    assert!(!stdout.contains("mismatch"), "{stdout}");
    assert!(!stdout.contains("risk"), "{stdout}");
    assert!(!stdout.contains("vulnerability"), "{stdout}");
    assert!(!stdout.contains("exploit"), "{stdout}");
    assert!(!stdout.contains("bug"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_bare_ambiguous_symbol_fails_with_candidates() {
    let dir = temp_repo("compare_ambiguous_symbol");
    write_file(&dir.join("src/a.rs"), "pub fn same() -> usize { 1 }\n");
    write_file(&dir.join("src/b.rs"), "pub fn same() -> usize { 2 }\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["compare", "same", "same", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = norm_path_separators(&String::from_utf8_lossy(&out.stderr));
    assert!(stderr.contains("ambiguous symbol target"), "{stderr}");
    assert!(stderr.contains("src/a.rs"), "{stderr}");
    assert!(stderr.contains("src/b.rs"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_preserves_repeated_shared_key_occurrences() {
    let dir = temp_repo("compare_repeated_shared_keys");
    write_file(
        &dir.join("src/lib.rs"),
        r#"pub fn left() {
    helper();
    helper();
    helper();
}

pub fn right() {
    helper();
    helper();
}
fn helper() {}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "compare",
            "src/lib.rs:left",
            "src/lib.rs:right",
            "--no-budget",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
    assert!(
        stdout.contains("metrics: A features=3 B features=2 shared=2 only_A=1 only_B=0"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("- call helper").count(), 3, "{stdout}");
    assert!(stdout.contains("only in A:\n- call helper"), "{stdout}");
    assert!(stdout.contains("only in B:\n- none"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compare_reports_omitted_counts_when_feature_groups_are_capped() {
    let dir = temp_repo("compare_omitted_counts");
    let mut source = String::from("pub fn left() {\n");
    for i in 0..12 {
        source.push_str(&format!("    helper{i}();\n"));
    }
    source.push_str("}\n\npub fn right() {\n");
    for i in 0..12 {
        source.push_str(&format!("    helper{i}();\n"));
    }
    source.push_str("}\n");
    for i in 0..12 {
        source.push_str(&format!("fn helper{i}() {{}}\n"));
    }
    write_file(&dir.join("src/lib.rs"), &source);

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "compare",
            "src/lib.rs:left",
            "src/lib.rs:right",
            "--no-budget",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
    assert!(stdout.contains("shared calls:"), "{stdout}");
    assert!(stdout.contains("omitted: 4 more shared calls"), "{stdout}");
    assert!(stdout.contains("> Next:"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}
