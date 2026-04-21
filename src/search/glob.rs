use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::Glob;

use crate::error::TilthError;
use crate::types::estimate_tokens;

const DEFAULT_LIMIT: usize = 20;
/// Soft threshold: emit a warning above this, but still collect all paths.
/// Agents paginate via `limit`/`offset` so large totals are safe; this is
/// just a signal to consider narrowing scope.
const WARN_THRESHOLD: usize = 100_000;

pub struct GlobFileEntry {
    pub path: PathBuf,
    pub preview: Option<String>,
}

pub struct GlobResult {
    pub pattern: String,
    pub files: Vec<GlobFileEntry>,
    pub total_found: usize,
    pub available_extensions: Vec<String>,
    pub offset: usize,
    pub limit: usize,
    /// Set when `total_found >= WARN_THRESHOLD`. Formatter surfaces this so
    /// agents know the match set is huge (slow walks, memory pressure).
    pub oversized: bool,
}

/// Glob search using `ignore::WalkBuilder` (parallel, .gitignore-aware).
/// Pagination: `limit` caps the returned slice (default 20); `offset` skips
/// entries from the start. `total_found` reflects the total match count
/// (bounded by `MAX_FILES` upper-limit = 200).
pub fn search(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
) -> Result<GlobResult, TilthError> {
    let glob = Glob::new(pattern).map_err(|e| TilthError::InvalidQuery {
        query: pattern.to_string(),
        reason: e.to_string(),
    })?;
    let matcher = glob.compile_matcher();

    let collected: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let total_found = std::sync::atomic::AtomicUsize::new(0);
    let extensions: std::sync::Mutex<HashSet<String>> = std::sync::Mutex::new(HashSet::new());

    let walker = super::walker(scope, None)?;

    walker.run(|| {
        let matcher = &matcher;
        let collected = &collected;
        let total_found = &total_found;
        let extensions = &extensions;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();

            // Collect extensions for zero-match suggestions
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                extensions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(ext.to_string());
            }

            // Match against filename or relative path
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let rel = path.strip_prefix(scope).unwrap_or(path);

            if matcher.is_match(name) || matcher.is_match(rel) {
                total_found.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Collect every match: pagination happens after a full,
                // deterministic sort. Paths are cheap (~100 B each).
                collected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(path.to_path_buf());
            }

            ignore::WalkState::Continue
        })
    });

    let total = total_found.load(std::sync::atomic::Ordering::Relaxed);
    let mut paths = collected
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let extensions = extensions
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Deterministic ordering: shallower paths first, then lexicographic.
    paths.sort_by(|a, b| {
        let da = a.components().count();
        let db = b.components().count();
        da.cmp(&db).then_with(|| a.cmp(b))
    });

    // Apply pagination after sort.
    let effective_limit = limit.unwrap_or(DEFAULT_LIMIT);
    let effective_offset = offset.min(paths.len());
    let page: Vec<PathBuf> = paths
        .into_iter()
        .skip(effective_offset)
        .take(effective_limit)
        .collect();

    // Compute previews only for the paginated slice (cheaper than all matches).
    let files: Vec<GlobFileEntry> = page
        .into_iter()
        .map(|p| {
            let preview = file_preview(&p);
            GlobFileEntry { path: p, preview }
        })
        .collect();

    let available_extensions: Vec<String> = if files.is_empty() && total == 0 {
        let mut exts: Vec<String> = extensions.into_iter().collect();
        exts.sort();
        exts.truncate(10);
        exts
    } else {
        Vec::new()
    };

    Ok(GlobResult {
        pattern: pattern.to_string(),
        files,
        total_found: total,
        available_extensions,
        offset: effective_offset,
        limit: effective_limit,
        oversized: total >= WARN_THRESHOLD,
    })
}

/// Quick preview: token estimate plus a one-line summary for code/text files.
/// For code files, the summary is the first non-empty doc/comment or code line
/// (truncated to ~80 chars). For other files, only the token estimate is shown.
fn file_preview(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let tokens = estimate_tokens(meta.len());

    let summary = first_meaningful_line(path).unwrap_or_default();
    if summary.is_empty() {
        Some(format!("~{tokens} tokens"))
    } else {
        Some(format!("~{tokens} tokens · {summary}"))
    }
}

/// Read up to ~4KB of `path` and return the first non-empty, non-shebang line
/// (preferring doc/module comments). Returns None for non-text or unreadable
/// files. Truncates to 80 displayable chars.
fn first_meaningful_line(path: &Path) -> Option<String> {
    use std::io::Read;
    if !is_textual_path(path) {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 4096];
    let n = f.read(&mut buf).ok()?;
    let s = std::str::from_utf8(&buf[..n]).ok()?;

    let mut chosen: Option<String> = None;
    for raw in s.lines().take(40) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("#!") {
            continue;
        }
        // Prefer the first doc-style comment, fall back to the first content line.
        let is_doc = line.starts_with("///")
            || line.starts_with("//!")
            || line.starts_with("/**")
            || line.starts_with("/*!")
            || line.starts_with("\"\"\"")
            || line.starts_with("'''")
            || line.starts_with("@doc")
            || line.starts_with("# ")
            || line.starts_with("## ");
        let cleaned = line
            .trim_start_matches("///")
            .trim_start_matches("//!")
            .trim_start_matches("//")
            .trim_start_matches("/**")
            .trim_start_matches("/*!")
            .trim_start_matches("/*")
            .trim_start_matches("*/")
            .trim_start_matches('*')
            .trim_start_matches("\"\"\"")
            .trim_start_matches("'''")
            .trim_start_matches('#')
            .trim();
        // Skip lines that are just comment markers / banners (e.g. "###",
        // "===", "---", license decoration). Require at least one letter so
        // previews always surface real prose or code.
        if cleaned.is_empty() || !cleaned.chars().any(char::is_alphabetic) {
            continue;
        }
        let truncated: String = cleaned.chars().take(80).collect();
        if is_doc {
            return Some(truncated);
        }
        if chosen.is_none() {
            chosen = Some(truncated);
        }
    }
    chosen
}

/// Heuristic allowlist of extensions worth previewing. Avoids reading binary
/// data, minified JS, or large data blobs in the hot loop.
fn is_textual_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "scala"
                | "rb"
                | "ex"
                | "exs"
                | "erl"
                | "hs"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "swift"
                | "m"
                | "mm"
                | "cs"
                | "php"
                | "lua"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "md"
                | "rst"
                | "txt"
                | "toml"
                | "yaml"
                | "yml"
                | "ini"
                | "cfg"
                | "sql"
                | "graphql"
                | "gql"
                | "proto"
        )
    )
}
