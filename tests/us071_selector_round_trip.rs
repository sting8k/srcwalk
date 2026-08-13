//! US-071: the canonical `<path>:<symbol>` target Discover prints must be
//! copyable, unchanged, into `show`, `context`, `trace callers`, and
//! `trace callees`.
//!
//! Every assertion here consumes the string the binary actually emitted. No
//! hand-authored "ideal" selector is substituted, because the regression being
//! guarded is exactly a printed target that looks canonical but does not
//! resolve (`System.Text.Json.GetTypeInfoInternal` instead of the outline
//! container `JsonSerializerOptions.GetTypeInfoInternal`).
//!
//! The subprocess CWD is always set explicitly, so a target that only resolves
//! because the temp CWD happens to equal `--scope` cannot pass.

use std::fs;
use std::path::{Path, PathBuf};
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
            "us071_round_trip_{name}_{}_{}",
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

    fn run_in(&self, cwd: &Path, args: &[&str]) -> (bool, String) {
        let out = srcwalk().current_dir(cwd).args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr),
        )
    }

    fn ok_in(&self, cwd: &Path, args: &[&str]) -> String {
        let (ok, out) = self.run_in(cwd, args);
        assert!(ok, "command {args:?} in {cwd:?} failed:\n{out}");
        out
    }

    fn ok(&self, args: &[&str]) -> String {
        let dir = self.dir.clone();
        self.ok_in(&dir, args)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Split the argument string after `> Next: srcwalk ` into argv with the
/// footer's single-quote shell quoting undone. Fixture names never contain an
/// apostrophe, so plain single-quote toggling is sufficient here.
fn parse_argv(rest: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut open = false;
    for ch in rest.chars() {
        match ch {
            '\'' => {
                quoted = !quoted;
                open = true;
            }
            c if c.is_whitespace() && !quoted => {
                if open || !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                    open = false;
                }
            }
            c => current.push(c),
        }
    }
    if open || !current.is_empty() {
        argv.push(current);
    }
    argv
}

/// The printed `> Next: srcwalk show ...` command, split into argv.
fn emitted_show_argv(output: &str) -> Vec<String> {
    let line = output
        .lines()
        .find(|l| l.starts_with("> Next: srcwalk show "))
        .unwrap_or_else(|| panic!("no `> Next: srcwalk show` line in:\n{output}"));
    let rest = line.trim_start_matches("> Next: srcwalk ").trim();
    let argv = parse_argv(rest);
    assert_eq!(argv.first().map(String::as_str), Some("show"), "{output}");
    argv
}

/// Every `> Next: srcwalk show ...` line, each parsed into `(target, flags)`.
/// Used for multi-symbol output where each per-query section prints its own
/// canonical target. `--section` is never canonical, so its presence fails.
fn emitted_targets_and_flags_all(output: &str) -> Vec<(String, Vec<String>)> {
    output
        .lines()
        .filter(|l| l.starts_with("> Next: srcwalk show "))
        .map(|line| {
            let rest = line.trim_start_matches("> Next: srcwalk ").trim();
            let argv = parse_argv(rest);
            assert!(
                !argv.iter().any(|a| a == "--section"),
                "canonical target must be one argument, got {argv:?}:\n{line}"
            );
            (argv[1].clone(), argv[2..].to_vec())
        })
        .collect()
}

/// The single copyable target plus the trailing flags the footer says to keep
/// (`--scope <dir>` when the display is scope-relative). `--section` is not a
/// canonical form, so its presence fails the contract.
fn emitted_target_and_flags(output: &str) -> (String, Vec<String>) {
    let argv = emitted_show_argv(output);
    assert!(
        !argv.iter().any(|a| a == "--section"),
        "canonical target must be one argument, got {argv:?}:\n{output}"
    );
    (argv[1].clone(), argv[2..].to_vec())
}

