use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_scoped_occurrences_{name}_{}_{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn context_target_output(dir: &Path, target: &str, artifact: bool) -> String {
    let mut command = srcwalk();
    command
        .current_dir(dir)
        .args(["context", target, "--scope", "."]);
    if artifact {
        command.arg("--artifact");
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn context_output(dir: &Path, relative: &str, line: u32) -> String {
    context_target_output(dir, &format!("{relative}:{line}"), false)
}

#[test]
fn javascript_context_limits_occurrences_to_parent_scope_and_nested_shadow_barrier() {
    let dir = temp_repo("javascript");
    fs::write(
        dir.join("app.js"),
        r#"function outer(value) {
  function helper(input) {
    return input + 1;
  }
  const first = helper(value);
  const label = "helper"; // helper is text, not an identifier occurrence
  function nested(helper) {
    return helper(value);
  }
  return helper(first);
}

function other(value) {
  return helper(value);
}

function helper(value) {
  return value;
}
helper(1);
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, "app.js", 2);
    assert!(
        stdout.contains("## Scoped name occurrences (2)"),
        "{stdout}"
    );
    assert!(stdout.contains("app.js:5"), "{stdout}");
    assert!(stdout.contains("app.js:10"), "{stdout}");
    for excluded_line in [2, 6, 8, 14, 20] {
        assert!(
            !stdout.contains(&format!("\n- app.js:{excluded_line}\n")),
            "unexpected scoped row at line {excluded_line}:\n{stdout}"
        );
    }
    assert_eq!(
        stdout
            .matches("scoped occurrences are not binding-")
            .count(),
        1,
        "{stdout}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn scoped_occurrences_are_capped_and_body_only_selectors_abstain() {
    let dir = temp_repo("bounded");
    let calls = (0..14)
        .map(|index| format!("  const value{index} = helper({index});"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.join("bounded.js"),
        format!("function outer() {{\n  function helper(value) {{ return value; }}\n{calls}\n}}\n"),
    )
    .unwrap();

    let stdout = context_output(&dir, "bounded.js", 2);
    assert!(
        stdout.contains("## Scoped name occurrences (14)"),
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("source: AST identifier").count(),
        12,
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("2 additional candidates omitted by the scoped-occurrence cap.")
            .count(),
        1,
        "{stdout}"
    );

    let body_stdout = context_output(&dir, "bounded.js", 3);
    assert!(
        !body_stdout.contains("## Scoped name occurrences"),
        "body-only selectors must not infer a target declaration:\n{body_stdout}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn parseable_artifact_context_uses_the_shared_scoped_extractor() {
    let dir = temp_repo("artifact");
    fs::write(
        dir.join("bundle.js"),
        r#"function outer(value) {
  function helper(input) { return input + 1; }
  const first = helper(value);
  return helper(first);
}
"#,
    )
    .unwrap();

    let exact = context_target_output(&dir, "bundle.js:2", true);
    assert!(exact.contains("source: artifact AST"), "{exact}");
    assert!(exact.contains("## Scoped name occurrences (2)"), "{exact}");
    assert!(exact.contains("source: artifact AST identifier"), "{exact}");
    assert!(
        exact.contains("no source-map or original-source identity"),
        "{exact}"
    );

    let symbol = context_target_output(&dir, "helper", true);
    assert!(symbol.contains("# Context: helper — artifact"), "{symbol}");
    assert!(
        symbol.contains("## Scoped name occurrences (2)"),
        "{symbol}"
    );
    assert!(
        symbol.contains("source: artifact AST identifier"),
        "{symbol}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn typescript_type_annotation_does_not_create_a_false_shadow_barrier() {
    let dir = temp_repo("typescript_type_annotation");
    fs::write(
        dir.join("typed.ts"),
        r#"function outer(value: number) {
  function helper(input: number) { return input + 1; }
  function nested(value: helper) {
    return helper(value);
  }
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, "typed.ts", 2);
    assert!(
        stdout.contains("## Scoped name occurrences (1)"),
        "{stdout}"
    );
    assert!(stdout.contains("typed.ts:4"), "{stdout}");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ambiguous_symbol_targets_abstain_from_scoped_structural_evidence() {
    let dir = temp_repo("ambiguous_targets");
    fs::write(
        dir.join("duplicate.js"),
        "function outer(){function helper(){};helper();} function helper(){}; helper();\n",
    )
    .unwrap();

    let source = context_target_output(&dir, "duplicate.js:helper", false);
    assert!(
        !source.contains("## Scoped name occurrences"),
        "ambiguous file:symbol target must abstain:\n{source}"
    );

    let artifact = context_target_output(&dir, "helper", true);
    assert!(
        !artifact.contains("## Scoped name occurrences"),
        "ambiguous artifact symbol target must abstain:\n{artifact}"
    );

    let one_line = context_output(&dir, "duplicate.js", 1);
    assert!(
        !one_line.contains("## Scoped name occurrences"),
        "one-line target with multiple declarations must abstain:\n{one_line}"
    );

    fs::write(
        dir.join("first.js"),
        "function cross(value) { return value; } cross(1);\n",
    )
    .unwrap();
    fs::write(
        dir.join("second.js"),
        "function cross(value) { return value + 1; } cross(2);\n",
    )
    .unwrap();
    let cross_file_artifact = context_target_output(&dir, "cross", true);
    assert!(
        !cross_file_artifact.contains("## Scoped name occurrences"),
        "cross-file artifact ambiguity must abstain:\n{cross_file_artifact}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn nested_local_bindings_are_conservative_shadow_barriers() {
    let dir = temp_repo("local_shadow_barriers");
    fs::write(
        dir.join("shadow.js"),
        r#"function outer(value) {
  function helper(input) { return input + 1; }
  const first = helper(value);
  function nested(value) {
    const helper = value;
    return helper(value);
  }
  return helper(first);
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, "shadow.js", 2);
    assert!(
        stdout.contains("## Scoped name occurrences (2)"),
        "{stdout}"
    );
    assert!(stdout.contains("\n- shadow.js:3\n"), "{stdout}");
    assert!(stdout.contains("\n- shadow.js:8\n"), "{stdout}");
    assert!(!stdout.contains("\n- shadow.js:5\n"), "{stdout}");
    assert!(!stdout.contains("\n- shadow.js:6\n"), "{stdout}");

    fs::write(
        dir.join("shadow.rs"),
        r#"fn outer(value: i32) -> i32 {
    fn helper(input: i32) -> i32 { input + 1 }
    let first = helper(value);
    fn nested(value: i32) -> i32 {
        let helper = |input: i32| input;
        helper(value)
    }
    helper(first)
}
"#,
    )
    .unwrap();
    fs::write(
        dir.join("shadow.py"),
        r#"def outer(value):
    def helper(input):
        return input + 1
    first = helper(value)
    def nested(value):
        helper = lambda item: item
        return helper(value)
    return helper(first)
"#,
    )
    .unwrap();

    for (file, target_line, kept, excluded) in [
        ("shadow.rs", 2, [3, 8], [5, 6]),
        ("shadow.py", 2, [4, 8], [6, 7]),
    ] {
        let output = context_output(&dir, file, target_line);
        assert!(
            output.contains("## Scoped name occurrences (2)"),
            "{output}"
        );
        for line in kept {
            assert!(output.contains(&format!("\n- {file}:{line}\n")), "{output}");
        }
        for line in excluded {
            assert!(
                !output.contains(&format!("\n- {file}:{line}\n")),
                "{output}"
            );
        }
    }

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn python_member_assignment_does_not_create_a_false_shadow_barrier() {
    let dir = temp_repo("python_member_assignment");
    fs::write(
        dir.join("member.py"),
        r#"def outer(value, obj, items):
    def helper(input):
        return input + 1
    def nested(value):
        obj.helper = value
        items[helper] = value
        return helper(value)
    return nested(value)
"#,
    )
    .unwrap();

    let output = context_output(&dir, "member.py", 2);
    assert!(
        output.contains("## Scoped name occurrences (3)"),
        "{output}"
    );
    for line in [5, 6, 7] {
        assert!(
            output.contains(&format!("\n- member.py:{line}\n")),
            "{output}"
        );
    }

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn tight_budget_omits_the_scoped_section_atomically() {
    let dir = temp_repo("budget_coherence");
    let calls = (0..20)
        .map(|index| format!("  const value{index} = helper({index});"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.join("budget.js"),
        format!("function outer() {{\n  function helper(value) {{ return value; }}\n{calls}\n}}\n"),
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args(["context", "budget.js:2", "--scope", ".", "--budget", "80"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("## Scoped name occurrences"),
        "budgeted output must not expose a partial scoped section:\n{stdout}"
    );
    assert!(
        stdout.contains("scoped name occurrences omitted by context budget"),
        "budget omission must be explicit:\n{stdout}"
    );

    let artifact_out = srcwalk()
        .current_dir(&dir)
        .args([
            "context",
            "budget.js:2",
            "--scope",
            ".",
            "--artifact",
            "--budget",
            "80",
        ])
        .output()
        .unwrap();
    assert!(
        artifact_out.status.success(),
        "artifact context failed: {}",
        String::from_utf8_lossy(&artifact_out.stderr)
    );
    let artifact_stdout = String::from_utf8_lossy(&artifact_out.stdout);
    assert!(
        !artifact_stdout.contains("## Scoped name occurrences"),
        "budgeted artifact output must not expose a partial scoped section:\n{artifact_stdout}"
    );
    assert!(
        artifact_stdout.contains("scoped name occurrences omitted by context budget"),
        "artifact budget omission must be explicit:\n{artifact_stdout}"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn rust_and_python_context_emit_structural_scope_candidates() {
    let dir = temp_repo("multilang");
    fs::write(
        dir.join("lib.rs"),
        r#"fn outer(value: i32) -> i32 {
    fn helper(input: i32) -> i32 {
        input + 1
    }
    let first = helper(value);
    helper(first)
}
"#,
    )
    .unwrap();
    fs::write(
        dir.join("app.py"),
        r#"def outer(value):
    def helper(input):
        return input + 1
    first = helper(value)
    return helper(first)
"#,
    )
    .unwrap();

    for (file, definition_line, expected_lines) in [("lib.rs", 2, [5, 6]), ("app.py", 2, [4, 5])] {
        let stdout = context_output(&dir, file, definition_line);
        assert!(
            stdout.contains("## Scoped name occurrences (2)"),
            "{stdout}"
        );
        for line in expected_lines {
            assert!(stdout.contains(&format!("{file}:{line}")), "{stdout}");
        }
        assert!(stdout.contains("source: AST identifier"), "{stdout}");
        assert!(
            stdout.contains("confidence: same-file structural scope candidate"),
            "{stdout}"
        );
        assert!(
            !stdout.to_ascii_lowercase().contains("find references"),
            "{stdout}"
        );
    }

    let _ = fs::remove_dir_all(dir);
}
