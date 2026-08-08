//! Integration tests for US-052 Phase 1: honest JS/TS/TSX/Python Flow Map
//! hard abstention on unrepresentable direct-flow constructs.
//!
//! A selected function containing a direct `try_statement`,
//! `break_statement` (loop-exit), or `continue_statement` must hard-abstain
//! atomically with the exact language/construct/line reason and zero partial
//! graph. Nested function-like/class scopes and comments/strings never count.
//! Switch-case `break` (a switch terminator, not an abrupt loop edge) remains
//! representable and byte-stable. `context` retains the packet + exact caveat;
//! unrelated `InvalidQuery` errors still propagate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// A temp repo that removes its directory on drop (RAII cleanup; matches
/// existing integration-test hygiene).
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "srcwalk_js_ts_abs_{name}_{}_{}",
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

fn norm_path_separators(s: &str) -> String {
    s.replace('\\', "/")
}

/// Run a srcwalk subcommand with cwd = repo and return (success, stdout, stderr).
fn run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = srcwalk()
        .current_dir(dir)
        .args(args)
        .output()
        .expect("srcwalk runs");
    (
        out.status.success(),
        norm_path_separators(&String::from_utf8_lossy(&out.stdout)),
        norm_path_separators(&String::from_utf8_lossy(&out.stderr)),
    )
}

/// 1-based line number of the first source line containing `needle`, matching
/// the tree-sitter line numbers the CLI reports (avoids brittle hand counts).
fn line_of(source: &str, needle: &str) -> usize {
    source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{source}"))
        + 1
}

/// Every direct-flow construct the IR cannot represent must hard-abstain with
/// the exact language, construct, line, and edge reason, and emit no partial
/// graph. Covers JS, TS, TSX, and Python for try/break/continue.
#[test]
fn js_ts_py_abstention_parameterized_never_partial_graph() {
    let dir = TempRepo::new("kinds");
    let cases = [
        (
            "js_try",
            "try.js",
            r#"function risky() {
  work();
  try {
    maybe();
  } catch (e) {
    handle(e);
  }
  done();
}
"#,
            "risky",
            "try_statement",
            "try {",
            "JavaScript Flow Map abstained:",
            "exception-handling/finally propagation edges",
        ),
        (
            "js_break",
            "loop.js",
            r#"function scan(items) {
  for (const it of items) {
    if (it.skip()) {
      break;
    }
    use(it);
  }
}
"#,
            "scan",
            "break_statement",
            "break;",
            "JavaScript Flow Map abstained:",
            "abrupt loop-exit edges",
        ),
        (
            "js_continue",
            "cont.js",
            r#"function scan(items) {
  for (const it of items) {
    if (it.skip()) {
      continue;
    }
    use(it);
  }
}
"#,
            "scan",
            "continue_statement",
            "continue;",
            "JavaScript Flow Map abstained:",
            "abrupt loop-continue edges",
        ),
        (
            "ts_try",
            "try.ts",
            r#"export function risky(): void {
  work();
  try {
    maybe();
  } catch (e) {
    handle(e);
  }
  done();
}
"#,
            "risky",
            "try_statement",
            "try {",
            "TypeScript Flow Map abstained:",
            "exception-handling/finally propagation edges",
        ),
        (
            "tsx_break",
            "comp.tsx",
            r#"export function Comp(props: { items: string[] }) {
  for (const it of props.items) {
    if (it === "stop") {
      break;
    }
    render(it);
  }
  return null;
}
"#,
            "Comp",
            "break_statement",
            "break;",
            "TSX Flow Map abstained:",
            "abrupt loop-exit edges",
        ),
        (
            "py_try",
            "parity.py",
            r#"def risky():
    work()
    try:
        maybe()
    except Exception:
        handle()
    done()
"#,
            "risky",
            "try_statement",
            "try:",
            "Python Flow Map abstained:",
            "exception-handling/finally propagation edges",
        ),
        (
            "py_break",
            "loop.py",
            r#"def scan(items):
    for it in items:
        if it.skip():
            break
        use(it)
"#,
            "scan",
            "break_statement",
            "break",
            "Python Flow Map abstained:",
            "abrupt loop-exit edges",
        ),
        (
            "py_continue",
            "cont.py",
            r#"def scan(items):
    for it in items:
        if it.skip():
            continue
        use(it)
"#,
            "scan",
            "continue_statement",
            "continue",
            "Python Flow Map abstained:",
            "abrupt loop-continue edges",
        ),
    ];

    for (name, path, source, symbol, kind, needle, marker, why) in cases {
        write_file(&dir.join(path), source);
        let line = line_of(source, needle);
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{path}:{symbol}")]);
        assert!(!ok, "{name} must fail loudly");
        let expected = format!("{marker} unsupported {kind} at line {line} requires {why}");
        assert!(
            stderr.contains(&expected),
            "{name}: expected {expected:?} in stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("[flow]"),
            "{name}: must not emit a partial graph:\n{stdout}"
        );
    }
}

