//! Symbol-addressed confirmed-structural-target footer + qualified `--section`
//! resolution.
//!
//! The discover footer points to `srcwalk show <path> --section <symbol>`
//! (bare or `Type.method`) instead of a numeric range, so models read the
//! parser-backed symbol body rather than guessing numeric ranges. The
//! `--section` reader resolves `Q.N` receiver/container-qualified symbols
//! matching US-064 discover semantics.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "us065_footer_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).unwrap();
        for (rel, content) in files {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        Self { dir }
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = srcwalk()
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr),
        )
    }

    fn discover(&self, args: &[&str]) -> String {
        let (ok, out) = self.run(args);
        assert!(ok, "command {:?} failed:\n{out}", args);
        out
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------- footer A

#[test]
fn discover_bare_function_emits_symbol_section_command() {
    let fx = Fixture::new(
        "bare_fn",
        &[(
            "lib.rs",
            "fn target() -> i32 {\n    helper()\n}\n\nfn helper() -> i32 { 1 }\n",
        )],
    );
    let out = fx.discover(&["discover", "target", "--scope", "."]);
    assert!(out.contains("## Confirmed structural targets"), "{out}");
    assert!(
        out.contains("> Next: srcwalk show lib.rs --section target"),
        "{out}"
    );
    assert!(
        !out.contains("srcwalk show 'lib.rs:1-3'") && !out.contains("srcwalk show lib.rs:1-3"),
        "no numeric read command should be emitted:\n{out}"
    );
    // The confirmed block owns the next action; no generic guidance may
    // follow it as a competing primary `> Next:`.
    assert!(
        !out.contains("read the confirmed structural target above"),
        "generic guidance must not follow a confirmed-target block:\n{out}"
    );
}

#[test]
fn discover_qualified_preserves_qualified_selector() {
    let fx = Fixture::new(
        "qualified",
        &[(
            "batch.go",
            "package db\n\ntype Batch struct{}\n\nfunc (b *Batch) Set(v int) {}\n",
        )],
    );
    let out = fx.discover(&["discover", "Batch.Set", "--as", "symbol", "--scope", "."]);
    assert!(
        out.contains("> Next: srcwalk show batch.go --section Batch.Set"),
        "{out}"
    );
}

#[test]
fn ambiguous_bare_name_falls_back_to_numeric_for_unresolvable_target() {
    let fx = Fixture::new(
        "ambiguous",
        &[(
            "lib.rs",
            "fn helper() -> i32 { 1 }\nfn other() -> i32 { 2 }\nfn helper() -> i32 { 3 }\n",
        )],
    );
    let out = fx.discover(&["discover", "helper", "--scope", "."]);
    // The first `helper` round-trips via the bare symbol; the second same-name
    // `helper` cannot round-trip (the bare selector reads the first), so it
    // must fall back to a numeric range rather than point at the wrong body.
    assert!(
        out.contains("> Next: srcwalk show lib.rs --section helper"),
        "{out}"
    );
    assert!(out.contains("> Next: srcwalk show lib.rs:3-3"), "{out}");
}

#[test]
fn discover_quotes_space_and_comma_paths_in_symbol_command() {
    let fx = Fixture::new("quoted", &[("a file.rs", "fn target() -> i32 { 1 }\n")]);
    let out = fx.discover(&["discover", "target", "--scope", "."]);
    assert!(
        out.contains("> Next: srcwalk show 'a file.rs' --section target"),
        "{out}"
    );

    fs::remove_file(fx.dir.join("a file.rs")).unwrap();
    fs::write(fx.dir.join("a,file.rs"), "fn target() -> i32 { 1 }\n").unwrap();
    let out = fx.discover(&["discover", "target", "--scope", "."]);
    assert!(
        out.contains("> Next: srcwalk show 'a,file.rs' --section target"),
        "comma path must stay quoted and single-target:\n{out}"
    );
}

#[test]
fn non_structural_search_keeps_numeric_exact_hit_guidance() {
    let fx = Fixture::new(
        "text_hit",
        &[("lib.rs", "fn main() {\n    // nothing structural here\n}\n")],
    );
    // A plain text search yields no confirmed structural target, so the
    // numeric exact-hit guidance must remain (not be replaced by nothing).
    let out = fx.discover(&["discover", "nothing", "--as", "text", "--scope", "."]);
    assert!(!out.contains("## Confirmed structural targets"), "{out}");
    assert!(
        out.contains("read exact hit evidence with `srcwalk show <path>:<line> -C 10`"),
        "non-structural search must keep numeric exact-hit guidance:\n{out}"
    );
}

// ------------------------------------------------------- section resolution B

#[test]
fn rust_impl_qualified_section_reads_correct_method() {
    let fx = Fixture::new(
        "rust_impl",
        &[(
            "service.rs",
            "pub struct Service;\nimpl Service {\n    pub fn run(&self) {}\n    pub fn stop(&self) {}\n}\n",
        )],
    );
    let (ok, out) = fx.run(&["show", "service.rs", "--section", "Service.run"]);
    assert!(ok, "{out}");
    assert!(out.contains("pub fn run(&self)"), "{out}");
    assert!(!out.contains("pub fn stop"), "wrong method body:\n{out}");

    // Bare method still works.
    let (ok, out) = fx.run(&["show", "service.rs", "--section", "stop"]);
    assert!(ok, "{out}");
    assert!(out.contains("pub fn stop(&self)"), "{out}");
}

#[test]
fn nested_classes_same_method_qualified_selects_correct_and_wrong_fails() {
    let fx = Fixture::new(
        "py_two_classes",
        &[(
            "config.py",
            "class Alpha:\n    def run(self):\n        return 1\n\nclass Beta:\n    def run(self):\n        return 2\n",
        )],
    );
    let (ok, out) = fx.run(&["show", "config.py", "--section", "Alpha.run"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("return 1"),
        "Alpha.run must select Alpha's body:\n{out}"
    );
    assert!(!out.contains("return 2"), "wrong container body:\n{out}");

    let (ok, out) = fx.run(&["show", "config.py", "--section", "Beta.run"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("return 2"),
        "Beta.run must select Beta's body:\n{out}"
    );

    // Wrong qualifier is rejected.
    let (ok, _) = fx.run(&["show", "config.py", "--section", "Nope.run"]);
    assert!(!ok, "wrong qualifier must not silently resolve");
}

#[test]
fn go_receiver_qualified_section_reads_pointer_value_generic() {
    let fx = Fixture::new(
        "go_receivers",
        &[(
            "db/batch.go",
            "package db\n\ntype Batch struct{}\ntype syncQueue[T any] struct{}\n\nfunc (b *Batch) Ptr() {}\nfunc (b Batch) Val() {}\nfunc (q *syncQueue[T]) Gen() {}\n",
        )],
    );
    for (selector, body) in [
        ("Batch.Ptr", "func (b *Batch) Ptr()"),
        ("Batch.Val", "func (b Batch) Val()"),
        ("syncQueue.Gen", "func (q *syncQueue[T]) Gen()"),
    ] {
        let (ok, out) = fx.run(&["show", "db/batch.go", "--section", selector]);
        assert!(ok, "selector {selector} failed:\n{out}");
        assert!(out.contains(body), "selector {selector} wrong body:\n{out}");
    }
}

#[test]
fn exact_dotted_outline_name_wins_before_qualified_interpretation() {
    let fx = Fixture::new(
        "elixir_dotted",
        &[(
            "app.ex",
            "defmodule Foo.Bar do\n  def hello do\n    :world\n  end\nend\n",
        )],
    );
    // `Foo.Bar` is a real dotted module name; it must resolve as the module,
    // not as qualifier `Foo` + member `Bar`.
    let (ok, out) = fx.run(&["show", "app.ex", "--section", "Foo.Bar"]);
    assert!(ok, "{out}");
    assert!(out.contains("defmodule Foo.Bar"), "{out}");
}

#[test]
fn multi_dot_and_empty_side_selectors_reject_without_exact_match() {
    let fx = Fixture::new(
        "invalid_dots",
        &[(
            "app.ex",
            "defmodule Foo.Bar do\n  def hello do\n    :world\n  end\nend\n",
        )],
    );
    // Multi-dot is not a qualified symbol (no exact dotted name matches).
    let (ok, _) = fx.run(&["show", "app.ex", "--section", "Foo.Bar.hello"]);
    assert!(!ok, "multi-dot must not resolve as a qualified symbol");
    // Emptydot side is invalid.
    let (ok, _) = fx.run(&["show", "app.ex", "--section", ".Bar"]);
    assert!(!ok, "leading-dot must not resolve");
}
