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

fn has_numbered_line(output: &str, line: usize) -> bool {
    let expected = format!("{line}  line {line}");
    output.lines().any(|actual| actual.trim_start() == expected)
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
fn context_lines_expand_line_ranges_without_cap() {
    let dir = temp_repo("context_range_uncapped");
    let body = (1..=50)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("lib.rs"), body).unwrap();

    let out = srcwalk()
        .arg("show")
        .arg("lib.rs:25-26")
        .arg("--scope")
        .arg(&dir)
        .arg("-C")
        .arg("20")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected range context read to succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        has_numbered_line(&stdout, 5)
            && has_numbered_line(&stdout, 25)
            && has_numbered_line(&stdout, 26)
            && has_numbered_line(&stdout, 46),
        "single range should use requested context without a max-10 cap:\n{stdout}"
    );
    assert!(
        !has_numbered_line(&stdout, 4) && !has_numbered_line(&stdout, 47),
        "single range should expand by exactly the requested context:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_lines_expand_symbol_sections_without_cap() {
    let dir = temp_repo("context_symbol_section_uncapped");
    let mut body = String::new();
    for line in 1..=24 {
        body.push_str(&format!("// pre line {line}\n"));
    }
    body.push_str("fn target() {\n    let value = 1;\n}\n");
    for line in 28..=55 {
        body.push_str(&format!("// post line {line}\n"));
    }
    fs::write(dir.join("lib.rs"), body).unwrap();

    let out = srcwalk()
        .arg("show")
        .arg("lib.rs")
        .arg("--scope")
        .arg(&dir)
        .arg("--section")
        .arg("target")
        .arg("-C")
        .arg("20")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected symbol section context read to succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("5  // pre line 5")
            && stdout.contains("25  fn target() {")
            && stdout.contains("27  }")
            && stdout.contains("47  // post line 47"),
        "single symbol section should use requested context without a max-10 cap:\n{stdout}"
    );
    assert!(
        !stdout.contains("4  // pre line 4") && !stdout.contains("48  // post line 48"),
        "single symbol section should expand by exactly the requested context:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_lines_expand_multiple_sections_with_cap() {
    let dir = temp_repo("context_multi_section_cap");
    let body = (1..=60)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("lib.rs"), body).unwrap();

    let out = srcwalk()
        .arg("show")
        .arg("lib.rs")
        .arg("--scope")
        .arg(&dir)
        .arg("--section")
        .arg("20-21,50")
        .arg("-C")
        .arg("99")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected multi-section context read to succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("## section: 20-21 [10-31]")
            && has_numbered_line(&stdout, 10)
            && has_numbered_line(&stdout, 31),
        "multi explicit range section should clamp -C to 10:\n{stdout}"
    );
    assert!(
        stdout.contains("## section: 50 [40-60]")
            && stdout.contains("►   50 │ line 50")
            && stdout.contains("60 │ line 60"),
        "multi focused line section should clamp -C to 10:\n{stdout}"
    );
    assert!(
        !has_numbered_line(&stdout, 9) && !stdout.contains("39 │ line 39"),
        "multi sections should not expand beyond the max-10 cap:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_lines_expand_comma_separated_show_locations_with_cap() {
    let dir = temp_repo("context_multi_show_locations_cap");
    let body = (1..=60)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("lib.rs"), body).unwrap();

    let out = srcwalk()
        .arg("show")
        .arg("lib.rs:20-21,lib.rs:50")
        .arg("--scope")
        .arg(&dir)
        .arg("-C")
        .arg("99")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "expected comma-separated show locations with context to succeed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("# Show: 2 locations"), "{stdout}");
    assert!(
        has_numbered_line(&stdout, 10)
            && has_numbered_line(&stdout, 31)
            && stdout.contains("►   50 │ line 50")
            && stdout.contains("60 │ line 60"),
        "expected -C to clamp to 10 for each comma-separated location:\n{stdout}"
    );
    assert!(
        !has_numbered_line(&stdout, 9) && !stdout.contains("39 │ line 39"),
        "comma-separated locations should not expand beyond the max-10 cap:\n{stdout}"
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