/// A nested arrow/function/class scope with unsupported constructs must not
/// poison the selected parent function.
#[test]
fn js_nested_scopes_do_not_poison_parent() {
    let dir = TempRepo::new("nested_js");
    write_file(
        &dir.join("nested.js"),
        r#"function outer(items) {
  const helper = () => {
    try {
      inner();
    } catch (e) {
      recover(e);
    }
  };
  function declared() {
    while (true) {
      break;
    }
  }
  class Worker {
    run() {
      for (const it of items) {
        continue;
      }
    }
  }
  const obj = {
    act() {
      try {
        go();
      } catch (e) {
        stop(e);
      }
    },
  };
  helper();
  declared();
  const w = new Worker();
  w.run();
  done(items);
}
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "nested.js:outer"]);
    assert!(
        ok,
        "outer must not abstain for nested scopes, stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("abstained"),
        "outer must not abstain:\n{stdout}"
    );
    assert!(stdout.contains("done(items)"), "{stdout}");
}

/// A nested Python def/class/lambda with unsupported constructs must not
/// poison the selected parent function.
#[test]
fn python_nested_scopes_do_not_poison_parent() {
    let dir = TempRepo::new("nested_py");
    write_file(
        &dir.join("nested.py"),
        r#"def outer(items):
    def inner():
        try:
            work()
        except Exception:
            recover()
    class Worker:
        def run(self):
            for it in items:
                break
    for it in items:
        inner()
    return items
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "nested.py:outer"]);
    assert!(
        ok,
        "outer must not abstain for nested defs/class, stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("abstained"),
        "outer must not abstain:\n{stdout}"
    );
    assert!(stdout.contains("inner()"), "{stdout}");
}

/// Comments and strings containing the keywords must never count.
#[test]
fn js_py_comments_and_strings_do_not_abstain() {
    let dir = TempRepo::new("literals");
    write_file(
        &dir.join("lit.js"),
        r#"function outer() {
  // try { break; continue; }
  const a = "try { break; }";
  const b = "continue; break; try";
  const c = `template try { break; continue; }`;
  work(a, b, c);
}
"#,
    );
    write_file(
        &dir.join("lit.py"),
        r#"def outer():
    # try: break continue
    a = "try: break continue"
    b = f"no {a}"
    work(a, b)
"#,
    );

    for (path, symbol, needle) in [
        ("lit.js", "outer", "work(a, b, c)"),
        ("lit.py", "outer", "work(a, b)"),
    ] {
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{path}:{symbol}")]);
        assert!(ok, "literals must not abstain ({path}), stderr:\n{stderr}");
        assert!(
            !stdout.contains("abstained"),
            "literals must not abstain ({path}):\n{stdout}"
        );
        assert!(
            stdout.contains(needle),
            "missing {needle:?} ({path}):\n{stdout}"
        );
    }
}

/// A switch-case `break` is a case terminator (representable), not an abrupt
/// loop edge; idiomatic switch Flow Maps must stay byte-stable.
#[test]
fn js_switch_case_break_remains_rendered() {
    let dir = TempRepo::new("switch_break");
    write_file(
        &dir.join("route.js"),
        r#"function route(mode) {
  switch (mode) {
    case "text":
      runText();
      break;
    case "bin":
      runBin();
      break;
    default:
      runDefault();
  }
}
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "route.js:route"]);
    assert!(ok, "switch-case break is representable, stderr:\n{stderr}");
    assert!(
        !stdout.contains("abstained"),
        "switch must not abstain: {stdout}"
    );
    for needle in [
        "\"text\" =>",
        "\"bin\" =>",
        "default =>",
        "runText",
        "runBin",
        "runDefault",
    ] {
        assert!(stdout.contains(needle), "missing {needle:?} in:\n{stdout}");
    }
}

