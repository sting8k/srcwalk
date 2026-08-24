//! Installation-aware `srcwalk update` (US-074): channel detection, package-manager
//! action, and download/integrity/transaction orchestration. Latest-version
//! resolution, strict semver, and target mapping stay in `version.rs`; this module
//! only decides *how* to apply an already-resolved newer version.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

/// Which package manager (if any) owns the running executable, detected from
/// `current_exe()`'s normalized path components. Conservative: anything not
/// recognizably under a `node_modules` tree is `Standalone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallChannel {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Standalone,
}

/// Pure channel classification for an explicit `windows` matching mode, so
/// tests can pin exact platform semantics regardless of which host runs
/// them. Splits on both `/` and `\` (path-component normalization applies on
/// every host). `windows == true` compares markers ASCII case-insensitively;
/// `windows == false` compares components exactly, so e.g. an uppercase
/// `NODE_MODULES` component never matches on Unix. Component-exact either
/// way: a decoy like `my_node_modules_backup` never matches `node_modules`
/// by substring, and a bare `yarn` directory name (no leading dot) is not a
/// Yarn marker on its own.
pub(crate) fn detect_channel_for(path: &Path, windows: bool) -> InstallChannel {
    let components: Vec<String> = path
        .to_string_lossy()
        .split(['/', '\\'])
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
        .collect();

    let matches = |component: &str, marker: &str| {
        if windows {
            component.eq_ignore_ascii_case(marker)
        } else {
            component == marker
        }
    };
    let any_matches = |markers: &[&str]| {
        components
            .iter()
            .any(|c| markers.iter().any(|m| matches(c, m)))
    };

    if !any_matches(&["node_modules"]) {
        return InstallChannel::Standalone;
    }
    if any_matches(&[".pnpm", "pnpm-global"]) {
        return InstallChannel::Pnpm;
    }
    if any_matches(&[".bun"]) {
        return InstallChannel::Bun;
    }
    if any_matches(&[".yarn"]) {
        return InstallChannel::Yarn;
    }
    InstallChannel::Npm
}

/// Runtime wrapper: uses the host's real matching mode.
pub(crate) fn detect_channel(path: &Path) -> InstallChannel {
    detect_channel_for(path, cfg!(windows))
}

