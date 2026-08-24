use std::ffi::OsStr;
use std::io::Read;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Text of the single primary action srcwalk prints when a newer release
/// exists, shared verbatim between `update`, `update --check`, and the
/// compatibility `version --check` path (US-074).
pub(crate) const UPDATE_HINT: &str = "Run: srcwalk update";

/// Cap on captured child stderr across every bounded process call. Only used
/// for error diagnostics, so a small cap is enough and keeps every call site
/// bounded without a dedicated cap parameter.
pub(crate) const MAX_STDERR_BYTES: usize = 4 * 1024;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_NPM_JSON_BYTES: usize = 16 * 1024;
const MAX_NPM_CLI_BYTES: usize = 1024;
const RESOLVER_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn run_version(check: bool) {
    println!("{}", version_line());

    if !check {
        return;
    }

    match resolve_update_decision(None) {
        Ok(decision) => print_check_result(&decision),
        Err(err) => {
            print_resolution_failure(&err);
            process::exit(1);
        }
    }
}

/// The `srcwalk X.Y.Z (provenance)` line. Provenance is the git short-SHA
/// (with `-dirty` when the working tree changed at build) plus the UTC build
/// date, or `unknown` when git was unavailable at build time (US-061).
pub(crate) fn version_line() -> String {
    let current = env!("CARGO_PKG_VERSION");
    let label = env!("SRCWALK_GIT_LABEL");
    if label == "unknown" {
        format!("srcwalk {current} (unknown)")
    } else {
        let date = env!("SRCWALK_BUILD_DATE");
        format!("srcwalk {current} ({label}, {date})")
    }
}

/// The one typed local-vs-latest decision, shared verbatim between `update`,
/// `update --check`, and compatibility `version --check` (US-074). Every
/// variant carries `latest` so the shared renderer can print it uniformly,
/// matching the pre-existing `version --check` contract for every outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateDecision {
    Equal { current: String, latest: String },
    LocalNewer { current: String, latest: String },
    Newer { current: String, latest: String },
}

impl UpdateDecision {
    fn latest(&self) -> &str {
        match self {
            UpdateDecision::Equal { latest, .. }
            | UpdateDecision::LocalNewer { latest, .. }
            | UpdateDecision::Newer { latest, .. } => latest,
        }
    }
}

/// Resolves the shared local-vs-latest decision. `path_override` replaces the
/// child processes' `PATH` (tests only); production callers pass `None` so
/// resolver commands inherit the real environment.
pub(crate) fn resolve_update_decision(
    path_override: Option<&OsStr>,
) -> Result<UpdateDecision, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = fetch_latest_version_with(path_override)?;
    match compare_versions(&current, &latest)? {
        VersionOrdering::Equal => Ok(UpdateDecision::Equal { current, latest }),
        VersionOrdering::LocalNewer => Ok(UpdateDecision::LocalNewer { current, latest }),
        VersionOrdering::LatestNewer => Ok(UpdateDecision::Newer { current, latest }),
    }
}

/// Exact status text for `Equal`/`LocalNewer`, identical across every caller.
/// `Newer` has no single-line text here: the shared renderer prints a blank
/// line and [`UPDATE_HINT`] instead (or, for a full `update`, proceeds to a
/// channel action).
pub(crate) fn decision_status_line(decision: &UpdateDecision) -> Option<String> {
    match decision {
        UpdateDecision::Equal { .. } => Some("Already up to date.".to_string()),
        UpdateDecision::LocalNewer { current, latest } => Some(format!(
            "Local build {current} is newer than latest release {latest}."
        )),
        UpdateDecision::Newer { .. } => None,
    }
}

/// The exact printed lines for a check-only result: `latest X.Y.Z` for every
/// decision (equal, local-newer, or newer alike), then either the shared
/// status line or a blank line + [`UPDATE_HINT`]. One renderer, reused by
/// `version --check` and `update --check`, so their text can never fork.
pub(crate) fn render_check_result(decision: &UpdateDecision) -> Vec<String> {
    let mut lines = vec![format!("latest {}", decision.latest())];
    match decision_status_line(decision) {
        Some(line) => lines.push(line),
        None => {
            lines.push(String::new());
            lines.push(UPDATE_HINT.to_string());
        }
    }
    lines
}

pub(crate) fn print_check_result(decision: &UpdateDecision) {
    for line in render_check_result(decision) {
        println!("{line}");
    }
}

