use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture_dir(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    match name {
        "many_matches" => {
            std::fs::write(
                root.join("lib.rs"),
                r#"fn needle() {}
fn caller_a() { needle(); }
fn caller_b() { needle(); }
fn caller_c() { needle(); }
"#,
            )
            .unwrap();
        }
        "glob" => {
            for name in ["a.rs", "b.rs", "c.rs"] {
                std::fs::write(root.join(name), "fn sample() {}\n").unwrap();
            }
        }
        _ => unreachable!(),
    }
    dir
}

#[test]
fn search_pagination_next_step_is_footer_and_survives_budget() {
    let dir = fixture_dir("many_matches");
    let out = srcwalk()
        .args(["needle", "--limit", "1", "--budget", "10", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("truncated"),
        "expected budget truncation:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable footer pagination next-step:\n{stdout}"
    );
    assert!(
        !stdout.contains("--files"),
        "invalid --files hint leaked:\n{stdout}"
    );
}

#[test]
fn glob_pagination_next_step_is_footer() {
    let dir = fixture_dir("glob");
    let out = srcwalk()
        .args(["files", "*.rs", "--limit", "1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("> Next:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable glob pagination next-step:\n{stdout}"
    );
}

#[test]
fn callers_pagination_next_step_is_footer() {
    let dir = fixture_dir("many_matches");
    let out = srcwalk()
        .args(["needle", "--callers", "--limit", "1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("> Next:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable callers pagination next-step:\n{stdout}"
    );
}

#[test]
fn bfs_cap_prints_caveat_footer() {
    let dir = fixture_dir("many_matches");
    let out = srcwalk()
        .args([
            "needle",
            "--callers",
            "--depth",
            "2",
            "--max-edges",
            "1",
            "--scope",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("edges capped") && stdout.contains("> Caveat: graph was capped"),
        "expected BFS cap caveat:\n{stdout}"
    );
}

#[test]
fn deps_budget_compaction_caveat_is_footer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut target = String::new();
    for idx in 0..20 {
        target.push_str(&format!("import dep{idx} from 'package-{idx}';\n"));
    }
    target.push_str("export function exported() { return dep0; }\n");
    std::fs::write(root.join("target.js"), target).unwrap();

    let out = srcwalk()
        .arg(root.join("target.js"))
        .args(["--deps", "--budget", "20", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("> Caveat: deps output was compacted for budget"),
        "expected deps budget footer caveat:\n{stdout}"
    );
}

fn deps_pagination_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("target.rs"), "fn exported() {}\n").unwrap();
    for idx in 0..20 {
        std::fs::write(
            root.join(format!("caller_{idx:02}.rs")),
            format!("fn caller_{idx}() {{ exported(); }}\n"),
        )
        .unwrap();
    }
    dir
}

#[test]
fn deps_used_by_groups_rows_by_directory_but_keeps_line_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/a")).unwrap();
    std::fs::create_dir_all(root.join("src/b")).unwrap();
    std::fs::write(root.join("src/target.rs"), "fn exported() {}\n").unwrap();
    std::fs::write(root.join("src/a/one.rs"), "fn one() { exported(); }\n").unwrap();
    std::fs::write(root.join("src/a/two.rs"), "fn two() { exported(); }\n").unwrap();
    std::fs::write(root.join("src/b/three.rs"), "fn three() { exported(); }\n").unwrap();

    let out = srcwalk()
        .arg(root.join("src/target.rs"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert!(
        normalized.contains("src/a/\n  one.rs:1")
            && normalized.contains("\n  two.rs:1")
            && normalized.contains("src/b/\n  three.rs:1"),
        "expected grouped dirs with file:line anchors:\n{stdout}"
    );
    assert!(
        !normalized.contains("src/a/one.rs:1") && !normalized.contains("src/b/three.rs:1"),
        "grouped output should avoid repeating directory prefixes per row:\n{stdout}"
    );
}

#[test]
fn deps_uses_local_groups_rows_by_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/app")).unwrap();
    std::fs::create_dir_all(root.join("src/lib")).unwrap();
    std::fs::create_dir_all(root.join("src/util")).unwrap();
    std::fs::write(
        root.join("src/app/main.ts"),
        "import { a } from '../lib/a';\nimport { b } from '../lib/b';\nimport { c } from '../util/c';\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib/a.ts"), "export const a = 1;\n").unwrap();
    std::fs::write(root.join("src/lib/b.ts"), "export const b = 1;\n").unwrap();
    std::fs::write(root.join("src/util/c.ts"), "export const c = 1;\n").unwrap();

    let out = srcwalk()
        .arg(root.join("src/app/main.ts"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let normalized = norm_path_separators(&stdout);

    assert!(
        normalized.contains("## Uses (local)\nsrc/lib/\n  a.ts\n  b.ts\nsrc/util/\n  c.ts"),
        "expected grouped local uses:\n{stdout}"
    );
    assert!(
        !normalized.contains("src/lib/a.ts") && !normalized.contains("src/util/c.ts"),
        "grouped output should avoid repeating directory prefixes per row:\n{stdout}"
    );
}

#[test]
fn deps_used_by_ignores_bare_child_member_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/target.rs"),
        "struct Service;\nimpl Service { fn run(&self) {} }\n",
    )
    .unwrap();
    std::fs::write(root.join("src/noise.rs"), "fn unrelated() { run(); }\n").unwrap();

    let out = srcwalk()
        .arg(root.join("src/target.rs"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("noise.rs"),
        "bare child method name should not create a reverse dependency:\n{stdout}"
    );
}

#[test]
fn deps_used_by_keeps_member_names_with_owner_context() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/target.rs"),
        "pub struct Service;\nimpl Service { pub fn run(&self) {} }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/caller.rs"),
        "fn call_service(service: Service) { service.run(); }\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg(root.join("src/target.rs"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("caller.rs:1") && stdout.contains("run"),
        "owner-context member call should create a reverse dependency:\n{stdout}"
    );
}