fn with_flags<'a>(head: &[&'a str], target: &'a str, flags: &'a [String]) -> Vec<&'a str> {
    let mut args: Vec<&str> = head.to_vec();
    args.push(target);
    args.extend(flags.iter().map(String::as_str));
    args
}

const OPTIONS_CS: &str = "namespace System.Text.Json\n{\n    public sealed class JsonSerializerOptions\n    {\n        internal JsonTypeInfo? GetTypeInfoInternal(Type type)\n        {\n            var info = Lookup(type);\n            return info;\n        }\n\n        internal JsonTypeInfo? Lookup(Type type)\n        {\n            return null;\n        }\n    }\n}\n";

const CALLER_CS: &str = "namespace System.Text.Json\n{\n    internal static class Caller\n    {\n        internal static void Run(JsonSerializerOptions o, Type t)\n        {\n            o.GetTypeInfoInternal(t);\n        }\n    }\n}\n";

/// C# namespace trap: the display name is `System.Text.Json.GetTypeInfoInternal`
/// but the only selector that resolves is the outline container form
/// `JsonSerializerOptions.GetTypeInfoInternal`. The emitted target is copied
/// unchanged into all four exact-target commands.
#[test]
fn discover_selector_round_trips_unchanged_through_all_four_commands() {
    let fx = Fixture::new(
        "csharp_namespace",
        &[
            ("src/JsonSerializerOptions.cs", OPTIONS_CS),
            ("src/Caller.cs", CALLER_CS),
        ],
    );

    let discovered = fx.ok(&[
        "discover",
        "GetTypeInfoInternal",
        "--as",
        "symbol",
        "--scope",
        "src",
    ]);
    let (target, flags) = emitted_target_and_flags(&discovered);

    // The emitted selector comes from outline primitives, never the display
    // qualified name that includes the namespace. This also fails if Discover
    // degrades a unique supported target to `path:range`.
    assert!(
        target.ends_with(":JsonSerializerOptions.GetTypeInfoInternal"),
        "emitted target must use the outline container, got `{target}`:\n{discovered}"
    );
    assert!(
        !target.contains("System.Text.Json."),
        "namespace-qualified display name must never be emitted as a target: `{target}`"
    );
    assert!(
        discovered.contains("trace callers")
            && discovered.contains("trace callees")
            && discovered.contains("by-name"),
        "the reuse note must name all four commands and keep the caller bound:\n{discovered}"
    );
    // `src` is under the discover CWD, so the display addresses the file on its
    // own and no `--scope` may be appended.
    assert!(
        flags.is_empty(),
        "a CWD-relative display must not repeat --scope, got {flags:?}:\n{discovered}"
    );

    // 1/4 show: the exact body only.
    let shown = fx.ok(&with_flags(&["show"], &target, &flags));
    assert!(shown.contains("GetTypeInfoInternal(Type type)"), "{shown}");
    assert!(
        !shown.contains("Lookup(Type type)\n"),
        "show must read only the requested body:\n{shown}"
    );

    // 2/4 context: same named-file target, no relocation.
    let context = fx.ok(&with_flags(
        &["context"],
        &target,
        &["--scope".into(), "src".into()],
    ));
    assert!(
        context.contains("GetTypeInfoInternal"),
        "context must root on the same definition:\n{context}"
    );

    // 3/4 trace callers: finds the caller and keeps the by-name caveat.
    let callers = fx.ok(&with_flags(
        &["trace", "callers"],
        &target,
        &["--scope".into(), "src".into()],
    ));
    assert!(
        callers.contains("Run"),
        "callers must find the call site:\n{callers}"
    );
    assert!(
        callers.contains("by name") || callers.contains("by-name"),
        "callers must keep the by-name evidence bound:\n{callers}"
    );

    // 4/4 trace callees: reads the same body's outgoing calls.
    let callees = fx.ok(&with_flags(
        &["trace", "callees"],
        &target,
        &["--scope".into(), "src".into()],
    ));
    assert!(
        callees.contains("Lookup"),
        "callees must root on the same body:\n{callees}"
    );
}