/// Shared failure rendering for a failed latest-version resolution: which
/// sources were tried, then a manual fallback. Used by both `version --check`
/// and `update`/`update --check`.
pub(crate) fn print_resolution_failure(err: &str) {
    eprintln!("error: could not check latest srcwalk release.");
    eprintln!();
    eprintln!("Tried:");
    for source in err.split(';') {
        let label = source
            .split_once(':')
            .map_or(source, |(label, _)| label)
            .trim();
        if !label.is_empty() {
            eprintln!("  - {label}");
        }
    }
    eprintln!();
    eprintln!("Update manually:");
    eprintln!("  npm install -g srcwalk@latest");
    eprintln!("  cargo install srcwalk --locked --force");
}

pub(crate) fn command_with_path(program: &str, path_override: Option<&OsStr>) -> process::Command {
    let mut cmd = process::Command::new(program);
    if let Some(path) = path_override {
        cmd.env("PATH", path);
    }
    cmd
}

/// Bounded capture of a finished child process: at most `max_stdout` bytes of
/// stdout (exceeding it kills the child and fails), stderr capped at
/// [`MAX_STDERR_BYTES`], and a wall-clock `timeout` that kills a hung child
/// even if it never approaches the byte cap. No call site may buffer an
/// unbounded response or wait forever.
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) status: process::ExitStatus,
}

pub(crate) fn run_bounded(
    mut cmd: process::Command,
    max_stdout: usize,
    timeout: Duration,
) -> Result<BoundedOutput, String> {
    let mut child = cmd
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not spawn: {e}"))?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let child = Arc::new(Mutex::new(child));

    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let watchdog_child = Arc::clone(&child);
    // Stays armed until explicitly told `done` below — which only happens
    // once the child has actually exited, not merely once its stdout has
    // been fully read. A child can close stdout early and keep running well
    // past `timeout` before actually exiting; disarming on read-completion
    // alone would let that case hang forever in the wait loop below.
    let watchdog = std::thread::spawn(move || -> bool {
        match done_rx.recv_timeout(timeout) {
            Ok(()) => false,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut guard) = watchdog_child.lock() {
                    let _ = guard.kill();
                }
                true
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => false,
        }
    });

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe
            .by_ref()
            .take(MAX_STDERR_BYTES as u64)
            .read_to_end(&mut buf);
        buf
    });

    let mut stdout_buf = Vec::new();
    let read_result = stdout_pipe
        .by_ref()
        .take(max_stdout as u64 + 1)
        .read_to_end(&mut stdout_buf);
    let exceeded = stdout_buf.len() > max_stdout;
    if exceeded {
        // Already known-bad; kill immediately instead of waiting out the
        // watchdog's timeout for no reason.
        if let Ok(mut guard) = child.lock() {
            let _ = guard.kill();
        }
    }

    // Poll for exit instead of a blocking `wait()`: holding the mutex across
    // a blocking wait would starve the watchdog (or the exceeded-kill above)
    // of the lock it needs to actually kill the child, deadlocking a hung
    // child against its own timeout. Each poll only holds the lock briefly.
    let status = loop {
        let mut guard = child.lock().expect("child mutex poisoned");
        match guard.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                drop(guard);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => break Err(format!("could not wait for child: {e}")),
        }
    }?;

    // The child has now actually exited (naturally, or via the exceeded-kill
    // above, or via the watchdog's timeout-kill); only now is it safe to
    // disarm the watchdog.
    let _ = done_tx.send(());
    let timed_out = watchdog.join().unwrap_or(false);
    let stderr = stderr_handle.join().unwrap_or_default();

    read_result.map_err(|e| format!("could not read output: {e}"))?;
    if timed_out {
        return Err(format!("timed out after {timeout:?}"));
    }
    if exceeded {
        return Err(format!("output exceeds {max_stdout} bytes"));
    }

    Ok(BoundedOutput {
        stdout: stdout_buf,
        stderr,
        status,
    })
}

/// Bounded, trimmed stderr detail for a failure message — never a full or
/// unbounded body, only whatever `run_bounded` already captured within
/// [`MAX_STDERR_BYTES`], trimmed. Empty when the child wrote nothing.
pub(crate) fn stderr_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    }
}