/// Fixed allowlisted program + argv per channel; `Standalone` has none.
pub(crate) fn package_manager_argv(
    channel: InstallChannel,
) -> Option<(&'static str, &'static [&'static str])> {
    match channel {
        InstallChannel::Npm => Some(("npm", &["install", "-g", "srcwalk@latest"])),
        InstallChannel::Pnpm => Some(("pnpm", &["add", "-g", "srcwalk@latest"])),
        InstallChannel::Yarn => Some(("yarn", &["global", "add", "srcwalk@latest"])),
        InstallChannel::Bun => Some(("bun", &["add", "-g", "srcwalk@latest"])),
        InstallChannel::Standalone => None,
    }
}

const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_VERSION_CHECK_BYTES: usize = 4 * 1024;

/// npm/pnpm/yarn/bun ship their global-install entry points as `.cmd` shims
/// on Windows, which `CreateProcess` cannot execute directly (only
/// `cmd.exe` resolves and runs `.cmd`/`.bat` files). This is the one
/// dedicated, fixed-argv helper permitted to go through `cmd.exe /C`: every
/// argument is still passed structurally via `.arg()`/`.args()`, never
/// concatenated into a shell string, so nothing here interpolates
/// user-controlled or network-derived text into a shell command line.
#[cfg(windows)]
fn spawn_package_manager(
    program: &str,
    args: &[&str],
    path_override: Option<&std::ffi::OsStr>,
) -> process::Command {
    let mut cmd = crate::version::command_with_path("cmd", path_override);
    cmd.arg("/C").arg(program).args(args);
    cmd
}

#[cfg(not(windows))]
fn spawn_package_manager(
    program: &str,
    args: &[&str],
    path_override: Option<&std::ffi::OsStr>,
) -> process::Command {
    let mut cmd = crate::version::command_with_path(program, path_override);
    cmd.args(args);
    cmd
}

/// Prints the exact package-manager command, executes it with inherited
/// stdio (prompts/errors stay visible), then spawns `verify_exe --version`
/// with bounded output and a finite timeout, requiring it to report exactly
/// `expected_version` before claiming success. `path_override` replaces the
/// child's `PATH` (tests only).
pub(crate) fn run_package_manager_update(
    channel: InstallChannel,
    expected_version: &str,
    verify_exe: &Path,
    path_override: Option<&OsStr>,
) -> Result<(), String> {
    let (program, args) = package_manager_argv(channel)
        .ok_or_else(|| "no package-manager command for a standalone install".to_string())?;

    println!("{program} {}", args.join(" "));

    let status = spawn_package_manager(program, args, path_override)
        .status()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if !status.success() {
        return Err(format!("{program} {} exited with {status}", args.join(" ")));
    }

    let reported = spawn_reported_version(verify_exe)?;
    if reported != expected_version {
        return Err(format!(
            "installed version reports {reported}, expected {expected_version}"
        ));
    }
    Ok(())
}

/// Spawns `path --version` with bounded output and a finite timeout, and
/// parses the reported `X.Y.Z`. Shared by the package-manager post-check
/// above and the standalone transaction post-check (the `verify` closure
/// passed to `unix_transactional_swap`/`windows_transactional_swap`), so
/// both post-checks apply the exact same bound/parse contract.
fn spawn_reported_version(path: &Path) -> Result<String, String> {
    let mut cmd = process::Command::new(path);
    cmd.arg("--version");
    let output = crate::version::run_bounded(cmd, MAX_VERSION_CHECK_BYTES, VERIFY_TIMEOUT)
        .map_err(|e| format!("could not verify installed version: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "version check exited with {}{}",
            output.status,
            crate::version::stderr_detail(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_reported_version(&stdout)
        .ok_or_else(|| format!("could not parse reported version from `{stdout}`"))
}

/// Extracts `X.Y.Z` from the canonical `srcwalk X.Y.Z (...)` first line.
fn parse_reported_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?;
    let rest = line.strip_prefix("srcwalk ")?;
    let version = rest.split_whitespace().next()?;
    Some(version.to_string())
}

/// Mirrors `npm/install.js`'s `parseChecksum`: exactly one non-empty line,
/// 64 hex characters, optional `*` prefix, and a basename exactly equal to
/// `expected_filename`.
pub(crate) fn parse_checksum(text: &str, expected_filename: &str) -> Result<String, String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() != 1 {
        return Err("checksum file must contain exactly one non-empty line".to_string());
    }
    let (hash, rest) = lines[0]
        .split_once(char::is_whitespace)
        .ok_or_else(|| "checksum file has an invalid SHA-256 format".to_string())?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("checksum file has an invalid SHA-256 format".to_string());
    }
    let rest = rest
        .trim_start()
        .strip_prefix('*')
        .unwrap_or(rest.trim_start());
    let listed_filename = rest.trim().rsplit(['/', '\\']).next().unwrap_or("");
    if listed_filename != expected_filename {
        return Err(format!(
            "checksum names {listed_filename}, expected {expected_filename}"
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

/// Mirrors `npm/install.js`'s `validateArchiveEntries`: exactly one listed
/// entry, normalized to forward slashes with any leading `./` stripped, must
/// equal `expected_binary` exactly — rejecting extra entries, absolute paths,
/// and traversal by construction (they never equal the plain binary name).
pub(crate) fn validate_archive_entries(listing: &str, expected_binary: &str) -> Result<(), String> {
    let entries: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if entries.len() != 1 {
        return Err(format!(
            "archive must contain exactly one entry, found {}",
            entries.len()
        ));
    }
    let mut normalized = entries[0].replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    if normalized != expected_binary {
        return Err(format!(
            "archive entry {} does not match expected binary {expected_binary}",
            entries[0]
        ));
    }
    Ok(())
}

/// Post-extract type check: the tar listing text alone never proves the
/// extracted entry's real filesystem type. Uses `lstat` semantics (does not
/// follow symlinks) and rejects anything but a regular file.
pub(crate) fn validate_extracted_binary(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("could not stat extracted binary {}: {e}", path.display()))?;
    if metadata.is_symlink() {
        return Err(format!(
            "extracted entry {} is a symlink, not a regular file",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "extracted entry {} is not a regular file",
            path.display()
        ));
    }
    Ok(())
}

fn run_downloader(
    program: &str,
    args: &[String],
    max_bytes: usize,
    timeout: Duration,
    path_override: Option<&OsStr>,
) -> Result<Vec<u8>, String> {
    let mut cmd = crate::version::command_with_path(program, path_override);
    cmd.args(args);
    let output = crate::version::run_bounded(cmd, max_bytes, timeout)
        .map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program}: exited with {}{}",
            output.status,
            crate::version::stderr_detail(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

fn curl_download_args(url: &str, max_bytes: usize, timeout_secs: &str) -> Vec<String> {
    vec![
        "-fsSL".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--max-time".to_string(),
        timeout_secs.to_string(),
        format!("--max-filesize={max_bytes}"),
        url.to_string(),
    ]
}

fn wget_download_args(url: &str, timeout_secs: &str) -> Vec<String> {
    vec![
        "-qO-".to_string(),
        "--https-only".to_string(),
        format!("--timeout={timeout_secs}"),
        url.to_string(),
    ]
}

fn fetch_bytes_via_curl_or_wget(
    url: &str,
    max_bytes: usize,
    timeout: Duration,
    timeout_secs: &str,
    path_override: Option<&OsStr>,
) -> Result<Vec<u8>, String> {
    let curl_args = curl_download_args(url, max_bytes, timeout_secs);
    let wget_args = wget_download_args(url, timeout_secs);

    match run_downloader("curl", &curl_args, max_bytes, timeout, path_override) {
        Ok(bytes) => Ok(bytes),
        Err(curl_err) => {
            match run_downloader("wget", &wget_args, max_bytes, timeout, path_override) {
                Ok(bytes) => Ok(bytes),
                Err(wget_err) => Err(format!("curl: {curl_err}; wget: {wget_err}")),
            }
        }
    }
}

/// Downloads small text (e.g. a `.sha256` file) via system curl, falling back
/// to wget, HTTPS-only with finite timeout and bounded output. Zero network
/// in tests: `path_override` replaces the child's `PATH`.
pub(crate) fn download_text(
    url: &str,
    max_bytes: usize,
    path_override: Option<&OsStr>,
) -> Result<String, String> {
    let bytes =
        fetch_bytes_via_curl_or_wget(url, max_bytes, Duration::from_secs(20), "20", path_override)?;
    // `run_bounded` already enforces `max_bytes`; this is a defensive
    // invariant check, not a second cap.
    debug_assert!(bytes.len() <= max_bytes);
    String::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 response: {e}"))
}

/// Downloads a release archive via curl/wget and writes it to `destination`
/// with create-new semantics: an existing destination is refused, never
/// overwritten. Nothing is written until the byte cap is already satisfied.
/// Returns the archive's lowercase-hex SHA-256, computed from the same
/// in-memory bytes that get written to disk (no second read pass).
pub(crate) fn download_archive(
    url: &str,
    destination: &Path,
    max_bytes: usize,
    path_override: Option<&OsStr>,
) -> Result<String, String> {
    let bytes =
        fetch_bytes_via_curl_or_wget(url, max_bytes, Duration::from_secs(60), "60", path_override)?;
    debug_assert!(bytes.len() <= max_bytes);

    let mut file = reserve_create_new(destination)?;
    if let Err(e) = file.write_all(&bytes) {
        drop(file);
        let _ = std::fs::remove_file(destination);
        return Err(format!("could not write archive: {e}"));
    }
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Creates `path` with `O_EXCL`/`CREATE_NEW` semantics: fails honestly if
/// anything — a regular file or an attacker-planted symlink — already sits at
/// that path, instead of following or overwriting it.
fn reserve_create_new(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("could not create {}: {e}", path.display()))
}

/// Two same-directory paths reserved for a transactional swap. Names only:
/// on Unix `backup` is created immediately (holds a byte-for-byte copy of
/// the current executable); on Windows `.old` must stay absent so the first
/// move step can claim it, so its path is planned but never pre-created.
pub(crate) struct StagingPaths {
    pub(crate) staged: PathBuf,
    pub(crate) backup: PathBuf,
}

/// Runs `create` then requires `remove` to also succeed — a write-probe that
/// could be created but not removed is not proof of usable same-directory
/// write capability, so the caller must never treat it as clean. Closures so
/// the "create succeeded, remove failed" branch is provable without forcing
/// that split on a real filesystem.
fn require_probe_cleanup(
    create: impl FnOnce() -> Result<(), String>,
    remove: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    create()?;
    remove()
}

/// Shared preflight: the current target must be an existing regular file —
/// checked with `symlink_metadata` (lstat) so a symlink at `current` is
/// rejected outright, never followed — and the same directory must accept a
/// create/write/remove probe before any staging name is trusted.
/// Staged/backup names are collision-planned (not yet created) next to the
/// current executable.
pub(crate) fn preflight_same_dir(current: &Path) -> Result<StagingPaths, String> {
    let metadata = std::fs::symlink_metadata(current).map_err(|e| {
        format!(
            "could not stat current executable {}: {e}",
            current.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "current executable target {} is not a regular file",
            current.display()
        ));
    }

    let dir = current
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", current.display()))?;
    let file_name = current
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has a non-UTF-8 file name", current.display()))?;

    let probe = dir.join(format!(
        ".{file_name}.srcwalk-update-probe-{}",
        process::id()
    ));
    require_probe_cleanup(
        || {
            reserve_create_new(&probe)
                .map(|_| ())
                .map_err(|e| format!("cannot write next to {}: {e}", current.display()))
        },
        || {
            std::fs::remove_file(&probe).map_err(|e| {
                format!(
                    "created a write probe next to {} but could not remove it ({}): {e}",
                    current.display(),
                    probe.display()
                )
            })
        },
    )?;

    let unique = format!(
        "{}-{}",
        process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let staged = dir.join(format!("{file_name}.srcwalk-update-{unique}"));
    let backup = dir.join(format!("{file_name}.srcwalk-update-{unique}.bak"));

    Ok(StagingPaths { staged, backup })
}

/// create_new the backup path (never overwrite/follow an existing file or
/// attacker symlink there), then copy the current executable's bytes and
/// permissions into it. No rename of the original: `current` stays in place
/// until the caller's own placement step.
#[cfg(unix)]
fn unix_backup_current(current: &Path, backup: &Path) -> Result<(), String> {
    unix_backup_current_with(current, backup, |p: &Path| std::fs::remove_file(p))
}

/// Every failure after `reserve_create_new` leaves a create_new-reserved
/// backup file (empty or partially written) sitting in the install
/// directory; any error from the inner write step must trigger cleanup
/// here, not just a copy failure, and a cleanup failure is folded into the
/// reported error too, never silently swallowed by a bare
/// `let _ = remove_file(...)`. `cleanup_remove` is injectable so the
/// "backup was created, then the write step failed, and the backup's own
/// removal then also fails" branch is testable: create and remove of the
/// same path are gated by the same directory permission bit, so the two
/// cannot be decoupled purely via chmod from outside this function (same
/// rationale as `windows_move_non_overwrite_with`).
#[cfg(unix)]
fn unix_backup_current_with(
    current: &Path,
    backup: &Path,
    cleanup_remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let backup_file = reserve_create_new(backup)?;
    match unix_backup_current_write(current, backup, backup_file) {
        Ok(()) => Ok(()),
        Err(e) => Err(fold_cleanup_failure_with(
            e,
            backup,
            "backup",
            cleanup_remove,
        )),
    }
}

#[cfg(unix)]
fn unix_backup_current_write(
    current: &Path,
    backup: &Path,
    mut backup_file: std::fs::File,
) -> Result<(), String> {
    let mut source = std::fs::File::open(current).map_err(|e| {
        format!(
            "could not read current executable {}: {e}",
            current.display()
        )
    })?;
    // Streamed, not buffered whole: avoids holding the entire executable in
    // memory just to copy it next to itself.
    std::io::copy(&mut source, &mut backup_file)
        .map_err(|e| format!("could not write backup {}: {e}", backup.display()))?;
    let perms = std::fs::metadata(current)
        .map_err(|e| {
            format!(
                "could not stat current executable {}: {e}",
                current.display()
            )
        })?
        .permissions();
    std::fs::set_permissions(backup, perms)
        .map_err(|e| format!("could not set backup permissions: {e}"))?;
    Ok(())
}

#[cfg(unix)]
fn unix_rollback(backup: &Path, current: &Path) -> Result<(), String> {
    std::fs::rename(backup, current).map_err(|e| format!("rollback failed: {e}"))
}

/// The placement-failure cleanup step in isolation: `rename(staged,
/// current)` failed, so the create_new-reserved `backup` created just
/// before it is now redundant and must be removed. `cleanup_remove` is
/// injectable so the "backup exists, but its own removal then also fails"
/// branch is testable without relying on a real filesystem race (create
/// and remove of the same path are gated by the same directory permission
/// bit; see `unix_backup_current_with` for the same rationale).
#[cfg(unix)]
fn placement_failed(
    place_err: String,
    backup: &Path,
    cleanup_remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> String {
    let primary = format!("could not place updated binary: {place_err}");
    fold_cleanup_failure_with(primary, backup, "backup", cleanup_remove)
}

/// The successful-placement-and-verify cleanup step in isolation. Once
/// `current` is placed and verified, the swap itself is already correct
/// and complete: a failure to then remove the now-redundant `backup` must
/// never roll back an already-verified-good binary just to "clean up" —
/// it is reported honestly instead (`Ok` is never returned on a removal
/// failure), so the caller never prints a clean `Updated OLD -> NEW.`
/// while a stray backup still sits next to the real executable.
/// `cleanup_remove` is injectable for the same reason as `placement_failed`.
#[cfg(unix)]
fn verified_swap_cleanup(
    backup: &Path,
    cleanup_remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    match cleanup_remove(backup) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "updated and verified, but could not remove backup {}: {e}",
            backup.display()
        )),
    }
}

/// A same-directory staged/backup file that a swap never consumed (creation
/// or placement never finished, or had to be rolled back) is a leftover
/// artifact next to the real executable, not a harmless temp file — every
/// failure path that could leave one behind routes its error through this,
/// so cleanup is never skipped for one branch and not another, and a
/// cleanup failure is never silently swallowed by a bare
/// `let _ = remove_file(...)`. `NotFound` means `path` was already consumed
/// (e.g. moved into `current`'s place, or never created), so it is not
/// itself folded into the reported error; any other cleanup failure is.
fn fold_cleanup_failure(primary: String, path: &Path, label: &str) -> String {
    fold_cleanup_failure_with(primary, path, label, |p: &Path| std::fs::remove_file(p))
}

/// Injectable core of [`fold_cleanup_failure`], so a genuine cleanup-removal
/// failure is testable without relying on a real filesystem race.
fn fold_cleanup_failure_with(
    primary: String,
    path: &Path,
    label: &str,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> String {
    match remove(path) {
        Ok(()) => primary,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => primary,
        Err(e) => format!(
            "{primary}; also could not remove leftover {label} {}: {e}",
            path.display()
        ),
    }
}

/// Unix transactional swap: create_new-backup the current bytes, atomically
/// rename the already-verified `staged` binary over `current`, then require
/// `verify(current)` to report `expected_version` before deleting the
/// backup. Any placement/post-check failure restores the backup; a failed
/// rollback never silently discards the primary failure, the returned
/// error always names both. Every failure also folds in a `staged`-cleanup
/// attempt via [`fold_cleanup_failure`]. Once `current` is placed and
/// verified, the swap itself is already correct and complete: a failure to
/// then remove the now-redundant `backup` is reported honestly (`Ok` is
/// never returned) instead of rolling back a proven-good binary just to
/// "clean up" — the caller must not print a clean success while a stray
/// backup still sits next to the real executable.
#[cfg(unix)]
pub(crate) fn unix_transactional_swap(
    current: &Path,
    staged: &Path,
    backup: &Path,
    expected_version: &str,
    verify: impl Fn(&Path) -> Result<String, String>,
) -> Result<(), String> {
    unix_transactional_swap_inner(current, staged, backup, expected_version, verify)
        .map_err(|e| fold_cleanup_failure(e, staged, "staged binary"))
}

#[cfg(unix)]
fn unix_transactional_swap_inner(
    current: &Path,
    staged: &Path,
    backup: &Path,
    expected_version: &str,
    verify: impl Fn(&Path) -> Result<String, String>,
) -> Result<(), String> {
    unix_backup_current(current, backup)?;

    if let Err(e) = std::fs::rename(staged, current) {
        return Err(placement_failed(e.to_string(), backup, |p: &Path| {
            std::fs::remove_file(p)
        }));
    }

    let primary = match verify(current) {
        // The swap itself is already done and proven correct here: `current`
        // now reports `expected_version`. See `verified_swap_cleanup` for
        // why a failure to remove `backup` from this point on is never
        // treated as a rollback trigger.
        Ok(reported) if reported == expected_version => {
            return verified_swap_cleanup(backup, |p: &Path| std::fs::remove_file(p));
        }
        Ok(reported) => format!("post-check reported {reported}, expected {expected_version}"),
        Err(e) => format!("post-check failed: {e}"),
    };
    match unix_rollback(backup, current) {
        Ok(()) => Err(primary),
        Err(rollback_err) => Err(format!("{primary}; rollback also failed: {rollback_err}")),
    }
}

/// Same-directory non-overwrite move: hard-link then remove the source.
/// Unlike `fs::rename` on Windows (which passes `MOVEFILE_REPLACE_EXISTING`),
/// this fails honestly if `destination` is already occupied — by a stale
/// leftover or a race — instead of silently overwriting or following it.
///
/// This is a genuine *move*, not "link with an optional leftover source": if
/// the source cannot be removed after linking, the just-created destination
/// link is cleaned up so the operation never claims a state it cannot prove
/// (a lingering source would make the caller's transaction/cleanup
/// non-deterministic). If that cleanup itself fails, both failures are
/// reported.
// Only reachable in production via the `#[cfg(windows)]` wrapper below; on a
// non-Windows production build it is otherwise dead. Kept unguarded (rather
// than `#[cfg(windows)]` itself) so its fault-injection tests can run on any
// host, per review — forcing a genuine post-hard_link remove failure isn't
// reliably reproducible on a real filesystem.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn windows_move_non_overwrite_with(
    source: &Path,
    destination: &Path,
    link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    remove_source: impl FnOnce(&Path) -> std::io::Result<()>,
    remove_destination: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    link(source, destination).map_err(|e| {
        format!(
            "could not link {} to {}: {e}",
            source.display(),
            destination.display()
        )
    })?;
    if let Err(remove_err) = remove_source(source) {
        return match remove_destination(destination) {
            Ok(()) => Err(format!(
                "could not remove {} after linking to {}: {remove_err}",
                source.display(),
                destination.display()
            )),
            Err(cleanup_err) => Err(format!(
                "could not remove {} after linking to {} ({remove_err}), and cleanup of {} also failed: {cleanup_err}",
                source.display(),
                destination.display(),
                destination.display()
            )),
        };
    }
    Ok(())
}

#[cfg(windows)]
fn windows_move_non_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
    windows_move_non_overwrite_with(
        source,
        destination,
        std::fs::hard_link,
        std::fs::remove_file,
        std::fs::remove_file,
    )
}

/// Restores `.old` back onto `current` after a failed second move or a
/// failed post-check. `current`'s prior state is already fully owned by this
/// transaction here, so an overwrite is the intended recovery, not an
/// attacker surface.
#[cfg(windows)]
fn windows_restore(old: &Path, current: &Path) -> Result<(), String> {
    std::fs::rename(old, current).map_err(|e| format!("rollback failed: {e}"))
}

/// Best-effort removal of a known stale `.old` artifact left by a previous
/// successful Windows update that Windows was still holding open. Never
/// fails an otherwise successful update solely because this is deferred.
#[cfg(windows)]
pub(crate) fn cleanup_stale_old(old: &Path) {
    let _ = std::fs::remove_file(old);
}

/// Windows transactional swap. `.old` must stay absent going in, so the
/// first move is non-overwrite by construction (see
/// [`windows_move_non_overwrite`]); a race that occupies it fails the update
/// honestly before `current` ever leaves its place. On a second-move or
/// post-check failure, `.old` is restored immediately; a failed rollback
/// never silently discards the primary failure, the returned error always
/// names both.
#[cfg(windows)]
pub(crate) fn windows_transactional_swap(
    current: &Path,
    staged: &Path,
    old: &Path,
    expected_version: &str,
    verify: impl Fn(&Path) -> Result<String, String>,
) -> Result<(), String> {
    windows_transactional_swap_inner(current, staged, old, expected_version, verify)
        .map_err(|e| fold_cleanup_failure(e, staged, "staged binary"))
}

#[cfg(windows)]
fn windows_transactional_swap_inner(
    current: &Path,
    staged: &Path,
    old: &Path,
    expected_version: &str,
    verify: impl Fn(&Path) -> Result<String, String>,
) -> Result<(), String> {
    windows_move_non_overwrite(current, old).map_err(|e| {
        format!(
            "could not rename running executable to {}: {e}",
            old.display()
        )
    })?;

    if let Err(e) = windows_move_non_overwrite(staged, current) {
        return match windows_restore(old, current) {
            Ok(()) => Err(format!("could not place updated binary: {e}")),
            Err(restore_err) => Err(format!(
                "could not place updated binary ({e}), and rollback to .old also failed ({restore_err})"
            )),
        };
    }

    let primary = match verify(current) {
        Ok(reported) if reported == expected_version => {
            cleanup_stale_old(old);
            return Ok(());
        }
        Ok(reported) => format!("post-check reported {reported}, expected {expected_version}"),
        Err(e) => format!("post-check failed: {e}"),
    };
    match windows_restore(old, current) {
        Ok(()) => Err(primary),
        Err(rollback_err) => Err(format!("{primary}; rollback also failed: {rollback_err}")),
    }
}

const MAX_CHECKSUM_BYTES: usize = 4 * 1024;
const MAX_ARCHIVE_BYTES: usize = 100 * 1024 * 1024;
const MAX_TAR_LISTING_BYTES: usize = 4 * 1024;
const TAR_TIMEOUT: Duration = Duration::from_secs(30);

const GITHUB_RELEASE_BASE: &str = "https://github.com/sting8k/srcwalk/releases/download";

/// Top-level orchestration for `srcwalk update`/`srcwalk update --check`.
/// Prints the current version, resolves the shared decision, and for
/// `Equal`/`LocalNewer` stops there (identical text for both `--check` and a
/// full run). Only `Newer` diverges: `--check` prints the shared
/// check-only render; a full run detects the install channel and performs
/// the channel action, printing `Updated OLD -> NEW.` only after the exact
/// installed binary is verified to report the resolved latest version.
pub(crate) fn run_update(check: bool) {
    println!("{}", crate::version::version_line());

    let decision = match crate::version::resolve_update_decision(None) {
        Ok(decision) => decision,
        Err(err) => {
            crate::version::print_resolution_failure(&err);
            process::exit(1);
        }
    };

    let (current, latest) = match &decision {
        crate::version::UpdateDecision::Equal { .. }
        | crate::version::UpdateDecision::LocalNewer { .. } => {
            crate::version::print_check_result(&decision);
            return;
        }
        crate::version::UpdateDecision::Newer { current, latest } => {
            (current.clone(), latest.clone())
        }
    };

    if check {
        crate::version::print_check_result(&decision);
        return;
    }
    println!("latest {latest}");

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: could not resolve current executable path: {e}");
            eprintln!();
            eprintln!("cargo install srcwalk --locked --force");
            process::exit(1);
        }
    };
    let channel = detect_channel(&current_exe);

    let result = match channel {
        InstallChannel::Standalone => standalone_update(&latest, &current_exe),
        _ => run_package_manager_update(channel, &latest, &current_exe, None),
    };

    match result {
        Ok(()) => println!("Updated {current} -> {latest}."),
        Err(err) => {
            eprintln!("error: {err}");
            print_channel_fallback(channel);
            process::exit(1);
        }
    }
}

/// One channel-appropriate manual command, plus the generic Cargo fallback
/// always offered on failure.
fn print_channel_fallback(channel: InstallChannel) {
    eprintln!();
    match channel {
        InstallChannel::Standalone => {
            if let Some(target) = crate::version::release_target() {
                eprintln!(
                    "Manual fallback: curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-{}.tar.gz | tar xz -C ~/.local/bin",
                    target.triple
                );
            }
        }
        _ => {
            if let Some((program, args)) = package_manager_argv(channel) {
                eprintln!("Manual fallback: {program} {}", args.join(" "));
            }
        }
    }
    eprintln!("cargo install srcwalk --locked --force");
}

/// Creates a fresh, create-new (never-existed) directory under the system
/// temp root, private on Unix (`0700`), for extracting/staging the
/// downloaded archive away from the installed executable's directory. On
/// Unix the mode is applied atomically by the underlying `mkdir(..., 0700)`
/// syscall itself — not by a separate post-create `chmod` — so there is no
/// window where the directory briefly exists with broader permissions
/// (0700 has no group/other bits for a permissive umask to strip anyway).
/// `create_dir`/`DirBuilder::create` never leaves a partial directory behind
/// on failure: the syscall either creates it or it doesn't.
fn create_private_temp_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    let unique = format!(
        "srcwalk-update-{}-{}",
        process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = base.join(unique);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("could not create temp dir {}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(&dir)
            .map_err(|e| format!("could not create temp dir {}: {e}", dir.display()))?;
    }
    Ok(dir)
}

/// Runs system `tar` with bounded, timed-out output, relative to `cwd`.
/// Relative operands (never an absolute archive path) avoid drive-letter
/// paths being parsed as remote archives by GNU tar on Windows.
fn run_tar(args: &[&str], cwd: &Path) -> Result<String, String> {
    let mut cmd = process::Command::new("tar");
    cmd.args(args).current_dir(cwd);
    let output = crate::version::run_bounded(cmd, MAX_TAR_LISTING_BYTES, TAR_TIMEOUT)
        .map_err(|e| format!("tar: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "tar {} failed: exited with {}{}",
            args.first().copied().unwrap_or(""),
            output.status,
            crate::version::stderr_detail(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("invalid UTF-8 from tar: {e}"))
}

/// Verified standalone replacement: resolve the release asset for this
/// target, download + verify the checksum text, download the archive and
/// compare its real SHA-256, list then extract with exactly one validated
/// entry, stage it next to the current executable, and hand off to the
/// platform transactional swap. Every failure before the swap step leaves
/// the installed executable untouched; the temp directory is always
/// best-effort removed on every path.
fn standalone_update(latest: &str, current_exe: &Path) -> Result<(), String> {
    let temp_root = create_private_temp_dir()?;
    let result = standalone_update_in(latest, current_exe, &temp_root);
    let _ = std::fs::remove_dir_all(&temp_root);
    result
}

fn standalone_update_in(latest: &str, current_exe: &Path, temp_root: &Path) -> Result<(), String> {
    let target = crate::version::release_target()
        .ok_or_else(|| "unsupported OS/architecture for a standalone update".to_string())?;

    // Preflight before any large download: prove same-directory write
    // capability and reserve collision-safe staging names first.
    let staging = preflight_same_dir(current_exe)?;

    let archive_name = format!("srcwalk-{}.tar.gz", target.triple);
    let archive_url = format!("{GITHUB_RELEASE_BASE}/v{latest}/{archive_name}");
    let checksum_url = format!("{archive_url}.sha256");

    let checksum_text = download_text(&checksum_url, MAX_CHECKSUM_BYTES, None)?;
    let expected_sha256 = parse_checksum(&checksum_text, &archive_name)?;

    let archive_path = temp_root.join(&archive_name);
    let actual_sha256 = download_archive(&archive_url, &archive_path, MAX_ARCHIVE_BYTES, None)?;
    if actual_sha256 != expected_sha256 {
        return Err(format!("SHA-256 mismatch for {archive_name}"));
    }

    let listing = run_tar(&["tzf", &archive_name], temp_root)?;
    validate_archive_entries(&listing, target.binary)?;

    let extract_dir = temp_root.join("staging");
    std::fs::create_dir(&extract_dir).map_err(|e| format!("could not create staging dir: {e}"))?;
    run_tar(&["xzf", &archive_name, "-C", "staging"], temp_root)?;

    let extracted_binary = extract_dir.join(target.binary);
    validate_extracted_binary(&extracted_binary)?;

    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&extracted_binary)
            .map_err(|e| format!("could not stat extracted binary: {e}"))?
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&extracted_binary, perms)
            .map_err(|e| format!("could not set extracted binary permissions: {e}"))?;
    }

    stage_same_dir(&extracted_binary, &staging.staged)?;

    #[cfg(unix)]
    {
        unix_transactional_swap(
            current_exe,
            &staging.staged,
            &staging.backup,
            latest,
            spawn_reported_version,
        )
    }
    #[cfg(windows)]
    {
        windows_transactional_swap(
            current_exe,
            &staging.staged,
            &staging.backup,
            latest,
            spawn_reported_version,
        )
    }
}

