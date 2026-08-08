//! End-to-end proof of US-052 Phase 5: a transparent `export_statement` wrapper
//! and its inner declaration are emitted as exactly ONE definition candidate,
//! not two.
//!
//! Before this phase, `discover X --as symbol` on any exported JS/TS declaration
//! rendered a self-contradictory "2 matches (2 definitions)" header over one
//! location, and `decision-flow <bare-name>` on an exported declaration HARD
//! FAILED with an "ambiguous symbol target" error listing the same range twice.
//! Both search/index walkers now treat `export_statement` as transparent when
//! its `declaration` field is itself a definition.

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
            "srcwalk_js_export_{name}_{}_{}",
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

/// One file with one exported declaration of each shape. Line numbers are
/// load-bearing for the default-export case.
const EXPORTED_TS: &str = r#"export function routeRequest(req: Request): Response {
  return new Response("ok");
}

export class Cache {
  evict(key: string): void {}
}

export const makeHandler = () => 1;

export default function defaultHandler() {}

const hidden = 2;
export { hidden };
"#;

/// TS-only `DEFINITION_KINDS` members behind `export`.
const TS_ONLY_TS: &str = r#"export interface Foo {}

export type Bar = string;

export enum Baz {}
"#;

/// Nested definition inside an exported class body must still be found.
const NESTED_TS: &str = r#"export class Box {
  inner(): number {
    return 1;
  }
}
"#;

/// A usage plus one exported definition: no multiple-definition caveat.
const USAGE_TS: &str = r#"export function handle() {}

function caller() {
  handle();
}
"#;

/// Two genuine same-name declarations across two files (still two candidates).
const GENUINE_A_TS: &str = "export function handle() {}\n";
const GENUINE_B_TS: &str = "export function handle() {}\n";

/// JS form (no type annotations) of the exported-definition shapes.
const EXPORTED_JS: &str = r#"export function greet(name) {
  return name;
}
"#;

/// Unrelated-language regression guard: Ruby has no `export_statement` and must
/// be unaffected by the JS/TS transparency change.
const UNRELATED_RB: &str = "class Person\n  def ruby_greet\n  end\nend\n";

fn discover_one_definition(dir: &Path, symbol: &str, file: &str, body: &str) {
    write_file(&dir.join(file), body);
    let (ok, stdout, _) = run(
        dir,
        &[
            "discover",
            symbol,
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover {symbol} failed");
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "{symbol} must resolve to exactly one definition:\n{stdout}"
    );
}

#[test]
fn exported_function_is_one_definition() {
    let dir = TempRepo::new("fn");
    discover_one_definition(&dir, "routeRequest", "flow.ts", EXPORTED_TS);
}

#[test]
fn exported_class_is_one_definition() {
    let dir = TempRepo::new("class");
    discover_one_definition(&dir, "Cache", "flow.ts", EXPORTED_TS);
}

#[test]
fn exported_const_arrow_is_one_definition() {
    let dir = TempRepo::new("arrow");
    discover_one_definition(&dir, "makeHandler", "flow.ts", EXPORTED_TS);
}

#[test]
fn default_exported_named_is_one_definition() {
    let dir = TempRepo::new("default");
    discover_one_definition(&dir, "defaultHandler", "flow.ts", EXPORTED_TS);
}

#[test]
fn ts_only_exported_members_are_one_definition() {
    let dir = TempRepo::new("tsonly");
    write_file(&dir.join("types.ts"), TS_ONLY_TS);
    for symbol in ["Foo", "Bar", "Baz"] {
        let (ok, stdout, _) = run(
            &dir,
            &[
                "discover",
                symbol,
                "--as",
                "symbol",
                "--scope",
                dir.to_str().unwrap(),
            ],
        );
        assert!(ok, "discover {symbol} failed");
        assert!(
            stdout.contains("1 matches (1 definitions)"),
            "{symbol} (TS-only exported member) must be one definition:\n{stdout}"
        );
    }
}

#[test]
fn nested_definition_inside_exported_class_still_found() {
    let dir = TempRepo::new("nested");
    write_file(&dir.join("box.ts"), NESTED_TS);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "inner",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover inner failed");
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "nested method inside exported class must still resolve:\n{stdout}"
    );
}

