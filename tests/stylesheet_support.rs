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

fn write_scss_fixture(dir: &tempfile::TempDir) {
    std::fs::write(dir.path().join("_tokens.scss"), "$gap: 8px;\n").unwrap();
    std::fs::write(
        dir.path().join("reset.css"),
        "html { box-sizing: border-box; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("theme.scss"),
        r#"@use "sass:math";
@use "tokens";
@forward "tokens";
@import "./reset.css";
@import url("https://cdn.example.com/base.css");

$gap: 12px;

/* Components */
@mixin card($radius) {
  border-radius: $radius;

  .card-inner {
    padding: $gap;
  }
}

@function spacing($n) {
  @return $n * $gap;
}

.button {
  @include card(4px);

  &__icon {
    margin-inline-start: spacing(1);
  }
}
"#,
    )
    .unwrap();
}

fn write_less_fixture(dir: &tempfile::TempDir) {
    std::fs::write(dir.path().join("theme.less"), "@brand: blue;\n").unwrap();
    std::fs::write(
        dir.path().join("mixins.less"),
        ".shadow() { box-shadow: none; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("style.less"),
        r#"@import "theme";
@import (reference) "./mixins.less";
@import "https://cdn.example.com/reset.less";

@gap: 8px;

/* Components */
.rounded(@radius) {
  border-radius: @radius;
}

.button {
  .rounded(4px);

  &__icon {
    margin-left: @gap;
  }
}
"#,
    )
    .unwrap();
}

#[test]
fn scss_outline_find_and_deps_use_scss_semantics() {
    let dir = tempfile::tempdir().unwrap();
    write_scss_fixture(&dir);
    let style = dir.path().join("theme.scss");

    let out = srcwalk().arg(&style).output().unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("imports: sass:math"), "got:\n{stdout}");
    assert!(stdout.contains("let $gap"), "got:\n{stdout}");
    assert!(stdout.contains("section Components"), "got:\n{stdout}");
    assert!(stdout.contains("mixin card"), "got:\n{stdout}");
    assert!(stdout.contains("fn spacing"), "got:\n{stdout}");
    assert!(stdout.contains("selector .button"), "got:\n{stdout}");
    assert!(stdout.contains("selector &__icon"), "got:\n{stdout}");

    let out = srcwalk()
        .args(["discover", "card", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[mixin] card theme.scss:10-16"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "gap", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[var] $gap theme.scss:7-7"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .args(["deps"])
        .arg(&style)
        .args(["--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("# Deps: theme.scss — 2 local, 2 external"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("_tokens.scss"), "got:\n{stdout}");
    assert!(stdout.contains("reset.css"), "got:\n{stdout}");
    assert!(stdout.contains("sass:math"), "got:\n{stdout}");
    assert!(
        stdout.contains("https://cdn.example.com/base.css"),
        "got:\n{stdout}"
    );
}

#[test]
fn less_outline_find_and_deps_use_less_semantics() {
    let dir = tempfile::tempdir().unwrap();
    write_less_fixture(&dir);
    let style = dir.path().join("style.less");

    let out = srcwalk().arg(&style).output().unwrap();
    let stdout = assert_success(&out);
    assert!(stdout.contains("imports: theme"), "got:\n{stdout}");
    assert!(stdout.contains("let @gap"), "got:\n{stdout}");
    assert!(stdout.contains("section Components"), "got:\n{stdout}");
    assert!(stdout.contains("mixin .rounded"), "got:\n{stdout}");
    assert!(stdout.contains("selector .button"), "got:\n{stdout}");
    assert!(stdout.contains("selector &__icon"), "got:\n{stdout}");

    let out = srcwalk()
        .args(["discover", ".rounded", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[mixin] .rounded style.less:8-10"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "gap", "--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("[var] @gap style.less:5-5"),
        "got:\n{stdout}"
    );

    let out = srcwalk()
        .args(["deps"])
        .arg(&style)
        .args(["--scope"])
        .arg(dir.path())
        .output()
        .unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("# Deps: style.less — 2 local, 1 external"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("theme.less"), "got:\n{stdout}");
    assert!(stdout.contains("mixins.less"), "got:\n{stdout}");
    assert!(
        stdout.contains("https://cdn.example.com/reset.less"),
        "got:\n{stdout}"
    );
}
