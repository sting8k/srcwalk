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
fn windows_absolute_same_file_range_shorthand_routes_to_section_reader() {
    let dir = fixture_repo("windows_show_same_file_shorthand");
    let file = dir.join("src").join("lib.rs");
    let inline_target = format!("{}:1,4", file.display());

    let inline = srcwalk().arg(&inline_target).output().unwrap();
    let explicit = srcwalk()
        .arg(&file)
        .args(["--section", "1,4"])
        .output()
        .unwrap();

    assert!(
        inline.status.success(),
        "absolute same-file shorthand failed:\n{}",
        String::from_utf8_lossy(&inline.stderr)
    );
    assert!(
        explicit.status.success(),
        "explicit section failed:\n{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&inline.stdout),
        String::from_utf8_lossy(&explicit.stdout)
    );
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

#[test]
fn windows_full_file_cap_footer_omits_eof_only_continuation_range() {
    let dir = temp_repo("windows_full_file_cap_footer");
    let path = dir.join("large.txt");
    fs::write(
        &path,
        (0..200).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();

    let out = srcwalk()
        .arg(&path)
        .arg("--full")
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "full read failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("full capped — tokens ~"), "{stdout}");
    assert!(
        stdout.contains("> Next: use --section <symbol|range[,symbol|range]>")
            && !stdout.contains("Continue from --section")
            && !stdout.contains("--section 201-201"),
        "expected EOF-only continuation range to stay suppressed:\n{stdout}"
    );
}

#[test]
fn windows_suppressed_structural_target_trace_uses_relative_path() {
    let dir = temp_repo("windows_suppressed_target_trace");
    let mut content = String::from("fn target() {\n");
    for line in 2..=200 {
        content.push_str(&format!("    let v{line} = {line};\n"));
    }
    content.push_str("}\n");
    fs::write(dir.join("long.rs"), content).unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "discover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("> Caveat: confirmed structural target long.rs:1-201 spans 201 lines, over the 200-line next-action bound."),
        "{stdout}"
    );
    assert!(
        !stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