/// The printed command must run verbatim from the CWD of the Discover run, not
/// only from a CWD that happens to equal `--scope`. When `--scope` points
/// outside the CWD the display is scope-relative, so the footer must carry the
/// `--scope` that makes the copied target resolve.
#[test]
fn emitted_command_runs_verbatim_when_scope_is_outside_the_cwd() {
    let fx = Fixture::new(
        "scope_outside_cwd",
        &[
            ("repo/src/JsonSerializerOptions.cs", OPTIONS_CS),
            ("repo/src/Caller.cs", CALLER_CS),
            ("elsewhere/.keep", ""),
        ],
    );
    let cwd = fx.dir.join("elsewhere");
    let scope = fx.dir.join("repo").join("src");
    let scope_arg = scope.to_str().unwrap().to_string();

    let discovered = fx.ok_in(
        &cwd,
        &[
            "discover",
            "GetTypeInfoInternal",
            "--as",
            "symbol",
            "--scope",
            &scope_arg,
        ],
    );
    let (target, flags) = emitted_target_and_flags(&discovered);
    assert!(
        target.ends_with(":JsonSerializerOptions.GetTypeInfoInternal"),
        "expected a canonical target, got `{target}`:\n{discovered}"
    );
    // The scope-relative branch must actually be exercised, otherwise this test
    // would pass for the same reason a same-CWD test does.
    assert!(
        flags.iter().any(|f| f == "--scope"),
        "a scope-relative display must print the --scope it needs, got {flags:?}:\n{discovered}"
    );

    // Copy the emitted argv byte-for-byte, from the same CWD.
    let shown = fx.ok_in(&cwd, &with_flags(&["show"], &target, &flags));
    assert!(
        shown.contains("GetTypeInfoInternal(Type type)"),
        "printed command must run verbatim from the discover CWD:\n{shown}"
    );

    // The same target plus the same scope reaches the relation commands too.
    let callers = fx.ok_in(&cwd, &with_flags(&["trace", "callers"], &target, &flags));
    assert!(callers.contains("Run"), "{callers}");
}

/// Same-file overloads make the selector ambiguous, so Discover must fall back
/// to the numeric `<path>:<start-end>` form. That fallback must also be
/// copyable unchanged into `show`.
#[test]
fn same_file_overload_emits_numeric_fallback_that_still_round_trips() {
    let fx = Fixture::new(
        "overload",
        &[(
            "src/Ctx.cs",
            "namespace App\n{\n    public class Ctx\n    {\n        public int GetTypeInfo()\n        {\n            return 1;\n        }\n\n        public int GetTypeInfo(int id)\n        {\n            return id;\n        }\n    }\n}\n",
        )],
    );

    let discovered = fx.ok(&[
        "discover",
        "GetTypeInfo",
        "--as",
        "symbol",
        "--scope",
        "src",
    ]);
    let (target, flags) = emitted_target_and_flags(&discovered);

    assert!(
        !target.contains("Ctx.GetTypeInfo"),
        "an ambiguous selector must never be advertised as canonical: `{target}`:\n{discovered}"
    );
    assert!(
        discovered.contains("numeric fallback"),
        "the numeric fallback must be labeled at block level:\n{discovered}"
    );

    let shown = fx.ok(&with_flags(&["show"], &target, &flags));
    assert!(shown.contains("GetTypeInfo"), "{shown}");

    // The ambiguous symbol form itself must fail explicitly rather than pick
    // the first of the two definitions.
    let path = target.rsplit_once(':').unwrap().0.to_string();
    let ambiguous = format!("{path}:Ctx.GetTypeInfo");
    let (ok, out) = fx.run_in(&fx.dir, &["show", &ambiguous]);
    assert!(
        !ok,
        "ambiguous selector must not silently first-pick:\n{out}"
    );
    assert!(out.contains("definitions"), "{out}");
}

