use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn assert_footers_are_trailing(stdout: &str) {
    fn is_footer_line(line: &str) -> bool {
        line.starts_with("> Next:") || line.starts_with("> Note:") || line.starts_with("> Caveat:")
    }

    let trimmed = stdout.trim_end();
    let lines: Vec<&str> = trimmed.lines().collect();
    let mut idx = lines.len();
    let mut saw_footer = false;
    while idx > 0 {
        let line = lines[idx - 1];
        if is_footer_line(line) {
            saw_footer = true;
            idx -= 1;
        } else if line.is_empty() && saw_footer {
            idx -= 1;
        } else {
            break;
        }
    }

    assert!(saw_footer, "expected at least one footer line:\n{stdout}");
    assert!(
        lines[idx..]
            .iter()
            .all(|line| line.is_empty() || is_footer_line(line)),
        "footers should be trailing lines, got:\n{stdout}"
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
fn symbol_search_pagination_footer_remains_trailing_with_semantic_context() {
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
        stdout.contains("> Next:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable pagination next-step, got:\n{stdout}"
    );
    assert_footers_are_trailing(&stdout);
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

import sdktranslator "example.com/sdk/translator"

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
            && go_stdout.contains("prefix=sdktranslator(pkg)")
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
            && python_stdout.contains("prefix=client(var)")
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
            && rust_stdout.contains("prefix=client(var)")
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

import sdktranslator "example.com/sdk/translator"

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
        stdout.contains("prefix=sdktranslator(pkg)"),
        "expected prefix metadata, got:\n{stdout}"
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
        stdout.contains("--expand[=N]") && stdout.contains("--count-by args|path"),
        "expected compact footer next-step to mention expand/count-by, got:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next:") && stdout.contains("--offset 1 --limit 1"),
        "expected caller pagination next-step, got:\n{stdout}"
    );
    assert_footers_are_trailing(&stdout);
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

import sdktranslator "example.com/sdk/translator"

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
            "args:5 prefix:sdktranslator",
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
            && stdout.contains("prefix=sdktranslator(pkg)")
            && stdout.contains("args=5"),
        "expected filtered caller row, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[fn] other"),
        "filter should exclude non-matching caller, got:\n{stdout}"
    );
    assert!(
        stdout.contains("> Note: filter matched 1/2 call sites"),
        "expected filter summary note, got:\n{stdout}"
    );
    assert_footers_are_trailing(&stdout);
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
        stdout.contains(
            "> Next: narrow with --filter 'args:N prefix:NAME caller:NAME path:TEXT text:TEXT'"
        ),
        "expected filter next-step, got:\n{stdout}"
    );
    assert_footers_are_trailing(&stdout);
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
        stdout.contains("> Next: 1 more groups available. Continue with --offset 2 --limit 2"),
        "expected group pagination next-step, got:\n{stdout}"
    );
    assert_footers_are_trailing(&stdout);
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

#[test]
fn impl_filter_displays_impl_block_not_associated_type_child() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        r#"trait Matcher {
    type Captures;
    fn find(&self);
}
struct RegexMatcher;
impl Matcher for RegexMatcher {
    type Captures = ();
    fn find(&self) {}
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["Matcher", "--filter", "kind:impl", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "impl-filter search should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("[impl] impl Matcher for RegexMatcher lib.rs:6-9"),
        "expected impl block row, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("RegexMatcher.Captures"),
        "impl row should not be mislabeled as associated type child, got:\n{stdout}"
    );
}

#[test]
fn kind_impl_finds_java_class_implements_interface() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("A.java"),
        r#"interface Matcher {
    void find();
}
class RegexMatcher implements Matcher {
    public void find() {}
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["Matcher", "--filter", "kind:impl", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "Java kind:impl search should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("[impl] RegexMatcher implements Matcher A.java:4-6"),
        "expected Java class implements row, got:\n{stdout}"
    );
}

#[test]
fn kind_impl_finds_typescript_class_implements_interface() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.ts"),
        r#"interface Matcher { find(): void }
class RegexMatcher implements Matcher {
  find(): void {}
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["Matcher", "--filter", "kind:impl", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "TypeScript kind:impl search should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("[impl] RegexMatcher implements Matcher a.ts:2-4"),
        "expected TypeScript class implements row, got:\n{stdout}"
    );
}

#[test]
fn kind_base_finds_csharp_base_list_without_claiming_impl() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("A.cs"),
        r#"interface IMatcher { void Find(); }
