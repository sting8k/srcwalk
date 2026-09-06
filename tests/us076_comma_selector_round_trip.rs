//! US-076: a comma inside an emitted generic selector is selector data, not a
//! target-list separator.
//!
//! srcwalk prints `> Next: srcwalk show '<path>:Cache<K, V>.get'` and promises
//! the printed command works unchanged. Target-list consumers used to split that
//! string at every comma, so the fragment `<path>:Cache<K` was resolved instead.

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
            "us076_{name}_{}_{}",
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

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = srcwalk()
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (success, stdout, stderr) = self.run(args);
        assert!(success, "{args:?} failed:\n{stderr}");
        stdout
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The exact target of the first emitted `> Next: srcwalk <cmd> <target>` line,
/// unquoted the way a shell would hand it to the process.
fn emitted_target(output: &str, command: &str) -> String {
    let needle = format!("> Next: srcwalk {command} ");
    let line = output
        .lines()
        .find(|line| line.starts_with(&needle))
        .unwrap_or_else(|| panic!("no emitted `{command}` target in:\n{output}"));
    let rest = line[needle.len()..].trim();
    let target = rest.strip_prefix('\'').map_or_else(
        || rest.split_whitespace().next().unwrap_or(rest).to_string(),
        |unquoted| {
            unquoted
                .split_once('\'')
                .map_or_else(|| unquoted.to_string(), |(target, _)| target.to_string())
        },
    );
    target
}

/// Two `get` definitions in one file: without its generic container the selector
/// is ambiguous, so the comma-bearing selector is the only way to address the
/// first body.
const RUST_FILES: &[(&str, &str)] = &[
    (
        "src/cache.rs",
        "pub struct Cache<K, V>(K, V);\n\nimpl<K, V> Cache<K, V> {\n    pub fn get(&self) -> u8 {\n        0\n    }\n}\n\npub struct Solo<K>(K);\n\nimpl<K> Solo<K> {\n    pub fn get(&self) -> u8 {\n        1\n    }\n}\n",
    ),
    (
        "src/alpha.rs",
        "pub struct Alpha;\n\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
    ),
];

#[test]
fn emitted_generic_selector_replays_through_show_and_context() {
    let fx = Fixture::new("replay", RUST_FILES);
    let discovered = fx.ok(&["discover", "get", "--as", "symbol", "--scope", "src"]);
    let target = emitted_target(&discovered, "show");
    assert_eq!(target, "src/cache.rs:Cache<K, V>.get", "{discovered}");

    let shown = fx.ok(&["show", &target]);
    assert!(shown.contains("pub fn get(&self) -> u8"), "{shown}");
    assert!(
        shown.contains("within fn get 4-6") && !shown.contains("12-14"),
        "show must select the generic container's body, not the sibling:\n{shown}"
    );

    let context = fx.ok(&["context", &target]);
    assert!(
        context.contains("# Context Packet: src/cache.rs:Cache<K, V>.get"),
        "context must resolve the exact target, not fall back to the file:\n{context}"
    );

    // Without the generic container the same name is ambiguous, so the comma is
    // load-bearing rather than decorative.
    let (success, _, stderr) = fx.run(&["show", "src/cache.rs:get"]);
    assert!(!success && stderr.contains("2 definitions"), "{stderr}");
}

#[test]
fn emitted_generic_selector_roots_trace_callers_and_callees() {
    // Trace already accepted one comma-bearing target; this guards that the
    // shared framing did not start splitting a trace root.
    let fx = Fixture::new("trace", RUST_FILES);
    let target = "src/cache.rs:Cache<K, V>.get";

    let callers = fx.ok(&["trace", "callers", target]);
    assert!(callers.contains(&format!("\"{target}\"")), "{callers}");

    let callees = fx.ok(&["trace", "callees", target]);
    assert!(callees.contains(target), "{callees}");

    for out in [&callers, &callees] {
        // Every mention of the container is part of the intact target, so no
        // `Cache<K` fragment leaked out of a split.
        assert_eq!(
            out.matches("Cache<K").count(),
            out.matches(target).count(),
            "trace must not reveal split fragments:\n{out}"
        );
    }
}

#[test]
fn generic_selector_and_plain_target_combine_in_one_target_list() {
    let fx = Fixture::new("multi", RUST_FILES);
    let generic = "src/cache.rs:Cache<K, V>.get";
    let plain = "src/alpha.rs:Alpha.run";

    for (first, second) in [(generic, plain), (plain, generic)] {
        let list = format!("{first},{second}");

        let shown = fx.ok(&["show", &list]);
        assert!(shown.contains("# Show: 2 locations"), "{list}\n{shown}");
        let first_at = shown.find(&format!("## Target: {first}")).unwrap();
        let second_at = shown.find(&format!("## Target: {second}")).unwrap();
        assert!(first_at < second_at, "input order must hold:\n{shown}");

        let context = fx.ok(&["context", &list]);
        assert!(
            context.contains("# Context: 2 exact targets"),
            "{list}\n{context}"
        );
    }
}

