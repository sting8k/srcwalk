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

fn write_access_fixture(dir: &Path) {
    write_file(
        &dir.join("src/script.c"),
        r#"typedef struct engine_s engine_t;

void mark_args(engine_t *e) {
    e->is_args = 1;
}

void regex_end(engine_t *e) {
    e->is_args = 0;
}

int copy_len(engine_t *e) {
    return e->is_args ? 2 : 1;
}

void copy_args(engine_t *e) {
    if (e->is_args) {
        use(e->is_args);
    }
}

void macro_case(engine_t *e) {
    USE_FIELD(is_args);
}
"#,
    );
}

#[test]
fn find_access_groups_member_reads_writes_resets_and_unknowns() {
    let dir = temp_repo("access_grouping");
    write_access_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "is_args", "--as", "access", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert!(
        normalized.starts_with("# Access: \"is_args\" in src — 6 hits"),
        "bad header:\n{stdout}"
    );
    assert!(
        stdout
            .contains("hits: total=6 shown=6 write=1 reset=1 read=3 unknown=1 files=1 functions=4"),
        "missing metrics:\n{stdout}"
    );
    assert!(
        stdout.contains("flags: --filter access:<write|reset|read|unknown>"),
        "missing filter flags:\n{stdout}"
    );
    assert!(
        normalized.contains("\n## src/script.c\n[write] 1"),
        "access output should group hits by file before listing access kinds:\n{stdout}"
    );
    assert!(
        stdout.contains("[write] 1"),
        "missing write group:\n{stdout}"
    );
    assert!(
        stdout.contains("mark_args | e->is_args = 1;"),
        "missing write hit:\n{stdout}"
    );
    assert!(
        stdout.contains("[reset] 1"),
        "missing reset group:\n{stdout}"
    );
    assert!(
        stdout.contains("regex_end | e->is_args = 0;"),
        "missing reset hit:\n{stdout}"
    );
    assert!(stdout.contains("[read] 3"), "missing read group:\n{stdout}");
    assert!(
        stdout.contains("copy_len | return e->is_args ? 2 : 1;"),
        "missing read hit:\n{stdout}"
    );
    assert!(
        stdout.contains("[unknown] 1"),
        "missing unknown group:\n{stdout}"
    );
    assert!(
        stdout.contains("macro_case | USE_FIELD(is_args);"),
        "missing unknown hit:\n{stdout}"
    );
    assert!(
        stdout.contains("confidence: structural syntax"),
        "missing structural confidence label:\n{stdout}"
    );
    assert!(
        normalized.contains("## functions (structural source-order summary)")
            && normalized.contains("\nsrc/script.c\n")
            && normalized.contains("  copy_args write=0 reset=0 read=2 unknown=0"),
        "missing grouped per-function lifecycle summary:\n{stdout}"
    );
    assert!(
        stdout.contains("## breadcrumbs (structural lexical order; not runtime order)"),
        "missing lifecycle breadcrumbs:\n{stdout}"
    );
    assert!(
        stdout.contains("- :16 read condition | if (e->is_args) {")
            && stdout.contains("- :17 read call_arg | use(e->is_args);"),
        "missing role-labelled breadcrumbs:\n{stdout}"
    );
    assert!(
        stdout.contains("mark_args | e->is_args = 1; [assignment_lhs]"),
        "missing role label in legacy access listing:\n{stdout}"
    );
    assert!(
        stdout.contains("Caveat: syntax-level access grouping; lexical breadcrumbs are not runtime order, type proof, alias proof, or security proof"),
        "missing caveat:\n{stdout}"
    );
    assert!(
        stdout.contains("Next: srcwalk context src/script.c:3-5"),
        "missing exact context footer:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("Next: srcwalk context src/script.c:3-5")
            .count(),
        1,
        "access context footer should be deduplicated:\n{stdout}"
    );
    assert!(
        !stdout.contains("> Next: rg"),
        "footer should not suggest rg:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn access_breadcrumbs_group_each_function_once_with_nested_functions() {
    let dir = temp_repo("access_nested_breadcrumbs");
    write_file(
        &dir.join("src/mod.py"),
        r#"class State:
    def audit(self):
        self.flag = True
        def inner():
            return self.flag
        if self.flag:
            inner()
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "flag", "--as", "access", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert_eq!(
        normalized.matches("\nsrc/mod.py:audit\n").count(),
        1,
        "outer function breadcrumb group should appear once even with nested hits:\n{stdout}"
    );
    assert!(
        normalized.contains("\nsrc/mod.py:inner\n"),
        "nested function breadcrumb group should be present:\n{stdout}"
    );
    assert!(
        normalized.contains("- :3 write assignment_lhs | self.flag = True")
            && normalized.contains("- :6 read condition | if self.flag:"),
        "outer group should keep both outer events:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn access_lifecycle_summary_uses_full_result_set_when_paginated() {
    let dir = temp_repo("access_lifecycle_pagination");
    write_access_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "is_args", "--as", "access", "--scope", "src", "--limit", "1",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert!(
        stdout.contains("hits: total=6 shown=1 write=1 reset=1 read=3 unknown=1"),
        "header metrics should stay full-result with shown page count:\n{stdout}"
    );
    assert!(
        normalized.contains("\nsrc/script.c\n")
            && normalized.contains("  copy_args write=0 reset=0 read=2 unknown=0"),
        "function summary should include full-result functions outside the current page:\n{stdout}"
    );
    assert!(
        stdout.contains("events: shown=1 total=5 page=current"),
        "breadcrumbs should say they are current-page events with full structural total:\n{stdout}"
    );
    assert!(
        !stdout.contains("- :16 copy_args | if (e->is_args)"),
        "legacy detailed listing should still respect --limit pagination:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_access_accepts_exact_file_scope() {
    let dir = temp_repo("access_file_scope");
    write_access_fixture(&dir);
    write_file(
        &dir.join("src/other.c"),
        r#"typedef struct engine_s engine_t;

void other_write(engine_t *e) {
    e->is_args = 1;
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "is_args",
            "--as",
            "access",
            "--scope",
            "src/script.c",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Access: \"is_args\" in src/script.c — 6 hits"),
        "file scope should use exact file and preserve existing hits:\n{stdout}"
    );
    assert!(
        !stdout.contains("other_write"),
        "file scope should not scan sibling files:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_access_accepts_exact_file_range_scope() {
    let dir = temp_repo("access_file_range_scope");
    write_access_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "is_args",
            "--as",
            "access",
            "--scope",
            "src/script.c:7-13",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);
    assert!(
        normalized.starts_with("# Access: \"is_args\" in src/script.c — 2 hits"),
        "range scope should narrow access hits before counts:\n{stdout}"
    );
    assert!(
        stdout.contains("hits: total=2 shown=2 write=0 reset=1 read=1 unknown=0"),
        "{stdout}"
    );
    assert!(stdout.contains("regex_end | e->is_args = 0;"), "{stdout}");
    assert!(
        stdout.contains("copy_len | return e->is_args ? 2 : 1;"),
        "{stdout}"
    );
    assert!(!stdout.contains("mark_args | e->is_args = 1;"), "{stdout}");
    assert!(!stdout.contains("copy_args | if (e->is_args)"), "{stdout}");
    assert!(
        !stdout.contains("macro_case | USE_FIELD(is_args);"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_access_accepts_glob_scope() {
    let dir = temp_repo("access_glob_scope");
    write_file(
        &dir.join("src/one.c"),
        r#"typedef struct engine_s engine_t;

void one_write(engine_t *e) {
    e->is_args = 1;
}
"#,
    );
    write_file(
        &dir.join("src/two.h"),
        r#"typedef struct engine_s engine_t;

void two_write(engine_t *e) {
    e->is_args = 1;
}
"#,
    );
    write_file(
        &dir.join("src/nested/three.c"),
        r#"typedef struct engine_s engine_t;

void three_write(engine_t *e) {
    e->is_args = 1;
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "is_args", "--as", "access", "--scope", "src/*.c",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("one_write"),
        "glob should include matching .c file:\n{stdout}"
    );
    assert!(
        !stdout.contains("two_write"),
        "glob scope should not include non-matching extension:\n{stdout}"
    );
    assert!(
        !stdout.contains("three_write"),
        "anchored glob scope should not include nested files for src/*.c:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_filter_narrows_by_access_kind() {
    let dir = temp_repo("access_filter");
    write_access_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "is_args",
            "--as",
            "access",
            "--filter",
            "access:reset",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hits: total=1 shown=1 write=0 reset=1 read=0 unknown=0"),
        "filter should update metrics:\n{stdout}"
    );
    assert!(
        stdout.contains("[reset] 1"),
        "missing reset group:\n{stdout}"
    );
    assert!(
        stdout.contains("regex_end | e->is_args = 0;"),
        "missing reset hit:\n{stdout}"
    );
    assert!(
        !stdout.contains("mark_args | e->is_args = 1;"),
        "write hit leaked:\n{stdout}"
    );
    assert!(
        !stdout.contains("copy_len | return e->is_args"),
        "read hit leaked:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_classifies_go_assignment_and_increment_writes() {
    let dir = temp_repo("access_go_writes");
    write_file(
        &dir.join("src/context.go"),
        r#"package src

type Context struct {
    index int
    handlers []func()
}

func reset(c *Context) {
    c.index = -1
    c.handlers = nil
}

func next(c *Context) {
    c.index++
    if c.index < len(c.handlers) {
        c.handlers[c.index]()
    }
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "index", "--as", "access", "--scope", "src"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("write=2 reset=0 read=1 unknown=2"),
        "expected Go assignments/increments as writes, got:\n{stdout}"
    );
    assert!(
        stdout.contains("reset | c.index = -1") && stdout.contains("next | c.index++"),
        "missing Go write hits:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_classifies_nested_lhs_member_writes() {
    let dir = temp_repo("access_nested_lhs");
    write_file(
        &dir.join("src/utils.js"),
        r#"function acceptParams(ret, key, value) {
  ret.params[key] = value;
  return ret.params[key];
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "params",
            "--as",
            "access",
            "--filter",
            "access:write",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hits: total=1 shown=1 write=1 reset=0 read=0 unknown=0"),
        "expected nested LHS member as write, got:\n{stdout}"
    );
    assert!(
        stdout.contains("acceptParams | ret.params[key] = value;"),
        "missing nested LHS write:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_does_not_mark_lhs_index_reads_as_writes() {
    let dir = temp_repo("access_lhs_index_read");
    write_file(
        &dir.join("src/main.cpp"),
        r#"struct Item { int size; };
struct Vec { int size(); int& operator[](int); };

void audit(Vec& thelist, Item& item) {
    thelist[thelist.size() - 1] = 7;
    item.size = 1;
}
"#,
    );

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "size", "--as", "access", "--scope", "src", "--glob", "*.cpp",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("write=1 reset=0 read=1 unknown=2"),
        "method call in LHS index should be read, not write:\n{stdout}"
    );
    assert!(
        stdout.contains("audit | thelist[thelist.size() - 1] = 7;")
            && stdout.contains("audit | item.size = 1;"),
        "missing expected C++ access hits:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_smoke_matrix_for_supported_parser_languages() {
    let dir = temp_repo("access_language_matrix");
    struct Case {
        name: &'static str,
        ext: &'static str,
        body: &'static str,
        metrics: &'static str,
    }
    let cases = [
        Case {
            name: "rust",
            ext: "rs",
            body: "struct State { flag: bool }\nfn audit(s: &mut State) {\n    s.flag = true;\n    s.flag = false;\n    if s.flag { call(); }\n}\nfn call() {}\n",
            metrics: "write=1 reset=1 read=1 unknown=1",
        },
        Case {
            name: "typescript",
            ext: "ts",
            body: "class State { flag = false; audit() { this.flag = true; this.flag = false; if (this.flag) call(); } }\nfunction call() {}\n",
            metrics: "write=1 reset=1 read=1 unknown=1",
        },
        Case {
            name: "tsx",
            ext: "tsx",
            body: "class State { flag = false; audit() { this.flag = true; this.flag = false; if (this.flag) call(); return <div>{this.flag}</div>; } }\nfunction call() { return null; }\n",
            metrics: "write=1 reset=1 read=2 unknown=1",
        },
        Case {
            name: "javascript",
            ext: "js",
            body: "class State { constructor(){ this.flag = false; } audit(){ this.flag = true; this.flag = false; if (this.flag) call(); } }\nfunction call() {}\n",
            metrics: "write=1 reset=2 read=1 unknown=0",
        },
        Case {
            name: "python",
            ext: "py",
            body: "class State:\n    def audit(self):\n        self.flag = True\n        self.flag = False\n        if self.flag:\n            call()\ndef call(): pass\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "go",
            ext: "go",
            body: "package main\ntype State struct { flag bool }\nfunc audit(s *State) {\n    s.flag = true\n    s.flag = false\n    if s.flag { call() }\n}\nfunc call() {}\n",
            metrics: "write=1 reset=1 read=1 unknown=1",
        },
        Case {
            name: "java",
            ext: "java",
            body: "class State { boolean flag; void audit(){ this.flag = true; this.flag = false; if (this.flag) call(); } void call() {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "scala",
            ext: "scala",
            body: "class State { var flag = false; def audit(): Unit = { this.flag = true; this.flag = false; if (this.flag) call() }; def call(): Unit = {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "c",
            ext: "c",
            body: "typedef struct { int flag; } State;\nvoid audit(State *s) { s->flag = 1; s->flag = 0; if (s->flag) call(); }\nvoid call(void) {}\n",
            metrics: "write=1 reset=1 read=1 unknown=1",
        },
        Case {
            name: "cpp",
            ext: "cpp",
            body: "struct State { bool flag; };\nvoid call();\nvoid audit(State &s) { s.flag = true; s.flag = false; if (s.flag) call(); }\n",
            metrics: "write=1 reset=1 read=1 unknown=1",
        },
        Case {
            name: "ruby",
            ext: "rb",
            body: "class State\n  def audit\n    @flag = true\n    @flag = false\n    call if @flag\n  end\n  def call; end\nend\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "php",
            ext: "php",
            body: "<?php\nclass State { public bool $flag = false; function audit() { $this->flag = true; $this->flag = false; if ($this->flag) { $this->call(); } } function call() {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "csharp",
            ext: "cs",
            body: "class State { bool flag; void Audit(){ this.flag = true; this.flag = false; if (this.flag) Call(); } void Call() {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "swift",
            ext: "swift",
            body: "class State { var flag = false; func audit() { self.flag = true; self.flag = false; if self.flag { call() } } func call() {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "kotlin",
            ext: "kt",
            body: "class State { var flag = false; fun audit() { this.flag = true; this.flag = false; if (this.flag) call() } fun call() {} }\n",
            metrics: "write=1 reset=1 read=1 unknown=0",
        },
        Case {
            name: "elixir",
            ext: "ex",
            body: "defmodule State do\n  def audit(state) do\n    state = %{state | flag: true}\n    state = %{state | flag: false}\n    if state.flag, do: call()\n  end\n  def call(), do: :ok\nend\n",
            metrics: "write=0 reset=0 read=1 unknown=2",
        },
    ];

    for case in cases {
        let scope = dir.join(case.name);
        write_file(&scope.join(format!("main.{}", case.ext)), case.body);
        let out = srcwalk()
            .current_dir(&dir)
            .args([
                "discover",
                "flag",
                "--as",
                "access",
                "--scope",
                case.name,
                "--glob",
                &format!("*.{}", case.ext),
                "--limit",
                "20",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} stderr:\n{}",
            case.name,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(case.metrics),
            "{} expected metrics `{}`, got:\n{}",
            case.name,
            case.metrics,
            stdout
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_groups_tsx_react_member_reads_and_degrades_destructured_props() {
    let dir = temp_repo("access_tsx_react_component");
    write_file(
        &dir.join("src/UserCard.tsx"),
        r#"type User = { id: string; name: string; enabled: boolean };
type Props = { user: User; onSave: (id: string) => void };

export function UserCard({ user, onSave }: Props) {
  const label = user.name.trim();
  const disabled = !user.enabled;
  return (
    <button disabled={disabled} onClick={() => onSave(user.id)}>
      {label}
    </button>
  );
}
"#,
    );

    let run_access = |query: &str| -> String {
        let out = srcwalk()
            .current_dir(&dir)
            .args([
                "discover", query, "--as", "access", "--scope", "src", "--glob", "*.tsx",
                "--limit", "20",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "query {query} stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        norm_path_separators(&String::from_utf8_lossy(&out.stdout))
    };

    let name = run_access("name");
    assert!(
        name.contains("write=0 reset=0 read=1 unknown=1")
            && name.contains("read receiver")
            && name.contains("const label = user.name.trim();"),
        "TSX member method receiver should be read evidence:\n{name}"
    );

    let enabled = run_access("enabled");
    assert!(
        enabled.contains("write=0 reset=0 read=1 unknown=1")
            && enabled.contains("read initializer")
            && enabled.contains("const disabled = !user.enabled;"),
        "TSX JSX prop source should be initializer read evidence:\n{enabled}"
    );

    let id = run_access("id");
    assert!(
        id.contains("write=0 reset=0 read=1 unknown=2")
            && id.contains("read call_arg")
            && id.contains("onClick={() => onSave(user.id)}"),
        "TSX JSX callback argument should be call_arg read evidence:\n{id}"
    );

    let on_save = run_access("onSave");
    assert!(
        on_save.contains("write=0 reset=0 read=0 unknown=3")
            && !on_save.contains("read call_arg"),
        "destructured callback identifiers should stay unknown instead of overclaiming member access:\n{on_save}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_groups_java_and_csharp_member_chains_without_overclaiming_method_names() {
    let dir = temp_repo("access_java_csharp_member_chains");
    write_file(
        &dir.join("src/OrderService.java"),
        r#"class Order { Customer customer; boolean enabled; String id; }
class Customer { String name; }
class OrderService {
  String label(Order order) {
    String name = order.customer.name.trim();
    if (order.enabled) {
      return format(name, order.id);
    }
    return name;
  }
  String format(String name, String id) { return name + id; }
}
"#,
    );
    write_file(
        &dir.join("src/OrderService.cs"),
        r#"class Order { public Customer Customer { get; set; } public bool Enabled { get; set; } public string Id { get; set; } }
class Customer { public string Name { get; set; } }
class OrderService {
  string Label(Order order) {
    var name = order.Customer.Name.Trim();
    if (order.Enabled) {
      return Format(name, order.Id);
    }
    return name;
  }
  string Format(string name, string id) { return name + id; }
}
"#,
    );

    let run_access = |query: &str, glob: &str| -> String {
        let out = srcwalk()
            .current_dir(&dir)
            .args([
                "discover", query, "--as", "access", "--scope", "src", "--glob", glob, "--limit",
                "20",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "query {query} glob {glob} stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        norm_path_separators(&String::from_utf8_lossy(&out.stdout))
    };

    let java_customer = run_access("customer", "*.java");
    assert!(
        java_customer.contains("read receiver")
            && java_customer.contains("String name = order.customer.name.trim();"),
        "Java member chain receiver should be read evidence:\n{java_customer}"
    );
    let java_enabled = run_access("enabled", "*.java");
    assert!(
        java_enabled.contains("read condition") && java_enabled.contains("if (order.enabled)"),
        "Java boolean field should be condition read evidence:\n{java_enabled}"
    );
    let java_id = run_access("id", "*.java");
    assert!(
        java_id.contains("read call_arg") && java_id.contains("return format(name, order.id);"),
        "Java field passed to local call should be call_arg read evidence:\n{java_id}"
    );
    let java_format = run_access("format", "*.java");
    assert!(
        java_format.contains("write=0 reset=0 read=0 unknown=2")
            && !java_format.contains("read call_arg"),
        "Java method identifiers should remain unknown access evidence, not member reads:\n{java_format}"
    );

    let csharp_customer = run_access("Customer", "*.cs");
    assert!(
        csharp_customer.contains("read receiver")
            && csharp_customer.contains("var name = order.Customer.Name.Trim();"),
        "C# property chain receiver should be read evidence:\n{csharp_customer}"
    );
    let csharp_enabled = run_access("Enabled", "*.cs");
    assert!(
        csharp_enabled.contains("read condition") && csharp_enabled.contains("if (order.Enabled)"),
        "C# property should be condition read evidence:\n{csharp_enabled}"
    );
    let csharp_id = run_access("Id", "*.cs");
    assert!(
        csharp_id.contains("read call_arg") && csharp_id.contains("return Format(name, order.Id);"),
        "C# property passed to local call should be call_arg read evidence:\n{csharp_id}"
    );
    let csharp_format = run_access("Format", "*.cs");
    assert!(
        csharp_format.contains("write=0 reset=0 read=0 unknown=2")
            && !csharp_format.contains("read call_arg"),
        "C# method identifiers should remain unknown access evidence, not member reads:\n{csharp_format}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn find_access_keeps_non_member_languages_unknown() {
    let dir = temp_repo("access_non_member_unknown");
    write_file(&dir.join("style.css"), ".card { color: red; }\n");
    write_file(&dir.join("Makefile"), "flag = true\nall:\n\techo $(flag)\n");

    let css = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "color", "--as", "access", "--scope", ".", "--glob", "*.css",
        ])
        .output()
        .unwrap();
    assert!(css.status.success());
    let css_stdout = String::from_utf8_lossy(&css.stdout);
    assert!(
        css_stdout.contains("write=0 reset=0 read=0 unknown=1"),
        "CSS property should stay unknown access evidence:\n{css_stdout}"
    );

    let make = srcwalk()
        .current_dir(&dir)
        .args([
            "discover", "flag", "--as", "access", "--scope", ".", "--glob", "Makefile",
        ])
        .output()
        .unwrap();
    assert!(make.status.success());
    let make_stdout = String::from_utf8_lossy(&make.stdout);
    assert!(
        make_stdout.contains("write=0 reset=0 read=0 unknown=2"),
        "Make variables should stay unknown access evidence:\n{make_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn access_filter_requires_access_mode() {
    let dir = temp_repo("access_filter_requires_mode");
    write_access_fixture(&dir);

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "is_args",
            "--filter",
            "access:write",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "expected filter without discover --as access to fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("filter qualifier `access` only applies with discover --as access"),
        "expected access filter diagnostic, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
