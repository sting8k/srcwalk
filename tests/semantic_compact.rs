use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn assert_tips_are_trailing(stdout: &str) {
    let trimmed = stdout.trim_end();
    let last_tip = trimmed
        .rfind("> Tip:")
        .unwrap_or_else(|| panic!("expected at least one footer tip:\n{stdout}"));
    assert!(
        trimmed[last_tip..]
            .lines()
            .all(|line| line.starts_with("> Tip:") || line.is_empty()),
        "tips should be trailing footer lines, got:\n{stdout}"
    );
}

#[test]
fn symbol_search_exposes_class_kind_range_and_child_context() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("DependencyProperty.cs"),
        r#"namespace Microsoft.UI.Xaml
{
    public partial class DependencyProperty
    {
        public DependencyProperty()
        {
        }
    }
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["DependencyProperty", "--glob", "*.cs", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "symbol search should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("class DependencyProperty") && stdout.contains("3-8"),
        "expected class kind/range semantic context, got:\n{stdout}"
    );
    assert!(
        stdout.contains("fn DependencyProperty") && stdout.contains("5-7"),
        "expected constructor/function child semantic context, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Microsoft.UI.Xaml"),
        "expected namespace/module context, got:\n{stdout}"
    );
}

#[test]
fn symbol_search_pagination_tip_remains_trailing_with_semantic_context() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("DependencyProperty.cs"),
        r#"namespace Microsoft.UI.Xaml
{
    public partial class DependencyProperty
    {
        public DependencyProperty()
        {
        }
    }
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "DependencyProperty",
            "--glob",
            "*.cs",
            "--limit",
            "1",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("class DependencyProperty"),
        "pagination should not remove semantic class context, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--offset 1 --limit 1"),
        "expected actionable pagination tip, got:\n{stdout}"
    );
    assert_tips_are_trailing(&stdout);
}

#[test]
fn symbol_search_facets_use_semantic_compact_definition_rows() {
    let dir = tempfile::tempdir().unwrap();
    for idx in 0..6 {
        std::fs::write(
            dir.path().join(format!("DependencyProperty{idx}.cs")),
            format!(
                r#"namespace Microsoft.UI.Xaml
{{
    public partial class DependencyProperty
    {{
        public DependencyProperty()
        {{
        }}
    }}
}}
"#
            ),
        )
        .unwrap();
    }

    let out = srcwalk()
        .args(["DependencyProperty", "--glob", "*.cs", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("### Definitions (6)"),
        "expected faceted definitions output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[class] Microsoft.UI.Xaml.DependencyProperty"),
        "expected semantic compact class rows with namespace context, got:\n{stdout}"
    );
    assert!(
        stdout.contains("+[fn] DependencyProperty"),
        "expected child function breadcrumbs, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("### File overview:"),
        "compact facets should avoid duplicate basename file overview, got:\n{stdout}"
    );
}