class RegexMatcher : IMatcher { public void Find() {} }
"#,
    )
    .unwrap();

    let base = srcwalk()
        .args(["IMatcher", "--filter", "kind:base", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let base_stdout = String::from_utf8_lossy(&base.stdout);
    assert!(
        base.status.success(),
        "C# kind:base search should succeed, stderr:\n{}\nstdout:\n{base_stdout}",
        String::from_utf8_lossy(&base.stderr)
    );
    assert!(
        base_stdout.contains("[base] RegexMatcher : IMatcher A.cs:2-2"),
        "expected neutral C# base relationship row, got:\n{base_stdout}"
    );

    let imp = srcwalk()
        .args(["IMatcher", "--filter", "kind:impl", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let imp_stdout = String::from_utf8_lossy(&imp.stdout);
    assert!(
        imp.status.success(),
        "C# kind:impl search should succeed, stderr:\n{}\nstdout:\n{imp_stdout}",
        String::from_utf8_lossy(&imp.stderr)
    );
    assert!(
        imp_stdout.contains("0 matches"),
        "C# base-list relationship should not be labeled kind:impl, got:\n{imp_stdout}"
    );
}

#[test]
fn javascript_iifes_are_outline_definitions_and_call_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("iife.js");
    std::fs::write(
        &file,
        r#"function init() { return 1; }
function anonymousWork() { return 2; }
function arrowWork() { return 3; }
async function fetchData() { return 4; }
function step() { return 5; }
function assignedWork() { return 6; }

(function boot() {
  init();
})();

(function () {
  anonymousWork();
}());

(() => {
  arrowWork();
})();

(async function asyncBoot() {
  await fetchData();
})();

(function* genBoot() {
  yield step();
})();

const assigned = (function assignedIife() {
  assignedWork();
})();
"#,
    )
    .unwrap();

    let outline = srcwalk().arg(&file).output().unwrap();
    let outline_stdout = String::from_utf8_lossy(&outline.stdout);
    assert!(
        outline.status.success(),
        "outline should succeed, stderr:\n{}\nstdout:\n{outline_stdout}",
        String::from_utf8_lossy(&outline.stderr)
    );
    for expected in [
        "fn boot",
        "fn <iife@12>",
        "fn <iife@16>",
        "fn asyncBoot",
        "fn genBoot",
        "fn assignedIife",
    ] {
        assert!(
            outline_stdout.contains(expected),
            "expected outline to contain {expected}, got:\n{outline_stdout}"
        );
    }

    let find = srcwalk()
        .args(["assignedIife", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let find_stdout = String::from_utf8_lossy(&find.stdout);
    assert!(find.status.success(), "find failed:\n{find_stdout}");
    assert!(
        find_stdout.contains("1 definitions") && find_stdout.contains("[fn] assignedIife"),
        "expected assigned named IIFE as function definition, got:\n{find_stdout}"
    );

    let callees = srcwalk()
        .args(["boot", "--callees", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let callees_stdout = String::from_utf8_lossy(&callees.stdout);
    assert!(
        callees.status.success(),
        "callees failed:\n{callees_stdout}"
    );
    assert!(
        callees_stdout.contains("init") && callees_stdout.contains("function init()"),
        "expected named IIFE callees to resolve helper, got:\n{callees_stdout}"
    );

    for (target, caller) in [
        ("init", "boot"),
        ("anonymousWork", "<iife@12>"),
        ("arrowWork", "<iife@16>"),
        ("fetchData", "asyncBoot"),
        ("step", "genBoot"),
        ("assignedWork", "assignedIife"),
    ] {
        let callers = srcwalk()
            .args([target, "--callers", "--scope"])
            .arg(dir.path())
            .output()
            .unwrap();
        let callers_stdout = String::from_utf8_lossy(&callers.stdout);
        assert!(
            callers.status.success(),
            "callers for {target} failed:\n{callers_stdout}"
        );
        assert!(
            callers_stdout.contains(&format!("[fn] {caller}")),
            "expected {target} caller context {caller}, got:\n{callers_stdout}"
        );
    }
}

#[test]
fn javascript_assigned_arrows_are_definitions_for_callees_and_callers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("assigned.js"),
        r#"const helper = () => 1;
const boot = () => {
  helper();
};
const bootIife = (() => {
  helper();
})();
"#,
    )
    .unwrap();

    let find = srcwalk()
        .args(["boot", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let find_stdout = String::from_utf8_lossy(&find.stdout);
    assert!(find.status.success(), "find failed:\n{find_stdout}");
    assert!(
        find_stdout.contains("1 definitions") && find_stdout.contains("[var] boot"),
        "expected assigned arrow as variable definition without duplicates, got:\n{find_stdout}"
    );

    let callees = srcwalk()
        .args(["bootIife", "--callees", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let callees_stdout = String::from_utf8_lossy(&callees.stdout);
    assert!(
        callees.status.success(),
        "callees failed:\n{callees_stdout}"
    );
    assert!(
        callees_stdout.contains("helper"),
        "expected assigned arrow IIFE callees to include helper, got:\n{callees_stdout}"
    );

    let callers = srcwalk()
        .args(["helper", "--callers", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let callers_stdout = String::from_utf8_lossy(&callers.stdout);
    assert!(
        callers.status.success(),
        "callers failed:\n{callers_stdout}"
    );
    assert!(
        callers_stdout.contains("[fn] boot") && callers_stdout.contains("[fn] bootIife"),
        "expected assigned arrow contexts for helper callers, got:\n{callers_stdout}"
    );
}

#[test]
fn typescript_iifes_and_assigned_arrows_are_symbol_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("iife.ts");
    std::fs::write(
        &file,
        r#"function init(): number { return 1; }
function helper(): number { return 2; }

(function boot(flag: boolean): number {
  return flag ? init() : 0;
})(true);

const bootArrow = <T>(value: T): T => {
  helper();
  return value;
};

const bootIife = (<T>(value: T): T => {
  helper();
  return value;
})(123);
"#,
    )
    .unwrap();

    let outline = srcwalk().arg(&file).output().unwrap();
    let outline_stdout = String::from_utf8_lossy(&outline.stdout);
    assert!(
        outline.status.success(),
        "outline failed:\n{outline_stdout}"
    );
    assert!(
        outline_stdout.contains("fn boot") && outline_stdout.contains("fn bootIife"),
        "expected TypeScript IIFE contexts in outline, got:\n{outline_stdout}"
    );

    let callees = srcwalk()
        .args(["boot", "--callees", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let callees_stdout = String::from_utf8_lossy(&callees.stdout);
    assert!(
        callees.status.success(),
        "callees failed:\n{callees_stdout}"
    );
    assert!(
        callees_stdout.contains("init"),
        "expected TypeScript named IIFE callees to include init, got:\n{callees_stdout}"
    );

    let callers = srcwalk()
        .args(["helper", "--callers", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let callers_stdout = String::from_utf8_lossy(&callers.stdout);
    assert!(
        callers.status.success(),
        "callers failed:\n{callers_stdout}"
    );
    assert!(
        callers_stdout.contains("[fn] bootArrow") && callers_stdout.contains("[fn] bootIife"),
        "expected TypeScript arrow contexts for helper callers, got:\n{callers_stdout}"
    );
}

fn position_of(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("expected `{needle}` in output:\n{haystack}"))
}

#[test]
fn php_callers_rank_explicit_receiver_before_self_and_duplicate_calls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cache.php"),
        r#"<?php
class A {
    public function caller() {
        $this->getCacheDir();
        $this->getCacheDir();
    }
}
class B {
    public function build() {
        $this->environment->getCacheDir();
    }
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["callers", "getCacheDir", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "callers should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        position_of(&stdout, "B.build") < position_of(&stdout, "A.caller"),
        "explicit receiver should rank before self receiver, got:\n{stdout}"
    );
    assert!(
        stdout.matches("A.caller").count() >= 2,
        "fixture should keep duplicate callsites visible, got:\n{stdout}"
    );
}

#[test]
fn typescript_callers_rank_named_context_before_top_level() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("factory.ts"),
        "export function makeClient() { return {}; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("usage.ts"),
        r#"import { makeClient } from "./factory";
const bootClient = makeClient();
export function start() {
    return makeClient();
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["callers", "makeClient", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "callers should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        position_of(&stdout, "start") < position_of(&stdout, "<top-level>"),
        "named TS caller should rank before top-level call, got:\n{stdout}"
    );
}

#[test]
fn csharp_callers_rank_explicit_receiver_before_local_bare_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Program.cs"),
        r#"class Kernel {
    public void Flush() {}
}
class Runner {
    public void Flush() {}
    public void Local() {
        Flush();
    }
    public void Remote(Kernel kernel) {
        kernel.Flush();
    }
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["callers", "Flush", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "callers should succeed, stderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        position_of(&stdout, "Runner.Remote") < position_of(&stdout, "Runner.Local"),
        "explicit C# receiver should rank before bare local call, got:\n{stdout}"
    );
}
