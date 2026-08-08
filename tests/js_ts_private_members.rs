//! End-to-end proof of US-052 Phase 4: private class-member (`#name`) call
//! relations are visible to name-based `trace callees` / `trace callers`
//! evidence for JS/TS/TSX, without any binding/dynamic-dispatch claims.
//!
//! Invariant: if a private method is a structural definition, direct syntactic
//! calls to the same private name must be visible to name-based evidence.
//! Phase 4 only closes the caller/callee relation gap — definition discovery
//! (`discover '#evict'`) already worked via `method_definition.name ->
//! private_property_identifier`. The `#` is part of the captured token text, so
//! public `evict` and private `#evict` stay distinct strings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// A temp repo that removes its directory on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "srcwalk_js_priv_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempRepo(dir)
    }
}

impl std::ops::Deref for TempRepo {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Normalize display separators so assertions are platform-neutral.
fn norm(s: &str) -> String {
    s.replace('\\', "/")
}

/// Run srcwalk with cwd = repo; returns (success, stdout, stderr) with
/// normalized separators.
fn run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = srcwalk()
        .current_dir(dir)
        .args(args)
        .output()
        .expect("srcwalk runs");
    (
        out.status.success(),
        norm(&String::from_utf8_lossy(&out.stdout)),
        norm(&String::from_utf8_lossy(&out.stderr)),
    )
}

/// One fixture exercising every private-member shape Phase 4 must see, plus
/// the decoys it must NOT see. Line numbers are load-bearing:
///   L2  `#evict` definition
///   L3  public `evict` definition (distinct name)
///   L4  static private `#purge` definition
///   L6  instance private call        -> caller of #evict
///   L7  static-style receiver call   -> caller of #evict
///   L8  optional-chaining private    -> caller of #evict (mandatory positive)
///   L9  bare `evict(...)`            -> caller of public evict only
///   L10 `this.evict(...)`            -> caller of public evict only
///   L11 static private call          -> caller of #purge
///   L12 comment mention of `#evict`  -> must NOT create a relation
///   L13 string mention of `"#evict"` -> must NOT create a relation
const FIXTURE: &str = "class Cache {\n\
  #evict(key) { return key; }\n\
  evict(key) { return key; }\n\
  static #purge() { return 1; }\n\
  run(key) {\n\
    this.#evict(key);\n\
    Cache.#evict(key);\n\
    this?.#evict(key);\n\
    evict(key);\n\
    this.evict(key);\n\
    Cache.#purge();\n\
    // this.#evict(key)\n\
    const s = \"#evict\";\n\
    return 0;\n\
  }\n\
}\n";

#[test]
fn discover_private_method_found_once() {
    let dir = TempRepo::new("discover");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (ok, stdout, _) = run(
        &dir,
        &["discover", "#evict", "--scope", dir.to_str().unwrap()],
    );
    assert!(ok, "discover failed");
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "exactly one definition, no duplicate:\n{stdout}"
    );
    assert!(
        stdout.contains("#evict"),
        "must surface the private definition:\n{stdout}"
    );
}

#[test]
fn callees_from_owning_method_shows_private() {
    let dir = TempRepo::new("callees");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (ok, stdout, _) = run(
        &dir,
        &["trace", "callees", "run", "--scope", dir.to_str().unwrap()],
    );
    assert!(ok, "trace callees failed");
    assert!(
        stdout.contains("#evict"),
        "owning method must list #evict callee:\n{stdout}"
    );
    assert!(
        stdout.contains("#purge"),
        "static private #purge must also resolve as a callee:\n{stdout}"
    );
}

#[test]
fn callers_private_attributed_to_enclosing_class_method() {
    let dir = TempRepo::new("callers_attr");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "trace",
            "callers",
            "#evict",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "trace callers failed");
    assert!(
        stdout.contains("— 3 call sites"),
        "exactly the three #evict sites:\n{stdout}"
    );
    for line in ["cache.js:6", "cache.js:7", "cache.js:8"] {
        assert!(stdout.contains(line), "call site {line} missing:\n{stdout}");
    }
    assert!(
        stdout.contains("[fn] Cache.run"),
        "call must be attributed to the enclosing class + method:\n{stdout}"
    );
}

