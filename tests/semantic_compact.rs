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
fn callers_output_preserves_scope_receiver_args_and_omits_call_text_by_default() {
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
        !stdout.contains("sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)"),
        "default caller output should omit call text; use --expand for source context, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--expand[=N]"),
        "expected footer tip to mention --expand, got:\n{stdout}"
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
        !compact_stdout
            .contains("sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)"),
        "default caller output should not include call source, got:\n{compact_stdout}"
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
        expanded_stdout.contains("```")
            && expanded_stdout.contains("►")
            && expanded_stdout
                .contains("sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)"),
        "explicit --expand should keep source window with call source, got:\n{expanded_stdout}"
    );
}

#[test]
fn callers_filter_qualifiers_narrow_callsite_rows() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        r#"package main

func wanted() {
    sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)
}

func other() {
    client.TranslateRequest(payload, stream)
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "TranslateRequest",
            "--callers",
            "--filter",
            "args:5 receiver:sdktranslator",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "filtered callers should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("[fn] wanted")
            && stdout.contains("recv=sdktranslator")
            && stdout.contains("args=5"),
        "expected filtered caller row, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[fn] other"),
        "filter should exclude non-matching caller, got:\n{stdout}"
    );
    assert!(
        stdout.contains("filter matched 1/2 call sites"),
        "expected filter summary tip, got:\n{stdout}"
    );
    assert_tips_are_trailing(&stdout);
}

#[test]
fn callers_count_by_aggregates_filtered_callsites() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        r#"package main

func a() {
    sdktranslator.TranslateRequest(from, to, model, rawJSON, stream)
}

func b() {
    sdktranslator.TranslateRequest(payload, stream)
}

func c() {
    client.TranslateRequest(payload, stream)
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "TranslateRequest",
            "--callers",
            "--filter",
            "receiver:sdktranslator",
            "--count-by",
            "args",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "count-by should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("# Slice: TranslateRequest — 2 call sites grouped by args matching `receiver:sdktranslator`"),
        "expected count header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[group] args=5 count=1"),
        "expected args=5 bucket, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[group] args=2 count=1"),
        "expected args=2 bucket, got:\n{stdout}"
    );
    assert!(
        stdout.contains("--filter 'args:N receiver:NAME caller:NAME path:TEXT text:TEXT'"),
        "expected filter tip, got:\n{stdout}"
    );
    assert_tips_are_trailing(&stdout);
}

#[test]
fn callers_filter_rejects_depth_bfs_until_supported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.go"),
        "package main\nfunc caller() { TranslateRequest(payload) }\n",
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "TranslateRequest",
            "--callers",
            "--depth",
            "2",
            "--filter",
            "args:1",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "depth+filter should fail until supported"
    );
    assert!(
        stderr.contains("direct --callers only"),
        "expected direct-callers guardrail, got:\n{stderr}"
    );
}

#[test]
fn callers_count_by_zero_matches_uses_no_callers_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.ts"),
        "export function caller() { return otherThing(); }\n",
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "missingCall",
            "--callers",
            "--count-by",
            "receiver",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "count-by no matches should be diagnostic success"
    );
    assert!(
        stdout.contains("no call sites found") && !stdout.contains("[group]"),
        "expected no-callers diagnostic, got:\n{stdout}"
    );
}

#[test]
fn callers_count_by_groups_are_paginated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("caller.ts"),
        r#"export function a(client: any) { client.callTool({}); }
export function b(thisClient: any) { thisClient.callTool({}); }
export function c(other: any) { other.callTool({}); }
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "callTool",
            "--callers",
            "--count-by",
            "receiver",
            "--limit",
            "2",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "count-by pagination should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout.matches("[group]").count(),
        2,
        "expected 2 grouped rows, got:\n{stdout}"
    );
    assert!(
        stdout.contains("more groups available. Continue with --offset 2 --limit 2"),
        "expected group pagination tip, got:\n{stdout}"
    );
    assert_tips_are_trailing(&stdout);
}

#[test]
fn general_filter_path_narrows_symbol_search_without_callers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("param_functions.py"),
        "from fastapi import Depends\n\ndef get_item(dep = Depends(lambda: 1)):\n    return dep\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("other.py"),
        "from fastapi import Depends\n\ndef get_other(dep = Depends(lambda: 2)):\n    return dep\n",
    )
    .unwrap();

    let out = srcwalk()
        .args(["Depends", "--filter", "path:param_functions", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "general path filter should work without --callers, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("param_functions.py"),
        "expected filtered path match, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("other.py"),
        "path filter should remove non-matching files, got:\n{stdout}"
    );
}

#[test]
fn caller_only_filter_qualifiers_are_rejected_without_callers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.py"), "def Depends(x):\n    return x\n").unwrap();

    let out = srcwalk()
        .args(["Depends", "--filter", "args:1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "caller-only args filter should fail without --callers"
    );
    assert!(
        stderr.contains("only applies with --callers"),
        "expected caller-only qualifier diagnostic, got:\n{stderr}"
    );
}
