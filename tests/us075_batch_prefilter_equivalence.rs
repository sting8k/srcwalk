//! US-075: batch prefilter equivalence.
//!
//! Batching supported symbol queries must not remove or downgrade definition
//! evidence that the same queries find individually. The regressions here cover
//! the two prefilter defects: a receiver/container-qualified term whose scan
//! needle differs from its literal spelling, and overlapping term names where a
//! shorter term previously masked a longer one.

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
            "us075_{name}_{}_{}",
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

    fn discover(&self, args: &[&str]) -> String {
        let out = srcwalk()
            .current_dir(&self.dir)
            .args(["discover"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "discover {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Per-query section of a batch packet, matched by query label rather than by
/// presentation order.
fn section<'a>(out: &'a str, query: &str) -> &'a str {
    let header = format!("# Search: \"{query}\"");
    let start = out
        .find(&header)
        .unwrap_or_else(|| panic!("no section for '{query}' in:\n{out}"));
    let rest = &out[start..];
    match rest[header.len()..].find("\n---\n") {
        Some(end) => &rest[..header.len() + end],
        None => rest,
    }
}

const GO_FILES: &[(&str, &str)] = &[(
    "sample.go",
    "package sample\n\ntype Batch struct{}\n\nfunc (b *Batch) Set(v int) {}\n\ntype Other struct{}\n\nfunc (o *Other) Set(v int) {}\n\nfunc helper() {}\n",
)];

// The long name lives in a file that does not contain the short name, so a
// masked needle cannot be rescued by the sibling query admitting the file.
const OVERLAP_FILES: &[(&str, &str)] = &[
    ("only_extra.rs", "pub fn helper_extra() -> u8 {\n    1\n}\n"),
    ("short.rs", "pub fn helper() -> u8 {\n    2\n}\n"),
];

#[test]
fn qualified_term_keeps_its_definition_inside_a_batch() {
    let fx = Fixture::new("qualified", GO_FILES);
    let single = fx.discover(&["Batch.Set", "--as", "symbol"]);
    assert!(single.contains("[fn] Batch.Set sample.go:5-5"), "{single}");

    for args in [
        ["Batch.Set,helper", "--as", "symbol"],
        ["helper,Batch.Set", "--as", "symbol"],
    ] {
        let out = fx.discover(&args);
        let qualified = section(&out, "Batch.Set");
        assert!(
            qualified.contains("1 matches (1 definitions)"),
            "{args:?}\n{qualified}"
        );
        assert!(
            qualified.contains("[fn] Batch.Set sample.go:5-5"),
            "{args:?}\n{qualified}"
        );
        assert!(
            qualified.contains("confidence: structural syntax"),
            "{args:?}\n{qualified}"
        );
        assert!(
            section(&out, "helper").contains("[fn] helper sample.go:11-11"),
            "{args:?}\n{out}"
        );
    }
}

#[test]
fn overlapping_names_keep_their_definitions_inside_a_batch() {
    let fx = Fixture::new("overlap", OVERLAP_FILES);

    for args in [
        ["helper,helper_extra", "--as", "symbol"],
        ["helper_extra,helper", "--as", "symbol"],
    ] {
        let out = fx.discover(&args);
        let long = section(&out, "helper_extra");
        assert!(
            long.contains("[fn] helper_extra only_extra.rs:1-3"),
            "{args:?}\n{long}"
        );
        assert!(
            long.contains("source: ast · kind: definition"),
            "{args:?}\n{long}"
        );
        let short = section(&out, "helper");
        assert!(
            short.contains("[fn] helper short.rs:1-3"),
            "{args:?}\n{short}"
        );
        // A prefix hit must not fabricate a definition inside the longer name.
        assert!(
            !short.contains("only_extra.rs:1-3\n  source: ast"),
            "{args:?}\n{short}"
        );
    }
}

#[test]
fn shared_scan_needle_keeps_each_qualifier_and_its_own_definitions() {
    let fx = Fixture::new("shared_needle", GO_FILES);

    // Maximum accepted batch size, mixing qualified, plain and absent terms.
    let out = fx.discover(&["Batch.Set,Other.Set,Set,helper,Nope.Set", "--as", "symbol"]);

    let batch_set = section(&out, "Batch.Set");
    assert!(
        batch_set.contains("1 matches (1 definitions)"),
        "{batch_set}"
    );
    assert!(
        batch_set.contains("[fn] Batch.Set sample.go:5-5"),
        "{batch_set}"
    );

    let other_set = section(&out, "Other.Set");
    assert!(
        other_set.contains("1 matches (1 definitions)"),
        "{other_set}"
    );
    assert!(
        other_set.contains("[fn] Other.Set sample.go:9-9"),
        "{other_set}"
    );

    let plain = section(&out, "Set");
    assert!(plain.contains("2 definitions"), "{plain}");

    // A wrong qualifier must not acquire a definition just because sibling
    // queries admitted the same file.
    let absent = section(&out, "Nope.Set");
    assert!(absent.contains("0 matches"), "{absent}");
}

#[test]
fn exact_dotted_outline_name_still_wins_inside_a_batch() {
    let fx = Fixture::new(
        "elixir_dotted",
        &[(
            "app.ex",
            "defmodule Foo.Bar do\n  def hello do\n    :world\n  end\nend\n",
        )],
    );
    let out = fx.discover(&["Foo.Bar,hello", "--as", "symbol"]);

    let dotted = section(&out, "Foo.Bar");
    assert!(dotted.contains("1 matches (1 definitions)"), "{dotted}");
    assert!(
        dotted.contains("[definition] Foo.Bar app.ex:1-5") && !dotted.contains("[fn] Foo.Bar"),
        "exact dotted-name match must win inside a batch, not a qualified fn:\n{dotted}"
    );
    assert!(
        section(&out, "hello").contains("[fn] Foo.Bar.hello app.ex:2-4"),
        "{out}"
    );
}
