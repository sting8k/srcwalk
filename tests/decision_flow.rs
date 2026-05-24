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
fn decision_flow_rust_path_symbol_outputs_compact_control_edges() {
    let dir = temp_repo("decision_flow_rust_path_symbol");
    write_file(
        &dir.join("src/lib.rs"),
        r#"
fn route(mode: Mode) {
    if matches!(mode, Mode::Files) {
        run_files();
        return;
    }
    match mode {
        Mode::Text => run_text(),
        _ => run_symbol(),
    }
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
    assert!(
        stdout.starts_with("# Decision-flow: src/lib.rs:route"),
        "{stdout}"
    );
    assert!(stdout.contains("[target] src/lib.rs:"), "{stdout}");
    assert!(stdout.contains("[decision]"), "{stdout}");
    assert!(stdout.contains("matches!(mode, Mode::Files)"), "{stdout}");
    assert!(stdout.contains("Mode::Text"), "{stdout}");
    assert!(stdout.contains("run_files"), "{stdout}");
    assert!(stdout.contains("yes =>"), "{stdout}");
    assert!(stdout.contains("no =>"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_bare_symbol_resolves_primary_definition() {
    let dir = temp_repo("decision_flow_bare_symbol");
    write_file(
        &dir.join("src/lib.rs"),
        r#"
pub fn route(flag: bool) {
    if flag {
        yes();
    } else {
        no();
    }
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "route", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
    assert!(stdout.contains("# Decision-flow: route"), "{stdout}");
    assert!(stdout.contains("flag"), "{stdout}");
    assert!(stdout.contains("yes()"), "{stdout}");
    assert!(stdout.contains("no()"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_js_arrow_and_switch_are_tree_sitter_backed() {
    let dir = temp_repo("decision_flow_js_arrow");
    write_file(
        &dir.join("src/router.js"),
        r#"
const route = (mode) => {
  switch (mode) {
    case "text":
      return runText();
    default:
      return runDefault();
  }
};
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/router.js:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[decision]"), "{stdout}");
    assert!(stdout.contains("mode"), "{stdout}");
    assert!(stdout.contains("text"), "{stdout}");
    assert!(stdout.contains("default"), "{stdout}");
    assert!(stdout.contains("runText"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_typescript_path_line_selects_containing_function() {
    let dir = temp_repo("decision_flow_ts_line");
    write_file(
        &dir.join("src/router.ts"),
        r#"
export function route(mode: string) {
  if (mode === "text") {
    return runText();
  }
  return runDefault();
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/router.ts:3"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = norm_path_separators(&String::from_utf8_lossy(&out.stdout));
    assert!(
        stdout.contains("# Decision-flow: src/router.ts:3"),
        "{stdout}"
    );
    assert!(stdout.contains("mode === \"text\""), "{stdout}");
    assert!(stdout.contains("runText"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_python_if_else_and_raise() {
    let dir = temp_repo("decision_flow_python");
    write_file(
        &dir.join("app.py"),
        r#"
def route(value):
    if value:
        call_a()
    else:
        raise RuntimeError("bad")
    return value
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "app.py:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("value"), "{stdout}");
    assert!(stdout.contains("call_a"), "{stdout}");
    assert!(stdout.contains("[throw]"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_go_java_c_cpp_and_csharp_adapters() {
    let cases = [
        (
            "go",
            "src/router.go",
            "src/router.go:route",
            r#"
package main
func route(mode string) int {
    if mode == "text" {
        return runText()
    }
    switch mode {
    case "file":
        return runFile()
    default:
        return runDefault()
    }
}
"#,
            ["mode == \"text\"", "runText", "runFile", "runDefault"],
        ),
        (
            "java",
            "src/Router.java",
            "src/Router.java:route",
            r#"
class Router {
  int route(String mode) {
    if (mode.equals("text")) {
      return runText();
    } else {
      log(mode);
    }
    switch (mode) {
      case "file": return runFile();
      default: return runDefault();
    }
  }
}
"#,
            ["mode.equals", "runText", "runFile", "runDefault"],
        ),
        (
            "c",
            "src/router.c",
            "src/router.c:route",
            r#"
int route(int mode) {
  if (mode == 1) {
    return run_text();
  }
  switch (mode) {
    case 2: return run_file();
    default: return run_default();
  }
}
"#,
            ["mode == 1", "run_text", "run_file", "run_default"],
        ),
        (
            "cpp",
            "src/router.cpp",
            "src/router.cpp:route",
            r#"
int route(int mode) {
  if (mode == 1) {
    return run_text();
  }
  switch (mode) {
    case 2: return run_file();
    default: return run_default();
  }
}
"#,
            ["mode == 1", "run_text", "run_file", "run_default"],
        ),
        (
            "csharp",
            "src/Router.cs",
            "src/Router.cs:Route",
            r#"
class Router {
  int Route(string mode) {
    if (mode == "text") {
      return RunText();
    }
    switch (mode) {
      case "file": return RunFile();
      default: throw new Exception("bad");
    }
  }
}
"#,
            ["mode == \"text\"", "RunText", "RunFile", "[throw]"],
        ),
    ];

    for (name, path, target, source, expected) in cases {
        let dir = temp_repo(&format!("decision_flow_{name}"));
        write_file(&dir.join(path), source);

        let out = srcwalk()
            .current_dir(&dir)
            .args(["decision-flow", target])
            .output()
            .unwrap();

        assert!(
            out.status.success(),
            "{name} stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("[decision]"), "{name}:\n{stdout}");
        for needle in expected {
            assert!(
                stdout.contains(needle),
                "{name} expected {needle}:\n{stdout}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn decision_flow_unsupported_language_fails_loudly() {
    let dir = temp_repo("decision_flow_unsupported");
    write_file(&dir.join("style.css"), "a { color: red; }\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "style.css:1"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "unsupported CSS should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "decision-flow currently supports Rust, JavaScript, TypeScript, TSX, Python, Go, Java, C, C++, and C#"
        ),
        "{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_line_target_focuses_selected_area_in_large_function() {
    let dir = temp_repo("decision_flow_focused_line");
    write_file(
        &dir.join("src/main.go"),
        r#"package main
func main() {
    setup1()
    setup2()
    setup3()
    if ready {
        before()
        target()
        if failed() {
            fail()
            return
        }
        after()
    } else {
        other()
    }
    done()
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/main.go:8", "--budget", "500"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pre-target statements x3"), "{stdout}");
    assert!(stdout.contains("pre-target statements x1"), "{stdout}");
    assert!(stdout.contains("target()"), "{stdout}");
    assert!(stdout.contains("failed()"), "{stdout}");
    assert!(stdout.contains("after()"), "{stdout}");
    assert!(stdout.contains("done()"), "{stdout}");
    assert!(!stdout.contains("setup1()"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_class_range_reports_function_target_requirement() {
    let dir = temp_repo("decision_flow_class_range");
    write_file(
        &dir.join("src/Thing.cs"),
        r#"class Thing {
  int Route() {
    return 1;
  }
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/Thing.cs:1-5"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "class range should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("line/range target must be inside one supported function"),
        "{stderr}"
    );
    assert!(
        stderr.contains("class/module ranges are not supported"),
        "{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_bare_symbol_rejects_ambiguous_definitions() {
    let dir = temp_repo("decision_flow_ambiguous_symbol");
    write_file(
        &dir.join("a/a.go"),
        "package a\nfunc Init() {\n    one()\n}\n",
    );
    write_file(
        &dir.join("b/b.go"),
        "package b\nfunc Init() {\n    two()\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "Init"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "ambiguous bare symbol should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ambiguous symbol target"), "{stderr}");
    assert!(stderr.contains("use file:symbol or file:line"), "{stderr}");
    assert!(stderr.contains("a/a.go"), "{stderr}");
    assert!(stderr.contains("b/b.go"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_js_identifier_switch_case_keeps_first_body_statement() {
    let dir = temp_repo("decision_flow_js_identifier_case");
    write_file(
        &dir.join("src/router.js"),
        r#"
const TEXT = "text";
function route(mode) {
  switch (mode) {
    case TEXT:
      firstBodyStatement();
      secondBodyStatement();
      return done();
    default:
      return fallback();
  }
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/router.js:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("TEXT"), "{stdout}");
    assert!(stdout.contains("firstBodyStatement"), "{stdout}");
    assert!(stdout.contains("secondBodyStatement"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_artifact_message_does_not_advertise_decision_flow() {
    let dir = temp_repo("decision_flow_artifact_message");
    write_file(&dir.join("src/lib.rs"), "fn route() {}\n");

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/lib.rs:route", "--artifact"])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "decision-flow --artifact should fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--artifact currently supports"), "{stderr}");
    assert!(
        !stderr.contains("decision-flow"),
        "artifact support message should not advertise decision-flow:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_empty_loop_body_does_not_relabel_incoming_edge() {
    let dir = temp_repo("decision_flow_empty_loop");
    write_file(
        &dir.join("src/lib.rs"),
        r#"
fn route() {
    before();
    loop {}
    after();
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("before()"), "{stdout}");
    assert!(stdout.contains("[loop]"), "{stdout}");
    assert!(stdout.contains("after()"), "{stdout}");
    assert!(stdout.contains("=> N2 [loop]"), "{stdout}");
    assert!(
        !stdout.contains("repeat => N2 [loop]"),
        "incoming edge to empty loop must not be mislabeled repeat:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_assignment_bound_if_keeps_decision_shape() {
    let dir = temp_repo("decision_flow_assignment_bound_if");
    write_file(
        &dir.join("src/lib.rs"),
        r#"fn route(flag: bool) -> bool {
    let value = if flag { true } else { false };
    value
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/lib.rs:route"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[decision]"), "{stdout}");
    assert!(stdout.contains("flag"), "{stdout}");
    assert!(
        !stdout.contains("let value = if"),
        "assignment-bound control expression should not flatten into one action:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decision_flow_line_target_on_function_signature_renders_body() {
    let dir = temp_repo("decision_flow_signature_line");
    write_file(
        &dir.join("src/lib.rs"),
        "fn route(flag: bool) {\n    if flag {\n        yes();\n    } else {\n        no();\n    }\n}\n",
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["decision-flow", "src/lib.rs:1"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("flag"), "{stdout}");
    assert!(stdout.contains("yes()"), "{stdout}");
    assert!(stdout.contains("no()"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}
