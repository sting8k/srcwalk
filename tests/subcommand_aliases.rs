use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SKILL_ENTRY: &str = include_str!("../skills/srcwalk/SKILL.md");

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
fn root_help_surfaces_guide_entry_point() {
    let output = srcwalk().arg("--help").output().unwrap();

    assert!(
        output.status.success(),
        "help command failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Start here:"));
    assert!(stdout.contains("srcwalk guide"));
    assert!(stdout.contains("Full embedded, version-matched agent guide"));
    assert!(stdout.contains("srcwalk version"));
    assert!(stdout.contains("Show version; add --check for latest"));
    assert!(!stdout.contains("overview"));
}

#[test]
fn version_subcommand_is_canonical_version_surface() {
    let output = srcwalk().arg("version").output().unwrap();

    assert!(
        output.status.success(),
        "version failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("srcwalk {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_help_exposes_check_flag() {
    let output = srcwalk().args(["version", "--help"]).output().unwrap();

    assert!(
        output.status.success(),
        "version help failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--check"));
    assert!(stdout.contains("latest release"));
}

#[test]
fn guide_subcommand_prints_full_embedded_skill() {
    let output = srcwalk().arg("guide").output().unwrap();

    assert!(
        output.status.success(),
        "guide command failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# srcwalk — agent routing policy"));
    assert!(stdout.contains("version-matched command guide"));
    assert!(stdout.contains("## Choose the command by intent"));
}

#[test]
fn skill_entry_points_to_embedded_guide() {
    assert!(SKILL_ENTRY.contains("# srcwalk — bootstrap entry"));
    assert!(SKILL_ENTRY.contains("srcwalk guide"));
    assert!(SKILL_ENTRY.contains("source of truth"));
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
