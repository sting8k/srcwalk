use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn map_default_is_compact_without_symbols() {
    let dir = temp_repo("map_compact");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("sizes ~= tokens"),
        "expected units in header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("lib.rs  ~") && stdout.contains("src/  ~"),
        "expected compact file/dir sizes, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("lib.rs: alpha") && !stdout.contains("~9 tokens"),
        "default map should not include symbols or repeated token units, got:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: add --symbols") && stdout.contains("--scope <dir>"),
        "expected compact map footer, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_uses_auto_depth_only_when_depth_is_omitted() {
    let dir = temp_repo("map_auto_depth");
    for i in 0..101 {
        fs::write(dir.join(format!("file_{i:03}.rs")), "pub fn alpha() {}\n").unwrap();
    }

    let auto = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(auto.status.success(), "expected auto map to succeed");
    let stdout = String::from_utf8_lossy(&auto.stdout);
    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| line.contains("(depth auto→2")),
        "expected auto depth header, got:\n{stdout}"
    );

    let explicit = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("3")
        .output()
        .unwrap();
    assert!(
        explicit.status.success(),
        "expected explicit map to succeed"
    );
    let stdout = String::from_utf8_lossy(&explicit.stdout);
    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| line.contains("(depth 3")),
        "expected explicit depth header, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_auto_depth_retries_lower_depth_to_fit_cap() {
    let dir = temp_repo("map_auto_depth_retry");
    let long_a = "a".repeat(220);
    let long_b = "b".repeat(220);
    for i in 0..140 {
        let top = format!("{i:03}_{long_a}");
        let sub = format!("{i:03}_{long_b}");
        let path = dir.join(top).join(sub);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("main.rs"), "pub fn alpha() {}\n").unwrap();
    }

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "auto depth should retry lower before failing:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| line.contains("(depth auto→1")),
        "expected reduced auto depth header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("# Note: depth reduced to fit cap."),
        "expected short reduced-depth note, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_filters_to_source_files_only() {
    let dir = temp_repo("map_source_filter");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("package.json"), "{}\n").unwrap();
    fs::write(dir.join("README.md"), "# docs\n").unwrap();
    fs::write(dir.join("composer.lock"), "{}\n").unwrap();
    fs::write(dir.join("logo.svg"), "<svg/>\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lib.rs"),
        "expected code files, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("README.md")
            && !stdout.contains("package.json")
            && !stdout.contains("composer.lock")
            && !stdout.contains("logo.svg"),
        "default map should omit non-source files, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_rejects_budget_controls() {
    let dir = temp_repo("map_budget_reject");
    fs::write(dir.join("lib.rs"), "pub fn alpha() {}\n").unwrap();

    let budget = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--budget")
        .arg("80")
        .output()
        .unwrap();
    assert!(!budget.status.success(), "expected --map --budget to fail");
    let stderr = String::from_utf8_lossy(&budget.stderr);
    assert!(
        stderr.contains("fixed 15k token cap"),
        "expected fixed-cap error, got:\n{stderr}"
    );

    let no_budget = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(
        !no_budget.status.success(),
        "expected --map --no-budget to fail"
    );
    let stderr = String::from_utf8_lossy(&no_budget.stderr);
    assert!(
        stderr.contains("fixed 15k token cap"),
        "expected fixed-cap error, got:\n{stderr}"
    );

    let command_budget = srcwalk()
        .arg("--budget")
        .arg("80")
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        !command_budget.status.success(),
        "expected top-level --budget with map command to fail"
    );
    let stderr = String::from_utf8_lossy(&command_budget.stderr);
    assert!(
        stderr.contains("fixed 15k token cap"),
        "expected fixed-cap error, got:\n{stderr}"
    );

    let command_no_budget = srcwalk()
        .arg("--no-budget")
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        !command_no_budget.status.success(),
        "expected top-level --no-budget with map command to fail"
    );
    let stderr = String::from_utf8_lossy(&command_no_budget.stderr);
    assert!(
        stderr.contains("fixed 15k token cap"),
        "expected fixed-cap error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_hard_cap_aborts_without_partial_output() {
    let dir = temp_repo("map_hard_cap");
    for i in 0..1200 {
        let name = format!(
            "file_{i:04}_{}_{}.rs",
            "very_long_component_name", "very_long_component_name"
        );
        fs::write(dir.join(name), "pub fn alpha() {}\n").unwrap();
    }

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(!out.status.success(), "expected hard-cap abort");
    assert!(
        out.stdout.is_empty(),
        "hard-cap abort must not print partial map, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("output too large")
            && stderr.contains("hard cap 15000")
            && stderr.contains("srcwalk deps <file>"),
        "expected actionable hard-cap error, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_degrades_to_structure_only_when_relations_exceed_cap() {
    let dir = temp_repo("map_relations_degrade");
    fs::write(dir.join("go.mod"), "module example.com/app\n").unwrap();
    let long = "very_long_relation_group_name";
    for i in 0..420 {
        let source_dir = dir.join(format!("m{i:03}_{long}"));
        let target_dir = dir.join(format!("n{i:03}_{long}"));
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(
            source_dir.join("main.go"),
            format!("package m{i}\n\nimport _ \"example.com/app/n{i:03}_{long}\"\n"),
        )
        .unwrap();
        fs::write(target_dir.join("target.go"), format!("package n{i}\n")).unwrap();
    }

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected structure-only degrade to succeed"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("relations omitted to fit 15000 token cap")
            && !stdout.contains("[relations]"),
        "expected explicit structure-only degrade, got:\n{stdout}"
    );
    assert!(
        stdout.contains("main.go"),
        "degraded output should keep structure, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_omits_relations_when_no_local_deps() {
    let dir = temp_repo("map_no_relations");
    fs::write(dir.join("lib.rs"), "pub fn alpha() {}\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("[relations]"),
        "empty relation section should be omitted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_relations_show_static_local_rust_deps() {
    let dir = temp_repo("map_rust_relations");
    fs::create_dir_all(dir.join("src/commands")).unwrap();
    fs::create_dir_all(dir.join("src/search")).unwrap();
    fs::write(
        dir.join("src/commands/flow.rs"),
        "use crate::search::callees;\npub fn run() {}\n",
    )
    .unwrap();
    fs::write(dir.join("src/search/callees.rs"), "pub fn resolve() {}\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(dir.join("src"))
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[relations]")
            && stdout.contains("commands deps:1")
            && stdout.contains("  -> search deps:1")
            && stdout.contains("not runtime calls"),
        "expected static relation row, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_relations_group_root_scope_files() {
    let dir = temp_repo("map_root_file_relations");
    fs::create_dir_all(dir.join("src/feature")).unwrap();
    fs::write(
        dir.join("src/feature/run.rs"),
        "use crate::types;\npub fn run() {}\n",
    )
    .unwrap();
    fs::write(dir.join("src/types.rs"), "pub struct Config;\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(dir.join("src"))
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("feature deps:1")
            && stdout.contains("  -> (root) deps:1")
            && !stdout.contains("feature ->"),
        "expected root-scope file deps to be grouped, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_relations_show_go_module_imports() {
    let dir = temp_repo("map_go_relations");
    fs::create_dir_all(dir.join("cmd/server")).unwrap();
    fs::create_dir_all(dir.join("internal/runtime")).unwrap();
    fs::write(dir.join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(
        dir.join("cmd/server/main.go"),
        "package main\n\nimport (\n    \"example.com/app/internal/runtime\"\n)\n",
    )
    .unwrap();
    fs::write(dir.join("internal/runtime/runtime.go"), "package runtime\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cmd/server deps:1") && stdout.contains("  -> internal/runtime deps:1"),
        "expected Go module relation, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_go_relations_respect_glob_visible_targets() {
    let dir = temp_repo("map_go_relations_glob_visibility");
    fs::create_dir_all(dir.join("cmd/server")).unwrap();
    fs::create_dir_all(dir.join("internal/runtime")).unwrap();
    fs::write(dir.join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(
        dir.join("cmd/server/main.go"),
        "package main\n\nimport \"example.com/app/internal/runtime\"\n",
    )
    .unwrap();
    fs::write(dir.join("internal/runtime/runtime.go"), "package runtime\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .arg("--glob")
        .arg("cmd/**/*.go")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("main.go") && !stdout.contains("internal/runtime"),
        "glob should hide target tree, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("cmd/server -> internal/runtime"),
        "relations should not reference hidden target groups, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_shows_outbound_go_deps_when_scope_is_narrow() {
    let dir = temp_repo("map_go_outbound_deps");
    fs::create_dir_all(dir.join("examples/custom-provider")).unwrap();
    fs::create_dir_all(dir.join("sdk/api")).unwrap();
    fs::create_dir_all(dir.join("sdk/auth")).unwrap();
    fs::create_dir_all(dir.join("sdk/cliproxy/auth")).unwrap();
    fs::create_dir_all(dir.join("sdk/cliproxy/executor")).unwrap();
    fs::write(dir.join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(
        dir.join("examples/custom-provider/main.go"),
        "package main\n\nimport (\n    _ \"example.com/app/sdk/api\"\n    _ \"example.com/app/sdk/auth\"\n    _ \"example.com/app/sdk/cliproxy/auth\"\n    _ \"example.com/app/sdk/cliproxy/executor\"\n)\n",
    )
    .unwrap();
    fs::write(dir.join("sdk/api/api.go"), "package api\n").unwrap();
    fs::write(dir.join("sdk/auth/auth.go"), "package auth\n").unwrap();
    fs::write(dir.join("sdk/cliproxy/auth/auth.go"), "package auth\n").unwrap();
    fs::write(
        dir.join("sdk/cliproxy/executor/executor.go"),
        "package executor\n",
    )
    .unwrap();
    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(dir.join("examples"))
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[relations] 0 in-scope groups"),
        "expected empty in-scope header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[outbound deps] 3 groups (targets outside scope)"),
        "expected outbound deps header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("examples/custom-provider deps:4")
            && stdout.contains("  -> sdk/api deps:1")
            && stdout.contains("  -> sdk/auth deps:1")
            && stdout.contains("  -> sdk/cliproxy deps:2")
            && !stdout.contains("examples/custom-provider ->")
            && !stdout.contains("sdk/cliproxy/auth")
            && !stdout.contains("sdk/cliproxy/executor"),
        "expected outbound deps grouped by source/target modules, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_go_relations_discover_nested_modules_from_repo_root() {
    let dir = temp_repo("map_go_nested_module_relations");
    fs::create_dir_all(dir.join("services/app/cmd")).unwrap();
    fs::create_dir_all(dir.join("services/app/internal/runtime")).unwrap();
    fs::write(dir.join("services/app/go.mod"), "module example.com/app\n").unwrap();
    fs::write(
        dir.join("services/app/cmd/main.go"),
        "package main\n\nimport \"example.com/app/internal/runtime\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("services/app/internal/runtime/runtime.go"),
        "package runtime\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("4")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("services/app/cmd deps:1")
            && stdout.contains("  -> services/app/internal/runtime deps:1"),
        "expected nested module relation from repo root, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_relations_smoke_js_and_python_imports() {
    let dir = temp_repo("map_js_python_relations");
    fs::create_dir_all(dir.join("web/app")).unwrap();
    fs::create_dir_all(dir.join("web/lib")).unwrap();
    fs::write(dir.join("web/app/main.ts"), "import '../lib/helper';\n").unwrap();
    fs::write(
        dir.join("web/lib/helper.ts"),
        "export function helper() {}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("py/app")).unwrap();
    fs::create_dir_all(dir.join("py/lib")).unwrap();
    fs::write(dir.join("py/app/main.py"), "from ..lib.util import util\n").unwrap();
    fs::write(dir.join("py/lib/util.py"), "def util(): pass\n").unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("web/app deps:1")
            && stdout.contains("  -> web/lib deps:1")
            && stdout.contains("py/app deps:1")
            && stdout.contains("  -> py/lib deps:1"),
        "expected JS and Python local relations, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_relations_show_php_psr4_imports() {
    let dir = temp_repo("map_php_psr4_relations");
    fs::create_dir_all(dir.join("classes")).unwrap();
    fs::create_dir_all(dir.join("src/Core")).unwrap();
    fs::write(
        dir.join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("classes/Foo.php"),
        "<?php\nuse App\\Core\\Bar;\nclass Foo {}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/Core/Bar.php"),
        "<?php\nnamespace App\\Core;\nclass Bar {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("classes deps:1") && stdout.contains("  -> src/Core deps:1"),
        "expected PHP PSR-4 relation, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_honors_depth() {
    let dir = temp_repo("map_depth");
    fs::create_dir_all(dir.join("src/nested")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("src/nested/deep.rs"), "pub fn beta() {}\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --depth to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("depth 1"),
        "expected depth in header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("src/"),
        "expected depth-1 dir, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("deep.rs") && !stdout.contains("nested/"),
        "expected deeper entries to be excluded, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_rolls_up_deep_source_dirs_beyond_depth() {
    let dir = temp_repo("map_deep_rollup");
    fs::create_dir_all(dir.join("src/nested/pkg")).unwrap();
    fs::write(
        dir.join("src/nested/pkg/deep.rs"),
        "pub fn only_deep_source() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --depth to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/"),
        "expected shallow dir rollup for deep source, got:\n{stdout}"
    );
    assert!(
        stdout.contains("~"),
        "expected token rollup for deep source, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("nested/") && !stdout.contains("deep.rs"),
        "expected deeper entries to stay hidden at depth 1, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_honors_glob() {
    let dir = temp_repo("map_glob");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    fs::write(dir.join("src/app.ts"), "export function beta() {}\n").unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--glob")
        .arg("*.rs")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --glob to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lib.rs"),
        "expected rs file, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("app.ts") && !stdout.contains("README.md"),
        "expected glob to exclude non-rs files, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_rejects_filter_and_json_noops() {
    let dir = temp_repo("map_noops");
    fs::write(dir.join("lib.rs"), "pub fn alpha() {}\n").unwrap();

    let filter = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--filter")
        .arg("path:src")
        .output()
        .unwrap();
    assert!(!filter.status.success(), "expected --map --filter to fail");

    let json = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--json")
        .output()
        .unwrap();
    assert!(!json.status.success(), "expected --map --json to fail");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_sorts_dirs_and_files_by_size() {
    let dir = temp_repo("map_sort");
    fs::create_dir_all(dir.join("small_dir")).unwrap();
    fs::create_dir_all(dir.join("large_dir")).unwrap();
    fs::write(dir.join("small_dir/tiny.rs"), "x\n").unwrap();
    fs::write(dir.join("large_dir/big.rs"), "x\n".repeat(200)).unwrap();
    fs::write(dir.join("small_root.rs"), "x\n").unwrap();
    fs::write(dir.join("large_root.rs"), "x\n".repeat(100)).unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1")
        .output()
        .unwrap();

    assert!(out.status.success(), "expected sorted map to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let large_dir = stdout.find("large_dir/").expect("large_dir missing");
    let small_dir = stdout.find("small_dir/").expect("small_dir missing");
    let large_file = stdout.find("large_root.rs").expect("large_root missing");
    let small_file = stdout.find("small_root.rs").expect("small_root missing");

    assert!(
        large_dir < small_dir,
        "expected larger dir before smaller dir, got:\n{stdout}"
    );
    assert!(
        small_dir < large_file,
        "expected dirs before root files, got:\n{stdout}"
    );
    assert!(
        large_file < small_file,
        "expected larger file before smaller file, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_symbols_includes_symbol_names() {
    let dir = temp_repo("map_symbols");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("--map")
        .arg("--symbols")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected map --symbols to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lib.rs: alpha, beta"),
        "expected symbol names with --symbols, got:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: narrow with --scope <dir>"),
        "expected symbols map footer, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_artifact_surfaces_capped_safe_anchors() {
    let dir = temp_repo("map_artifact_anchors");
    fs::create_dir_all(dir.join("dist")).unwrap();
    fs::write(
        dir.join("dist/app.min.js"),
        "exports.Widget=function(){};module.exports.Helper=class{};j6.exports.internal=1;",
    )
    .unwrap();
    let modules = (0..10)
        .map(|i| format!("ace.define(\"pkg/module{i}\",[],function(){{}});"))
        .collect::<Vec<_>>()
        .join("");
    fs::write(dir.join("dist/amd.min.js"), modules).unwrap();

    let default = srcwalk()
        .args(["map", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&default.stdout);
    assert!(!stdout.contains("anchors:"), "{stdout}");
    assert!(
        !stdout.contains("dist/"),
        "default map should skip artifact dirs:\n{stdout}"
    );

    let artifact = srcwalk()
        .args(["map", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "map --artifact failed:\n{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    assert!(stdout.contains("dist/"), "{stdout}");
    assert!(
        stdout.contains("export Widget") && stdout.contains("export Helper"),
        "{stdout}"
    );
    assert!(!stdout.contains("internal"), "{stdout}");
    assert!(stdout.contains("anchors: mod pkg/module"), "{stdout}");
    assert!(
        stdout.contains("... +4"),
        "module anchors should be capped:\n{stdout}"
    );
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
    assert!(stdout.contains("srcwalk <path> --artifact"), "{stdout}");
    assert!(
        stdout.contains("srcwalk find <name> --artifact"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
