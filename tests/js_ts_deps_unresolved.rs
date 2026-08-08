//! End-to-end proof of US-052 Phase 2: unresolved local-looking JS/TS/TSX
//! imports remain visible when resolution fails.
//!
//! `deps` keeps a distinct `Uses (unresolved local-looking)` evidence class
//! for canonical static module specifiers beginning `./`, `../`, `@/`, or
//! `~/` that resolve to no existing file. Resolved local, external, and
//! package-like sources never land there; no reverse dependents are invented;
//! existing behavior for other languages stays unchanged.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// A unique temp repo so parallel tests never share a directory.
fn fixture(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "srcwalk-js-deps-{name}-{}-{unique}",
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

fn deps_output(root: &Path, target: &str, extra: &[&str]) -> (bool, String, String) {
    let out = srcwalk()
        .arg("deps")
        .arg(root.join(target))
        .args(["--scope"])
        .arg(root)
        .args(extra)
        .output()
        .expect("run deps");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).replace('\\', "/"),
        String::from_utf8_lossy(&out.stderr).replace('\\', "/"),
    )
}

/// A mixed fixture: one resolved local + one external + four unresolved
/// local-looking imports. Each class must appear exactly once with its own
/// section and exact source lines.
#[test]
fn mixed_classes_each_appear_exactly_once() {
    let root = fixture("mixed");
    write(&root, "local.js", "export function local() {}\n");
    write(
        &root,
        "main.js",
        "import { local } from './local.js';\n\
         import ext from 'lodash';\n\
         import a from './missing-a.js';\n\
         import b from './missing-b.js';\n\
         import c from '@/store/c';\n\
         import d from '~/util/d';\n",
    );

    let (ok, stdout, _) = deps_output(&root, "main.js", &[]);
    assert!(ok, "{stdout}");
    // Exact header proves the positive unresolved count is reported.
    assert_eq!(
        stdout.lines().next().expect("header line"),
        "# Deps: main.js — 1 local, 1 external, 4 unresolved, 0 dependents",
        "{stdout}"
    );
    // Resolved local.
    assert!(
        stdout.contains("## Uses (local)\n(root)/\n  local.js"),
        "{stdout}"
    );
    // External.
    assert!(stdout.contains("## Uses (external)\nlodash"), "{stdout}");
    // Unresolved rows with exact lines.
    for (line, src) in [
        (3, "./missing-a.js"),
        (4, "./missing-b.js"),
        (5, "@/store/c"),
        (6, "~/util/d"),
    ] {
        assert!(
            stdout.contains(&format!("  {line}  {src}")),
            "missing {line} {src} in:\n{stdout}"
        );
    }
    // Each class exactly once.
    assert_eq!(stdout.matches("./missing-a.js").count(), 1, "{stdout}");
    assert_eq!(stdout.matches("lodash").count(), 1, "{stdout}");
    assert_eq!(stdout.matches("local.js").count(), 1, "{stdout}");
}

