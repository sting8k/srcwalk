//! Task 2c — JS/TS alias resolution reaches read related-files and callees.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "srcwalk-js-read-alias-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn normalized(output: Output) -> (bool, String, String) {
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace('\\', "/"),
        String::from_utf8_lossy(&output.stderr).replace('\\', "/"),
    )
}

#[test]
fn alias_import_resolves_read_related_file_and_callee() {
    let root = fixture("callee-and-read");
    write(
        &root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@x/*":["src/*"]}}}"#,
    );
    write(
        &root,
        "src/main.ts",
        "import { helper } from '@x/helper';\n\nexport function run() {\n  return helper();\n}\n",
    );
    write(
        &root,
        "src/helper.ts",
        "export function helper() {\n  return 42;\n}\n",
    );

    let trace = normalized(
        srcwalk()
            .args(["trace", "callees", "run", "--scope"])
            .arg(&root)
            .output()
            .expect("run trace callees"),
    );
    assert!(
        trace.0,
        "trace failed:\nstdout={}\nstderr={}",
        trace.1, trace.2
    );
    assert!(
        trace.1.contains("helper") && trace.1.contains("src/helper.ts"),
        "alias callee should resolve to local helper:\n{}",
        trace.1
    );

    let read = normalized(
        srcwalk()
            .args(["show"])
            .arg(root.join("src/main.ts"))
            .args(["--scope"])
            .arg(&root)
            .output()
            .expect("run show"),
    );
    assert!(read.0, "read failed:\nstdout={}\nstderr={}", read.1, read.2);
    assert!(
        read.1.contains("> Related: src/helper.ts"),
        "alias related-file hint should retain local path:\n{}",
        read.1
    );

    let _ = fs::remove_dir_all(root);
}
