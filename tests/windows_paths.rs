#![cfg(windows)]

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

fn fixture_repo(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    let src = dir.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("lib.rs"),
        "pub fn alpha() {\n    beta();\n}\npub fn beta() {}\n",
    )
    .unwrap();
    fs::write(
        src.join("server.js"),
        "if (pathname === '/api/gold') handleGold(); function handleGold() {}\n",
    )
    .unwrap();
    dir
}

#[test]
fn windows_absolute_path_range_and_relative_backslash_line_work() {
    let dir = fixture_repo("windows_path_range");
    let file = dir.join("src").join("lib.rs");
    let abs_range = format!("{}:2-3", file.display());

    let out = srcwalk().arg(&abs_range).output().unwrap();
    assert!(
        out.status.success(),
        "absolute path range failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("beta();"), "{stdout}");

    let out = srcwalk()
        .args(["discover", "beta", "--scope"])
        .arg(&abs_range)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "absolute discover scope range failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(":2"), "{stdout}");
    assert!(
        !stdout.contains(":4"),
        "range scope should exclude definition outside range:\n{stdout}"
    );

    let out = srcwalk()
        .arg(r".\src\lib.rs:2")
        .arg("--scope")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "relative backslash path line failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("beta();"), "{stdout}");
}

#[test]
fn windows_compare_accepts_absolute_file_symbol_targets() {
    let dir = fixture_repo("windows_compare_absolute_targets");
    let file = dir.join("src").join("lib.rs");
    let alpha = format!("{}:alpha", file.display());
    let beta = format!("{}:beta", file.display());

    let out = srcwalk()
        .args(["compare"])
        .arg(&alpha)
        .arg(&beta)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "absolute compare targets failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Compare:"), "{stdout}");
    assert!(stdout.contains("targets:"), "{stdout}");
    assert!(stdout.contains("> Next: srcwalk show"), "{stdout}");
}

#[test]
fn windows_globs_and_slash_route_queries_work() {
    let dir = fixture_repo("windows_glob_route");

    let out = srcwalk()
        .args(["discover", "**/*.rs", "--as", "file", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "slash glob failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lib.rs"), "{stdout}");

    let out = srcwalk()
        .args(["discover", "/api/gold", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "slash route query failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/api/gold"), "{stdout}");
}

#[test]
fn windows_path_filters_accept_slash_and_backslash() {
    let dir = fixture_repo("windows_path_filter");

    for filter in [r"path:src\lib.rs", "path:src/lib.rs"] {
        let out = srcwalk()
            .args(["trace", "callers", "beta", "--scope"])
            .arg(&dir)
            .args(["--filter", filter])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "filter {filter} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("alpha"),
            "filter {filter} missed caller:\n{stdout}"
        );
    }
}
