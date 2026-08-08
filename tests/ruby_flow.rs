//! Integration tests for Ruby Flow Maps (US-051): the `context` packet and the
//! raw `decision-flow` graph over Ruby `method`/`singleton_method` bodies, plus
//! the conservative hard-abstention contract.
//!
//! Assertions are focused (substring/structural), not brittle full snapshots.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// A temp repo that removes its directory on drop (RAII cleanup; matches
/// existing integration-test hygiene).
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "srcwalk_ruby_flow_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempRepo(dir)
    }
}

impl std::ops::Deref for TempRepo {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn norm_path_separators(s: &str) -> String {
    s.replace('\\', "/")
}

/// Run a srcwalk subcommand with cwd = repo and return (success, stdout, stderr).
fn run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = srcwalk()
        .current_dir(dir)
        .args(args)
        .output()
        .expect("srcwalk runs");
    (
        out.status.success(),
        norm_path_separators(&String::from_utf8_lossy(&out.stdout)),
        norm_path_separators(&String::from_utf8_lossy(&out.stderr)),
    )
}

/// The line of `stdout` that contains `needle`, or panic with context.
fn line_containing<'a>(stdout: &'a str, needle: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{stdout}"))
}

/// 1-based line number of the first source line containing `needle`, matching
/// the tree-sitter line numbers the CLI reports (avoids brittle hand counts).
fn line_of(source: &str, needle: &str) -> usize {
    source
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{source}"))
        + 1
}

#[path = "ruby_flow/abstention.rs"]
mod abstention;
#[path = "ruby_flow/supported.rs"]
mod supported;
