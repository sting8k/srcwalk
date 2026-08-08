use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "srcwalk-deps-nonjs-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn deps(root: &Path, target: &str) -> String {
    let output = srcwalk()
        .args(["deps", target, "--scope"])
        .arg(root)
        .current_dir(root)
        .output()
        .expect("run deps");
    let stdout = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
    assert!(
        output.status.success(),
        "deps failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn python_resolves_unique_modules_omits_stdlib_and_abstains_on_ambiguity() {
    let root = fixture("python");
    write(&root, "pyproject.toml", "[project]\nname = \"fixture\"\n");
    write(
        &root,
        "app/main.py",
        "from tools.general_tools import helper\nimport requests\nimport json\nimport ambiguous\n",
    );
    write(&root, "tools/__init__.py", "");
    write(&root, "tools/general_tools.py", "def helper(): pass\n");
    write(&root, "ambiguous.py", "value = 1\n");
    write(&root, "src/ambiguous.py", "value = 2\n");

    let stdout = deps(&root, "app/main.py");
    assert!(
        stdout.contains("# Deps: app/main.py — 1 local, 1 external, 1 unresolved"),
        "Python resolution state counts must be evidence-backed:\n{stdout}"
    );
    assert!(
        stdout.contains("general_tools.py"),
        "unique Python module must be local:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (external)\nrequests"),
        "third-party module must remain external:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  4  ambiguous"),
        "ambiguous Python candidates must not be guessed:\n{stdout}"
    );
    assert!(
        !stdout.contains("\n  3  json"),
        "stdlib imports must stay omitted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn php_resolves_psr4_and_relative_files_but_keeps_unknown_namespace_unresolved() {
    let root = fixture("php");
    write(
        &root,
        "composer.json",
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    );
    write(
        &root,
        "src/Controller.php",
        "<?php\nuse App\\Service\\Runner;\nuse function App\\Fns\\run;\nuse App\\{Entity\\User, Support\\Clock as Time};\nuse Vendor\\Package\\Thing;\nrequire \"../bootstrap.php\";\ninclude_once \"../config.php\";\n",
    );
    for relative in [
        "src/Service/Runner.php",
        "src/Fns/run.php",
        "src/Entity/User.php",
        "src/Support/Clock.php",
        "bootstrap.php",
        "config.php",
    ] {
        write(&root, relative, "<?php\n");
    }

    let stdout = deps(&root, "src/Controller.php");
    assert!(
        stdout.contains("# Deps: src/Controller.php — 6 local, 0 external, 1 unresolved"),
        "PHP resolution state counts must be evidence-backed:\n{stdout}"
    );
    assert!(
        stdout.contains("Runner.php") && stdout.contains("bootstrap.php"),
        "PSR-4/relative imports must be local:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (unresolved local-looking)\n  5  Vendor/Package/Thing"),
        "unknown Composer namespace must remain unresolved:\n{stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn c_quoted_includes_need_unique_local_candidates_and_angle_includes_stay_external() {
    let root = fixture("c");
    write(
        &root,
        "src/main.cpp",
        "#include \"local/header.hpp\"\n#include \"ambiguous.hpp\"\n#include <vector>\n#include \"missing.hpp\"\n",
    );
    write(&root, "include/local/header.hpp", "#pragma once\n");
    write(&root, "src/ambiguous.hpp", "#pragma once\n");
    write(&root, "include/ambiguous.hpp", "#pragma once\n");

    let stdout = deps(&root, "src/main.cpp");
    assert!(
        stdout.contains("# Deps: src/main.cpp — 1 local, 1 external, 2 unresolved"),
        "C include state counts must be evidence-backed:\n{stdout}"
    );
    assert!(
        stdout.contains("header.hpp"),
        "unique quoted include must be local:\n{stdout}"
    );
    assert!(
        stdout.contains("## Uses (external)\n<vector>"),
        "angle include must remain external:\n{stdout}"
    );
    assert!(
        stdout.contains("  2  \"ambiguous.hpp\"") && stdout.contains("  4  \"missing.hpp\""),
        "ambiguous/missing quoted includes must remain unresolved:\n{stdout}"
    );

    let _ = fs::remove_dir_all(root);
}
