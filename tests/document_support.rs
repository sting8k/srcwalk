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

#[test]
fn markdown_document_outline_discover_section_and_deps() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("docs/setup.md"), "# Setup\n").unwrap();
    std::fs::write(dir.path().join("docs/ref.md"), "# Reference\n").unwrap();
    std::fs::write(dir.path().join("assets/logo.svg"), "<svg/>\n").unwrap();
    let guide = dir.path().join("guide.md");
    std::fs::write(
        &guide,
        r#"# Guide {#guide}
See [Setup](docs/setup.md), ![Logo](assets/logo.svg), and [External](https://example.com/docs).
[ref]: docs/ref.md "Reference"

```md
# Fake
[secret](secret.md)
```

## Install
Step one.

## Install
Duplicate heading.

```rust
fn main() {}
```
## Getting Started
More setup.
"#,
    )
    .unwrap();

    let out = srcwalk().arg(&guide).output().unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("source: document · kind: outline · confidence: document navigation"),
        "document outline should have packet label:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "caveat: document evidence is navigation evidence, not rendered DOM/browser behavior."
        ),
        "document outline should have packet caveat:\n{stdout}"
    );
    assert!(
        stdout.contains("links/assets: docs/setup.md"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("assets/logo.svg"), "got:\n{stdout}");
    assert!(
        stdout.contains("https://example.com/docs"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("section Guide #guide"), "got:\n{stdout}");
    assert!(stdout.contains("section Install"), "got:\n{stdout}");
    assert!(stdout.contains("code-block rust"), "got:\n{stdout}");
    assert!(
        !stdout.contains("section Fake"),
        "fenced heading became a section:\n{stdout}"
    );
    assert!(
        !stdout.contains("secret.md"),
        "fenced link became a dep:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "Install", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[section] Install guide.md:10-12"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("[section] Install guide.md:13-18"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("source: document · kind: section · confidence: document navigation"),
        "document discover sections should have provenance labels:\n{stdout}"
    );
    let out = srcwalk()
        .args(["discover", "Getting Started", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[section] Getting Started guide.md:19-20"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .arg(&guide)
        .args(["--section", "guide"])
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("# Guide"), "got:\n{stdout}");
    assert!(
        stdout.contains("source: document · kind: section · confidence: document navigation"),
        "document section reads should have packet labels:\n{stdout}"
    );
    assert!(stdout.contains("Step one."), "got:\n{stdout}");

    let long_doc = dir.path().join("long.md");
    let repeated = "word ".repeat(1_200);
    std::fs::write(
        &long_doc,
        format!("# Long\n\n{repeated}\n\n## Child\n\nshort\n"),
    )
    .unwrap();
    let out = srcwalk()
        .arg(&long_doc)
        .args(["--section", "Long", "--budget", "200"])
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[section, outline (over limit)]") && stdout.contains("section cap"),
        "small section budget should force outline degrade without global truncation:\n{stdout}"
    );
    assert!(
        stdout.contains("source: document · kind: section · confidence: document navigation"),
        "over-limit document section degrade should retain packet labels:\n{stdout}"
    );

    let out = srcwalk()
        .args(["deps"])
        .arg(&guide)
        .args(["--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("# Deps: guide.md — 3 local, 1 external"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("docs/"), "got:\n{stdout}");
    assert!(stdout.contains("setup.md"), "got:\n{stdout}");
    assert!(stdout.contains("ref.md"), "got:\n{stdout}");
    assert!(stdout.contains("assets/"), "got:\n{stdout}");
    assert!(stdout.contains("logo.svg"), "got:\n{stdout}");
    assert!(
        stdout.contains("https://example.com/docs"),
        "got:\n{stdout}"
    );
    assert!(!stdout.contains("secret.md"), "got:\n{stdout}");
}

#[test]
fn html_document_outline_discover_section_and_deps() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    for file in [
        "style.css",
        "app.js",
        "docs/setup.html",
        "assets/button.png",
        "assets/logo.svg",
        "assets/logo-small.svg",
        "assets/logo-large.svg",
    ] {
        std::fs::write(dir.path().join(file), "fixture\n").unwrap();
    }
    let index = dir.path().join("index.html");
    std::fs::write(
        &index,
        r##"<!doctype html>
<html>
<head>
  <title>Home</title>
  <link rel="stylesheet" href="./style.css">
  <link rel="preconnect" href="https://cdn.example.com">
</head>
<body>
  <main id="app">
    <h1 id="hero">Welcome</h1>
    <section id="intro"><h2>Intro</h2><a href="./docs/setup.html">Setup</a></section>
    <form name="login"><input src="./assets/button.png"></form>
    <my-card id="card"></my-card>
    <img src="./assets/logo.svg" srcset="./assets/logo-small.svg 1x, ./assets/logo-large.svg 2x">
    <script src="./app.js"></script>
    <a href="#local">Local anchor</a>
    <img src="data:image/svg+xml,%3Csvg%3E">
  </main>
</body>
</html>
"##,
    )
    .unwrap();

    let out = srcwalk().arg(&index).output().unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("source: document · kind: outline · confidence: document navigation"),
        "HTML outline should have packet label:\n{stdout}"
    );
    assert!(
        stdout.contains("links/assets: ./style.css"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("https://cdn.example.com"), "got:\n{stdout}");
    assert!(stdout.contains("section title: Home"), "got:\n{stdout}");
    assert!(stdout.contains("element main#app"), "got:\n{stdout}");
    assert!(stdout.contains("section Welcome #hero"), "got:\n{stdout}");
    assert!(stdout.contains("element section#intro"), "got:\n{stdout}");
    assert!(
        stdout.contains("element form[name=login]"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("element my-card#card"), "got:\n{stdout}");
    assert!(
        !stdout.contains("data:image"),
        "data URL should not be a dep:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "hero", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[section] Welcome #hero index.html:10-10"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("source: document · kind: section · confidence: document navigation"),
        "HTML discover sections should have provenance labels:\n{stdout}"
    );

    let out = srcwalk()
        .arg(&index)
        .args(["--section", "hero"])
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("Welcome"), "got:\n{stdout}");
    assert!(
        !stdout.contains("<script"),
        "heading section crossed too far:\n{stdout}"
    );

    let out = srcwalk()
        .args(["deps"])
        .arg(&index)
        .args(["--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("# Deps: index.html — 7 local, 1 external"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("style.css"), "got:\n{stdout}");
    assert!(stdout.contains("app.js"), "got:\n{stdout}");
    assert!(stdout.contains("docs/"), "got:\n{stdout}");
    assert!(stdout.contains("setup.html"), "got:\n{stdout}");
    assert!(stdout.contains("assets/"), "got:\n{stdout}");
    assert!(stdout.contains("logo-small.svg"), "got:\n{stdout}");
    assert!(stdout.contains("logo-large.svg"), "got:\n{stdout}");
    assert!(stdout.contains("https://cdn.example.com"), "got:\n{stdout}");
    assert!(!stdout.contains("data:image"), "got:\n{stdout}");
}
