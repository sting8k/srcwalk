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
        path.display()
    )
}

/// Build header for binary files: `# path (binary, size, mime) [skipped]`
pub fn binary_header(path: &Path, byte_len: u64, mime: &str) -> String {
    let size_str = format_size(byte_len);
    format!(
        "# {} (binary, {size_str}, {mime}) [skipped]",
        path.display()
    )
}

/// Build header for search results.
pub fn search_header(
    query: &str,
    scope: &Path,
    total: usize,
    defs: usize,
    usages: usize,
) -> String {
    let parts = match (defs, usages) {
        (0, _) => format!("{total} matches"),
        (d, u) => format!("{total} matches ({d} definitions, {u} usages)"),
    };
    format!("# Search: \"{query}\" in {} — {parts}", scope.display())
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

/// Path relative to scope for cleaner output. Falls back to full path.
pub(crate) fn rel(path: &Path, scope: &Path) -> String {
    path.strip_prefix(scope)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Non-empty display path for headers.
///
/// If `rel(path, scope)` is empty (e.g. `--scope` points to the file itself),
/// fall back to `dir/file` (or just `file` when parent dir is unavailable).
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
