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
