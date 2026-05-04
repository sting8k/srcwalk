use std::fs;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn flow_filter_slices_ordered_calls_and_resolves_matching_callee() {
    let dir = temp_dir("flow_filter");
    fs::write(
        dir.join("lib.rs"),
        r#"
mod format;

fn entry() {
    let value = helper();
    noisy();
    format();
}

fn helper() -> i32 {
    1
}

fn noisy() {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["entry", "--flow", "--filter", "callee:helper", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow filter should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("-> calls (ordered, filtered callee:helper)"),
        "expected filtered calls header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("helper()"),
        "expected matching helper call, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("noisy()"),
        "filter should exclude non-matching call, got:\n{stdout}"
    );
    assert!(
        stdout.contains("filter matched 1/3 call sites"),
        "expected filter count footer, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[fn] helper"),
        "expected matching helper resolve, got:\n{stdout}"
    );
}

#[test]
fn flow_shows_call_arg_slots() {
    let dir = temp_dir("flow_arg_slots");
    fs::write(
        dir.join("lib.rs"),
        r#"
fn entry() {
    let value = helper(1, "two");
    finish(value);
}

fn helper(a: i32, b: &str) -> i32 { a }
fn finish(value: i32) {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["entry", "--flow", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("helper(arg1=1, arg2=\"two\")"),
        "expected arg slots for helper call, got:\n{stdout}"
    );
    assert!(
        stdout.contains("finish(arg1=value)"),
        "expected arg slot for finish call, got:\n{stdout}"
    );
}

#[test]
fn flow_resolves_skip_module_like_noise() {
    let dir = temp_dir("flow_resolve_noise");
    fs::write(
        dir.join("lib.rs"),
        r#"
mod format;

fn entry() {
    helper();
    format();
}

fn helper() {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["entry", "--flow", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("-> resolves (selected local helpers)"),
        "expected local-helper resolves heading, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[fn] helper"),
        "expected helper resolve, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[fn] format"),
        "module-like format resolve should be skipped, got:\n{stdout}"
    );
}