type VersionFetchAttempt = (&'static str, fn(Option<&OsStr>) -> Result<String, String>);

pub(crate) fn fetch_latest_version_with(path_override: Option<&OsStr>) -> Result<String, String> {
    let attempts: &[VersionFetchAttempt] = &[
        ("GitHub latest via curl", fetch_github_latest_with_curl),
        ("GitHub latest via wget", fetch_github_latest_with_wget),
        ("npm registry via curl", fetch_npm_latest_with_curl),
        ("npm registry via wget", fetch_npm_latest_with_wget),
        ("npm CLI", fetch_npm_latest_with_npm),
    ];

    let mut errors = Vec::new();
    for (label, attempt) in attempts {
        match attempt(path_override) {
            Ok(version) => return Ok(version),
            Err(err) => errors.push(format!("{label}: {err}")),
        }
    }

    Err(errors.join("; "))
}

/// Every candidate version must pass strict stable semver before it can be
/// accepted; a malformed candidate is a failed attempt, not a silent 0.0.0.
fn accept_candidate(raw: &str) -> Result<String, String> {
    parse_strict_semver(raw).map_err(|e| format!("malformed version `{raw}`: {e}"))?;
    Ok(raw.to_string())
}

const GITHUB_LATEST_URL: &str = "https://github.com/sting8k/srcwalk/releases/latest";
const NPM_LATEST_URL: &str = "https://registry.npmjs.org/srcwalk/latest";

fn curl_header_check_args() -> Vec<String> {
    vec![
        "-fsSI".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--max-time".to_string(),
        "5".to_string(),
        GITHUB_LATEST_URL.to_string(),
    ]
}

fn wget_header_check_args() -> Vec<String> {
    vec![
        "--server-response".to_string(),
        "--spider".to_string(),
        "--max-redirect=0".to_string(),
        "--https-only".to_string(),
        "--timeout=5".to_string(),
        GITHUB_LATEST_URL.to_string(),
    ]
}

fn curl_npm_json_args() -> Vec<String> {
    vec![
        "-fsSL".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--max-time".to_string(),
        "5".to_string(),
        NPM_LATEST_URL.to_string(),
    ]
}

fn wget_npm_json_args() -> Vec<String> {
    vec![
        "-qO-".to_string(),
        "--https-only".to_string(),
        "--timeout=5".to_string(),
        NPM_LATEST_URL.to_string(),
    ]
}

fn fetch_github_latest_with_curl(path_override: Option<&OsStr>) -> Result<String, String> {
    let mut cmd = command_with_path("curl", path_override);
    cmd.args(curl_header_check_args());
    let output = run_bounded(cmd, MAX_HEADER_BYTES, RESOLVER_TIMEOUT)?;
    let headers = command_stdout(&output, "curl")?;
    let tag = parse_latest_tag_from_headers(&headers)
        .ok_or_else(|| "missing latest release redirect".to_string())?;
    accept_candidate(&tag)
}

fn fetch_github_latest_with_wget(path_override: Option<&OsStr>) -> Result<String, String> {
    let mut cmd = command_with_path("wget", path_override);
    cmd.args(wget_header_check_args());
    let output = run_bounded(cmd, MAX_HEADER_BYTES, RESOLVER_TIMEOUT)?;

    // `wget --spider` writes headers to stderr and may exit non-zero on a 302
    // when redirects are disabled. Treat parseable headers as success.
    let mut headers = String::from_utf8_lossy(&output.stdout).into_owned();
    headers.push_str(&String::from_utf8_lossy(&output.stderr));
    let tag = parse_latest_tag_from_headers(&headers)
        .ok_or_else(|| format!("wget exited with {}", output.status))?;
    accept_candidate(&tag)
}

fn fetch_npm_latest_with_curl(path_override: Option<&OsStr>) -> Result<String, String> {
    let mut cmd = command_with_path("curl", path_override);
    cmd.args(curl_npm_json_args());
    let output = run_bounded(cmd, MAX_NPM_JSON_BYTES, RESOLVER_TIMEOUT)?;
    let json = command_stdout(&output, "curl")?;
    let version =
        parse_npm_version(&json).ok_or_else(|| "missing npm version field".to_string())?;
    accept_candidate(&version)
}

fn fetch_npm_latest_with_wget(path_override: Option<&OsStr>) -> Result<String, String> {
    let mut cmd = command_with_path("wget", path_override);
    cmd.args(wget_npm_json_args());
    let output = run_bounded(cmd, MAX_NPM_JSON_BYTES, RESOLVER_TIMEOUT)?;
    let json = command_stdout(&output, "wget")?;
    let version =
        parse_npm_version(&json).ok_or_else(|| "missing npm version field".to_string())?;
    accept_candidate(&version)
}

fn fetch_npm_latest_with_npm(path_override: Option<&OsStr>) -> Result<String, String> {
    let mut cmd = command_with_path("npm", path_override);
    cmd.args(["view", "srcwalk", "version", "--silent"]);
    let output = run_bounded(cmd, MAX_NPM_CLI_BYTES, RESOLVER_TIMEOUT)?;
    let version = command_stdout(&output, "npm")?;
    accept_candidate(version.trim())
}

fn command_stdout(output: &BoundedOutput, command: &str) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!(
            "{command} exited with {}{}",
            output.status,
            stderr_detail(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout.clone()).map_err(|e| format!("invalid UTF-8 response: {e}"))
}

fn parse_latest_tag_from_headers(headers: &str) -> Option<String> {
    let location = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("location") {
            return None;
        }
        Some(value.trim().to_string())
    })?;
    validated_release_tag_from_url(&location)
}

