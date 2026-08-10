//! US-064: receiver-qualified symbol queries.
//!
//! `discover 'Q.N' --as symbol` resolves the method `N` defined on receiver /
//! inside container `Q` (Go receiver parsing + structural containment),
//! additively on top of existing exact-name matching. Also verifies the
//! reverse Qualify assist, the rule-5 wrong-qualifier recovery, and that
//! multi-dot / dotfile / extension queries and dotted outline names stay
//! byte-identical.

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
            "us064_{name}_{}_{}",
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

    fn discover_raw(&self, args: &[&str]) -> (bool, String) {
        let out = srcwalk()
            .current_dir(&self.dir)
            .args(["discover"])
            .args(args)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).into_owned()
                + &String::from_utf8_lossy(&out.stdout),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const GO_FILES: &[(&str, &str)] = &[
    (
        "db/batch.go",
        "package db\n\ntype Batch struct{ n int }\ntype Set struct{ m int }\n\n// Set adds a key to the batch.\nfunc (b *Batch) Set(key, value []byte) error { return nil }\n\n// Set on generic queue\nfunc (q *syncQueue[T]) Set(v T) {}\n\nfunc Set(x int) int { return x }\n",
    ),
    (
        "db/queue.go",
        "package db\n\ntype syncQueue[T any] struct{}\n\nfunc (q syncQueue[T]) pop() T { var z T; return z }\n",
    ),
];

#[test]
fn go_pointer_receiver_qualified_match() {
    let fx = Fixture::new("go_pointer", GO_FILES);
    let out = fx.discover(&["Batch.Set", "--as", "symbol"]);
    assert!(out.contains("1 matches (1 definitions)"), "{out}");
    assert!(out.contains("[fn] Batch.Set db/batch.go:7-7"), "{out}");
    assert!(out.contains("confidence: structural syntax"), "{out}");
}

#[test]
fn go_generic_receiver_qualified_match() {
    let fx = Fixture::new("go_generic", GO_FILES);
    let out = fx.discover(&["syncQueue.Set", "--as", "symbol"]);
    assert!(out.contains("1 matches (1 definitions)"), "{out}");
    assert!(
        out.contains("[fn] syncQueue.Set db/batch.go:10-10"),
        "{out}"
    );
}

#[test]
fn go_value_receiver_qualified_match() {
    let fx = Fixture::new("go_value", GO_FILES);
    let out = fx.discover(&["syncQueue.pop", "--as", "symbol"]);
    assert!(out.contains("1 matches (1 definitions)"), "{out}");
    assert!(out.contains("[fn] syncQueue.pop db/queue.go:5-5"), "{out}");
}

#[test]
fn plain_name_query_adds_qualify_assist() {
    let fx = Fixture::new("go_plain", GO_FILES);
    let out = fx.discover(&["Set", "--as", "symbol"]);
    // All three definitions still present, in the same relative order.
    assert!(out.contains("3 definitions"), "{out}");
    assert!(out.contains("[fn] Set :7-7"), "{out}");
    assert!(out.contains("[fn] Set :10-10"), "{out}");
    assert!(out.contains("[fn] Set :12-12"), "{out}");
    // Qualify assist lists the KNOWN distinct qualifiers, deterministically.
    assert!(
        out.contains("> Qualify: 'Batch.Set' | 'syncQueue.Set'"),
        "missing Qualify line:\n{out}"
    );
}

#[test]
fn wrong_qualifier_recovers_with_plain_name() {
    let fx = Fixture::new("go_wrong", GO_FILES);
    let out = fx.discover(&["Nope.Set", "--as", "symbol"]);
    assert!(out.contains("0 matches"), "{out}");
    assert!(
        out.contains(
            "No definition of 'Set' under 'Nope'. 3 definitions named 'Set' exist — Try: srcwalk discover 'Set' --as symbol"
        ),
        "missing rule-5 recovery:\n{out}"
    );
}

const CONTAINER_FILES: &[(&str, &str)] = &[(
    "config.py",
    "class Config:\n    \"\"\"App config.\"\"\"\n\n    def load(self):\n        return {\"x\": 1}\n\n    def save(self):\n        return None\n",
)];

#[test]
fn container_language_containment_match() {
    let fx = Fixture::new("py_contain", CONTAINER_FILES);
    let out = fx.discover(&["Config.load", "--as", "symbol"]);
    assert!(out.contains("1 matches (1 definitions)"), "{out}");
    assert!(out.contains("[fn] Config.load config.py:4-5"), "{out}");
}

#[test]
fn wrong_container_qualifier_recovers() {
    let fx = Fixture::new("py_wrong", CONTAINER_FILES);
    let out = fx.discover(&["Nope.load", "--as", "symbol"]);
    assert!(out.contains("0 matches"), "{out}");
    assert!(
        out.contains("No definition of 'load' under 'Nope'"),
        "{out}"
    );
    assert!(out.contains("1 definitions named 'load' exist"), "{out}");
}

#[test]
fn multi_dot_and_extension_and_dotfile_queries_unchanged() {
    let fx = Fixture::new("unchanged", GO_FILES);
    // Multi-dot `a.b.c` — file route, not symbol; a 0-file packet is fine and
    // must not mention any qualified-symbol interpretation.
    let out = fx.discover(&["a.b.c", "--as", "symbol"]);
    assert!(
        out.contains("# Files:"),
        "expected file route for multi-dot:\n{out}"
    );
    assert!(!out.contains("No definition of"), "{out}");
    // Extension-bearing query stays a filename route.
    let out = fx.discover(&["Batch.rs", "--as", "symbol"]);
    assert!(
        out.contains("# Files:"),
        "expected file route for extension:\n{out}"
    );
    assert!(!out.contains("No definition of"), "{out}");
    // Dotfile query: unchanged (errors as an unresolvable name today).
    let (ok, out) = fx.discover_raw(&[".go", "--as", "symbol"]);
    assert!(!ok && out.contains("no matches for \".go\""), "{out}");
    assert!(!out.contains("No definition of"), "{out}");
}

const ELIXIR_FILES: &[(&str, &str)] = &[(
    "app.ex",
    "defmodule Foo.Bar do\n  def hello do\n    :world\n  end\nend\n",
)];

#[test]
fn dotted_outline_name_still_matches_exact_and_first() {
    let fx = Fixture::new("elixir_dotted", ELIXIR_FILES);
    let out = fx.discover(&["Foo.Bar", "--as", "symbol"]);
    assert!(out.contains("1 matches (1 definitions)"), "{out}");
    assert!(
        out.contains("Foo.Bar app.ex:1-5") && !out.contains("[fn] Foo.Bar app.ex"),
        "exact dotted-name match must win, not a qualified fn:\n{out}"
    );
}

#[test]
fn qualified_output_is_deterministic() {
    let fx = Fixture::new("go_determinism", GO_FILES);
    let first = fx.discover(&["Batch.Set", "--as", "symbol"]);
    let second = fx.discover(&["Batch.Set", "--as", "symbol"]);
    assert_eq!(first, second);
}
