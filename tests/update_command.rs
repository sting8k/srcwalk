//! CLI integration tests for `srcwalk update`/`update --check` (US-074).
//!
//! Zero real network: every test either strips `PATH` of network tools
//! entirely (proving no phone-home) or replaces `PATH` with fake curl/npm
//! scripts plus the real system `tar` (proving the full pipeline runs, with
//! network calls intercepted). No test ever touches a real developer
//! binary: every mutating test operates on a disposable copy of the just-
//! built test binary under a throwaway temp directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// RAII-cleaned: the returned `TempDir` removes its directory tree on drop,
/// so a disposable binary/archive built inside it never leaks into `/tmp`
/// even if an assertion below panics partway through a test.
fn temp_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("srcwalk_update_test_{name}_"))
        .tempdir()
        .unwrap()
}

#[cfg(unix)]
fn write_fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&path, perms).unwrap();
}

#[cfg(unix)]
fn disposable_copy(dir: &Path, name: &str) -> PathBuf {
    let copy_path = dir.join(name);
    fs::copy(env!("CARGO_BIN_EXE_srcwalk"), &copy_path).unwrap();
    let mut perms = fs::metadata(&copy_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&copy_path, perms).unwrap();
    copy_path
}

#[cfg(unix)]
const CURL_REDIRECT_ONLY: &str =
    "echo 'HTTP/2 302'\necho 'location: https://github.com/sting8k/srcwalk/releases/tag/v9.9.9'";

