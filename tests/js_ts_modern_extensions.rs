//! End-to-end proof of US-052 Phase 3: `.mjs`/`.cjs` -> JavaScript and
//! `.mts`/`.cts` -> TypeScript, consistently across exact-file detection,
//! structural discovery/reads, dependency resolution, and artifact routing.
//!
//! Detection-only support is rejected: every extension must enter the same
//! structural and dependency surface as its language. No new language tier; no
//! gjs/gts, Vue, or Svelte; no Node runtime / `package.json` `type`/`exports` /
//! conditional-exports / bundler / compiler-resolution claims.

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
            "srcwalk_js_mod_ext_{name}_{}_{}",
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

/// Normalize display separators so assertions are platform-neutral; also
/// proves the CLI never emits raw backslashes for these fixtures.
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

/// Structural matrix: smart outline (`show`) and `discover --as symbol` find a
/// real function/class definition in all four modern extensions.
#[test]
fn outline_and_discover_find_definitions_in_all_four_extensions() {
    let dir = TempRepo::new("structural_matrix");
    let cases = [
        (
            "util.mjs",
            "mjsAdd",
            "export function mjsAdd(a, b) { return a + b; }\n",
        ),
        (
            "util.cjs",
            "cjsMul",
            "function cjsMul(a, b) { return a * b; }\nmodule.exports = { cjsMul };\n",
        ),
        (
            "util.mts",
            "mtsGreet",
            "export function mtsGreet(name: string): string { return `hi ${name}`; }\n",
        ),
        (
            "util.cts",
            "ctsTriple",
            "function ctsTriple(x: number): number { return x * 3; }\nmodule.exports = { ctsTriple };\n",
        ),
    ];
    for (file, _, body) in cases {
        write_file(&dir.join(file), body);
    }

    for (file, sym, _) in cases {
        // Smart outline via `show`.
        let (ok, stdout, stderr) = run(&dir, &["show", file]);
        assert!(ok, "show {file} failed: {stderr}");
        assert!(
            stdout.contains(sym),
            "show {file} must surface definition {sym}:\n{stdout}"
        );
        // discover --as symbol routes through structural definitions.
        let scope = dir.to_str().unwrap();
        let (ok, stdout, _) = run(&dir, &["discover", sym, "--as", "symbol", "--scope", scope]);
        assert!(ok, "discover {sym} failed");
        assert!(
            stdout.contains(sym),
            "discover {sym} must find a real definition:\n{stdout}"
        );
    }
}

/// One representative context/decision-flow smoke per tier: `.mjs` routes to
/// the JavaScript Flow Map and `.mts` to the TypeScript Flow Map, both
/// supported (no hard-abstention on a plain loop+return body).
#[test]
fn context_decision_flow_smoke_per_js_and_ts_tier() {
    let dir = TempRepo::new("flow_smoke");
    write_file(
        &dir.join("flow.mjs"),
        r#"function first(items) {
  for (const it of items) {
    if (it.match()) {
      return it;
    }
  }
  return null;
}
"#,
    );
    write_file(
        &dir.join("flow.mts"),
        r#"export function first(items: string[]): string | null {
  for (const it of items) {
    if (it.length > 0) {
      return it;
    }
  }
  return null;
}
"#,
    );

    for (file, marker) in [("flow.mjs", "JavaScript"), ("flow.mts", "TypeScript")] {
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{file}:first")]);
        assert!(
            ok,
            "{file} decision-flow must be supported, stderr:\n{stderr}"
        );
        assert!(!stdout.contains("abstained"), "{file}:\n{stdout}");
        for needle in ["[loop]", "[return]"] {
            assert!(
                stdout.contains(needle),
                "{file}: missing {needle:?} in:\n{stdout}"
            );
        }
        let _ = marker;
    }
}

