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

#[test]
fn root_help_surfaces_guide_entry_point_and_intent_inventory() {
    let output = srcwalk().arg("--help").output().unwrap();

    assert!(
        output.status.success(),
        "help command failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Start here:"));
    assert!(stdout.contains("srcwalk guide"));
    assert!(stdout.contains("srcwalk discover <query>"));
    assert!(stdout.contains("srcwalk trace callers <symbol>"));
    assert!(stdout.contains("srcwalk context <symbol-or-file:line>"));
    assert!(stdout.contains("srcwalk review <range-or-staged>"));
    assert!(stdout.contains("srcwalk assess <symbol>"));
    assert!(stdout.contains("srcwalk version"));
    let common = stdout.split("Common:").nth(1).expect("Common block");
    let overview = common.find("srcwalk overview").expect("overview example");
    let context = common.find("srcwalk context").expect("context example");
    let discover = common.find("srcwalk discover").expect("discover example");
    let show = common.find("srcwalk show").expect("show example");
    assert!(
        overview < discover,
        "overview should precede discover in Common block"
    );
    assert!(
        context < show,
        "context should precede show in Common block"
    );
    let commands = stdout
        .split("Commands:")
        .nth(1)
        .and_then(|tail| tail.split("Arguments:").next())
        .expect("Commands block");
    let command_overview = commands.find("overview").expect("overview command");
    let command_context = commands.find("context").expect("context command");
    let command_discover = commands.find("discover").expect("discover command");
    let command_show = commands.find("show").expect("show command");
    assert!(
        command_overview < command_discover,
        "overview command should precede discover"
    );
    assert!(
        command_context < command_show,
        "context command should precede show"
    );
    assert!(!stdout.contains("Compatibility:"));
    assert!(!stdout.contains("srcwalk find <query>"));
    assert!(!stdout.contains("srcwalk decision-flow"));
    assert!(!stdout.contains("srcwalk diff"));
}

#[test]
fn artifact_help_is_discoverable_on_root_and_relation_commands() {
    let root = srcwalk().arg("--help").output().unwrap();
    assert!(
        root.status.success(),
        "root help failed:\n{}",
        String::from_utf8_lossy(&root.stderr)
    );
    let root_stdout = String::from_utf8_lossy(&root.stdout);
    assert!(root_stdout.contains("--artifact"), "{root_stdout}");
    assert!(
        root_stdout.contains("exact artifact file reads may auto-enable this"),
        "{root_stdout}"
    );

    for args in [
        ["discover", "--help"].as_slice(),
        ["trace", "callers", "--help"].as_slice(),
        ["trace", "callees", "--help"].as_slice(),
    ] {
        let output = srcwalk().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "help failed for {args:?}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--artifact"), "{stdout}");
        assert!(
            stdout.contains("artifact-level evidence"),
            "help should label artifact evidence:\n{stdout}"
        );
    }

    for args in [
        ["trace", "callers", "--help"].as_slice(),
        ["trace", "callees", "--help"].as_slice(),
    ] {
        let output = srcwalk().args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("direct-only"),
            "relation help should name artifact relation limits:\n{stdout}"
        );
    }
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
    assert!(stdout.contains("# srcwalk — agent evidence contract"));
    assert!(stdout.contains("Default to srcwalk first for code-structure work"));
    assert!(stdout.contains("## Routes"));
}

#[test]
fn skill_entry_points_to_embedded_guide() {
    assert!(SKILL_ENTRY.contains("# srcwalk — bootstrap entry"));
    assert!(SKILL_ENTRY.contains("srcwalk guide"));
    assert!(SKILL_ENTRY.contains("source of truth"));
}

#[test]
fn root_level_options_before_subcommands_are_rejected() {
    for args in [
        ["--scope", "src", "discover", "RunConfig"].as_slice(),
        ["--budget", "100", "discover", "RunConfig"].as_slice(),
        ["--artifact", "trace", "callers", "RunConfig"].as_slice(),
    ] {
        let output = srcwalk().args(args).output().unwrap();
        assert!(
            !output.status.success(),
            "root-level option before subcommand should fail: {args:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("root-level options do not apply to subcommands")
                && stderr.contains("put options after the subcommand"),
            "expected root-option placement error, got:\n{stderr}"
        );
    }
}

#[test]
fn discover_command_searches_candidates() {
    let dir = temp_repo("discover_command");
    fs::write(
        dir.join("lib.rs"),
        "fn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();

    let output = srcwalk()
        .arg("discover")
        .arg("alpha")
        .arg("--scope")
        .arg(&dir)
        .arg("--limit")
        .arg("1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("alpha"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn intent_commands_route_to_existing_capabilities() {
    let dir = temp_repo("intent_commands");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "mod helper;\nfn alpha() {}\nfn beta() { alpha(); }\n",
    )
    .unwrap();
    fs::write(dir.join("src/helper.rs"), "pub fn helper() {}\n").unwrap();

    for args in [
        ["trace", "callers", "alpha", "--scope"].as_slice(),
        ["trace", "callees", "beta", "--scope"].as_slice(),
        ["context", "beta", "--scope"].as_slice(),
        ["assess", "alpha", "--scope"].as_slice(),
        ["deps", "src/lib.rs", "--scope"].as_slice(),
        ["overview", "--scope"].as_slice(),
    ] {
        let output = srcwalk().args(args).arg(&dir).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn removed_action_first_commands_fail() {
    for args in [
        ["find", "alpha"].as_slice(),
        ["files", "*.rs"].as_slice(),
        ["callers", "alpha"].as_slice(),
        ["callees", "alpha"].as_slice(),
        ["flow", "alpha"].as_slice(),
        ["impact", "alpha"].as_slice(),
        ["alpha", "--callers"].as_slice(),
    ] {
        let output = srcwalk().args(args).output().unwrap();
        assert!(
            !output.status.success(),
            "removed surface should fail: {args:?}"
        );
    }
}