const EXPECTED_HOST: &str = "github.com";
const EXPECTED_PATH_PREFIX: &str = "/sting8k/srcwalk/releases/tag/v";

/// Accepts only an HTTPS URL whose host is exactly `github.com` and whose
/// path is exactly `/sting8k/srcwalk/releases/tag/vX.Y.Z` \u2014 no other host,
/// scheme, query string, fragment, or trailing path segment. Prevents a
/// crafted redirect from turning into an arbitrary accepted "latest" tag.
fn validated_release_tag_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, path_and_rest) = rest.split_once('/')?;
    if !host.eq_ignore_ascii_case(EXPECTED_HOST) {
        return None;
    }
    let path = format!("/{path_and_rest}");
    if path.contains('?') || path.contains('#') {
        return None;
    }
    let tag = path.strip_prefix(EXPECTED_PATH_PREFIX)?;
    if tag.is_empty() || tag.contains('/') {
        return None;
    }
    Some(tag.to_string())
}

fn parse_npm_version(json: &str) -> Option<String> {
    parse_json_string_field(json, "version")
}

fn parse_json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\"");
    let start = json.find(&marker)?;
    let after_marker = &json[start + marker.len()..];
    let colon = after_marker.find(':')?;
    let after_colon = after_marker[colon + 1..].trim_start();
    let quoted = after_colon.strip_prefix('"')?;
    let end = quoted.find('"')?;
    Some(quoted[..end].to_string())
}

/// Strict stable `X.Y.Z`: exactly three non-empty ASCII-decimal components,
/// no prerelease/build metadata, whitespace, or overflow. Never silently
/// defaults an invalid component to `0`.
pub(crate) fn parse_strict_semver(version: &str) -> Result<(u64, u64, u64), String> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("expected exactly X.Y.Z, got `{version}`"));
    }
    let mut nums = [0u64; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("invalid version component `{part}` in `{version}`"));
        }
        nums[i] = part
            .parse::<u64>()
            .map_err(|_| format!("version component overflow in `{version}`"))?;
    }
    Ok((nums[0], nums[1], nums[2]))
}

/// Three-way strict-semver ordering, distinguishing equality from local-newer
/// so dev builds never appear to need a downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionOrdering {
    Equal,
    LocalNewer,
    LatestNewer,
}

pub(crate) fn compare_versions(current: &str, latest: &str) -> Result<VersionOrdering, String> {
    let c = parse_strict_semver(current)?;
    let l = parse_strict_semver(latest)?;
    Ok(match c.cmp(&l) {
        std::cmp::Ordering::Equal => VersionOrdering::Equal,
        std::cmp::Ordering::Greater => VersionOrdering::LocalNewer,
        std::cmp::Ordering::Less => VersionOrdering::LatestNewer,
    })
}

/// One release asset's target triple and expected binary name. Mirrors
/// `npm/install.js`'s `PLATFORM_MAP` exactly; the two tables are cross-checked
/// against a shared pinned fixture so they cannot silently drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReleaseTarget {
    pub(crate) triple: &'static str,
    pub(crate) binary: &'static str,
}

/// Pure OS/arch -> release-asset mapping, independent of the running host, so
/// every row (including unsupported pairs) is directly testable.
pub(crate) fn release_target_for(os: &str, arch: &str) -> Option<ReleaseTarget> {
    let (triple, binary) = match (os, arch) {
        ("linux", "x86_64") => ("x86_64-unknown-linux-musl", "srcwalk"),
        ("linux", "aarch64") => ("aarch64-unknown-linux-musl", "srcwalk"),
        ("macos", "x86_64") => ("x86_64-apple-darwin", "srcwalk"),
        ("macos", "aarch64") => ("aarch64-apple-darwin", "srcwalk"),
        ("windows", "x86_64") => ("x86_64-pc-windows-msvc", "srcwalk.exe"),
        // Windows on ARM64 runs the x64 binary via OS emulation; no native
        // ARM64 asset is published (mirrors npm/install.js precedent).
        ("windows", "aarch64") => ("x86_64-pc-windows-msvc", "srcwalk.exe"),
        _ => return None,
    };
    Some(ReleaseTarget { triple, binary })
}

pub(crate) fn release_target() -> Option<ReleaseTarget> {
    release_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
#[path = "main_version_tests.rs"]
mod version_tests;