/// One representative callers/callees smoke per tier: callees from a `.mjs`
/// entry, callers of a `.mts` symbol.
#[test]
fn callers_callees_smoke_per_js_and_ts_tier() {
    let dir = TempRepo::new("call_smoke");
    write_file(
        &dir.join("entry.mjs"),
        "import { helper } from './helper.mjs';\nexport function entry() { return helper(); }\n",
    );
    write_file(
        &dir.join("helper.mjs"),
        "export function helper() { return 1; }\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["trace", "callees", "entry", "--scope", scope]);
    assert!(ok, "mjs callees failed");
    assert!(
        stdout.contains("helper"),
        "mjs callees must surface helper:\n{stdout}"
    );

    write_file(
        &dir.join("lib.mts"),
        "export function libFn(): number { return 42; }\n",
    );
    write_file(
        &dir.join("user.mts"),
        "import { libFn } from './lib.mts';\nexport function user(): number { return libFn(); }\n",
    );
    let (ok, stdout, _) = run(&dir, &["trace", "callers", "libFn", "--scope", scope]);
    assert!(ok, "mts callers failed");
    assert!(
        stdout.contains("user"),
        "mts callers must surface user.mts:\n{stdout}"
    );
}

/// Forward local dependency evidence for exact static ESM imports and literal
/// CommonJS `require` across all four new forms.
#[test]
fn deps_static_esm_and_literal_require_across_new_forms() {
    let dir = TempRepo::new("deps_exact");
    write_file(&dir.join("a.mjs"), "export function a() { return 1; }\n");
    write_file(&dir.join("b.cjs"), "module.exports = { b: 2 };\n");
    write_file(
        &dir.join("c.mts"),
        "export function c(): number { return 3; }\n",
    );
    write_file(&dir.join("d.cts"), "exports.d = 4;\n");
    write_file(
        &dir.join("main.mjs"),
        "import { a } from './a.mjs';\n\
         export { b } from './b.cjs';\n\
         const c = require('./c.mts');\n\
         const d = require('./d.cts');\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.mjs", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.contains("# Deps: main.mjs — 4 local, 0 external, 0 dependents"),
        "exact header:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (local)\n(root)/"),
        "local section:\n{stdout}"
    );
    for rel in ["a.mjs", "b.cjs", "c.mts", "d.cts"] {
        assert!(
            stdout.lines().any(|l| l.trim_start().starts_with(rel)),
            "local use of {rel} missing:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains('\\'),
        "path display must be normalized:\n{stdout}"
    );
}

/// Runtime-to-source swaps: a missing `./x.mjs` specifier resolves to `x.mts`,
/// and a missing `./y.cjs` resolves to `y.cts`.
#[test]
fn deps_runtime_to_source_swaps_mjs_to_mts_and_cjs_to_cts() {
    let dir = TempRepo::new("runtime_to_source");
    write_file(
        &dir.join("x.mts"),
        "export function x(): number { return 1; }\n",
    );
    write_file(&dir.join("y.cts"), "exports.y = 2;\n");
    write_file(
        &dir.join("main.mts"),
        "import { x } from './x.mjs';\nconst y = require('./y.cjs');\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.mts", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.contains("# Deps: main.mts — 2 local, 0 external, 0 dependents"),
        "exact header:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.trim_start().starts_with("x.mts")),
        "mjs specifier must resolve to x.mts:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.trim_start().starts_with("y.cts")),
        "cjs specifier must resolve to y.cts:\n{stdout}"
    );
}

/// Extensionless and `index.*` candidates include the four new forms, appended
/// after the existing `.ts/.tsx/.js/.jsx` winner order.
#[test]
fn deps_extensionless_and_index_candidates_for_new_forms() {
    let dir = TempRepo::new("index_candidates");
    write_file(
        &dir.join("mod.mts"),
        "export function modFn(): number { return 1; }\n",
    );
    write_file(
        &dir.join("pkg_a/index.mjs"),
        "export function pkgAFn() { return 2; }\n",
    );
    write_file(&dir.join("pkg_b/index.cts"), "exports.pkgBVal = 3;\n");
    // A decoy index in a different directory proves attribution: a matching
    // basename alone must not win from the wrong directory.
    write_file(&dir.join("decoy/index.mjs"), "export const decoy = 0;\n");
    write_file(
        &dir.join("main.mjs"),
        "import { modFn } from './mod';\n\
         import { pkgAFn } from './pkg_a';\n\
         const pkgBVal = require('./pkg_b');\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.mjs", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.contains("# Deps: main.mjs — 3 local, 0 external, 0 dependents"),
        "exact header:\n{stdout}"
    );
    // Directory attribution: each candidate must resolve from its own package
    // directory, not merely any file with a matching basename.
    assert!(
        stdout.contains("\n(root)/\n  mod.mts"),
        "extensionless ./mod must resolve to root mod.mts:\n{stdout}"
    );
    assert!(
        stdout.contains("\npkg_a/\n  index.mjs"),
        "./pkg_a must resolve to pkg_a/index.mjs:\n{stdout}"
    );
    assert!(
        stdout.contains("\npkg_b/\n  index.cts"),
        "./pkg_b must resolve to pkg_b/index.cts:\n{stdout}"
    );
    assert!(
        !stdout.contains("decoy/"),
        "decoy index.mjs must not be attributed to ./pkg_a or ./pkg_b:\n{stdout}"
    );
}

/// Reverse `Used by` evidence with exact lines across the new extensions.
#[test]
fn deps_reverse_used_by_exact_line() {
    let dir = TempRepo::new("used_by");
    write_file(
        &dir.join("lib.mjs"),
        "export function helper() { return 1; }\n",
    );
    // Single-line caller bodies so the reverse evidence line is deterministic
    // (reverse rows carry the call-site line).
    write_file(
        &dir.join("main.mjs"),
        "import { helper } from './lib.mjs'; export function main() { return helper(); }\n",
    );
    write_file(
        &dir.join("other.mts"),
        "import { helper } from './lib.mjs'; export function other(): number { return helper(); }\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "lib.mjs", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.contains("## Used by"),
        "reverse section missing:\n{stdout}"
    );
    assert!(
        stdout.contains("main.mjs:1"),
        "reverse evidence must carry exact line main.mjs:1:\n{stdout}"
    );
    assert!(
        stdout.contains("other.mts:1"),
        "reverse evidence must carry exact line other.mts:1:\n{stdout}"
    );
}

/// Collision: adding `.mts/.cts/.mjs/.cjs` candidates must not change the
/// existing `.ts/.tsx/.js/.jsx` winner priority for extensionless specifiers.
#[test]
fn collision_existing_winner_priority_unchanged() {
    let dir = TempRepo::new("collision_winner");
    // Every candidate exists; `.ts` must still win for `./mod` and `./pair`.
    write_file(&dir.join("mod.ts"), "export const mod: number = 1;\n");
    write_file(&dir.join("mod.mts"), "export const mod: number = 2;\n");
    write_file(&dir.join("mod.mjs"), "export const mod = 3;\n");
    write_file(&dir.join("pair.ts"), "export const pair: number = 1;\n");
    write_file(&dir.join("pair.tsx"), "export const pair = 2;\n");
    write_file(&dir.join("pair.mts"), "export const pair: number = 3;\n");
    write_file(
        &dir.join("main.ts"),
        "import { mod } from './mod';\nimport { pair } from './pair';\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.ts", "--scope", scope]);
    assert!(ok, "deps failed");
    // Both resolve to their `.ts` (and only `.ts`) winners.
    assert!(
        stdout.contains("# Deps: main.ts — 2 local, 0 external, 0 dependents"),
        "exact header:\n{stdout}"
    );
    for winner in ["mod.ts", "pair.ts"] {
        assert!(
            stdout.lines().any(|l| l.trim_start().starts_with(winner)),
            "winner {winner} must be selected:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("mod.mts") && !stdout.contains("mod.mjs"),
        "substitutions must not win over existing .ts:\n{stdout}"
    );
    assert!(
        !stdout.contains("pair.tsx") && !stdout.contains("pair.mts"),
        "existing .ts/.tsx order must be preserved:\n{stdout}"
    );
}

/// Collision: an exact existing file beats a runtime-to-source substitution.
#[test]
fn collision_exact_file_beats_substitution() {
    let dir = TempRepo::new("collision_exact");
    write_file(
        &dir.join("x.mjs"),
        "export function x() { return 'exact'; }\n",
    );
    write_file(
        &dir.join("x.mts"),
        "export function x(): string { return 'src'; }\n",
    );
    write_file(&dir.join("main.mjs"), "import { x } from './x.mjs';\n");

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.mjs", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.lines().any(|l| l.trim_start().starts_with("x.mjs")),
        "exact x.mjs must win over x.mts:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|l| l.trim_start().starts_with("x.mts")),
        "substitution must not shadow the exact file:\n{stdout}"
    );
}

