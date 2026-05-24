use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn assert_success(out: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "command failed\nstderr:\n{}\nstdout:\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

fn write_fixture(dir: &tempfile::TempDir) {
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(
        dir.path().join("base.css"),
        ":root { --main-color: red; }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("assets/logo.svg"), "<svg/>\n").unwrap();
    std::fs::write(dir.path().join("assets/paren).svg"), "<svg/>\n").unwrap();
    std::fs::write(dir.path().join("assets/escaped).svg"), "<svg/>\n").unwrap();
    std::fs::write(
        dir.path().join("style.css"),
        r#"@import "./base.css";
@import url("https://cdn.example.com/reset.css");
@import "@fontsource/roboto-mono/400.css";

/* This file keeps reset styles close to token declarations so themes stay synced; docs mention url("https://noise.example/reset.css"). */
code { font-family: inherit; }

/* Base */
:root {
  --accent: blue;
}

/* Components */
.button,
#cta:hover {
  color: var(--accent);
  background: url("./assets/logo.svg");
  background-image: url( './assets/paren).svg' );
  border-image-source: url(./assets/escaped\).svg);
  mask-image: url("data:image/svg+xml,%3Csvg%3E%3C/svg%3E");
}

/* Responsive */
@media (max-width: 600px) {
  .button { display: none; }
}

/* Motion */
@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
"#,
    )
    .unwrap();
}

#[test]
fn css_outline_sections_and_find_use_css_anchors() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(&dir);
    let style = dir.path().join("style.css");

    let out = srcwalk().arg(&style).output().unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("imports: ./base.css, https://cdn.example.com/reset.css"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("selector code"), "got:\n{stdout}");
    assert!(
        !stdout.contains("section This file keeps reset styles"),
        "ordinary sentence comment became a section:\n{stdout}"
    );
    assert!(stdout.contains("section Base"), "got:\n{stdout}");
    assert!(stdout.contains("selector :root"), "got:\n{stdout}");
    assert!(stdout.contains("prop --accent"), "got:\n{stdout}");
    assert!(stdout.contains("section Components"), "got:\n{stdout}");
    assert!(
        stdout.contains("selector .button, #cta:hover"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("section Responsive"), "got:\n{stdout}");
    assert!(
        stdout.contains("at-rule @media (max-width: 600px)"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("section Motion"), "got:\n{stdout}");
    assert!(stdout.contains("at-rule @keyframes spin"), "got:\n{stdout}");

    let out = srcwalk()
        .arg(&style)
        .args(["--section", "button"])
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains(".button,"), "got:\n{stdout}");
    assert!(stdout.contains("background: url"), "got:\n{stdout}");
    assert!(
        !stdout.contains("@media"),
        "section crossed selector boundary:\n{stdout}"
    );

    let out = srcwalk()
        .arg(&style)
        .args(["--section", "spin"])
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("@keyframes spin"), "got:\n{stdout}");
    assert!(stdout.contains("rotate(360deg)"), "got:\n{stdout}");

    let out = srcwalk()
        .args(["discover", ".button", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[selector] .button, #cta:hover style.css:14-21"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("[selector] .button style.css:25-25"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "@keyframes spin", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[at-rule] @keyframes spin style.css:29-32"),
        "got:\n{stdout}"
    );
}

#[test]
fn css_deps_resolve_imports_and_url_references() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(&dir);

    let out = srcwalk()
        .args(["deps"])
        .arg(dir.path().join("style.css"))
        .args(["--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);

    assert!(
        stdout.contains("# Deps: style.css — 4 local, 2 external"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("base.css"), "got:\n{stdout}");
    assert!(stdout.contains("assets/"), "got:\n{stdout}");
    assert!(stdout.contains("logo.svg"), "got:\n{stdout}");
    assert!(stdout.contains("paren).svg"), "got:\n{stdout}");
    assert!(stdout.contains("escaped).svg"), "got:\n{stdout}");
    assert!(
        stdout.contains("https://cdn.example.com/reset.css"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("@fontsource/roboto-mono/400.css"),
        "got:\n{stdout}"
    );
    assert!(
        !stdout.contains("data:image"),
        "data URLs should not be deps:\n{stdout}"
    );
    assert!(
        !stdout.contains("https://noise.example/reset.css"),
        "comment URL should not be a dep:\n{stdout}"
    );
}

#[test]
fn css_does_not_invent_call_graph_edges() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(&dir);

    let out = srcwalk()
        .args(["trace", "callees", ".button", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("(no calls found)"), "got:\n{stdout}");
}
