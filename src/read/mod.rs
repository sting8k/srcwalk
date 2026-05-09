pub mod imports;
pub mod outline;

mod directory;
mod full;
mod section;
mod suggest;

use std::fs;
use std::path::Path;

use memmap2::Mmap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format;
use crate::lang::detect_file_type;
use crate::types::{FileType, ViewMode};

pub use full::read_file_with_budget;
pub(crate) use suggest::edit_distance;
pub use suggest::suggest_similar_file;

#[cfg(test)]
use crate::types::estimate_tokens;
#[cfg(test)]
use section::{read_section, resolve_heading, suggest_symbols};

pub(crate) const RAW_TOKEN_CAP: u64 = 5_000;
const RAW_LINE_CAP: u32 = 200;
const FILE_SIZE_CAP: u64 = 500_000; // 500KB

pub fn read_file(
    path: &Path,
    section: Option<&str>,
    full: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SrcwalkError::NotFound {
                path: path.to_path_buf(),
                suggestion: suggest::suggest_similar(path),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(SrcwalkError::PermissionDenied {
                path: path.to_path_buf(),
            });
        }
        Err(e) => {
            return Err(SrcwalkError::IoError {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };

    // Directory → list contents
    if meta.is_dir() {
        return directory::list_directory(path);
    }

    let byte_len = meta.len();

    // Empty check before mmap — mmap on 0-byte file may fail on some platforms
    if byte_len == 0 {
        return Ok(format::file_header(path, 0, 0, ViewMode::Empty));
    }

    // Section param → return those lines verbatim when within the section token limit.
    if let Some(range) = section {
        return section::read_section(path, range, None, cache);
    }

    // Binary detection
    let file = fs::File::open(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let buf = &mmap[..];

    if crate::lang::detection::is_binary(buf) {
        let mime = full::mime_from_ext(path);
        return Ok(format::binary_header(path, byte_len, mime));
    }

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Generated
    if crate::lang::detection::is_generated_by_name(name)
        || crate::lang::detection::is_generated_by_content(buf)
    {
        let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;
        return Ok(format::file_header(
            path,
            byte_len,
            line_count,
            ViewMode::Generated,
        ));
    }

    let content = String::from_utf8_lossy(buf);
    let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;

    let file_type = detect_file_type(path);
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    // Raw body output requires explicit `--full` and is capped by both tokens
    // and lines. Default path reads always return a structural/smart view.
    if full {
        return full::render_full_body(
            path,
            buf,
            &content,
            byte_len,
            line_count,
            file_type,
            mtime,
            RAW_TOKEN_CAP,
            Some(RAW_LINE_CAP),
            cache,
        );
    }

    let capped = byte_len > FILE_SIZE_CAP;

    let outline = cache.get_or_compute(path, mtime, || {
        outline::generate(path, file_type, &content, buf, capped)
    });

    let mode = match file_type {
        FileType::StructuredData => ViewMode::Keys,
        _ => ViewMode::Outline,
    };
    let header = format::file_header(path, byte_len, line_count, mode);
    Ok(format!(
        "{header}\n\n{outline}\n\n> Next: drill into a symbol with --section <name> or a line range\n> Next: need raw file text? retry with --full, or use --section <range> for a smaller slice."
    ))
}

/// Would this file produce an outline (rather than full content) in default read mode?
/// Used by the MCP layer to decide whether to append related-file hints.
pub fn would_outline(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.is_dir() && m.len() > 0)
}

#[cfg(test)]
mod tests;