/// Phase 2 unresolved-local behavior extends to the new extensions: a
/// canonical local-looking specifier that resolves to nothing stays visible
/// as unresolved with its exact line, and the positive count is reported.
#[test]
fn unresolved_local_looking_applies_to_new_extensions() {
    let dir = TempRepo::new("unresolved_new");
    write_file(
        &dir.join("main.mts"),
        "import { a } from './a.mjs';\n\
         import { b } from './missing.mjs';\n\
         const c = require('./missing.cjs');\n",
    );
    write_file(
        &dir.join("a.mts"),
        "export function a(): number { return 1; }\n",
    );

    let scope = dir.to_str().unwrap();
    let (ok, stdout, _) = run(&dir, &["deps", "main.mts", "--scope", scope]);
    assert!(ok, "deps failed");
    assert!(
        stdout.contains("# Deps: main.mts — 1 local, 0 external, 2 unresolved, 0 dependents"),
        "exact header with unresolved count:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (unresolved local-looking)"),
        "unresolved section missing:\n{stdout}"
    );
    for (line, src) in [(2, "./missing.mjs"), (3, "./missing.cjs")] {
        assert!(
            stdout.contains(&format!("  {line}  {src}")),
            "missing unresolved {line} {src} in:\n{stdout}"
        );
    }
    // The resolved mjs->mts swap must not leak into unresolved.
    assert!(
        !stdout.contains("./a.mjs"),
        "resolved specifier must not be unresolved:\n{stdout}"
    );
}

