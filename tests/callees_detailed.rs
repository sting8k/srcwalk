use std::path::Path;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/callees"
    ))
}

/// Set up a minimal fixture with known call structure.
fn setup_fixtures() {
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
}

#[test]
fn callees_default_shows_hint() {
    setup_fixtures();
    let out = srcwalk()
        .args(["--callees", "process", "--scope"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Tip: use --detailed"),
        "default output should contain hint, got:\n{stdout}"
    );
}

#[test]
fn callees_budget_truncation_keeps_hint() {
    setup_fixtures();
    let out = srcwalk()
        .args(["--callees", "process", "--scope"])
        .arg(fixture_dir())
        .args(["--budget", "30"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("truncated") && stdout.contains("Tip: use --detailed"),
        "budgeted output should keep hint after truncation, got:\n{stdout}"
    );
}

#[test]
fn callees_detailed_no_hint() {
    setup_fixtures();
    let out = srcwalk()
        .args(["--callees", "process", "--scope"])
        .arg(fixture_dir())
        .arg("--detailed")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("Tip: use --detailed"),
        "--detailed output should NOT contain hint, got:\n{stdout}"
    );
}

#[test]
fn callees_detailed_shows_assignments() {
    setup_fixtures();
    let out = srcwalk()
        .args(["--callees", "process", "--detailed", "--scope"])
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
        .args(["--callees", "greet", "--detailed", "--scope"])
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
        .args(["--callees", "process", "--scope"])
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
fn callees_python_detailed() {
    setup_fixtures();
    let out = srcwalk()
        .args(["--callees", "compute", "--detailed", "--scope"])
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
