use std::path::Path;
use std::process::Command;
use std::sync::Once;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

static SETUP_FIXTURES: Once = Once::new();

fn fixture_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/callees"
    ))
}

/// Set up a minimal fixture with known call structure.
fn setup_fixtures() {
    SETUP_FIXTURES.call_once(|| {
        let dir = fixture_dir();
        std::fs::create_dir_all(dir).unwrap();

        std::fs::write(
            dir.join("main.rs"),
            r#"fn greet(name: &str) -> String {
    let msg = format!("hello {}", name);
    println!("{}", msg);
    msg
}

fn process() {
    let result = greet("world");
    let trimmed = result.trim();
    send(trimmed);
}

fn send(data: &str) {
    println!("sending: {}", data);
}

fn nested_unresolved() {
    outer(
        inner(),
    );
}

fn many_unresolved() {
    call00();
    call01();
    call02();
    call03();
    call04();
    call05();
    call06();
    call07();
    call08();
    call09();
    call10();
    call11();
    call12();
    call13();
}
"#,
        )
        .unwrap();

        std::fs::write(
            dir.join("helper.py"),
            r#"def compute(x, y):
    total = add(x, y)
    result = multiply(total, 2)
    return result

def add(a, b):
    return a + b

def multiply(a, b):
    return a * b
"#,
        )
        .unwrap();
    });
}

#[test]
fn callees_default_shows_footer() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Next: use --detailed"),
        "default output should contain footer, got:\n{stdout}"
    );
}

#[test]
fn callees_budget_truncation_keeps_footer() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--scope"])
        .arg(fixture_dir())
        .args(["--budget", "30"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("truncated") && stdout.contains("Next: use --detailed"),
        "budgeted output should keep footer after truncation, got:\n{stdout}"
    );
}

#[test]
fn callees_detailed_has_own_budget_caveat() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--scope"])
        .arg(fixture_dir())
        .arg("--detailed")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Caveat: detailed call sites can be long")
            && !stdout.contains("Next: use --detailed"),
        "--detailed output should contain its own caveat, got:\n{stdout}"
    );
}

#[test]
fn callees_detailed_shows_assignments() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--detailed", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Should show assignment: result = greet(...)
    assert!(
        stdout.contains("result = greet("),
        "should show assignment context, got:\n{stdout}"
    );
}

#[test]
fn callees_detailed_shows_return() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "greet", "--detailed", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `msg` is an implicit return — should show ->ret
    // or at minimum show the format! call with assignment
    assert!(
        stdout.contains("msg = format!(") || stdout.contains("->ret"),
        "should show assignment or return, got:\n{stdout}"
    );
}

#[test]
fn callees_default_lists_resolved() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Default output should list resolved callees with signatures
    assert!(
        stdout.contains("greet") && stdout.contains("main.rs"),
        "default should list resolved callees, got:\n{stdout}"
    );
}

#[test]
fn callees_default_shows_unresolved_call_site_evidence() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "process", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("unresolved call sites (reason not classified)"),
        "default output should label unresolved call-site evidence, got:\n{stdout}"
    );
    assert!(
        stdout.contains("L9") && stdout.contains("result.trim"),
        "unresolved evidence should keep source line and call text, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("(unresolved):"),
        "default output should not collapse unresolved calls to a bare name list:\n{stdout}"
    );
}

#[test]
fn callees_default_preserves_unrendered_unresolved_names() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "nested_unresolved", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("unresolved call sites (reason not classified)"),
        "default output should show unresolved call-site section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("outer("),
        "outer multiline call should have a call-site row, got:\n{stdout}"
    );
    assert!(
        stdout.contains("unresolved names without call-site rows: inner"),
        "nested unresolved name without a rendered row must be preserved, got:\n{stdout}"
    );
}

#[test]
fn callees_default_preserves_unresolved_names_after_row_cap() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "many_unresolved", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("... 2 more unresolved call sites"),
        "default output should report capped unresolved call-site rows, got:\n{stdout}"
    );
    assert!(
        stdout.contains("unresolved names without call-site rows: call12, call13"),
        "unresolved names after the rendered row cap must be preserved, got:\n{stdout}"
    );
}

#[test]
fn callees_python_detailed() {
    setup_fixtures();
    let out = srcwalk()
        .args(["trace", "callees", "compute", "--detailed", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Should show: total = add(...) and result = multiply(...)
    assert!(
        stdout.contains("total = add(") || stdout.contains("add("),
        "Python callees should include add call, got:\n{stdout}"
    );
    assert!(
        stdout.contains("result = multiply(") || stdout.contains("multiply("),
        "Python callees should include multiply call, got:\n{stdout}"
    );
}