/// Exact minified `.mjs`/`.cjs` bundles auto-route to artifact evidence
/// through existing artifact behavior — no `--artifact` flag required — and
/// the artifact code fence for `.mts` uses TypeScript fencing.
#[test]
fn minified_bundles_auto_artifact_and_mts_fence_is_typescript() {
    let dir = TempRepo::new("minified_auto");
    let bundle_mjs = dir.join("bundle.min.mjs");
    fs::write(
        &bundle_mjs,
        "export function alpha(){return 1}export function beta(){return 2}\n",
    )
    .unwrap();
    let bundle_cjs = dir.join("bundle.min.cjs");
    fs::write(
        &bundle_cjs,
        "exports.widget=function(){};module.exports.Helper=class{};\n",
    )
    .unwrap();

    // Bare show read (no --artifact) must auto-route to artifact anchors.
    for (path, sym) in [(&bundle_mjs, "alpha"), (&bundle_cjs, "widget")] {
        let out = srcwalk().arg(path).output().expect("run auto artifact");
        assert!(out.status.success(), "auto-artifact read failed");
        let stdout = norm(&String::from_utf8_lossy(&out.stdout));
        assert!(
            stdout.contains("Artifact anchors:"),
            "auto-artifact must route to artifact evidence:\n{stdout}"
        );
        assert!(stdout.contains(sym), "missing {sym} anchor:\n{stdout}");
    }

    // Existing artifact paths on `.mts` emit a TypeScript code fence in
    // symbol/byte section reads.
    let typed = dir.join("mod.mts");
    fs::write(
        &typed,
        "export function greet(name: string): string { return `hi ${name}`; }\n",
    )
    .unwrap();
    let out = srcwalk()
        .arg(&typed)
        .args(["--artifact", "--section", "greet"])
        .output()
        .expect("run mts artifact section");
    assert!(out.status.success(), "mts artifact read failed");
    let stdout = norm(&String::from_utf8_lossy(&out.stdout));
    assert!(
        stdout.contains("```ts"),
        ".mts artifact fence must be TypeScript:\n{stdout}"
    );
}
