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
fn path_line_query_reads_focused_context() {
    let dir = temp_repo("path_line_query");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    let before = 1;\n    let target = before + 1;\n    println!(\"{target}\");\n}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("src/main.rs:3")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected path:line read to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/main.rs")
            && stdout.contains("    2 │     let before = 1;")
            && stdout.contains("►    3 │     let target = before + 1;")
            && stdout.contains("    4 │     println!(\"{target}\");"),
        "expected focused context with marker, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_line_range_query_reads_exact_section() {
    let dir = temp_repo("path_line_range_query");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    let one = 1;\n    let two = 2;\n    let three = 3;\n}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("src/main.rs:2-4")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();

    assert!(out.status.success(), "expected path:range read to succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("2      let one = 1;")
            && stdout.contains("3      let two = 2;")
            && stdout.contains("4      let three = 3;")
            && !stdout.contains("fn main()"),
        "expected exact line range, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn numeric_section_reads_focused_context() {
    let dir = temp_repo("numeric_section");
    fs::write(
        dir.join("lib.rs"),
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\n",
    )
    .unwrap();

    let out = srcwalk()
        .arg("lib.rs")
        .arg("--scope")
        .arg(&dir)
        .arg("--section")
        .arg("2")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected numeric section read to succeed"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("    1 │ pub fn a() {}")
            && stdout.contains("►    2 │ pub fn b() {}")
            && stdout.contains("    3 │ pub fn c() {}"),
        "expected numeric section focused context, got:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn long_symbol_section_reads_source_when_budget_allows() {
    let dir = temp_repo("long_symbol_section");
    let mut body = String::from("fn long_fn() {\n");
    for i in 0..220 {
        body.push_str(&format!("    let value_{i} = {i};\n"));
    }
    body.push_str("}\n");
    fs::write(dir.join("lib.rs"), body).unwrap();

    let out = srcwalk()
        .arg("lib.rs")
        .arg("--scope")
        .arg(&dir)
        .arg("--section")
        .arg("long_fn")
        .arg("--budget")
        .arg("10000")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected long section read to succeed"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[section]") && stdout.contains("let value_219 = 219;"),
        "expected long function source, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[section, outline (over limit)]"),
        "long low-token function should not degrade to outline:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