/// A `break` inside a loop nested under a switch case is still an abrupt loop
/// exit and must abstain.
#[test]
fn js_loop_break_inside_switch_case_still_abstains() {
    let dir = TempRepo::new("switch_loop_break");
    write_file(
        &dir.join("route.js"),
        r#"function route(items, mode) {
  switch (mode) {
    case "text":
      for (const it of items) {
        if (it.skip()) {
          break;
        }
        use(it);
      }
      break;
    default:
      runDefault();
  }
}
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "route.js:route"]);
    assert!(!ok, "loop break under switch must abstain");
    assert!(
        stderr.contains(
            "JavaScript Flow Map abstained: unsupported break_statement at line 6 requires abrupt loop-exit edges"
        ),
        "stderr:\n{stderr}"
    );
    assert!(!stdout.contains("[flow]"), "{stdout}");
}

/// Unlabeled switch-case `break` stays representable; a *labeled* direct switch
/// break (`break outer;`) escapes the switch to an enclosing labeled loop and
/// must hard-abstain as an abrupt exit. Covers JS, TS, and TSX grammar parity.
#[test]
fn labeled_switch_break_abstains_but_unlabeled_renders_all_grammars() {
    let dir = TempRepo::new("labeled_switch_break");
    let cases = [
        (
            "js_labeled",
            "route.js",
            "JavaScript Flow Map abstained:",
            r#"function route(mode) {
  outer: for (;;) {
    switch (mode) {
      case "x":
        runText();
        break outer;
      default:
        runDefault();
    }
  }
}
"#,
        ),
        (
            "ts_labeled",
            "route.ts",
            "TypeScript Flow Map abstained:",
            r#"export function route(mode: string): void {
  outer: for (;;) {
    switch (mode) {
      case "x":
        runText();
        break outer;
      default:
        runDefault();
    }
  }
}
"#,
        ),
        (
            "tsx_labeled",
            "route.tsx",
            "TSX Flow Map abstained:",
            r#"export function Route(mode: string): JSX.Element {
  outer: for (;;) {
    switch (mode) {
      case "x":
        runText();
        break outer;
      default:
        runDefault();
    }
  }
  return null;
}
"#,
        ),
    ];
    for (name, path, marker, source) in cases {
        write_file(&dir.join(path), source);
        let line = line_of(source, "break outer;");
        let symbol = if path.ends_with(".tsx") {
            "Route"
        } else {
            "route"
        };
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{path}:{symbol}")]);
        assert!(!ok, "{name} must fail loudly");
        let expected = format!(
            "{marker} unsupported break_statement at line {line} requires abrupt loop-exit edges"
        );
        assert!(
            stderr.contains(&expected),
            "{name}: expected {expected:?} in stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("[flow]"),
            "{name}: must not emit a partial graph:\n{stdout}"
        );
    }

    // Unlabeled direct switch break stays representable in all three grammars.
    let unlabeled = [
        (
            "js_unlabeled",
            "u.js",
            "route",
            r#"function route(mode) {
  switch (mode) {
    case "x":
      runText();
      break;
  }
}
"#,
        ),
        (
            "ts_unlabeled",
            "u.ts",
            "route",
            r#"export function route(mode: string): void {
  switch (mode) {
    case "x":
      runText();
      break;
  }
}
"#,
        ),
        (
            "tsx_unlabeled",
            "u.tsx",
            "Route",
            r#"export function Route(mode: string): JSX.Element {
  switch (mode) {
    case "x":
      runText();
      break;
  }
  return null;
}
"#,
        ),
    ];
    for (name, path, symbol, source) in unlabeled {
        write_file(&dir.join(path), source);
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{path}:{symbol}")]);
        assert!(ok, "{name} must stay representable, stderr:\n{stderr}");
        assert!(
            !stdout.contains("abstained"),
            "{name} must not abstain:\n{stdout}"
        );
        assert!(stdout.contains("runText"), "{name}:\n{stdout}");
    }
}