/// Nested paths put the OS path separator inside the canonical target, so this
/// also covers the Windows display form (`src/deep/service.rs:Service.run`,
/// normalized from `src\deep\service.rs`): the grammar must split on the
/// separating colon, not on a path character. The target is never hand-written.
#[test]
fn nested_path_target_round_trips_on_the_host_path_separator() {
    let fx = Fixture::new(
        "nested_path",
        &[(
            "src/deep/service.rs",
            "pub struct Service;\nimpl Service {\n    pub fn run(&self) {\n        helper();\n    }\n}\nfn helper() {}\n",
        )],
    );

    let discovered = fx.ok(&[
        "discover",
        "Service.run",
        "--as",
        "symbol",
        "--scope",
        "src",
    ]);
    let (target, flags) = emitted_target_and_flags(&discovered);
    assert!(
        target.ends_with(":Service.run"),
        "expected a canonical target, got `{target}`:\n{discovered}"
    );
    assert!(
        target.contains("deep"),
        "target must carry the nested path, got `{target}`:\n{discovered}"
    );
    // The file lives under the discover CWD, so the display already addresses
    // it: no redundant `--scope` may be printed on any platform.
    assert!(
        flags.is_empty(),
        "a CWD-relative display must not repeat --scope, got {flags:?}:\n{discovered}"
    );

    let shown = fx.ok(&with_flags(&["show"], &target, &flags));
    assert!(shown.contains("pub fn run(&self)"), "{shown}");
    let callees = fx.ok(&with_flags(
        &["trace", "callees"],
        &target,
        &["--scope".into(), "src".into()],
    ));
    assert!(callees.contains("helper"), "{callees}");
}

/// Multi-symbol Discover has one section per term, and each section must emit the
/// same canonical target the single-symbol route would. Every emitted target is
/// copied unchanged through all four exact-target commands.
#[test]
fn multi_symbol_discover_round_trips_each_section_target() {
    let fx = Fixture::new(
        "multi_symbol",
        &[
            (
                "src/alpha.rs",
                "pub struct Alpha;\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
            ),
            (
                "src/beta.rs",
                "pub struct Beta;\nimpl Beta {\n    pub fn stop(&self) {}\n}\n",
            ),
        ],
    );

    let discovered = fx.ok(&["discover", "run,stop", "--as", "symbol", "--scope", "src"]);
    // Exactly two per-query sections, each with a canonical target.
    let emitted = emitted_targets_and_flags_all(&discovered);
    assert_eq!(
        emitted.len(),
        2,
        "expected one target per term:\n{discovered}"
    );
    let targets: Vec<&str> = emitted.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        targets.iter().any(|t| t.ends_with(":Alpha.run")),
        "expected Alpha.run among {targets:?}:\n{discovered}"
    );
    assert!(
        targets.iter().any(|t| t.ends_with(":Beta.stop")),
        "expected Beta.stop among {targets:?}:\n{discovered}"
    );
    // No `--section`, no scope suffix for a CWD-relative display.
    for (target, flags) in &emitted {
        assert!(flags.is_empty(), "{target} flags {flags:?}:\n{discovered}");
    }

    // Copy each emitted target unchanged through all four commands.
    for (target, flags) in &emitted {
        let shown = fx.ok(&with_flags(&["show"], target, flags));
        assert!(shown.contains("pub fn"), "show {target}:\n{shown}");

        let context = fx.ok(&with_flags(
            &["context"],
            target,
            &["--scope".into(), "src".into()],
        ));
        assert!(
            context.contains(target.rsplit(':').next().unwrap()),
            "{context}"
        );

        let callers = fx.ok(&with_flags(
            &["trace", "callers"],
            target,
            &["--scope".into(), "src".into()],
        ));
        assert!(!callers.contains("error"), "callers {target}:\n{callers}");

        let callees = fx.ok(&with_flags(
            &["trace", "callees"],
            target,
            &["--scope".into(), "src".into()],
        ));
        assert!(!callees.contains("error"), "callees {target}:\n{callees}");
    }
}