#[test]
fn symbol_search_semantic_rows_work_across_rust_typescript_and_python() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.rs"),
        "pub struct Widget {}\nimpl Widget { pub fn build(&self) {} }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("widget.ts"),
        "export class Widget {\n  build(arg: string) { return arg; }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("widget.py"),
        "class Widget:\n    def build(self):\n        return 1\n",
    )
    .unwrap();

    let rust = srcwalk()
        .args(["Widget", "--glob", "*.rs", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let rust_stdout = String::from_utf8_lossy(&rust.stdout);
    assert!(
        rust_stdout.contains("[struct] Widget") || rust_stdout.contains("struct Widget"),
        "expected Rust struct semantic row/context, got:\n{rust_stdout}"
    );

    let typescript = srcwalk()
        .args(["Widget", "--glob", "*.ts", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let typescript_stdout = String::from_utf8_lossy(&typescript.stdout);
    assert!(
        typescript_stdout.contains("[class] Widget"),
        "expected TypeScript exported class semantic row, got:\n{typescript_stdout}"
    );
    assert!(
        !typescript_stdout.contains("[export] export class Widget"),
        "TypeScript exported class should not fall back to generic export row, got:\n{typescript_stdout}"
    );

    let python = srcwalk()
        .args(["Widget", "--glob", "*.py", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let python_stdout = String::from_utf8_lossy(&python.stdout);
    assert!(
        python_stdout.contains("[class] Widget") || python_stdout.contains("class Widget"),
        "expected Python class semantic row/context, got:\n{python_stdout}"
    );
    assert!(
        python_stdout.contains("+[fn] build") || python_stdout.contains("fn build"),
        "expected Python child function semantic breadcrumb/context, got:\n{python_stdout}"
    );
}

#[test]
fn callers_semantic_rows_work_across_go_python_and_rust() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        r#"package main

func caller() {
    sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.py"),
        "def py_caller(client):\n    return client.translate_request(payload, stream)\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.rs"),
        "fn rust_caller(client: Client) { client.translate_request(payload, stream); }\n",
    )
    .unwrap();

    let go = srcwalk()
        .args(["TranslateRequest", "--callers", "--glob", "*.go", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let go_stdout = String::from_utf8_lossy(&go.stdout);
    assert!(
        go_stdout.contains("[fn] caller")
            && go_stdout.contains("recv=sdktranslator")
            && go_stdout.contains("args=5"),
        "expected Go caller semantic row, got:\n{go_stdout}"
    );

    let python = srcwalk()
        .args([
            "translate_request",
            "--callers",
            "--glob",
            "*.py",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let python_stdout = String::from_utf8_lossy(&python.stdout);
    assert!(
        python_stdout.contains("[fn] py_caller")
            && python_stdout.contains("recv=client")
            && python_stdout.contains("args=2"),
        "expected Python caller semantic row, got:\n{python_stdout}"
    );

    let rust = srcwalk()
        .args([
            "translate_request",
            "--callers",
            "--glob",
            "*.rs",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let rust_stdout = String::from_utf8_lossy(&rust.stdout);
    assert!(
        rust_stdout.contains("[fn] rust_caller")
            && rust_stdout.contains("recv=client")
            && rust_stdout.contains("args=2"),
        "expected Rust caller semantic row, got:\n{rust_stdout}"
    );
}

#[test]
fn callers_output_preserves_scope_receiver_args_and_call_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        r#"package main

func caller() {
    sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)
}

func other() {
    TranslateRequest(from, to, model, rawJSON, stream)
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["TranslateRequest", "--callers", "--limit", "1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "callers search should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("caller: caller") || stdout.contains("[fn] caller"),
        "expected caller function scope, got:\n{stdout}"
    );
    assert!(
        stdout.contains("receiver: sdktranslator") || stdout.contains("recv=sdktranslator"),
        "expected receiver metadata, got:\n{stdout}"
    );
    assert!(
        stdout.contains("args: 5") || stdout.contains("args=5"),
        "expected argument count metadata, got:\n{stdout}"
    );
    assert!(
        stdout.contains("sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)"),
        "expected call text, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--offset 1 --limit 1"),
        "expected caller pagination tip, got:\n{stdout}"
    );
    assert_tips_are_trailing(&stdout);
}

#[test]
fn callers_default_is_compact_but_expand_still_shows_source_window() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        r#"package main

func caller() {
    sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)
}
"#,
    )
    .unwrap();

    let compact = srcwalk()
        .args(["TranslateRequest", "--callers", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let compact_stdout = String::from_utf8_lossy(&compact.stdout);
    assert!(
        compact_stdout.contains("<- calls") && compact_stdout.contains("[fn] caller"),
        "expected semantic compact caller row, got:\n{compact_stdout}"
    );
    assert!(
        !compact_stdout.contains("```"),
        "default caller output should not include source fence, got:\n{compact_stdout}"
    );

    let expanded = srcwalk()
        .args(["TranslateRequest", "--callers", "--expand=1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let expanded_stdout = String::from_utf8_lossy(&expanded.stdout);
    assert!(
        expanded_stdout.contains("```") && expanded_stdout.contains("►"),
        "explicit --expand should keep source window, got:\n{expanded_stdout}"
    );
}