/// Braced case bodies (`case "x": { ... break; }`) keep the unlabeled switch
/// break representable in JS/TS/TSX: a `statement_block` between the break and
/// its switch_case/default is transparent scope, not a control boundary. A
/// conditional break (`if (ready) break;`) crosses an `if` and must abstain.
#[test]
fn braced_switch_case_break_renders_but_conditional_break_abstains() {
    let dir = TempRepo::new("braced_switch_break");

    // Braced case bodies render in all three grammars.
    let braced = [
        (
            "js_braced",
            "b.js",
            "route",
            r#"function route(mode) {
  switch (mode) {
    case "text": {
      runText();
      break;
    }
    default: {
      runDefault();
      break;
    }
  }
}
"#,
        ),
        (
            "ts_braced",
            "b.ts",
            "route",
            r#"export function route(mode: string): void {
  switch (mode) {
    case "text": {
      runText();
      break;
    }
    default: {
      runDefault();
      break;
    }
  }
}
"#,
        ),
        (
            "tsx_braced",
            "b.tsx",
            "Route",
            r#"export function Route(mode: string): JSX.Element {
  switch (mode) {
    case "text": {
      runText();
      break;
    }
    default: {
      runDefault();
      break;
    }
  }
  return null;
}
"#,
        ),
    ];
    for (name, path, symbol, source) in braced {
        write_file(&dir.join(path), source);
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("{path}:{symbol}")]);
        assert!(
            ok,
            "{name} braced switch break must stay representable, stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("abstained"),
            "{name} must not abstain:\n{stdout}"
        );
        assert!(stdout.contains("runText"), "{name}:\n{stdout}");
    }

    // A conditional break crosses an `if` control node and must abstain.
    let cond_src = r#"function route(mode, ready) {
  switch (mode) {
    case "text":
      if (ready) {
        break;
      }
      runText();
  }
}
"#;
    write_file(&dir.join("cond.js"), cond_src);
    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "cond.js:route"]);
    assert!(!ok, "conditional break under switch must abstain");
    let cond_line = line_of(cond_src, "break;");
    let expected = format!(
        "JavaScript Flow Map abstained: unsupported break_statement at line {cond_line} requires abrupt loop-exit edges"
    );
    assert!(
        stderr.contains(&expected),
        "expected {expected:?} in stderr:\n{stderr}"
    );
    assert!(!stdout.contains("[flow]"), "{stdout}");
}

/// Supported loop/return Flow Maps with no abrupt edges remain rendered.
#[test]
fn js_loop_and_return_flow_maps_remain_supported() {
    let dir = TempRepo::new("loop_positive");
    write_file(
        &dir.join("scan.js"),
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

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "scan.js:first"]);
    assert!(
        ok,
        "loop+return flow map must stay supported, stderr:\n{stderr}"
    );
    assert!(!stdout.contains("abstained"), "{stdout}");
    for needle in ["[loop]", "it.match()", "[return]"] {
        assert!(stdout.contains(needle), "missing {needle:?} in:\n{stdout}");
    }
}

/// `context` must retain the packet, surface the exact language/construct/line
/// caveat, and omit the partial Flow Map.
#[test]
fn js_context_fallback_retains_packet_and_exact_caveat() {
    let dir = TempRepo::new("context_fallback");
    let src = r#"function risky() {
  work();
  try {
    maybe();
  } catch (e) {
    handle(e);
  }
}
"#;
    write_file(&dir.join("try.js"), src);

    let (ok, stdout, stderr) = run(&dir, &["context", "try.js:risky"]);
    assert!(ok, "context must fall back gracefully, stderr:\n{stderr}");
    assert!(stdout.contains("## Target"), "{stdout}");
    assert!(stdout.contains("## Flow Map"), "{stdout}");
    assert!(stdout.contains("## Exits"), "{stdout}");
    assert!(
        stdout.contains(
            "file-level evidence only; structural function map unavailable for this target"
        ),
        "{stdout}"
    );
    let try_line = line_of(src, "try {");
    let caveat = format!(
        "caveat: JavaScript Flow Map abstained: unsupported try_statement at line {try_line} requires exception-handling/finally propagation edges"
    );
    assert!(
        stdout.contains(&caveat),
        "expected {caveat:?} in:\n{stdout}"
    );
    // No partial graph in the fallback.
    assert!(!stdout.contains("shape:"), "{stdout}");
    assert!(!stdout.contains("decision :"), "{stdout}");
    assert!(
        stdout.contains("not available from structural parser"),
        "{stdout}"
    );
}

/// An unrelated `InvalidQuery` must still propagate, not fall back.
#[test]
fn js_unrelated_invalid_query_is_not_swallowed() {
    let dir = TempRepo::new("unrelated_error");
    write_file(&dir.join("txt.txt"), "plain text, no code\n");

    let (ok, stdout, stderr) = run(&dir, &["context", "txt.txt"]);
    assert!(!ok, "unrelated InvalidQuery must propagate, not fall back");
    assert!(
        stderr.contains("target needs a symbol, line, or range"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("## Flow Map"),
        "must not render a fallback packet:\n{stdout}"
    );
}
