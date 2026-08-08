use std::path::Path;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn deps(root: &Path) -> String {
    let output = srcwalk()
        .arg("deps")
        .arg(root.join("main.go"))
        .args(["--scope"])
        .arg(root)
        .output()
        .expect("srcwalk deps should run");
    assert!(
        output.status.success(),
        "srcwalk deps failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("deps output should be UTF-8")
}

fn write_fixture(root: &Path, go_mod: Option<&str>) {
    if let Some(go_mod) = go_mod {
        std::fs::write(root.join("go.mod"), go_mod).unwrap();
    }
    std::fs::write(
        root.join("main.go"),
        "package main\n\nimport \"fmt\"\nimport (\n    \"myapp/internal/config\"\n    alias \"golang.org/x/tools\"\n    _ \"myapp/side\"\n)\nimport \"net/http\"\n\nfunc main() { fmt.Println(http.StatusOK, config.Value) }\n",
    )
    .unwrap();
}

#[test]
fn dotless_module_local_import_survives_stdlib_omission() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), Some("module\tmyapp\n\ngo 1.23\n"));

    let stdout = deps(dir.path());
    assert!(
        stdout.contains("# Deps: main.go — 0 local, 3 external, 0 dependents"),
        "module-local dotless import must remain visible:\n{stdout}"
    );
    assert!(
        stdout
            .contains("## Uses (external)\ngolang.org/x/tools\nmyapp/internal/config\nmyapp/side"),
        "grouped and aliased sources must not be omitted:\n{stdout}"
    );
    assert!(!stdout.contains("fmt") && !stdout.contains("net/http"));
}

#[test]
fn missing_go_mod_keeps_unknown_dotless_import_but_omits_known_stdlib() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), None);

    let stdout = deps(dir.path());
    assert!(
        stdout
            .contains("## Uses (external)\ngolang.org/x/tools\nmyapp/internal/config\nmyapp/side"),
        "without go.mod, uncertain dotless imports must stay visible:\n{stdout}"
    );
    assert!(!stdout.contains("fmt") && !stdout.contains("net/http"));
}