#[test]
fn public_name_distinct_from_private() {
    let dir = TempRepo::new("distinct");
    write_file(&dir.join("cache.js"), FIXTURE);
    let scope = dir.to_str().unwrap();
    let (_, priv_out, _) = run(&dir, &["trace", "callers", "#evict", "--scope", scope]);
    assert!(
        !priv_out.contains("cache.js:9") && !priv_out.contains("cache.js:10"),
        "private #evict must not claim the public evict sites:\n{priv_out}"
    );
    let (_, pub_out, _) = run(&dir, &["trace", "callers", "evict", "--scope", scope]);
    assert!(
        pub_out.contains("— 2 call sites"),
        "public evict must see only its two sites:\n{pub_out}"
    );
    assert!(
        pub_out.contains("cache.js:9") && pub_out.contains("cache.js:10"),
        "public evict must see the bare + member sites:\n{pub_out}"
    );
    assert!(
        !pub_out.contains("cache.js:6") && !pub_out.contains("cache.js:8"),
        "public evict must not claim the #evict sites:\n{pub_out}"
    );
}

#[test]
fn comment_and_string_mentions_do_not_create_relations() {
    let dir = TempRepo::new("no_fake_relations");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (_, stdout, _) = run(
        &dir,
        &[
            "trace",
            "callers",
            "#evict",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(
        !stdout.contains("cache.js:12") && !stdout.contains("cache.js:13"),
        "comment/string mentions must not create relations:\n{stdout}"
    );
}

#[test]
fn static_private_call_visible_symmetrically() {
    let dir = TempRepo::new("static_private");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "trace",
            "callers",
            "#purge",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "trace callers #purge failed");
    assert!(
        stdout.contains("— 1 call site"),
        "exactly one #purge site:\n{stdout}"
    );
    assert!(
        stdout.contains("cache.js:11"),
        "static private Cache.#purge() must be found:\n{stdout}"
    );
}

#[test]
fn optional_chaining_private_call_is_visible_positive() {
    let dir = TempRepo::new("optional_chain");
    write_file(&dir.join("cache.js"), FIXTURE);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "trace",
            "callers",
            "#evict",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "trace callers failed");
    assert!(
        stdout.contains("cache.js:8"),
        "this?.#evict() must be a positive call site (optional_chain is a field on member_expression):\n{stdout}"
    );
}

#[test]
fn ts_and_tsx_private_calls_visible() {
    let dir = TempRepo::new("ts_tsx");
    write_file(
        &dir.join("cache.ts"),
        "class Cache {\n\
           #evict(key: number): number { return key; }\n\
           run(key: number): number {\n\
             this.#evict(key);\n\
             this?.#evict(key);\n\
             return 0;\n\
           }\n\
         }\n",
    );
    write_file(
        &dir.join("widget.tsx"),
        "export class Widget {\n\
           #render(): string { return \"<div/>\"; }\n\
           mount(): string {\n\
             this.#render();\n\
             this?.#render();\n\
             return \"\";\n\
           }\n\
         }\n",
    );
    let scope = dir.to_str().unwrap();
    let (_, ts_out, _) = run(&dir, &["trace", "callers", "#evict", "--scope", scope]);
    assert!(
        ts_out.contains("cache.ts:4") && ts_out.contains("cache.ts:5"),
        "TS private calls (instance + optional chain) must be found:\n{ts_out}"
    );
    let (_, tsx_out, _) = run(&dir, &["trace", "callers", "#render", "--scope", scope]);
    assert!(
        tsx_out.contains("widget.tsx:4") && tsx_out.contains("widget.tsx:5"),
        "TSX private calls must be found:\n{tsx_out}"
    );
}