#[test]
fn nested_generic_selector_is_one_target() {
    let fx = Fixture::new(
        "nested",
        &[
            (
                "src/nested.rs",
                "pub struct Inner<A, B>(A, B);\n\npub struct Outer<K, V>(K, V);\n\nimpl<K, V> Outer<K, Inner<V, u8>> {\n    pub fn deep(&self) -> u8 {\n        7\n    }\n}\n",
            ),
            (
                "src/alpha.rs",
                "pub struct Alpha;\n\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
            ),
        ],
    );

    let discovered = fx.ok(&["discover", "deep", "--as", "symbol", "--scope", "src"]);
    let target = emitted_target(&discovered, "show");
    assert_eq!(
        target, "src/nested.rs:Outer<K, Inner<V, u8>>.deep",
        "{discovered}"
    );

    let shown = fx.ok(&["show", &target]);
    assert!(shown.contains("pub fn deep(&self) -> u8"), "{shown}");

    let list = format!("{target},src/alpha.rs:Alpha.run");
    let shown = fx.ok(&["show", &list]);
    assert!(shown.contains("# Show: 2 locations"), "{shown}");
    assert!(shown.contains(&format!("## Target: {target}")), "{shown}");
}

#[test]
fn unbalanced_angle_brackets_fail_once_without_running_any_target() {
    let fx = Fixture::new("malformed", RUST_FILES);
    // The first item is a valid target, so a partial run would be visible.
    let malformed = "src/alpha.rs:Alpha.run,src/cache.rs:Cache<K, V.get";

    for command in ["show", "context"] {
        let (success, stdout, stderr) = fx.run(&[command, malformed]);
        assert!(!success, "{command} must reject:\n{stdout}");
        assert!(
            stderr.contains("unbalanced `<...>` in comma-separated target list"),
            "{command} stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("pub fn run(&self) {}") && !stdout.contains("## Target:"),
            "{command} must not execute the valid prefix target:\n{stdout}"
        );
        assert_eq!(
            stderr.matches("unbalanced").count(),
            1,
            "one framing error only:\n{stderr}"
        );
    }
}

#[test]
fn comma_path_keeps_section_emission_and_its_pre_existing_replay_failure() {
    let fx = Fixture::new(
        "comma_path",
        &[("od,d/f.rs", "pub fn commapath() -> u8 {\n    9\n}\n")],
    );

    // Emission is unchanged: a comma in the *path* still routes to the quoted
    // path + `--section` form rather than an inline `path:selector` target.
    let discovered = fx.ok(&["discover", "commapath", "--as", "symbol"]);
    assert!(
        discovered.contains("> Next: srcwalk show 'od,d/f.rs' --section commapath"),
        "{discovered}"
    );

    // Replaying that emitted command fails, identically on the base and on this
    // change. US-076 does not add path escaping, so the failure is recorded
    // rather than asserted as a working round-trip.
    let (success, _, stderr) = fx.run(&["show", "od,d/f.rs", "--section", "commapath"]);
    assert!(
        !success && stderr.contains("--section applies to one show target"),
        "pre-existing comma-path replay failure changed shape:\n{stderr}"
    );
}

/// US-076 AC-10: the emitted-target contract holds for generic containers in
/// other supported languages. Their outlines currently emit a non-generic
/// selector, so the assertion is on the actual emitted string rather than on an
/// assumed generic spelling.
#[test]
fn generic_containers_in_other_languages_replay_their_emitted_target() {
    let fx = Fixture::new(
        "languages",
        &[
            (
                "src/cache.ts",
                "export class TsCache<K, V> {\n  get(key: K): V | undefined {\n    return undefined;\n  }\n}\n",
            ),
            (
                "src/Cache.java",
                "public class JavaCache<K, V> {\n    public V get(K key) {\n        return null;\n    }\n}\n",
            ),
            (
                "src/Cache.cs",
                "public class CsCache<K, V> {\n    public V Get(K key) {\n        return default(V);\n    }\n}\n",
            ),
        ],
    );

    for (query, expected_target, body) in [
        (
            "get",
            "src/cache.ts:TsCache.get",
            "get(key: K): V | undefined",
        ),
        ("get", "src/Cache.java:JavaCache.get", "public V get(K key)"),
        ("Get", "src/Cache.cs:CsCache.Get", "public V Get(K key)"),
    ] {
        let discovered = fx.ok(&["discover", query, "--as", "symbol", "--scope", "src"]);
        assert!(
            discovered.contains(&format!("> Next: srcwalk show {expected_target}")),
            "expected `{expected_target}` in:\n{discovered}"
        );

        let shown = fx.ok(&["show", expected_target]);
        assert!(shown.contains(body), "show {expected_target}:\n{shown}");
    }
}
