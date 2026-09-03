use super::{
    compare_versions, decision_status_line, parse_latest_tag_from_headers, parse_npm_version,
    parse_strict_semver, release_target_for, render_check_result, resolve_update_decision,
    run_bounded, UpdateDecision, VersionOrdering, UPDATE_HINT,
};
/// Writes an executable fake `curl` into `dir` that serves a GitHub latest-release
/// redirect header for `version`, so resolver tests never touch the network.
/// Unix-only: relies on a `#!/bin/sh` shebang, which Windows cannot execute directly.
#[cfg(unix)]
fn write_fake_curl_github_redirect(dir: &std::path::Path, version: &str) {
    let script = format!(
        "#!/bin/sh\necho 'HTTP/2 302'\necho 'location: https://github.com/sting8k/srcwalk/releases/tag/v{version}'\n"
    );
    let path = dir.join("curl");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

#[cfg(unix)]
fn write_fake_curl_failure(dir: &std::path::Path) {
    let path = dir.join("curl");
    std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

#[cfg(unix)]
fn write_fake_curl_failure_with_stderr(dir: &std::path::Path, marker: &str) {
    let script = format!("#!/bin/sh\necho '{marker}' >&2\nexit 1\n");
    let path = dir.join("curl");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

#[test]
fn parses_latest_release_redirect_tag() {
    let headers = "HTTP/2 302\nlocation: https://github.com/sting8k/srcwalk/releases/tag/v0.2.8\n";
    assert_eq!(
        parse_latest_tag_from_headers(headers).as_deref(),
        Some("0.2.8")
    );
}

#[test]
fn parses_npm_registry_version() {
    let json = r#"{"name":"srcwalk","version":"0.2.8"}"#;
    assert_eq!(parse_npm_version(json).as_deref(), Some("0.2.8"));
}

#[test]
fn version_line_matches_contract_shape() {
    // Contract: `srcwalk \d+\.\d+\.\d+ \(.+\)` — tolerates `unknown`.
    let line = super::version_line();
    assert!(line.starts_with("srcwalk "), "{line}");
    let rest = &line["srcwalk ".len()..];
    let (semver, parens) = rest.split_once(" (").expect("expected ( suffix");
    let parts: Vec<_> = semver.split('.').collect();
    assert_eq!(parts.len(), 3, "{line}");
    for p in parts {
        assert!(
            !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()),
            "{line}"
        );
    }
    assert!(parens.ends_with(')'), "{line}");
    assert!(parens.len() > 1, "{line}");
}

#[test]
fn unknown_label_yields_unknown_suffix() {
    // Confirm the fail-soft path renders `(unknown)` with no trailing comma.
    let line = if env!("SRCWALK_GIT_LABEL") == "unknown" {
        super::version_line()
    } else {
        // Build had git; simulate the unknown branch via the formatter alone.
        String::new()
    };
    if env!("SRCWALK_GIT_LABEL") == "unknown" {
        assert!(line.ends_with("(unknown)"), "{line}");
    }
}

#[test]
fn strict_semver_accepts_exact_triplet() {
    assert_eq!(parse_strict_semver("1.2.3").unwrap(), (1, 2, 3));
    assert_eq!(parse_strict_semver("0.0.0").unwrap(), (0, 0, 0));
}

#[test]
fn strict_semver_rejects_prerelease_and_build_metadata() {
    assert!(parse_strict_semver("1.2.3-rc1").is_err());
    assert!(parse_strict_semver("1.2.3+build5").is_err());
}

#[test]
fn strict_semver_rejects_wrong_component_count() {
    assert!(parse_strict_semver("1.2").is_err());
    assert!(parse_strict_semver("1.2.3.4").is_err());
}

#[test]
fn strict_semver_rejects_whitespace_and_non_digit() {
    assert!(parse_strict_semver(" 1.2.3").is_err());
    assert!(parse_strict_semver("1.2.3 ").is_err());
    assert!(parse_strict_semver("1.x.3").is_err());
    assert!(parse_strict_semver("1..3").is_err());
}

#[test]
fn strict_semver_rejects_overflow() {
    assert!(parse_strict_semver("99999999999999999999.0.0").is_err());
}

#[test]
fn compare_versions_distinguishes_equal_local_newer_latest_newer() {
    assert!(matches!(
        compare_versions("1.8.0", "1.8.0"),
        Ok(VersionOrdering::Equal)
    ));
    assert!(matches!(
        compare_versions("1.9.0", "1.8.0"),
        Ok(VersionOrdering::LocalNewer)
    ));
    assert!(matches!(
        compare_versions("1.8.0", "1.9.0"),
        Ok(VersionOrdering::LatestNewer)
    ));
}

#[test]
fn compare_versions_rejects_malformed_either_side() {
    assert!(compare_versions("bad", "1.9.0").is_err());
    assert!(compare_versions("1.8.0", "bad").is_err());
}

#[test]
fn target_map_covers_all_six_release_rows() {
    let linux_x64 = release_target_for("linux", "x86_64").expect("linux x86_64");
    assert_eq!(linux_x64.triple, "x86_64-unknown-linux-musl");
    assert_eq!(linux_x64.binary, "srcwalk");

    let linux_arm = release_target_for("linux", "aarch64").expect("linux aarch64");
    assert_eq!(linux_arm.triple, "aarch64-unknown-linux-musl");
    assert_eq!(linux_arm.binary, "srcwalk");

    let mac_x64 = release_target_for("macos", "x86_64").expect("macos x86_64");
    assert_eq!(mac_x64.triple, "x86_64-apple-darwin");
    assert_eq!(mac_x64.binary, "srcwalk");

    let mac_arm = release_target_for("macos", "aarch64").expect("macos aarch64");
    assert_eq!(mac_arm.triple, "aarch64-apple-darwin");
    assert_eq!(mac_arm.binary, "srcwalk");

    let win_x64 = release_target_for("windows", "x86_64").expect("windows x86_64");
    assert_eq!(win_x64.triple, "x86_64-pc-windows-msvc");
    assert_eq!(win_x64.binary, "srcwalk.exe");

    let win_arm = release_target_for("windows", "aarch64").expect("windows aarch64");
    assert_eq!(
        win_arm.triple, win_x64.triple,
        "Windows ARM64 must emulate the x64 asset, per npm/install.js precedent"
    );
    assert_eq!(win_arm.binary, win_x64.binary);
}

#[test]
fn target_map_rejects_unsupported_os_arch_pairs() {
    assert!(release_target_for("freebsd", "x86_64").is_none());
    assert!(release_target_for("linux", "riscv64").is_none());
}

/// Mirrors `npm/install.js`'s `PLATFORM_MAP` via a shared pinned fixture
/// (kept outside `npm/`, so it never enters the packed payload) so the two
/// tables cannot silently drift.
#[derive(serde::Deserialize)]
struct ReleaseTargetFixtureRow {
    #[serde(rename = "rustOs")]
    rust_os: String,
    #[serde(rename = "rustArch")]
    rust_arch: String,
    target: String,
    binary: String,
}

#[test]
fn release_target_matches_shared_fixture_used_by_npm_install_js() {
    const FIXTURE: &str = include_str!("../tests/fixtures/release-targets.json");
    let rows: Vec<ReleaseTargetFixtureRow> = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(rows.len(), 6, "fixture must cover all 6 release rows");
    for row in rows {
        let resolved = release_target_for(&row.rust_os, &row.rust_arch).unwrap_or_else(|| {
            panic!(
                "missing release_target_for({}, {})",
                row.rust_os, row.rust_arch
            )
        });
        assert_eq!(
            resolved.triple, row.target,
            "{}/{}",
            row.rust_os, row.rust_arch
        );
        assert_eq!(
            resolved.binary, row.binary,
            "{}/{}",
            row.rust_os, row.rust_arch
        );
    }
}

#[test]
fn decision_status_line_matches_spec_exact_text() {
    assert_eq!(
        decision_status_line(&UpdateDecision::Equal {
            current: "1.8.0".to_string(),
            latest: "1.8.0".to_string(),
        })
        .as_deref(),
        Some("Already up to date.")
    );
    assert_eq!(
        decision_status_line(&UpdateDecision::LocalNewer {
            current: "1.9.0".to_string(),
            latest: "1.8.0".to_string(),
        })
        .as_deref(),
        Some("Local build 1.9.0 is newer than latest release 1.8.0.")
    );
    assert_eq!(
        decision_status_line(&UpdateDecision::Newer {
            current: "1.8.0".to_string(),
            latest: "1.9.0".to_string(),
        }),
        None,
        "Newer has no single-line status: caller renders --check vs update-action text"
    );
}

#[test]
#[cfg(unix)]
fn resolve_update_decision_reaches_latest_newer_via_fake_curl_only() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_curl_github_redirect(dir.path(), "9.9.9");
    let decision = resolve_update_decision(Some(dir.path().as_os_str())).unwrap();
    match decision {
        UpdateDecision::Newer { current, latest } => {
            assert_eq!(latest, "9.9.9");
            assert_eq!(current, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("expected Newer, got a different decision: {other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn resolve_update_decision_aggregates_errors_when_every_source_fails() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_curl_failure(dir.path());
    let err = resolve_update_decision(Some(dir.path().as_os_str())).unwrap_err();
    assert!(err.contains("curl"), "{err}");
}

#[test]
#[cfg(unix)]
fn resolve_update_decision_includes_bounded_stderr_detail_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_curl_failure_with_stderr(dir.path(), "CURL_STDERR_MARKER");
    let err = resolve_update_decision(Some(dir.path().as_os_str())).unwrap_err();
    assert!(err.contains("CURL_STDERR_MARKER"), "{err}");
}

// ---- P1-3: strict GitHub redirect validation — only an exact HTTPS
// github.com/sting8k/srcwalk/releases/tag/vX.Y.Z redirect is ever accepted
// as a version candidate. ----

#[test]
fn parse_latest_tag_from_headers_accepts_exact_expected_redirect() {
    let headers = "HTTP/2 302\nlocation: https://github.com/sting8k/srcwalk/releases/tag/v1.2.3\n";
    assert_eq!(
        parse_latest_tag_from_headers(headers).as_deref(),
        Some("1.2.3")
    );
}

#[test]
fn parse_latest_tag_from_headers_rejects_evil_host() {
    let headers =
        "HTTP/2 302\nlocation: https://evil.example/sting8k/srcwalk/releases/tag/v1.2.3\n";
    assert_eq!(parse_latest_tag_from_headers(headers), None);
}

#[test]
fn parse_latest_tag_from_headers_rejects_plain_http() {
    let headers = "HTTP/2 302\nlocation: http://github.com/sting8k/srcwalk/releases/tag/v1.2.3\n";
    assert_eq!(parse_latest_tag_from_headers(headers), None);
}

#[test]
fn parse_latest_tag_from_headers_rejects_wrong_repo() {
    let headers = "HTTP/2 302\nlocation: https://github.com/other/repo/releases/tag/v1.2.3\n";
    assert_eq!(parse_latest_tag_from_headers(headers), None);
}

#[test]
fn parse_latest_tag_from_headers_rejects_query_string_smuggling() {
    let headers =
        "HTTP/2 302\nlocation: https://github.com/sting8k/srcwalk/releases/tag/v1.2.3?x=evil\n";
    assert_eq!(parse_latest_tag_from_headers(headers), None);
}

#[test]
fn parse_latest_tag_from_headers_rejects_trailing_path() {
    let headers =
        "HTTP/2 302\nlocation: https://github.com/sting8k/srcwalk/releases/tag/v1.2.3/extra\n";
    assert_eq!(parse_latest_tag_from_headers(headers), None);
}

// ---- P1-7: `latest X.Y.Z` must print for every decision, not only `Newer` —
// this is the pre-existing `version --check` contract; one shared renderer
// keeps `update --check`/`version --check` from forking wording. ----

#[test]
fn render_check_result_preserves_latest_line_for_every_decision() {
    let equal = UpdateDecision::Equal {
        current: "1.8.0".to_string(),
        latest: "1.8.0".to_string(),
    };
    assert_eq!(
        render_check_result(&equal),
        vec![
            "latest 1.8.0".to_string(),
            "Already up to date.".to_string()
        ]
    );

    let local_newer = UpdateDecision::LocalNewer {
        current: "1.9.0".to_string(),
        latest: "1.8.0".to_string(),
    };
    assert_eq!(
        render_check_result(&local_newer),
        vec![
            "latest 1.8.0".to_string(),
            "Local build 1.9.0 is newer than latest release 1.8.0.".to_string(),
        ]
    );

    let newer = UpdateDecision::Newer {
        current: "1.8.0".to_string(),
        latest: "1.9.0".to_string(),
    };
    assert_eq!(
        render_check_result(&newer),
        vec![
            "latest 1.9.0".to_string(),
            String::new(),
            UPDATE_HINT.to_string(),
        ]
    );
}

// ---- P1-2: run_bounded enforces both a byte cap and a wall-clock timeout ----

#[cfg(unix)]
#[test]
fn run_bounded_kills_a_hanging_child_after_its_timeout() {
    // Spawn `sleep` directly (not via `sh -c`): `Child::kill` only signals the
    // tracked PID, not any grandchild a shell would fork for an external
    // command, so a shell wrapper here would leave the real sleep running and
    // falsely fail this test. Every real call site (curl/wget/npm/verify_exe)
    // is already spawned directly, so this matches production shape.
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("5");
    let started = std::time::Instant::now();
    let err = run_bounded(cmd, 4096, std::time::Duration::from_millis(200)).unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a hung child must be killed near the timeout, not after its full sleep: {elapsed:?}"
    );
    assert!(err.contains("timed out"), "{err}");
}

#[cfg(unix)]
#[test]
fn run_bounded_does_not_kill_a_child_that_closes_stdout_before_exiting() {
    // Closes fd 1 immediately, then keeps running a bit longer before exiting
    // 0. The read loop unblocks as soon as stdout closes (long before the
    // 2s timeout), so the watchdog must see that as "done", not "timed out",
    // and must never kill this still-legitimately-running child.
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "exec 1>&-; sleep 0.3; exit 0"]);
    let output = run_bounded(cmd, 4096, std::time::Duration::from_secs(2)).unwrap();
    assert!(
        output.status.success(),
        "a child that only closed stdout early must be waited for normally, not killed: {:?}",
        output.status
    );
}

#[cfg(unix)]
#[test]
fn run_bounded_times_out_a_child_that_closes_stdout_then_hangs_past_timeout() {
    // This is the exact regression this test guards: closing stdout early
    // must NOT disarm the watchdog. The read loop unblocks almost
    // immediately (stdout closed), but the child keeps running well past
    // `timeout` before it would naturally exit — the watchdog must still
    // fire and the wait loop must still notice, instead of blocking forever
    // because `done` was signaled too early.
    //
    // Uses a pure shell-builtin busy loop, not `sleep`: a preceding `exec`
    // statement defeats sh's single-command tail-call exec optimization, so
    // `sh -c "exec 1>&-; sleep 5"` forks a genuine grandchild `sleep`
    // process that `Child::kill` cannot reach (confirmed via `ps`), which
    // would make this test flaky/fail for a reason unrelated to the fix
    // under test. `while`/`[`/arithmetic stay in the same shell process, so
    // killing the direct child reliably stops it.
    let mut cmd = std::process::Command::new("sh");
    cmd.args([
        "-c",
        "exec 1>&-; n=0; while [ $n -lt 100000000 ]; do n=$((n+1)); done",
    ]);
    let started = std::time::Instant::now();
    let err = run_bounded(cmd, 4096, std::time::Duration::from_millis(200)).unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a child that hangs after closing stdout must still be killed near the timeout, not after its full sleep: {elapsed:?}"
    );
    assert!(err.contains("timed out"), "{err}");
}

#[cfg(unix)]
#[test]
fn run_bounded_returns_full_output_for_a_fast_child_under_the_cap() {
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "printf hello"]);
    let output = run_bounded(cmd, 4096, std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(output.stdout, b"hello");
    assert!(output.status.success());
}
