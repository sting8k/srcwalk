//! US-060b: no offered range wider than W=40 lines appears bare in discover /
//! trace output. Wide ranges are anchored to a bounded `> anchor: path:line`
//! evidence line plus a primary `> Next:` command (symbol-addressed when the
//! selector round-trips; a numeric range otherwise), so the full body is
//! always exactly one printed command away.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_us060b_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    for i in 0..4 {
        fs::write(
            dir.join(format!("caller{i}.js")),
            format!("import {{ target }} from './target.js';\nexport function caller{i}() {{ return target(); }}\n"),
        )
        .unwrap();
    }
    let mut target = String::from("export function target() {\n  const x = 1;\n");
    for i in 3..82 {
        target.push_str(&format!("  const y{i} = {i}; // long body\n"));
    }
    target.push_str("  return x;\n}\n");
    fs::write(dir.join("target.js"), &target).unwrap();
    dir
}

/// Extract widths of any bare `path:A-B` ranges offered inside `> Next:` show
/// commands (i.e. not already behind an `> expand:` line).
fn bare_offered_widths(stdout: &str) -> Vec<usize> {
    let mut widths = Vec::new();
    for line in stdout.lines() {
        if !line.starts_with("> Next: srcwalk show") || line.contains("--section") {
            continue;
        }
        // Find `path:A-B` tokens in the quoted arg.
        let after = line.split("srcwalk show").nth(1).unwrap_or("");
        for tok in after.split(',') {
            let tok = tok.trim().trim_matches('\'');
            if let Some((_, range)) = tok.rsplit_once(':') {
                if let Some((a, b)) = range.split_once('-') {
                    if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        if b >= a {
                            widths.push((b - a + 1) as usize);
                        }
                    }
                }
            }
        }
    }
    widths
}

#[test]
fn discover_offers_no_bare_range_wider_than_forty() {
    let dir = fixture("discover_no_wide_bare");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "discover failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The wide definition range is symbol-addressed: the symbol command is the
    // primary `> Next:` action, and the numeric range is non-action evidence
    // metadata (a bounded START-line preview), never a competing `> expand:`
    // or an action-shaped `> anchor:` line repeating the full body range.
    assert!(
        stdout.contains("> Next: srcwalk show target.js:target"),
        "wide range should be symbol-addressed as the primary next action, got:\n{stdout}"
    );
    assert!(
        stdout.contains("  evidence anchor: target.js:1 (bounded preview; not the body address)"),
        "START-line anchor should appear as plain metadata, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("> anchor:"),
        "anchor must not be action-shaped (no '> anchor:'), got:\n{stdout}"
    );
    assert!(
        !stdout.contains("evidence anchor: target.js:1-83"),
        "anchor must not repeat the full body range, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("> expand:"),
        "no expand line may compete with the symbol next action:\n{stdout}"
    );

    // No bare offer exposes a >40-line range.
    let widths = bare_offered_widths(&stdout);
    assert!(
        widths.iter().all(|w| *w <= 40),
        "found a bare offered range >40 lines: {widths:?}\n\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn trace_callers_offers_no_bare_range_wider_than_forty() {
    let dir = fixture("trace_no_wide_bare");
    // target has many callers; its own range is wide.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["trace", "callers", "target", "--scope", "."])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "trace callers failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let widths = bare_offered_widths(&stdout);
    assert!(
        widths.iter().all(|w| *w <= 40),
        "trace callers exposed a bare range >40 lines: {widths:?}\n\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}
