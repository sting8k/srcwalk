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

fn write_nested_csharp_fixture(dir: &std::path::Path) {
    fs::write(
        dir.join("Sample.cs"),
        r#"namespace Demo
{
    internal class JavaScriptReader
    {
        public object Read()
        {
            object v = ReadCore();
            return v;
        }

        private object ReadCore()
        {
            return null;
        }
    }
}
"#,
    )
    .unwrap();
}

#[test]
fn csharp_nested_methods_are_symbol_definitions() {
    let dir = temp_dir("csharp_nested_def");
    write_nested_csharp_fixture(&dir);

    let out = srcwalk()
        .args(["ReadCore", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "symbol search should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("1 definitions"),
        "expected one definition, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[fn] Demo.ReadCore") || stdout.contains("[fn] ReadCore"),
        "expected ReadCore definition row, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Sample.cs:11-14"),
        "expected nested method range, got:\n{stdout}"
    );
}

#[test]
fn flow_works_for_csharp_nested_method_targets() {
    let dir = temp_dir("csharp_nested_flow");
    write_nested_csharp_fixture(&dir);

    let out = srcwalk()
        .args(["ReadCore", "--flow", "--scope"])
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
        stdout.contains("[symbol] ReadCore"),
        "expected flow symbol header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("<- callers"),
        "expected callers section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("JavaScriptReader.Read"),
        "expected Read caller, got:\n{stdout}"
    );
}
