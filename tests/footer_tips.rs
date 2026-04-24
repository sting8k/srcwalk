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
fn search_pagination_tip_is_footer_and_survives_budget() {
    let dir = fixture_dir("many_matches");
    let out = srcwalk()
        .args(["needle", "--limit", "1", "--budget", "30", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("truncated"),
        "expected budget truncation:\n{stdout}"
    );
    assert!(
        stdout.contains("> Tip:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable footer pagination tip:\n{stdout}"
    );
    assert!(
        !stdout.contains("--files"),
        "invalid --files hint leaked:\n{stdout}"
    );
}

#[test]
fn glob_pagination_tip_is_footer() {
    let dir = fixture_dir("glob");
    let out = srcwalk()
        .args(["*.rs", "--limit", "1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("> Tip:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable glob pagination tip:\n{stdout}"
    );
}

#[test]
fn callers_pagination_tip_is_footer() {
    let dir = fixture_dir("many_matches");
    let out = srcwalk()
        .args(["needle", "--callers", "--limit", "1", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("> Tip:") && stdout.contains("--offset 1 --limit 1"),
        "expected actionable callers pagination tip:\n{stdout}"
    );
}

#[test]
fn bfs_cap_prints_actionable_tip() {
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
        stdout.contains("edges capped") && stdout.contains("> Tip: graph was capped"),
        "expected BFS cap tip:\n{stdout}"
    );
}

#[test]
fn deps_budget_compaction_tip_is_footer() {
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
        stdout.contains("> Tip: deps output was compacted for budget"),
        "expected deps budget footer tip:\n{stdout}"
    );
}

#[test]
fn deps_dependent_cap_tip_is_footer() {
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

    let out = srcwalk()
        .arg(root.join("target.rs"))
        .args(["--deps", "--scope"])
        .arg(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("... and 5 more dependents")
            && stdout.contains("> Tip: dependent list was capped"),
        "expected dependent hard-cap footer tip:\n{stdout}"
    );
}

#[test]
fn full_file_cap_tip_is_footer() {
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
        .env("SRCWALK_FULL_SIZE_CAP", "100")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("full=true capped")
            && stdout.contains("> Tip: full output was capped")
            && stdout.contains("--section"),
        "expected full-file cap footer tip:\n{stdout}"
    );
}

#[test]
fn expanded_smart_truncate_tip_is_footer() {
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
            && stdout.contains("> Tip: expanded source was smart-truncated"),
        "expected smart-truncate footer tip:\n{stdout}"
    );
}