/// `@scope/pkg` and package-like unresolved sources stay external and never
/// appear as unresolved local-looking.
#[test]
fn scoped_and_package_sources_stay_external_not_unresolved() {
    let root = fixture("scoped");
    write(
        &root,
        "main.ts",
        "import a from '@scope/pkg';\nimport b from 'react-dom/client';\n",
    );

    let (ok, stdout, _) = deps_output(&root, "main.ts", &[]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("## Uses (external)\n@scope/pkg\nreact-dom/client"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("unresolved local-looking"),
        "no unresolved section expected:\n{stdout}"
    );
    // Zero unresolved => pre-Phase-2 header shape, byte-identical.
    assert!(
        stdout.contains("# Deps: main.ts — 0 local, 2 external, 0 dependents"),
        "{stdout}"
    );
}

/// Resolved local-looking imports never duplicate into unresolved, including
/// via JS/TS extension swap (`./a.js` -> `a.ts`, `./a` -> `a.ts`).
#[test]
fn resolved_local_looking_never_duplicates_into_unresolved() {
    let root = fixture("resolved");
    write(&root, "a.ts", "export const a = 1;\n");
    write(
        &root,
        "main.js",
        "import x from './a.js';\nimport y from './a';\nimport z from './missing.js';\n",
    );

    let (ok, stdout, _) = deps_output(&root, "main.js", &[]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  3  ./missing.js"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("./a"),
        "resolved local must not be unresolved:\n{stdout}"
    );
    assert_eq!(stdout.matches("a.ts").count(), 1, "{stdout}");
}

/// Comments, strings, dynamic `import(expr)`, and dynamic `require(expr)`
/// create no unresolved rows beyond the existing canonical static extraction.
#[test]
fn comments_strings_and_dynamic_forms_create_no_unresolved_rows() {
    let root = fixture("literals");
    write(
        &root,
        "main.js",
        "// import './commented.js';\n\
         const s = \"import './string.js'\";\n\
         import('./dynamic.js');\n\
         const c = require(someVar);\n\
         import real from './real.js';\n",
    );

    let (ok, stdout, _) = deps_output(&root, "main.js", &[]);
    assert!(ok, "{stdout}");
    assert!(stdout.contains("1 unresolved"), "{stdout}");
    assert!(stdout.contains("  5  ./real.js"), "{stdout}");
    assert!(!stdout.contains("commented.js"), "{stdout}");
    assert!(!stdout.contains("string.js"), "{stdout}");
    assert!(!stdout.contains("dynamic.js"), "{stdout}");
}

/// Duplicate unresolved imports dedupe deterministically to the earliest
/// line, sorted by line then source.
#[test]
fn duplicate_imports_dedupe_to_earliest_line() {
    let root = fixture("dedupe");
    write(
        &root,
        "main.js",
        "import './dup.js';\nimport './dup.js';\nimport './other.js';\n",
    );

    let (ok, stdout, _) = deps_output(&root, "main.js", &[]);
    assert!(ok, "{stdout}");
    assert_eq!(
        stdout.matches("./dup.js").count(),
        1,
        "dup must appear once:\n{stdout}"
    );
    assert!(stdout.contains("  1  ./dup.js"), "{stdout}");
    assert!(!stdout.contains("  2  ./dup.js"), "{stdout}");
    let i_dup = stdout.find("./dup.js").expect("dup present");
    let i_other = stdout.find("./other.js").expect("other present");
    assert!(i_dup < i_other, "line-then-source order:\n{stdout}");
}

/// Rows are capped (matching `MAX_UNRESOLVED_ROWS` in deps.rs) with an
/// omitted count, and a tight budget compacts without losing the unresolved
/// count from the header.
#[test]
fn cap_and_omitted_count_survive_tight_budget() {
    let root = fixture("cap");
    let mut content = String::new();
    for i in 0..25 {
        content.push_str(&format!("import './mod{i}.js';\n"));
    }
    write(&root, "main.js", &content);

    // No budget: rows capped at 20 with an omitted note.
    let (ok, stdout, _) = deps_output(&root, "main.js", &[]);
    assert!(ok, "{stdout}");
    assert_eq!(stdout.matches("./mod").count(), 20, "{stdout}");
    assert!(
        stdout.contains("… and 5 more unresolved local-looking imports"),
        "{stdout}"
    );

    // Tight budget: compacts and surfaces the caveat; header keeps the count.
    let (ok2, stdout2, _) = deps_output(&root, "main.js", &["--budget", "20"]);
    assert!(ok2, "{stdout2}");
    assert!(
        stdout2.contains("> Caveat: deps output was compacted for budget"),
        "{stdout2}"
    );
    assert!(stdout2.contains("25 unresolved"), "{stdout2}");
}

/// Unresolved import paths must never invent reverse `Used by` dependents.
#[test]
fn unresolved_imports_do_not_invent_reverse_dependents() {
    let root = fixture("reverse");
    write(&root, "a.js", "export function foo() {}\n");
    write(&root, "b.js", "import './missing.js';\n");

    let (ok, stdout, _) = deps_output(&root, "a.js", &[]);
    assert!(ok, "{stdout}");
    assert!(stdout.contains("0 dependents"), "{stdout}");
    assert!(
        !stdout.contains("b.js"),
        "b.js must not be a dependent:\n{stdout}"
    );

    let (ok2, stdout2, _) = deps_output(&root, "b.js", &[]);
    assert!(ok2, "{stdout2}");
    assert!(
        stdout2.contains("## Uses (unresolved local-looking)\n  1  ./missing.js"),
        "{stdout2}"
    );
    assert!(stdout2.contains("## Used by\n(none)"), "{stdout2}");
}

/// Regression: the `, N unresolved` header fragment appears only when
/// unresolved local-looking evidence exists. Every zero-unresolved header
/// (JS/TS included) stays byte-identical to the pre-Phase-2 shape, and a
/// positive unresolved count is reported when rows exist.
#[test]
fn zero_unresolved_headers_remain_byte_identical() {
    let root = fixture("headers");
    // Referenced files exist so local counts are deterministic.
    write(&root, "a.css", "body {}\n");
    write(&root, "a.md", "# a\n");
    // (language, path, source, exact expected first line)
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "rust",
            "r.rs",
            "use std::collections::HashMap;\n",
            "# Deps: r.rs — 0 local, 0 external, 0 dependents",
        ),
        (
            "python",
            "p.py",
            "import os\n",
            "# Deps: p.py — 0 local, 0 external, 0 dependents",
        ),
        (
            "go",
            "g.go",
            "package main\nimport \"fmt\"\n",
            "# Deps: g.go — 0 local, 0 external, 0 dependents",
        ),
        (
            "php",
            "ph.php",
            "<?php\nrequire_once \"vendor/autoload.php\";\n",
            "# Deps: ph.php — 0 local, 0 external, 0 dependents",
        ),
        (
            "css",
            "s.css",
            "@import \"a.css\";\n",
            "# Deps: s.css — 1 local, 0 external, 0 dependents",
        ),
        (
            "html",
            "i.html",
            "<link rel=\"stylesheet\" href=\"a.css\">\n",
            "# Deps: i.html — 1 local, 0 external, 0 dependents",
        ),
        (
            "markdown",
            "m.md",
            "[x](a.md)\n",
            "# Deps: m.md — 1 local, 0 external, 0 dependents",
        ),
        (
            "js-clean",
            "clean.js",
            "import ext from 'lodash';\n",
            "# Deps: clean.js — 0 local, 1 external, 0 dependents",
        ),
        (
            "ts-clean",
            "clean.ts",
            "import ext from 'lodash';\n",
            "# Deps: clean.ts — 0 local, 1 external, 0 dependents",
        ),
    ];
    for (name, path, source, expected) in cases {
        write(&root, path, source);
        let (ok, stdout, _) = deps_output(&root, path, &[]);
        assert!(ok, "{name}:\n{stdout}");
        let first_line = stdout.lines().next().expect("header line");
        assert_eq!(
            first_line, *expected,
            "{name} header must be byte-identical when unresolved == 0:\n{stdout}"
        );
        assert!(!stdout.contains("unresolved"), "{name}:\n{stdout}");
    }
}

