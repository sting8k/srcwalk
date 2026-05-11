use std::fmt::Write;
use std::path::Path;

use crate::types::{estimate_tokens, ViewMode};

/// Build the standard header line:
/// `# path/to/file.ts (N lines, ~X.Xk tokens) [mode]`
pub fn file_header(path: &Path, byte_len: u64, line_count: u32, mode: ViewMode) -> String {
    let tokens = estimate_tokens(byte_len);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k tokens", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens} tokens")
    };
    format!(
        "# {} ({line_count} lines, {token_str}) [{mode}]",
        display_path(path)
    )
}

/// Build header for binary files: `# path (binary, size, mime) [skipped]`
pub fn binary_header(path: &Path, byte_len: u64, mime: &str) -> String {
    let size_str = format_size(byte_len);
    format!(
        "# {} (binary, {size_str}, {mime}) [skipped]",
        display_path(path)
    )
}

/// Build header for search results.
pub fn search_header(
    query: &str,
    scope: &Path,
    total: usize,
    defs: usize,
    usages: usize,
    comments: usize,
) -> String {
    let parts = match (defs, usages, comments) {
        (0, _, 0) => format!("{total} matches"),
        (0, _, c) => format!("{total} matches ({c} in comments)"),
        (d, u, 0) => format!("{total} matches ({d} definitions, {u} usages)"),
        (d, u, c) => format!("{total} matches ({d} definitions, {u} usages, {c} in comments)"),
    };
    format!("# Search: \"{query}\" in {} — {parts}", display_path(scope))
}

/// Human-readable file size. Integer math only — no floats.
fn format_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b}B"),
        b if b < 1024 * 1024 => format!("{}KB", b / 1024),
        b => format!(
            "{}.{}MB",
            b / (1024 * 1024),
            (b % (1024 * 1024)) * 10 / (1024 * 1024)
        ),
    }
}

/// Prefix each line with its 1-indexed line number, right-aligned.
pub fn number_lines(content: &str, start: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let last = (start as usize + lines.len()).max(1);
    let width = (last.ilog10() + 1) as usize;
    let mut out = String::with_capacity(content.len() + lines.len() * (width + 2));
    for (i, line) in lines.iter().enumerate() {
        let num = start as usize + i;
        let _ = writeln!(out, "{num:>width$}  {line}");
    }
    out
}

/// Human display path. Prefer cwd-relative paths so output can be copied back
/// into `srcwalk <path>:<line>` from the user's current directory.
pub(crate) fn display_path(path: &Path) -> String {
    normalize_display_path(cwd_relative(path).unwrap_or_else(|| path.display().to_string()))
}

/// Path for human result rows. Prefer cwd-relative copy-pasteable paths, then
/// fall back to scope-relative legacy display when the scope lives elsewhere.
pub(crate) fn rel(path: &Path, scope: &Path) -> String {
    normalize_display_path(cwd_relative(path).unwrap_or_else(|| {
        path.strip_prefix(scope)
            .unwrap_or(path)
            .display()
            .to_string()
    }))
}

fn cwd_relative(path: &Path) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let rel = path.strip_prefix(&cwd).ok()?;
    if rel.as_os_str().is_empty() {
        Some(".".to_string())
    } else {
        Some(normalize_display_path(rel.display().to_string()))
    }
}

fn normalize_display_path(path: String) -> String {
    if !cfg!(windows) {
        return path;
    }

    let path = path.replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = path.strip_prefix("//?/") {
        rest.to_string()
    } else {
        path
    }
}

/// Non-empty display path for headers/result rows.
pub(crate) fn rel_nonempty(path: &Path, scope: &Path) -> String {
    let rel_path = rel(path, scope);
    if !rel_path.is_empty() && rel_path != "." {
        return rel_path;
    }
    short_path(path)
}

fn short_path(path: &Path) -> String {
    let file = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    if !dir.is_empty() && !file.is_empty() {
        format!("{dir}/{file}")
    } else if !file.is_empty() {
        file.to_string()
    } else {
        path.display().to_string()
    }
}