#[test]
fn usage_plus_one_exported_definition_has_no_caveat() {
    let dir = TempRepo::new("usage");
    write_file(&dir.join("u.ts"), USAGE_TS);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "handle",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover handle failed");
    assert!(
        stdout.contains("1 definitions"),
        "one definition plus a usage, no duplicate:\n{stdout}"
    );
    assert!(
        !stdout.contains("ambiguous"),
        "a single exported definition must not trigger the ambiguity caveat:\n{stdout}"
    );
}

#[test]
fn two_genuine_declarations_still_two_candidates() {
    let dir = TempRepo::new("genuine");
    write_file(&dir.join("a.ts"), GENUINE_A_TS);
    write_file(&dir.join("b.ts"), GENUINE_B_TS);

    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "handle",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover handle failed");
    assert!(
        stdout.contains("2 matches (2 definitions)"),
        "two genuine same-name declarations must stay two candidates:\n{stdout}"
    );

    // decision-flow must still report ambiguity, but across two DISTINCT
    // ranges, not the same range duplicated by the wrapper+inner pair.
    let (ok, stdout, stderr) = run(
        &dir,
        &["decision-flow", "handle", "--scope", dir.to_str().unwrap()],
    );
    assert!(!ok, "two genuine declarations should still hard-fail");
    let all = format!("{stdout}\n{stderr}");
    assert!(
        all.contains("ambiguous symbol target"),
        "genuine duplicates keep the ambiguity error:\n{all}"
    );
    assert!(
        all.contains("a.ts") && all.contains("b.ts"),
        "ambiguity must list both distinct files:\n{all}"
    );
}

#[test]
fn batch_search_agrees_with_single_symbol_search() {
    let dir = TempRepo::new("batch");
    write_file(&dir.join("types.ts"), TS_ONLY_TS);
    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "Foo,Bar",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "batch discover failed");
    // Each query's section must agree with its single-symbol result.
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "batch search must agree with single-symbol search:\n{stdout}"
    );
}

#[test]
fn decision_flow_on_exported_bare_name_succeeds() {
    let dir = TempRepo::new("df");
    write_file(&dir.join("flow.ts"), EXPORTED_TS);

    // REQUIRED acceptance: exported top-level function, const-arrow, and
    // default-exported named declaration each resolve by bare name.
    for symbol in ["routeRequest", "makeHandler", "defaultHandler"] {
        let (ok, stdout, stderr) = run(
            &dir,
            &["decision-flow", symbol, "--scope", dir.to_str().unwrap()],
        );
        let all = format!("{stdout}\n{stderr}");
        assert!(ok, "decision-flow {symbol} must succeed:\n{all}");
        assert!(
            !all.contains("ambiguous"),
            "decision-flow {symbol} must not be ambiguous:\n{all}"
        );
        assert!(
            stdout.contains("Decision-flow"),
            "decision-flow {symbol} must render output:\n{all}"
        );
    }

    let (ok, stdout, stderr) = run(
        &dir,
        &["context", "routeRequest", "--scope", dir.to_str().unwrap()],
    );
    let all = format!("{stdout}\n{stderr}");
    assert!(
        ok,
        "context routeRequest must resolve one exported definition:\n{all}"
    );
    assert!(
        !all.contains("ambiguous"),
        "context routeRequest must not report a duplicate candidate:\n{all}"
    );
}

#[test]
fn js_export_and_unrelated_language_unchanged() {
    let dir = TempRepo::new("regress");
    write_file(&dir.join("greet.js"), EXPORTED_JS);
    write_file(&dir.join("person.rb"), UNRELATED_RB);

    // JS exported function -> one definition.
    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "greet",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover greet (js) failed");
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "JS exported function must be one definition:\n{stdout}"
    );

    // Unrelated language (Ruby) definitions are untouched by the JS/TS change.
    let (ok, stdout, _) = run(
        &dir,
        &[
            "discover",
            "ruby_greet",
            "--as",
            "symbol",
            "--scope",
            dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "discover ruby_greet failed");
    assert!(
        stdout.contains("1 matches (1 definitions)"),
        "Ruby definition must stay one candidate:\n{stdout}"
    );
}