/// TSX grammar, re-export (`export * from`), and extension-swap resolution are
/// covered; files without unresolved imports render no unresolved section.
#[test]
fn tsx_reexports_and_clean_files_stay_stable() {
    let root = fixture("tsx");
    write(&root, "mod.ts", "export function helper() {}\n");
    write(
        &root,
        "main.tsx",
        "import { helper } from './mod.js';\nexport * from './gone';\nimport { x } from '@/store';\n",
    );
    write(&root, "clean.js", "import ext from 'lodash';\n");

    let (ok, stdout, _) = deps_output(&root, "main.tsx", &[]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("mod.ts"),
        "extension swap resolves:\n{stdout}"
    );
    assert_eq!(stdout.matches("mod.ts").count(), 1, "{stdout}");
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  2  ./gone\n  3  @/store"),
        "{stdout}"
    );

    // A JS file with no unresolved imports renders no unresolved section.
    let (ok2, stdout2, _) = deps_output(&root, "clean.js", &[]);
    assert!(ok2, "{stdout2}");
    assert!(
        stdout2.contains("# Deps: clean.js — 0 local, 1 external, 0 dependents"),
        "{stdout2}"
    );
    assert!(
        !stdout2.contains("unresolved local-looking"),
        "no unresolved section expected:\n{stdout2}"
    );
}
