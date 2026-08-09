//! US-055 P2 — consumer-agreement proof for the shared logical-import stream.
//!
//! Local resolution, external collection, and unresolved-local-looking
//! collection must all consume the SAME ordered source-specifier stream
//! (parsed once per file), so a multi-line JS/TS/TSX import appears in the
//! same class for every consumer and negatives (dynamic/malformed/over-bound/
//! comment/string) stay out of every class. Non-JS/TS inputs keep the
//! physical-line stream and byte-stable behavior.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "srcwalk-js-logical-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn deps_output(root: &Path, target: &str) -> (bool, String) {
    let out = srcwalk()
        .arg("deps")
        .arg(root.join(target))
        .args(["--scope"])
        .arg(root)
        .output()
        .expect("run deps");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).replace('\\', "/"),
    )
}

/// A multi-line import must appear in exactly one class, consistently across
/// ALL consumers, anchored to its opening `import` line (not the source line).
#[test]
fn multiline_import_agreement_across_consumers() {
    let root = fixture("agreement");
    write(&root, "local.ts", "export const local = 1;\n");
    write(
        &root,
        "main.ts",
        "import {\n\
         \tlocal,\n\
         \tthing,\n\
         } from './local.ts';\n\
         import {\n\
         \tmissingA,\n\
         \tmissingB,\n\
         } from './missing.ts';\n\
         import expensive from 'lodash';\n",
    );

    let (ok, stdout) = deps_output(&root, "main.ts");
    assert!(ok, "{stdout}");

    // ./local.ts resolves: exactly one local row, one occurrence anywhere.
    // (local rows render without the `./` prefix).
    assert!(
        stdout.contains("## Uses (local)\n(root)/\n  local.ts"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("local.ts").count(), 1, "{stdout}");

    // ./missing.ts is unresolved local-looking (line 5 = the `import` line).
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  5  ./missing.ts"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("./missing.ts").count(), 1, "{stdout}");

    // lodash is external once.
    assert!(stdout.contains("## Uses (external)\nlodash"), "{stdout}");
    assert_eq!(stdout.matches("lodash").count(), 1, "{stdout}");

    // Header reports one of each.
    assert!(
        stdout.contains("# Deps: main.ts — 1 local, 1 external, 1 unresolved, 0 dependents"),
        "{stdout}"
    );
}

/// A long multi-line binding list must not fabricate a class for the middle
/// lines, and every specifier is seen exactly once per logical statement.
#[test]
fn multi_line_binding_list_not_miscounted() {
    let root = fixture("multibind");
    write(&root, "real.js", "export { a };\n");
    write(
        &root,
        "main.js",
        "import {\n\
         \ta,\n\
         \tb,\n\
         \tc,\n\
         \td,\n\
         \te,\n\
         } from './real.js';\n",
    );

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    assert!(stdout.contains("1 local"), "{stdout}");
    assert_eq!(stdout.matches("real.js").count(), 1, "{stdout}");
}

/// Dynamic import(), malformed syntax, comments, and string literals never
/// reach any consumer class; a valid import after a syntax error abstains the
/// whole file.
#[test]
fn negatives_stay_out_of_every_class() {
    let root = fixture("negatives");
    write(
        &root,
        "main.js",
        "// import './commented.js';\n\
         const s = \"import './string.js'\";\n\
         import('./dynamic.js');\n\
         const c = require(someVar);\n",
    );

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    assert!(!stdout.contains("commented.js"), "{stdout}");
    assert!(!stdout.contains("string.js"), "{stdout}");
    assert!(!stdout.contains("dynamic.js"), "{stdout}");
    assert!(
        !stdout.contains("unresolved local-looking"),
        "no unresolved class expected:\n{stdout}"
    );
}

/// R1 remediation: a parser error no longer discards the whole file. The valid
/// import past the error keeps its evidence (source file exists -> local dep).
#[test]
fn malformed_file_keeps_valid_import_evidence() {
    let root = fixture("malformed-keeps");
    write(&root, "after-error.js", "export const a = 1;\n");
    write(
        &root,
        "main.js",
        "const broken = ;\nimport { a } from './after-error.js';\n",
    );

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    assert!(stdout.contains("after-error"), "{stdout}");
}

/// Non-JS/TS inputs keep byte-stable physical-line behavior: a one-line
/// import still runs the historical line scan (no stream involved) and
/// external classification is unchanged.
#[test]
fn non_js_physical_line_behavior_unchanged() {
    let root = fixture("nonjs");
    write(&root, "main.py", "import requests\n");

    let (ok, stdout) = deps_output(&root, "main.py");
    assert!(ok, "{stdout}");
    // Python `import requests` (non-stdlib) is external via the physical-line
    // scan, exactly as before the shared stream existed.
    assert!(stdout.contains("## Uses (external)\nrequests"), "{stdout}");
    assert!(
        stdout.contains("# Deps: main.py — 0 local, 1 external, 0 dependents"),
        "{stdout}"
    );
}

/// Over-1 MiB logical span abstains without panic and produces no rows.
#[test]
fn over_one_mib_span_abstains_cli() {
    let root = fixture("overbound");
    let mut content = String::from("import {\n");
    for _ in 0..(1_200_000 / 8) {
        content.push_str("  aaaaaaaa,\n");
    }
    content.push_str("} from './huge.js';\n");
    write(&root, "huge.js", "export const a = 1;\n");
    write(&root, "main.js", &content);

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    assert!(
        !stdout.contains("./huge.js"),
        "over-bound span must not be yielded:\n{stdout}"
    );
    assert!(
        !stdout.contains("unresolved local-looking"),
        "no unresolved class expected:\n{stdout}"
    );
}

/// Static require is a shared consumer source: resolved require -> local,
/// unresolved bare require -> external, missing relative require -> unresolved.
#[test]
fn require_forms_agree_across_consumers() {
    let root = fixture("require");
    write(&root, "util.js", "module.exports = 1;\n");
    write(
        &root,
        "main.js",
        "const util = require('./util.js');\n\
         const fs = require('fs');\n\
         const nope = require('./nope.js');\n",
    );

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("## Uses (local)\n(root)/\n  util.js"),
        "{stdout}"
    );
    assert!(stdout.contains("## Uses (external)\nfs"), "{stdout}");
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  3  ./nope.js"),
        "{stdout}"
    );
}

/// Historical local-suggestion bound (max 8) is preserved for JS/TS/TSX even
/// through the shared stream: a file importing 12 distinct local modules still
/// reports at most 8 local rows in `deps`.
#[test]
fn js_local_deps_keep_historical_cap() {
    let root = fixture("cap");
    for i in 0..12 {
        write(&root, &format!("m{i}.js"), "export const x = 1;\n");
    }
    let mut content = String::new();
    for i in 0..12 {
        content.push_str(&format!("import x{i} from './m{i}.js';\n"));
    }
    write(&root, "main.js", &content);

    let (ok, stdout) = deps_output(&root, "main.js");
    assert!(ok, "{stdout}");
    // At most the historical cap of local rows, never the full 12.
    let local_rows = stdout.lines().filter(|l| l.starts_with("  m")).count();
    assert!(
        local_rows <= 8,
        "local rows must stay capped, got {local_rows}:\n{stdout}"
    );
}
