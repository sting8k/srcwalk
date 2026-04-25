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
fn impact_labels_name_matched_calls_and_groups_receivers() {
    let dir = temp_dir("impact_receivers");
    fs::write(
        dir.join("sample.ts"),
        r#"
export async function shutdown(conn: Conn, watcher: Watcher) {
  await conn.close();
  watcher.close();
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["close", "--impact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "impact should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("<- name-matched calls from"),
        "expected honest heading, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[group] receiver=conn count=1"),
        "expected conn receiver group, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[group] receiver=watcher count=1"),
        "expected watcher receiver group, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Warning: no definitions found; showing name-matched call sites only"),
        "expected no-definition warning, got:\n{stdout}"
    );
}

#[test]
fn impact_warns_for_broad_name_matched_symbols() {
    let dir = temp_dir("impact_broad");
    let mut body = String::from("export function many(items: any[]) {\n");
    for i in 0..51 {
        body.push_str(&format!("  items[{i}].close();\n"));
    }
    body.push_str("}\n");
    fs::write(dir.join("sample.ts"), body).unwrap();

    let out = srcwalk()
        .args(["close", "--impact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "impact should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("... 31 more call sites"),
        "expected capped callsite output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Warning: broad symbol name; impact is name-matched"),
        "expected broad symbol warning, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Impact output is capped for readability"),
        "expected capped footer, got:\n{stdout}"
    );
}
