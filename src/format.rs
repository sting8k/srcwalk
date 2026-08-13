use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::types::{estimate_tokens, SearchEvidenceCounts, ViewMode};

static CANONICAL_CWD: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Build the standard header line:
/// `# path/to/file.ts (N lines, ~X.Xk tokens) [mode]`
pub(crate) fn file_header(path: &Path, byte_len: u64, line_count: u32, mode: ViewMode) -> String {
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
pub(crate) fn binary_header(path: &Path, byte_len: u64, mime: &str) -> String {
    let size_str = format_size(byte_len);
    format!(
        "# {} (binary, {size_str}, {mime}) [skipped]",
        display_path(path)
    )
}

/// Build header for search results.
pub(crate) fn search_header(
    query: &str,
    scope: &Path,
    total: usize,
    counts: SearchEvidenceCounts,
) -> String {
    let parts = search_count_parts(total, counts);
    format!("# Search: \"{query}\" in {} — {parts}", display_path(scope))
}

pub(crate) fn search_count_parts(total: usize, counts: SearchEvidenceCounts) -> String {
    if counts.definitions == 0 {
        if counts.comments == 0 {
            format!("{total} matches")
        } else {
            format!("{total} matches ({} in comments)", counts.comments)
        }
    } else {
        let mut details = vec![format!("{} definitions", counts.definitions)];
        if counts.name_occurrences > 0 {
            details.push(format!("{} name occurrences", counts.name_occurrences));
        }
        if counts.text_matches > 0 {
            details.push(format!("{} text matches", counts.text_matches));
        }
        if counts.comments > 0 {
            details.push(format!("{} in comments", counts.comments));
        }
        format!("{total} matches ({})", details.join(", "))
    }
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
pub(crate) fn number_lines(content: &str, start: u32) -> String {
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

/// True when `rel`/`rel_nonempty` render `path` relative to the process CWD.
/// A printed command using that display string runs verbatim; a scope-relative
/// display needs the same `--scope` to resolve.
pub(crate) fn is_cwd_relative(path: &Path) -> bool {
    cwd_relative(path).is_some()
}

fn cwd_relative(path: &Path) -> Option<String> {
    let cwd = CANONICAL_CWD.get_or_init(|| {
        let cwd = std::env::current_dir().ok()?;
        Some(cwd.canonicalize().unwrap_or(cwd))
    });
    let cwd = cwd.as_deref()?;
    let rel = path.strip_prefix(cwd).ok()?;
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

/// Return a paste-safe shell argument, or `None` when control characters make it unsafe.
pub fn shell_quote_arg(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    if value.chars().all(is_shell_safe_path_char) {
        return Some(value.to_string());
    }

    #[cfg(windows)]
    {
        Some(format!("'{}'", value.replace('\'', "''")))
    }
    #[cfg(not(windows))]
    {
        Some(format!("'{}'", value.replace('\'', "'\\''")))
    }
}

fn is_shell_safe_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '-' | '.' | '/' | ':')
        || cfg!(windows) && c == '\\'
}

/// Split trailing footer guidance from primary output.
#[must_use]
pub fn split_trailing_footer(output: &str) -> Option<(&str, &str)> {
    let mut cursor = output.trim_end_matches(['\r', '\n']).len();
    let mut footer_start = cursor;
    let mut saw_footer = false;

    while cursor > 0 {
        let line_start = output[..cursor].rfind('\n').map_or(0, |index| index + 1);
        let line = output[line_start..cursor].trim_end_matches('\r');
        if line.starts_with("> ") {
            saw_footer = true;
            footer_start = line_start;
            cursor = line_start.saturating_sub(1);
            continue;
        }
        if saw_footer && line.trim().is_empty() {
            footer_start = line_start;
            cursor = line_start.saturating_sub(1);
            continue;
        }
        break;
    }

    saw_footer.then(|| output.split_at(footer_start))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_count_parts_omits_zero_name_occurrence_bucket() {
        let counts = SearchEvidenceCounts {
            definitions: 1,
            name_occurrences: 0,
            text_matches: 1,
            comments: 0,
        };

        assert_eq!(
            search_count_parts(2, counts),
            "2 matches (1 definitions, 1 text matches)"
        );
    }
}
