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
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with(&format!("srcwalk {} (", env!("CARGO_PKG_VERSION"))),
        "version should carry provenance suffix:\n{stdout}"
    );
    assert!(
        stdout.trim_end().ends_with(')'),
        "provenance suffix:\n{stdout}"
    );
}

#[test]
fn root_version_flags_match_version_subcommand() {
    for flag in ["--version", "-V"] {
        let output = srcwalk().arg(flag).output().unwrap();

        assert!(
            output.status.success(),
            "{flag} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with(&format!("srcwalk {} (", env!("CARGO_PKG_VERSION"))),
            "version should carry provenance suffix:\n{stdout}"
        );
        assert!(
            stdout.trim_end().ends_with(')'),
            "provenance suffix:\n{stdout}"
        );
    }
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
fn guide_subcommand_surfaces_compact_decision_contract_and_guardrails() {
    let output = srcwalk().arg("guide").output().unwrap();

    assert!(
        output.status.success(),
        "guide command failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Choose one route first"));
    assert!(stdout.contains("unknown area | `srcwalk overview --scope <dir>`"));
    assert!(
        stdout.contains("unknown target in known area | `srcwalk discover <query> --scope <dir>`")
    );
    assert!(stdout.contains("known body/citation | `srcwalk show <path>:<line-or-range>`"));
    assert!(stdout.contains("need rich local packet | `srcwalk context <target> --scope <dir>`"));
    assert!(stdout.contains("## Batch by evidence dependency"));
    assert!(stdout.contains("independent discoveries or exact reads in parallel"));
    assert!(stdout.contains("Multi-root symbol discovery may repeat the flag"));
    assert!(stdout.contains("`context` accepts up to 3 comma-separated exact"));
    assert!(
        stdout.contains("Structural syntax/source is navigation evidence, not runtime behavior")
    );
    let choose = stdout.find("## Choose one route first").unwrap();
    let dependency_wave = stdout.find("## Batch by evidence dependency").unwrap();
    let guardrails = stdout.find("## Command-shape guardrails").unwrap();
    let trust = stdout.find("## Evidence trust bounds").unwrap();
    let reference = stdout.find("## Routes and examples").unwrap();
    assert!(
        choose < dependency_wave
            && dependency_wave < guardrails
            && guardrails < trust
            && trust < reference,
        "routing contract must precede reference details:\n{stdout}"
    );
    assert!(!stdout.contains("## Default workflow"));
    assert!(
        stdout.lines().count() <= 155,
        "embedded guide exceeded compactness budget:\n{stdout}"
    );
    assert!(!stdout.contains("srcwalk hints"));
}

#[test]
fn discover_help_surfaces_symbol_batch_cap_and_repeatable_scope() {
    let output = srcwalk().args(["discover", "--help"]).output().unwrap();

    assert!(
        output.status.success(),
        "discover help failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2-5 comma-separated symbol batch"));
    assert!(stdout.contains("use --as symbol"));
    assert!(stdout.contains("comma-separated literal OR for text"));
    assert!(stdout.contains("Symbol discovery may repeat --scope"));
    assert!(stdout.contains("text/file/access modes use one scope"));
}

#[test]
fn context_help_surfaces_multi_target_exact_limit() {
    let output = srcwalk().args(["context", "--help"]).output().unwrap();

    assert!(
        output.status.success(),
        "context help failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("up to 3 comma-separated exact path targets"),
        "context help should document multi-target exact syntax:\n{stdout}"
    );
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