#[test]
fn deps_dependents_default_page_has_continuation_next_step() {
    let dir = deps_pagination_fixture();
    let root = dir.path();
    let out = srcwalk()
        .arg(root.join("target.rs"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("... and 5 more dependents")
            && stdout.contains(
                "> Next: 5 more dependents available. Continue with --offset 15 --limit 15."
            ),
        "expected dependent pagination footer next-step:\n{stdout}"
    );
}

#[test]
fn deps_dependents_limit_offset_page_and_end_note() {
    let dir = deps_pagination_fixture();
    let root = dir.path();
    let page = srcwalk()
        .arg(root.join("target.rs"))
        .args(["--deps", "--limit", "7", "--offset", "7", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let page_stdout = String::from_utf8_lossy(&page.stdout);
    assert!(
        page_stdout.contains("... and 6 more dependents")
            && page_stdout.contains(
                "> Next: 6 more dependents available. Continue with --offset 14 --limit 7."
            ),
        "expected second deps page continuation next-step:\n{page_stdout}"
    );

    let end = srcwalk()
        .arg(root.join("target.rs"))
        .args(["--deps", "--limit", "7", "--offset", "21", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let end_stdout = String::from_utf8_lossy(&end.stdout);
    assert!(
        end_stdout.contains("> Note: end of dependent results at offset 21."),
        "expected deps end-of-results footer note:\n{end_stdout}"
    );
}

#[test]
fn full_file_cap_next_step_is_footer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.txt");
    std::fs::write(
        &path,
        (0..200).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();

    let out = srcwalk()
        .arg(&path)
        .arg("--full")
        .arg("--no-budget")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("full capped — tokens ~")
            && stdout.contains("> Next: use --section <symbol|range[,symbol|range]>")
            && stdout.contains("--section 201-<end>"),
        "expected full-file cap footer next-step:\n{stdout}"
    );
}

#[test]
fn full_file_explicit_budget_overrides_default_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budgeted.txt");
    std::fs::write(
        &path,
        (0..260).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();

    let out = srcwalk()
        .arg(&path)
        .arg("--full")
        .arg("--budget")
        .arg("9000")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("line 259"),
        "explicit budget should read past default line cap:\n{stdout}"
    );
    assert!(
        !stdout.contains("full capped"),
        "file should fit explicit budget:\n{stdout}"
    );
}

#[test]
fn expanded_output_omits_bodies_to_fit_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many.rs");
    let mut body = String::new();
    for func in 0..6 {
        body.push_str(&format!("fn target_{func}() {{\n"));
        for line in 0..80 {
            body.push_str(&format!("    let value_{func}_{line} = {line};\n"));
        }
        body.push_str("}\n\n");
    }
    std::fs::write(path, body).unwrap();

    let out = srcwalk()
        .arg("target_0,target_1,target_2,target_3,target_4")
        .arg("--expand=5")
        .arg("--budget")
        .arg("900")
        .arg("--scope")
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("[fn] target_"),
        "hit list should remain visible:\n{stdout}"
    );
    assert!(
        stdout.contains("expand cap ~")
            && stdout.contains("expanded ")
            && stdout.contains("omitted ")
            && stdout.contains("Next: drill into omitted hits"),
        "expected expand budget note:\n{stdout}"
    );
}

#[test]
fn expanded_smart_truncate_caveat_is_footer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.rs");
    let mut body = String::from("fn huge() {\n");
    for idx in 0..140 {
        body.push_str(&format!("    let value_{idx} = {idx};\n"));
    }
    body.push_str("    println!(\"done\");\n}\n");
    std::fs::write(path, body).unwrap();

    let out = srcwalk()
        .arg("huge")
        .arg("--expand=1")
        .arg("--no-budget")
        .arg("--scope")
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("lines omitted")
            && stdout.contains("> Caveat: expanded source truncated")
            && stdout.contains("> Next: use shown line range with --section <start-end>"),
        "expected smart-truncate footer caveat:\n{stdout}"
    );
}
fn norm_path_separators(s: &str) -> String {
    s.replace('\\', "/")
}
