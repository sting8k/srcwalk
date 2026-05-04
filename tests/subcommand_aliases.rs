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

fn assert_same_stdout(mut left: Command, mut right: Command) {
    let left = left.output().unwrap();
    let right = right.output().unwrap();

    assert!(
        left.status.success(),
        "left command failed:\n{}",
        String::from_utf8_lossy(&left.stderr)
    );
    assert!(
        right.status.success(),
        "right command failed:\n{}",
        String::from_utf8_lossy(&right.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&left.stdout),
        String::from_utf8_lossy(&right.stdout)
    );
}

#[test]
fn find_subcommand_matches_legacy_query_search() {
    let dir = temp_repo("find_alias");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let mut legacy = srcwalk();
    legacy
        .arg("alpha")
        .arg("--scope")
        .arg(&dir)
        .arg("--limit")
        .arg("1");

    let mut command = srcwalk();
    command
        .arg("find")
        .arg("alpha")
        .arg("--scope")
        .arg(&dir)
        .arg("--limit")
        .arg("1");

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callers_subcommand_matches_legacy_flag() {
    let dir = temp_repo("callers_alias");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let mut legacy = srcwalk();
    legacy
        .arg("alpha")
        .arg("--callers")
        .arg("--scope")
        .arg(&dir);

    let mut command = srcwalk();
    command.arg("callers").arg("alpha").arg("--scope").arg(&dir);

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callees_subcommand_matches_legacy_flag() {
    let dir = temp_repo("callees_alias");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let mut legacy = srcwalk();
    legacy.arg("beta").arg("--callees").arg("--scope").arg(&dir);

    let mut command = srcwalk();
    command.arg("callees").arg("beta").arg("--scope").arg(&dir);

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn flow_subcommand_matches_legacy_flag() {
    let dir = temp_repo("flow_alias");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let mut legacy = srcwalk();
    legacy.arg("beta").arg("--flow").arg("--scope").arg(&dir);

    let mut command = srcwalk();
    command.arg("flow").arg("beta").arg("--scope").arg(&dir);

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn impact_subcommand_matches_legacy_flag() {
    let dir = temp_repo("impact_alias");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let mut legacy = srcwalk();
    legacy.arg("alpha").arg("--impact").arg("--scope").arg(&dir);

    let mut command = srcwalk();
    command.arg("impact").arg("alpha").arg("--scope").arg(&dir);

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn deps_subcommand_matches_legacy_flag() {
    let dir = temp_repo("deps_alias");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "mod helper;\nfn alpha() {}\n").unwrap();
    fs::write(dir.join("src/helper.rs"), "pub fn helper() {}\n").unwrap();

    let mut legacy = srcwalk();
    legacy
        .arg("src/lib.rs")
        .arg("--deps")
        .arg("--scope")
        .arg(&dir);

    let mut command = srcwalk();
    command
        .arg("deps")
        .arg("src/lib.rs")
        .arg("--scope")
        .arg(&dir);

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_subcommand_matches_legacy_flag() {
    let dir = temp_repo("map_alias");
    fs::write(dir.join("lib.rs"), "fn alpha() {}\n").unwrap();

    let mut legacy = srcwalk();
    legacy
        .arg("--map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1");

    let mut command = srcwalk();
    command
        .arg("map")
        .arg("--scope")
        .arg(&dir)
        .arg("--depth")
        .arg("1");

    assert_same_stdout(legacy, command);
    let _ = fs::remove_dir_all(&dir);
}
