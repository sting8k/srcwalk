//! IO layer for search: file readers, walkers, path utilities.
//!
//! Split out of `search/mod.rs` so the dispatch/format code there isn't
//! entangled with filesystem plumbing.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use ignore::WalkBuilder;

use crate::error::SrcwalkError;

// Directories that are always skipped — build artifacts, dependencies, VCS internals.
// We skip these explicitly instead of relying on .gitignore so that locally-relevant
// gitignored files (docs/, configs, generated code) are still searchable.
pub(crate) const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".pycache",
    "vendor",
    ".next",
    ".nuxt",
    "coverage",
    ".cache",
    ".tox",
    ".venv",
    ".eggs",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".turbo",
    ".parcel-cache",
    ".svelte-kit",
    "out",
    ".output",
    ".vercel",
    ".netlify",
    ".gradle",
    ".idea",
    ".scala-build",
    "target",
    ".bloop",
    ".metals",
];

/// Threshold below which `read` outperforms `mmap` due to syscall overhead.
const MMAP_THRESHOLD: u64 = 16_384;

/// Size above which we apply minified-file detection.
/// Below this, parsing cost is negligible anyway.
pub(crate) const MINIFIED_CHECK_THRESHOLD: u64 = 100_000;

/// Open a file for fast byte-level scanning without UTF-8 validation.
/// Returns None if the file can't be opened or is empty.
///
/// Uses `read` (heap) for small files and `mmap` for large ones — syscall
/// overhead of mmap isn't worth it below ~16KB.
///
/// The returned `FileBytes` derefs to `&[u8]` — pass to `memchr` for a fast
/// miss check. Only validate UTF-8 (via `std::str::from_utf8`) on files that
/// actually contain the query. This skips UTF-8 validation on ~90% of files
/// in typical searches, saving significant time on large repos.
pub(crate) enum FileBytes {
    Heap(Vec<u8>),
    Mmap(memmap2::Mmap),
}

impl std::ops::Deref for FileBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FileBytes::Heap(v) => v,
            FileBytes::Mmap(m) => m,
        }
    }
}

/// Is this path obviously a minified file based on filename convention?
///
/// Only flags `.min.` / `-min.` stems — a 10+ year-old strong convention.
/// No attempt to guess from bundler output names like "bundle.js" or
/// "vendor.js" — those can be genuine user code. Content-based detection
/// (`looks_minified`) catches the rest.
pub(crate) fn is_minified_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(stem_end) = name.rfind('.') else {
        return false;
    };
    let stem = &name[..stem_end];
    let dot_min = std::path::Path::new(stem)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("min"));
    dot_min || stem.to_ascii_lowercase().ends_with("-min")
}

/// Heuristic: does this content look minified? Samples first 2KB and checks
/// average line length. Minified code typically has <4 newlines per 2KB.
///
/// Only call this on files ≥ `MINIFIED_CHECK_THRESHOLD` — for small files
/// the cost of parsing is bounded regardless.
pub(crate) fn looks_minified(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(2048)];
    let newlines = memchr::memchr_iter(b'\n', sample).count();
    newlines < 4
}

pub(crate) fn read_file_bytes(path: &Path, size: u64) -> Option<FileBytes> {
    if size == 0 {
        return None;
    }
    if size < MMAP_THRESHOLD {
        fs::read(path).ok().map(FileBytes::Heap)
    } else {
        // Safety: mmap on a regular file. If the file is truncated during
        // the scan, access could SIGBUS — acceptable risk for a read-only
        // search tool; ripgrep uses the same pattern.
        let file = std::fs::File::open(path).ok()?;
        unsafe { memmap2::Mmap::map(&file).ok().map(FileBytes::Mmap) }
    }
}

/// Build a parallel directory walker that searches ALL files except known junk directories.
/// Does NOT respect .gitignore — ensures gitignored but locally-relevant files are found.
/// When `glob` is Some, applies a file-pattern override (whitelist or negation).
pub(crate) fn walker(scope: &Path, glob: Option<&str>) -> Result<ignore::WalkParallel, SrcwalkError> {
    let threads = std::env::var("SRCWALK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            // Tree-sitter parsing is CPU-bound → hyperthreading gives little benefit
            // and oversubscription regresses 10-20% on tested workloads.
            // Heuristic: use logical cores up to 8, then 75% beyond that.
            // - 2 cores → 2, 4 cores → 4, 8 cores → 8 (typical dev laptop sweet spot)
            // - 16 cores → 12, 32 cores → 24, 64 cores → 48 (server cap to avoid IO thrash)
            // Override via SRCWALK_THREADS env var.
            std::thread::available_parallelism().map_or(4, |n| {
                let logical = n.get();
                if logical <= 8 {
                    logical
                } else {
                    (logical * 3 / 4).min(24)
                }
            })
        });

    let mut builder = WalkBuilder::new(scope);
    builder
        .follow_links(true)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .threads(threads)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    return !SKIP_DIRS.contains(&name);
                }
            }
            true
        });

    if let Some(pattern) = glob {
        if !pattern.is_empty() {
            let mut overrides = ignore::overrides::OverrideBuilder::new(scope);
            overrides
                .add(pattern)
                .map_err(|e| SrcwalkError::InvalidQuery {
                    query: pattern.to_string(),
                    reason: format!("invalid glob: {e}"),
                })?;
            builder.overrides(overrides.build().map_err(|e| SrcwalkError::InvalidQuery {
                query: pattern.to_string(),
                reason: format!("invalid glob: {e}"),
            })?);
        }
    }

    Ok(builder.build_parallel())
}

/// Parse `/pattern/` regex syntax. Returns (pattern, `is_regex`).
pub(crate) fn parse_pattern(query: &str) -> (&str, bool) {
    if query.starts_with('/') && query.ends_with('/') && query.len() > 2 {
        (&query[1..query.len() - 1], true)
    } else {
        (query, false)
    }
}

/// Get `file_lines` estimate and mtime from metadata. One `stat()` per file.
pub(crate) fn file_metadata(path: &Path) -> (u32, SystemTime) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let est_lines = (meta.len() / 40).max(1) as u32;
            (est_lines, mtime)
        }
        Err(_) => (0, SystemTime::UNIX_EPOCH),
    }
}