/// Copies the validated extracted binary into its create-new same-directory
/// staged path, preserving executable permissions on Unix.
fn stage_same_dir(extracted_binary: &Path, staged: &Path) -> Result<(), String> {
    stage_same_dir_with(extracted_binary, staged, |p: &Path| std::fs::remove_file(p))
}

/// Any failure below — open, copy, stat, or chmod — leaves a
/// create_new-reserved staged file (empty or partially written) sitting
/// next to the real executable, so every error path here must clean it up,
/// not just the copy failure, and a cleanup failure is folded into the
/// reported error too, never silently swallowed by a bare
/// `let _ = remove_file(...)`. `cleanup_remove` is injectable for the same
/// reason as `unix_backup_current_with`.
fn stage_same_dir_with(
    extracted_binary: &Path,
    staged: &Path,
    cleanup_remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let dest = reserve_create_new(staged)?;
    match stage_same_dir_write(extracted_binary, staged, dest) {
        Ok(()) => Ok(()),
        Err(e) => Err(fold_cleanup_failure_with(
            e,
            staged,
            "staged binary",
            cleanup_remove,
        )),
    }
}

fn stage_same_dir_write(
    extracted_binary: &Path,
    staged: &Path,
    mut dest: std::fs::File,
) -> Result<(), String> {
    let mut src = std::fs::File::open(extracted_binary)
        .map_err(|e| format!("could not open extracted binary: {e}"))?;
    std::io::copy(&mut src, &mut dest)
        .map_err(|e| format!("could not stage updated binary: {e}"))?;
    #[cfg(unix)]
    {
        drop(dest);
        let perms = std::fs::metadata(extracted_binary)
            .map_err(|e| format!("could not stat extracted binary: {e}"))?
            .permissions();
        std::fs::set_permissions(staged, perms)
            .map_err(|e| format!("could not set staged binary permissions: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_fake_tool(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    // ---- RED 3: channel detection (pure, no I/O). Every test below pins an
    // explicit `windows` matching mode via `detect_channel_for` so the
    // asserted semantics never depend on which host CI runs the suite. ----

    #[test]
    fn detect_channel_for_standalone_when_no_node_modules_component() {
        assert_eq!(
            detect_channel_for(Path::new("/usr/local/bin/srcwalk"), false),
            InstallChannel::Standalone
        );
        assert_eq!(
            detect_channel_for(Path::new("/Users/dev/.cargo/bin/srcwalk"), false),
            InstallChannel::Standalone
        );
    }

    #[test]
    fn detect_channel_for_npm_under_generic_node_modules() {
        assert_eq!(
            detect_channel_for(
                Path::new("/usr/lib/node_modules/srcwalk/bin/srcwalk"),
                false
            ),
            InstallChannel::Npm
        );
    }

    #[test]
    fn detect_channel_for_pnpm_marker() {
        assert_eq!(
            detect_channel_for(
                Path::new(
                    "/home/dev/.local/share/pnpm/global/5/node_modules/.pnpm/srcwalk@1.8.0/node_modules/srcwalk/bin/srcwalk"
                ),
                false
            ),
            InstallChannel::Pnpm
        );
    }

    #[test]
    fn detect_channel_for_bun_marker() {
        assert_eq!(
            detect_channel_for(
                Path::new("/home/dev/.bun/install/global/node_modules/srcwalk/bin/srcwalk"),
                false
            ),
            InstallChannel::Bun
        );
    }

    #[test]
    fn detect_channel_for_yarn_marker() {
        assert_eq!(
            detect_channel_for(
                Path::new("/home/dev/.yarn/berry/global/node_modules/srcwalk/bin/srcwalk"),
                false
            ),
            InstallChannel::Yarn
        );
    }

    #[test]
    fn detect_channel_for_bare_yarn_directory_name_is_not_a_marker() {
        // A plain `yarn` component (no leading dot) is just a project/directory
        // name, not a Yarn-global marker — this must route to Npm, matching an
        // ordinary npm install that merely happens to live under a `yarn/` dir.
        assert_eq!(
            detect_channel_for(
                Path::new("/home/dev/yarn/node_modules/srcwalk/bin/srcwalk"),
                false
            ),
            InstallChannel::Npm
        );
    }

    #[test]
    fn detect_channel_for_windows_mode_is_case_insensitive_and_accepts_backslashes() {
        assert_eq!(
            detect_channel_for(
                Path::new(r"C:\Users\dev\AppData\Roaming\NPM\Node_Modules\srcwalk\bin\srcwalk.exe"),
                true
            ),
            InstallChannel::Npm
        );
        assert_eq!(
            detect_channel_for(
                Path::new(
                    r"C:\Users\dev\AppData\Local\Yarn\.YARN\global\node_modules\srcwalk\bin\srcwalk.exe"
                ),
                true
            ),
            InstallChannel::Yarn
        );
    }

    #[test]
    fn detect_channel_for_unix_mode_is_case_sensitive() {
        // Unix matching is component-exact: an uppercase `NODE_MODULES` must
        // never match the lowercase marker, regardless of which host CI runs
        // this test.
        assert_eq!(
            detect_channel_for(
                Path::new("/usr/lib/NODE_MODULES/srcwalk/bin/srcwalk"),
                false
            ),
            InstallChannel::Standalone
        );
    }

    #[test]
    fn detect_channel_for_ignores_misleading_substrings() {
        // "my_node_modules_backup" is a decoy component, not the exact `node_modules`
        // marker; component-exact matching must not treat it as a substring hit.
        assert_eq!(
            detect_channel_for(Path::new("/home/dev/my_node_modules_backup/srcwalk"), false),
            InstallChannel::Standalone
        );
    }

    #[test]
    fn detect_channel_wrapper_delegates_to_detect_channel_for() {
        // Standalone has no OS-conditional branch to exercise; this just
        // proves the thin runtime wrapper is actually wired up.
        assert_eq!(
            detect_channel(Path::new("/usr/local/bin/srcwalk")),
            InstallChannel::Standalone
        );
    }

    // ---- RED 4: package-manager argv table ----

    #[test]
    fn package_manager_argv_matches_spec_table() {
        assert_eq!(
            package_manager_argv(InstallChannel::Npm),
            Some(("npm", ["install", "-g", "srcwalk@latest"].as_slice()))
        );
        assert_eq!(
            package_manager_argv(InstallChannel::Pnpm),
            Some(("pnpm", ["add", "-g", "srcwalk@latest"].as_slice()))
        );
        assert_eq!(
            package_manager_argv(InstallChannel::Yarn),
            Some(("yarn", ["global", "add", "srcwalk@latest"].as_slice()))
        );
        assert_eq!(
            package_manager_argv(InstallChannel::Bun),
            Some(("bun", ["add", "-g", "srcwalk@latest"].as_slice()))
        );
        assert_eq!(package_manager_argv(InstallChannel::Standalone), None);
    }

    #[cfg(unix)]
    #[test]
    fn run_package_manager_update_succeeds_and_verifies_reported_version() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "npm", "exit 0");
        let verify_exe = dir.path().join("fake-srcwalk");
        std::fs::write(
            &verify_exe,
            "#!/bin/sh\necho 'srcwalk 1.9.0 (abc1234, 2026-01-01)'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&verify_exe).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&verify_exe, perms).unwrap();

        let result = run_package_manager_update(
            InstallChannel::Npm,
            "1.9.0",
            &verify_exe,
            Some(dir.path().as_os_str()),
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn run_package_manager_update_fails_on_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "npm", "exit 1");
        let verify_exe = dir.path().join("unused");
        let err = run_package_manager_update(
            InstallChannel::Npm,
            "1.9.0",
            &verify_exe,
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();
        assert!(err.contains("npm"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_package_manager_update_fails_when_reported_version_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "npm", "exit 0");
        let verify_exe = dir.path().join("fake-srcwalk");
        std::fs::write(
            &verify_exe,
            "#!/bin/sh\necho 'srcwalk 1.8.0 (abc1234, 2026-01-01)'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&verify_exe).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&verify_exe, perms).unwrap();

        let err = run_package_manager_update(
            InstallChannel::Npm,
            "1.9.0",
            &verify_exe,
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();
        assert!(err.contains("1.9.0") && err.contains("1.8.0"), "{err}");
    }

    // ---- RED 6: checksum / archive-listing helpers (pure) ----

    #[test]
    fn parse_checksum_accepts_exact_one_line_hex_filename() {
        let hash = "a".repeat(64);
        let text = format!("{hash} srcwalk-x86_64-unknown-linux-musl.tar.gz\n");
        assert_eq!(
            parse_checksum(&text, "srcwalk-x86_64-unknown-linux-musl.tar.gz").unwrap(),
            hash
        );
    }

    #[test]
    fn parse_checksum_accepts_uppercase_and_star_prefix() {
        let hash = "B".repeat(64);
        let text = format!("{hash} *archive.tar.gz\n");
        assert_eq!(
            parse_checksum(&text, "archive.tar.gz").unwrap(),
            hash.to_lowercase()
        );
    }

    #[test]
    fn parse_checksum_rejects_wrong_filename() {
        let hash = "a".repeat(64);
        let text = format!("{hash} other.tar.gz\n");
        assert!(parse_checksum(&text, "archive.tar.gz").is_err());
    }

    #[test]
    fn parse_checksum_rejects_multi_line_or_malformed_hex() {
        let hash = "a".repeat(64);
        assert!(
            parse_checksum(&format!("{hash} a.tar.gz\n{hash} a.tar.gz\n"), "a.tar.gz").is_err()
        );
        assert!(parse_checksum("nothex a.tar.gz\n", "a.tar.gz").is_err());
    }

    #[test]
    fn validate_archive_entries_accepts_single_expected_binary() {
        assert!(validate_archive_entries("srcwalk\n", "srcwalk").is_ok());
        assert!(validate_archive_entries("./srcwalk.exe\r\n", "srcwalk.exe").is_ok());
    }

    #[test]
    fn validate_archive_entries_rejects_extra_traversal_and_wrong_name() {
        assert!(validate_archive_entries("srcwalk\nREADME.md\n", "srcwalk").is_err());
        assert!(validate_archive_entries("../srcwalk\n", "srcwalk").is_err());
        assert!(validate_archive_entries("/etc/srcwalk\n", "srcwalk").is_err());
        assert!(validate_archive_entries("wrong-binary\n", "srcwalk").is_err());
    }

    // ---- P1-1: HTTPS-only argv (curl/wget must never be able to follow a
    // plain-HTTP redirect for either the resolver or the asset downloader) ----

    #[test]
    fn curl_download_args_forces_https_only() {
        let args = curl_download_args("https://example.test/a", 4096, "20");
        assert!(args.contains(&"--proto".to_string()));
        assert!(args.contains(&"=https".to_string()));
        assert!(args.contains(&"--proto-redir".to_string()));
        assert_eq!(
            args.iter().filter(|a| a.as_str() == "=https").count(),
            2,
            "both --proto and --proto-redir must be pinned to https: {args:?}"
        );
    }

    #[test]
    fn wget_download_args_forces_https_only() {
        let args = wget_download_args("https://example.test/a", "20");
        assert!(args.contains(&"--https-only".to_string()), "{args:?}");
    }

    // ---- RED 7: curl-then-wget downloader abstraction (fake tools, zero network) ----

    #[cfg(unix)]
    #[test]
    fn download_text_uses_curl_when_curl_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "printf 'hello-from-curl'");
        write_fake_tool(dir.path(), "wget", "exit 1");
        let text =
            download_text("https://example.test/x", 4096, Some(dir.path().as_os_str())).unwrap();
        assert_eq!(text, "hello-from-curl");
    }

    #[cfg(unix)]
    #[test]
    fn download_text_falls_back_to_wget_when_curl_fails() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "exit 1");
        write_fake_tool(dir.path(), "wget", "printf 'hello-from-wget'");
        let text =
            download_text("https://example.test/x", 4096, Some(dir.path().as_os_str())).unwrap();
        assert_eq!(text, "hello-from-wget");
    }

    #[cfg(unix)]
    #[test]
    fn download_text_reports_both_failures_when_no_tool_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "exit 1");
        write_fake_tool(dir.path(), "wget", "exit 1");
        let err = download_text("https://example.test/x", 4096, Some(dir.path().as_os_str()))
            .unwrap_err();
        assert!(err.contains("curl") && err.contains("wget"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn download_text_includes_bounded_stderr_detail_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "echo 'CURL_ERR_DETAIL' >&2; exit 1");
        write_fake_tool(dir.path(), "wget", "echo 'WGET_ERR_DETAIL' >&2; exit 1");
        let err = download_text("https://example.test/x", 4096, Some(dir.path().as_os_str()))
            .unwrap_err();
        assert!(
            err.contains("CURL_ERR_DETAIL") && err.contains("WGET_ERR_DETAIL"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn download_text_enforces_byte_cap() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "printf '0123456789'");
        write_fake_tool(dir.path(), "wget", "exit 1");
        let err =
            download_text("https://example.test/x", 4, Some(dir.path().as_os_str())).unwrap_err();
        assert!(err.contains("exceeds") || err.contains("bytes"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn download_archive_writes_create_new_destination_and_cleans_up_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "printf 'archive-bytes'");
        write_fake_tool(dir.path(), "wget", "exit 1");
        let dest = dir.path().join("out.tar.gz");
        let sha256 = download_archive(
            "https://example.test/a.tar.gz",
            &dest,
            4096,
            Some(dir.path().as_os_str()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"archive-bytes");
        assert_eq!(
            sha256,
            "0c982986710a026635603031674053ca851fc0e3ea760094a34f59b84f7f6da6",
            "sha256_hex must match the real SHA-256 of the downloaded bytes, not just a checksum-shaped string"
        );

        let dir2 = tempfile::tempdir().unwrap();
        write_fake_tool(dir2.path(), "curl", "exit 1");
        write_fake_tool(dir2.path(), "wget", "exit 1");
        let dest2 = dir2.path().join("out2.tar.gz");
        let err = download_archive(
            "https://example.test/a.tar.gz",
            &dest2,
            4096,
            Some(dir2.path().as_os_str()),
        )
        .unwrap_err();
        assert!(
            !dest2.exists(),
            "{err}: partial archive must not be left behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn download_archive_refuses_to_overwrite_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "printf 'new-bytes'");
        let dest = dir.path().join("out.tar.gz");
        std::fs::write(&dest, b"pre-existing").unwrap();

        let err = download_archive(
            "https://example.test/a.tar.gz",
            &dest,
            4096,
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();

        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"pre-existing",
            "{err}: an existing destination must never be overwritten"
        );
    }

    #[cfg(unix)]
    #[test]
    fn download_archive_cleans_up_when_content_exceeds_cap() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tool(dir.path(), "curl", "printf '0123456789'");
        write_fake_tool(dir.path(), "wget", "exit 1");
        let dest = dir.path().join("out.tar.gz");

        let err = download_archive(
            "https://example.test/a.tar.gz",
            &dest,
            4,
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();

        assert!(
            !dest.exists(),
            "{err}: oversize archive must not leave partial bytes on disk"
        );
    }

    // ---- RED (gap 1): post-extract type safety — tar listing text alone never
    // proves the extracted entry's real filesystem type. ----

    #[cfg(unix)]
    #[test]
    fn validate_extracted_binary_accepts_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("srcwalk");
        std::fs::write(&bin, b"binary-bytes").unwrap();
        assert!(validate_extracted_binary(&bin).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn validate_extracted_binary_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, b"binary-bytes").unwrap();
        let link = dir.path().join("srcwalk");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(validate_extracted_binary(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validate_extracted_binary_rejects_directory_and_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("srcwalk");
        std::fs::create_dir(&subdir).unwrap();
        assert!(validate_extracted_binary(&subdir).is_err());
        assert!(validate_extracted_binary(&dir.path().join("missing")).is_err());
    }

    // ---- P1: temp dir must be private atomically (mkdir(0700), not a
    // separate post-create chmod — no window with broader permissions) ----

    #[cfg(unix)]
    #[test]
    fn create_private_temp_dir_is_mode_0700_atomically() {
        let dir = create_private_temp_dir().unwrap();
        let perms = std::fs::metadata(&dir).unwrap().permissions();
        let mode = std::os::unix::fs::PermissionsExt::mode(&perms) & 0o777;
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            mode, 0o700,
            "temp dir must be created private, mode was {mode:o}"
        );
    }

    // ---- RED (gap 2): preflight / create-new safety ----

    #[test]
    fn require_probe_cleanup_succeeds_when_both_create_and_remove_succeed() {
        assert!(require_probe_cleanup(|| Ok(()), || Ok(())).is_ok());
    }

    #[test]
    fn require_probe_cleanup_fails_when_remove_fails_even_though_create_succeeded() {
        // A write-probe that was created but could not be removed is not
        // proof of usable write capability — the caller must see this as a
        // failure, not silently continue as if the directory were clean.
        let err = require_probe_cleanup(|| Ok(()), || Err("simulated remove failure".to_string()))
            .unwrap_err();
        assert!(err.contains("simulated remove failure"), "{err}");
    }

    #[test]
    fn require_probe_cleanup_never_calls_remove_when_create_fails() {
        let err = require_probe_cleanup(
            || Err("simulated create failure".to_string()),
            || panic!("remove must not run when create never succeeded"),
        )
        .unwrap_err();
        assert!(err.contains("simulated create failure"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn reserve_create_new_succeeds_for_a_fresh_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reserved");
        assert!(reserve_create_new(&path).is_ok());
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn reserve_create_new_refuses_an_existing_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("taken");
        std::fs::write(&path, b"already here").unwrap();
        assert!(reserve_create_new(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"already here");
    }

    #[cfg(unix)]
    #[test]
    fn reserve_create_new_refuses_an_attacker_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"do not touch").unwrap();
        let trap = dir.path().join("reserved");
        std::os::unix::fs::symlink(&victim, &trap).unwrap();
        assert!(
            reserve_create_new(&trap).is_err(),
            "create-new must never follow an existing symlink, attacker-planted or not"
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"do not touch");
    }

    #[cfg(unix)]
    #[test]
    fn preflight_same_dir_reserves_distinct_paths_next_to_current() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        std::fs::write(&current, b"old-bytes").unwrap();

        let paths = preflight_same_dir(&current).unwrap();
        assert_ne!(paths.staged, paths.backup);
        assert_ne!(paths.staged, current);
        assert_ne!(paths.backup, current);
        assert_eq!(paths.staged.parent(), Some(dir.path()));
        assert_eq!(paths.backup.parent(), Some(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn preflight_same_dir_rejects_non_regular_current_target() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk-dir");
        std::fs::create_dir(&current).unwrap();
        assert!(preflight_same_dir(&current).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preflight_same_dir_rejects_current_symlink() {
        // symlink_metadata (lstat) must be used, not metadata (which follows
        // the link): a symlink at `current` must be rejected outright, never
        // treated as if it were the regular file it points to.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, b"old-bytes").unwrap();
        let current = dir.path().join("srcwalk");
        std::os::unix::fs::symlink(&target, &current).unwrap();
        assert!(preflight_same_dir(&current).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preflight_same_dir_rejects_unwritable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        std::fs::write(&current, b"old-bytes").unwrap();
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
        std::fs::set_permissions(dir.path(), perms.clone()).unwrap();
        let result = preflight_same_dir(&current);
        // Restore write permission so tempfile can clean up the directory afterward.
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        if result.is_ok() {
            // Effective root (or another privilege bypass) ignores the mode
            // bit and the write-probe genuinely succeeded; nothing to assert.
            eprintln!(
                "skipping preflight_same_dir_rejects_unwritable_directory: chmod 0555 did not block writes in this environment (likely running as root)"
            );
            return;
        }
        assert!(
            result.is_err(),
            "a same-dir write probe must fail before any swap is attempted"
        );
    }

    // ---- P1: same-dir artifact leaks on error — a create_new-reserved
    // staged/backup file left behind on a non-copy failure (open/stat/chmod
    // before or after the copy) is a leftover artifact next to the real
    // executable, not a harmless temp file. ----

    #[test]
    fn stage_same_dir_cleans_up_staged_when_extracted_binary_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist-extracted-binary");
        let staged = dir.path().join("srcwalk.staged");

        let err = stage_same_dir(&missing, &staged).unwrap_err();

        assert!(err.contains("could not open extracted binary"), "{err}");
        assert!(
            !staged.exists(),
            "a create_new-reserved staged file must not be left behind when opening the source fails: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_backup_current_cleans_up_reserved_backup_when_current_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let missing_current = dir.path().join("does-not-exist-current");
        let backup = dir.path().join("srcwalk.bak");

        let err = unix_backup_current(&missing_current, &backup).unwrap_err();

        assert!(err.contains("could not read current executable"), "{err}");
        assert!(
            !backup.exists(),
            "a create_new-reserved backup file must not be left behind when opening current fails: {err}"
        );
    }

    // The two tests above prove the ordinary case (cleanup succeeds); the
    // two below prove a genuine cleanup-removal failure is folded into the
    // reported error rather than swallowed by a bare `let _ = ...`. This is
    // not reproducible via real chmod (create and remove of the same path
    // are gated by the identical directory permission bit, and there is no
    // way to intervene between them from outside a single function call),
    // so `cleanup_remove` is fault-injected instead — same rationale as
    // `windows_move_non_overwrite_with`.

    #[test]
    fn stage_same_dir_with_reports_both_errors_when_reserved_staged_cleanup_also_fails() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist-extracted-binary");
        let staged = dir.path().join("srcwalk.staged");

        let err = stage_same_dir_with(&missing, &staged, |_p| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cleanup-locked",
            ))
        })
        .unwrap_err();

        assert!(err.contains("could not open extracted binary"), "{err}");
        assert!(
            err.contains("also could not remove leftover staged binary") && err.contains("cleanup-locked"),
            "{err}: a cleanup failure for the reserved staged file must be folded into the reported error, not silently swallowed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_backup_current_with_reports_both_errors_when_reserved_backup_cleanup_also_fails() {
        let dir = tempfile::tempdir().unwrap();
        let missing_current = dir.path().join("does-not-exist-current");
        let backup = dir.path().join("srcwalk.bak");

        let err = unix_backup_current_with(&missing_current, &backup, |_p| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cleanup-locked",
            ))
        })
        .unwrap_err();

        assert!(err.contains("could not read current executable"), "{err}");
        assert!(
            err.contains("also could not remove leftover backup") && err.contains("cleanup-locked"),
            "{err}: a cleanup failure for the reserved backup file must be folded into the reported error, not silently swallowed"
        );
    }

    // ---- P1: `unix_transactional_swap_inner`'s own two backup-cleanup
    // steps (placement failure, and post-verify success) in isolation —
    // same fault-injection rationale as above. ----

    #[cfg(unix)]
    #[test]
    fn placement_failed_reports_both_errors_when_backup_cleanup_also_fails() {
        let err = placement_failed(
            "rename failed".to_string(),
            Path::new("/some/backup"),
            |_p| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cleanup-locked",
                ))
            },
        );

        assert!(
            err.contains("could not place updated binary") && err.contains("rename failed"),
            "{err}"
        );
        assert!(
            err.contains("also could not remove leftover backup") && err.contains("cleanup-locked"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn placement_failed_reports_only_primary_when_backup_cleanup_succeeds() {
        let err = placement_failed(
            "rename failed".to_string(),
            Path::new("/some/backup"),
            |_p| Ok(()),
        );
        assert_eq!(err, "could not place updated binary: rename failed");
    }

    #[cfg(unix)]
    #[test]
    fn verified_swap_cleanup_succeeds_when_backup_removal_succeeds() {
        assert!(verified_swap_cleanup(Path::new("/some/backup"), |_p| Ok(())).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn verified_swap_cleanup_succeeds_when_backup_is_already_gone() {
        assert!(verified_swap_cleanup(Path::new("/some/backup"), |_p| {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
        })
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn verified_swap_cleanup_reports_honest_error_without_claiming_success_when_removal_fails() {
        let err = verified_swap_cleanup(Path::new("/some/backup"), |_p| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "cleanup-locked",
            ))
        })
        .unwrap_err();

        assert!(err.contains("updated and verified"), "{err}");
        assert!(err.contains("could not remove backup"), "{err}");
        assert!(err.contains("cleanup-locked"), "{err}");
    }

    // ---- RED 8: Unix transactional swap + rollback ----

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_replaces_current_on_successful_verify() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        let staged = dir.path().join("srcwalk.staged");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        unix_transactional_swap(&current, &staged, &backup, "1.9.0", |_path| {
            Ok("1.9.0".to_string())
        })
        .unwrap();

        assert_eq!(std::fs::read(&current).unwrap(), b"new-bytes");
        assert!(
            !backup.exists(),
            "backup must be removed after a verified swap"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_rolls_back_on_verify_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        let staged = dir.path().join("srcwalk.staged");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        let err = unix_transactional_swap(&current, &staged, &backup, "1.9.0", |_path| {
            Ok("1.8.0".to_string())
        })
        .unwrap_err();

        assert_eq!(
            std::fs::read(&current).unwrap(),
            b"old-bytes",
            "current executable must be restored after a failed post-check: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_preserves_current_when_placement_fails() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        // Rename source is deliberately missing, forcing the placement step
        // itself to fail — distinct from a post-check verify mismatch.
        let staged = dir.path().join("does-not-exist");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();

        let err = unix_transactional_swap(&current, &staged, &backup, "1.9.0", |_path| {
            Ok("1.9.0".to_string())
        })
        .unwrap_err();

        assert_eq!(
            std::fs::read(&current).unwrap(),
            b"old-bytes",
            "placement failure must never touch the original executable: {err}"
        );
        assert!(
            !backup.exists(),
            "a stray backup must not be left when placement never happened: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_reports_both_errors_when_rollback_itself_fails() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        let staged = dir.path().join("srcwalk.staged");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        let backup_for_closure = backup.clone();
        let err = unix_transactional_swap(&current, &staged, &backup, "1.9.0", move |_path| {
            // Simulate the backup becoming unavailable exactly when rollback
            // needs it, forcing the rollback step itself to fail.
            std::fs::remove_file(&backup_for_closure).unwrap();
            Ok("1.8.0".to_string())
        })
        .unwrap_err();

        assert!(
            err.contains("1.9.0") && err.contains("1.8.0"),
            "{err}: primary post-check detail must survive a failed rollback"
        );
        assert!(
            err.contains("rollback"),
            "{err}: rollback failure detail must be reported too, not silently swallowed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_cleans_up_leftover_staged_when_backup_creation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        let staged = dir.path().join("srcwalk.staged");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();
        // Pre-occupy the backup path so create_new fails before placement
        // ever starts, leaving `staged` fully intact and never consumed.
        std::fs::write(&backup, b"pre-existing").unwrap();

        let err = unix_transactional_swap(&current, &staged, &backup, "1.9.0", |_path| {
            panic!("verify must not run when backup creation never succeeded")
        })
        .unwrap_err();

        assert!(err.contains("could not create"), "{err}");
        assert!(
            !staged.exists(),
            "a staged binary must never be left behind when the swap never even started placing it: {err}"
        );
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"pre-existing",
            "a pre-existing backup path must never be clobbered by a failed create_new attempt"
        );
        assert_eq!(std::fs::read(&current).unwrap(), b"old-bytes");
    }

    #[cfg(unix)]
    #[test]
    fn unix_transactional_swap_reports_cleanup_failure_when_leftover_staged_cannot_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let staged_dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk");
        let staged = staged_dir.path().join("srcwalk.staged");
        let backup = dir.path().join("srcwalk.bak");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();
        // Pre-occupy the backup path so create_new fails before placement
        // ever starts — the same forced failure as above, but this time
        // `staged`'s own directory is made unwritable so its cleanup attempt
        // itself fails and must be folded into the reported error.
        std::fs::write(&backup, b"pre-existing").unwrap();
        let mut perms = std::fs::metadata(staged_dir.path()).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
        std::fs::set_permissions(staged_dir.path(), perms.clone()).unwrap();

        let err = unix_transactional_swap(&current, &staged, &backup, "1.9.0", |_path| {
            panic!("verify must not run when backup creation never succeeded")
        })
        .unwrap_err();

        // Restore write permission unconditionally so tempfile can clean up afterward.
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(staged_dir.path(), perms).unwrap();

        assert!(err.contains("could not create"), "{err}");
        if staged.exists() {
            assert!(
                err.contains("leftover staged binary"),
                "{err}: a cleanup failure for the leftover staged binary must be folded into the reported error, not silently swallowed"
            );
        } else {
            eprintln!(
                "skipping strict cleanup-failure assertion in unix_transactional_swap_reports_cleanup_failure_when_leftover_staged_cannot_be_removed: chmod 0555 did not block unlink in this environment (likely running as root)"
            );
        }
    }

    // ---- P1-4: windows_move_non_overwrite_with (pure, fault-injected — forcing a
    // genuine post-hard_link remove-source failure on a real filesystem is not
    // reliably reproducible, so the branch is proven with injected closures
    // instead; this also makes it testable on any host, not just Windows CI) ----

    #[test]
    fn windows_move_non_overwrite_with_succeeds_when_link_and_remove_both_succeed() {
        let result = windows_move_non_overwrite_with(
            Path::new("src"),
            Path::new("dst"),
            |_s, _d| Ok(()),
            |_s| Ok(()),
            |_d| panic!("cleanup must not run when remove_source succeeded"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn windows_move_non_overwrite_with_fails_honestly_when_link_fails() {
        let result = windows_move_non_overwrite_with(
            Path::new("src"),
            Path::new("dst"),
            |_s, _d| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "occupied",
                ))
            },
            |_s| panic!("remove_source must not run when link never succeeded"),
            |_d| panic!("cleanup must not run when link never succeeded"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn windows_move_non_overwrite_with_cleans_up_link_when_remove_source_fails() {
        use std::cell::Cell;
        let destination_removed = Cell::new(false);
        let result = windows_move_non_overwrite_with(
            Path::new("src"),
            Path::new("dst"),
            |_s, _d| Ok(()),
            |_s| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "locked",
                ))
            },
            |_d| {
                destination_removed.set(true);
                Ok(())
            },
        );
        assert!(
            result.is_err(),
            "a move that could not remove its source is a failure, not a harmless leftover link"
        );
        assert!(
            destination_removed.get(),
            "the just-created link must be cleaned up when the source cannot be removed"
        );
    }

    #[test]
    fn windows_move_non_overwrite_with_reports_both_errors_when_cleanup_also_fails() {
        let err = windows_move_non_overwrite_with(
            Path::new("src"),
            Path::new("dst"),
            |_s, _d| Ok(()),
            |_s| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "remove-source-locked",
                ))
            },
            |_d| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cleanup-also-locked",
                ))
            },
        )
        .unwrap_err();
        assert!(
            err.contains("remove-source-locked") && err.contains("cleanup-also-locked"),
            "{err}: both the remove-source failure and the cleanup failure must be reported"
        );
    }

    // ---- P1: the first-move step's hard_link+remove semantics against a
    // *genuinely running* process image — the tests below this marker use a
    // stand-in file that is never actually loaded/executing, which does not
    // prove the load-bearing assumption behind step 1 of the Windows swap
    // (current -> .old) on a real Windows loader. This test copies a
    // disposable, throwaway `cmd.exe` (the real system one is never touched)
    // and keeps it genuinely running — deterministically, via a piped stdin
    // that is never written to or closed, no sleep/timing race and no
    // network — then exercises the exact `windows_move_non_overwrite` helper
    // used in production against that running copy's own file. ----

    #[cfg(windows)]
    #[test]
    fn windows_move_non_overwrite_succeeds_against_a_genuinely_running_process_image() {
        let dir = tempfile::tempdir().unwrap();
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let system_cmd = Path::new(&system_root).join("System32").join("cmd.exe");
        let running = dir.path().join("srcwalk-running-copy.exe");
        std::fs::copy(&system_cmd, &running).expect(
            "copy a disposable cmd.exe for this test only; the real system cmd.exe is never touched",
        );

        let mut child = process::Command::new(&running)
            .stdin(process::Stdio::piped())
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .spawn()
            .expect("spawn the disposable running copy");

        let old = dir.path().join("srcwalk-running-copy.exe.old");
        let result = windows_move_non_overwrite(&running, &old);

        // Always terminate + reap the disposable child before asserting, so
        // a failed assertion never leaks a live process.
        let _ = child.kill();
        let _ = child.wait();

        result.expect(
            "hard_link + remove_file must succeed against a currently-running process image on \
             Windows -- this is the load-bearing assumption behind the first-move step of the \
             Windows transactional swap (current -> .old). If this fails on real Windows CI, the \
             swap design needs to change before shipping; do not silently patch over it.",
        );
        assert!(
            old.exists(),
            "the running copy must have been moved to .old"
        );
        assert!(
            !running.exists(),
            "the running copy's original path must be gone after the move"
        );
    }

    // ---- RED 9: Windows transactional swap + rollback (Windows CI only) ----

    #[cfg(windows)]
    #[test]
    fn windows_transactional_swap_replaces_current_on_successful_verify() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk.exe");
        let staged = dir.path().join("srcwalk.exe.staged");
        let old = dir.path().join("srcwalk.exe.old");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        windows_transactional_swap(&current, &staged, &old, "1.9.0", |_path| {
            Ok("1.9.0".to_string())
        })
        .unwrap();

        assert_eq!(std::fs::read(&current).unwrap(), b"new-bytes");
    }

    #[cfg(windows)]
    #[test]
    fn windows_transactional_swap_rolls_back_on_verify_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk.exe");
        let staged = dir.path().join("srcwalk.exe.staged");
        let old = dir.path().join("srcwalk.exe.old");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        let err = windows_transactional_swap(&current, &staged, &old, "1.9.0", |_path| {
            Ok("1.8.0".to_string())
        })
        .unwrap_err();

        assert_eq!(
            std::fs::read(&current).unwrap(),
            b"old-bytes",
            "current executable must be restored after a failed post-check: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_transactional_swap_restores_old_when_second_move_fails() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk.exe");
        // Staged is deliberately missing, forcing the second move (staged ->
        // original) to fail after the first rename (current -> old) already
        // succeeded — the contract requires immediate restoration of `.old`.
        let staged = dir.path().join("does-not-exist.exe");
        let old = dir.path().join("srcwalk.exe.old");
        std::fs::write(&current, b"old-bytes").unwrap();

        let err = windows_transactional_swap(&current, &staged, &old, "1.9.0", |_path| {
            Ok("1.9.0".to_string())
        })
        .unwrap_err();

        assert_eq!(
            std::fs::read(&current).unwrap(),
            b"old-bytes",
            "second-move failure must restore .old onto the original path: {err}"
        );
        assert!(
            !old.exists(),
            "`.old` must be moved back, not left behind, after a restored second-move failure: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_stale_old_best_effort_removes_a_leftover_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("srcwalk.exe.old");
        std::fs::write(&old, b"stale-from-previous-update").unwrap();
        cleanup_stale_old(&old);
        assert!(!old.exists());
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_stale_old_is_a_harmless_no_op_when_nothing_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("srcwalk.exe.old");
        // Must never panic or error just because there is nothing to clean up;
        // an update must not fail solely because `.old` cleanup is deferred.
        cleanup_stale_old(&old);
    }

    #[cfg(windows)]
    #[test]
    fn windows_transactional_swap_reports_both_errors_when_rollback_itself_fails() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("srcwalk.exe");
        let staged = dir.path().join("srcwalk.exe.staged");
        let old = dir.path().join("srcwalk.exe.old");
        std::fs::write(&current, b"old-bytes").unwrap();
        std::fs::write(&staged, b"new-bytes").unwrap();

        let old_for_closure = old.clone();
        let err = windows_transactional_swap(&current, &staged, &old, "1.9.0", move |_path| {
            // Simulate `.old` becoming unavailable exactly when rollback
            // needs it, forcing the rollback step itself to fail.
            std::fs::remove_file(&old_for_closure).unwrap();
            Ok("1.8.0".to_string())
        })
        .unwrap_err();

        assert!(
            err.contains("1.9.0") && err.contains("1.8.0"),
            "{err}: primary post-check detail must survive a failed rollback"
        );
        assert!(
            err.contains("rollback"),
            "{err}: rollback failure detail must be reported too, not silently swallowed"
        );
    }
}