/// Looks up this host's release target triple from the same pinned
/// `tests/fixtures/release-targets.json` used by the Rust<->npm target-map
/// cross-check, instead of a third hardcoded (OS, ARCH) match living only
/// in this integration test.
#[cfg(unix)]
fn release_target_triple_for_this_host() -> String {
    let fixture = include_str!("fixtures/release-targets.json");
    let rows: Vec<serde_json::Value> = serde_json::from_str(fixture).unwrap();
    rows.iter()
        .find(|row| {
            row["rustOs"] == std::env::consts::OS && row["rustArch"] == std::env::consts::ARCH
        })
        .map(|row| row["target"].as_str().unwrap().to_string())
        .unwrap_or_else(|| {
            panic!(
                "unsupported test host os={} arch={}; extend tests/fixtures/release-targets.json",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })
}

// ---- 1: arg parsing — `update` accepts only `--check` ----

#[test]
fn update_rejects_unknown_flag() {
    let output = srcwalk().args(["update", "--force"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap must reject an unknown update flag before any resolver runs: {output:?}"
    );
}

#[test]
fn update_check_rejects_unknown_flag_too() {
    let output = srcwalk()
        .args(["update", "--check", "--force"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

// ---- 2: no-phone-home for unrelated/plain commands ----

#[cfg(unix)]
#[test]
fn plain_version_and_unrelated_commands_never_touch_network_tools() {
    let empty_path = temp_dir("empty-path");

    let output = srcwalk()
        .arg("version")
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "plain `version` must never need curl/wget/npm: {output:?}"
    );

    let output = srcwalk()
        .args(["discover", "fn", "--scope", "src"])
        .env("PATH", empty_path.path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "an unrelated command must never need curl/wget/npm: {output:?}"
    );
}

// ---- 3: `update --check` / `version --check` — shared renderer, zero mutation ----

#[cfg(unix)]
#[test]
fn update_check_reports_newer_without_any_mutation_tool_on_path() {
    let dir = temp_dir("update-check");
    // Only curl is on PATH: no wget/npm/tar. If `--check` ever attempted a
    // download, channel detection, or package-manager exec, it would fail
    // here with a missing-tool error instead of succeeding.
    write_fake_tool(dir.path(), "curl", CURL_REDIRECT_ONLY);

    let output = srcwalk()
        .args(["update", "--check"])
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("latest 9.9.9\n\nRun: srcwalk update\n"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn version_check_and_update_check_render_identical_newer_text() {
    let dir = temp_dir("compat-check");
    write_fake_tool(dir.path(), "curl", CURL_REDIRECT_ONLY);

    let update_out = srcwalk()
        .args(["update", "--check"])
        .env("PATH", dir.path())
        .output()
        .unwrap();
    let version_out = srcwalk()
        .args(["version", "--check"])
        .env("PATH", dir.path())
        .output()
        .unwrap();

    assert!(update_out.status.success(), "{update_out:?}");
    assert!(version_out.status.success(), "{version_out:?}");
    let update_stdout = String::from_utf8_lossy(&update_out.stdout);
    let version_stdout = String::from_utf8_lossy(&version_out.stdout);
    // `version --check` additionally prints the provenance line first; the
    // shared check-only render (latest + status/hint) must be identical text
    // after that, one renderer, not two forks of the same wording.
    let shared = "latest 9.9.9\n\nRun: srcwalk update\n";
    assert!(update_stdout.ends_with(shared), "{update_stdout}");
    assert!(version_stdout.ends_with(shared), "{version_stdout}");
}

// ---- 4: package-manager path — exact print + real exec, honest post-check ----

#[cfg(unix)]
#[test]
fn update_under_node_modules_prints_and_execs_npm_for_real() {
    let root = temp_dir("npm-path");
    let node_modules_dir = root.path().join("lib/node_modules/srcwalk/bin");
    fs::create_dir_all(&node_modules_dir).unwrap();
    let copy_path = disposable_copy(&node_modules_dir, "srcwalk");

    let tool_dir = temp_dir("npm-path-tools");
    write_fake_tool(tool_dir.path(), "curl", CURL_REDIRECT_ONLY);
    write_fake_tool(tool_dir.path(), "npm", "echo 'FAKE_NPM_INVOKED'\nexit 0");

    let output = Command::new(&copy_path)
        .arg("update")
        .env("PATH", tool_dir.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("npm install -g srcwalk@latest"), "{stdout}");
    assert!(
        stdout.contains("FAKE_NPM_INVOKED"),
        "{stdout}: npm's inherited stdio must show it was actually executed, not just printed"
    );
    // The disposable copy's own `--version` still reports the original
    // build (a fake npm script cannot really update a compiled binary), so
    // the post-check honestly fails here. That is the correct, honest
    // outcome: it proves print+exec+verify-attempt all ran for real,
    // without faking success.
    assert!(
        !output.status.success(),
        "an unverifiable npm update must not claim success: {output:?}"
    );
}

// ---- 5: standalone path — full verified swap via fake curl + real tar ----

#[cfg(unix)]
#[test]
fn standalone_update_performs_full_verified_swap() {
    use sha2::{Digest, Sha256};

    let root = temp_dir("standalone-root");
    let copy_path = disposable_copy(root.path(), "srcwalk");

    // Build the fake "new" binary: reports the fake latest version in the
    // canonical `srcwalk X.Y.Z (...)` shape `parse_reported_version` expects.
    let fixture_dir = temp_dir("standalone-fixture");
    let staging_dir = fixture_dir.path().join("staging");
    fs::create_dir_all(&staging_dir).unwrap();
    let fake_binary_path = staging_dir.join("srcwalk");
    fs::write(
        &fake_binary_path,
        "#!/bin/sh\necho 'srcwalk 9.9.9 (fake-e2e)'\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_binary_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&fake_binary_path, perms).unwrap();

    // Production always builds the archive name from the real host OS/arch;
    // looked up from the same pinned `tests/fixtures/release-targets.json`
    // that `release_target_matches_shared_fixture_used_by_npm_install_js`
    // (src/main_version_tests.rs) and `install.test.js` cross-check, so the
    // target-triple table never has a third, drifting copy.
    let target_triple = release_target_triple_for_this_host();
    let archive_name = format!("srcwalk-{target_triple}.tar.gz");
    let archive_path = fixture_dir.path().join(&archive_name);

    // Real system tar builds the fixture archive (exactly one entry).
    let status = Command::new("tar")
        .args(["czf", archive_name.as_str(), "-C", "staging", "srcwalk"])
        .current_dir(fixture_dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "fixture archive build failed");

    let archive_bytes = fs::read(&archive_path).unwrap();
    let sha256_hex: String = Sha256::digest(&archive_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let checksum_path = fixture_dir.path().join(format!("{archive_name}.sha256"));
    fs::write(&checksum_path, format!("{sha256_hex} {archive_name}\n")).unwrap();

    let tool_dir = temp_dir("standalone-tools");
    write_fake_tool(
        tool_dir.path(),
        "curl",
        &format!(
            "case \"$*\" in\n  *-fsSI*)\n    echo 'HTTP/2 302'\n    echo 'location: https://github.com/sting8k/srcwalk/releases/tag/v9.9.9'\n    exit 0\n    ;;\nesac\nfor a in \"$@\"; do url=\"$a\"; done\ncase \"$url\" in\n  *.sha256) cat '{}' ;;\n  *) cat '{}' ;;\nesac",
            checksum_path.display(),
            archive_path.display(),
        ),
    );
    // Real `tar` must still resolve: prepend the fake curl-only dir onto the
    // inherited PATH rather than replacing it. No fake `wget`/`npm` is
    // needed or present -- curl always succeeds here, so neither is ever
    // reached.
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let combined_path = format!("{}:{inherited_path}", tool_dir.path().display());

    let output = Command::new(&copy_path)
        .arg("update")
        .env("PATH", combined_path)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("latest 9.9.9"), "{stdout}");
    assert!(stdout.contains("-> 9.9.9."), "{stdout}");

    // The installed copy must now literally be the fake replacement.
    let installed = fs::read_to_string(&copy_path).unwrap();
    assert!(
        installed.contains("fake-e2e"),
        "installed binary was not actually replaced: {installed}"
    );

    let version_output = Command::new(&copy_path).arg("--version").output().unwrap();
    let version_stdout = String::from_utf8_lossy(&version_output.stdout);
    assert!(version_stdout.contains("9.9.9"), "{version_stdout}");
}
