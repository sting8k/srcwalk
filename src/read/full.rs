use std::fs;
use std::path::Path;

use memmap2::Mmap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format;
use crate::lang::detect_file_type;
use crate::types::{estimate_tokens, FileType, ViewMode};

use super::directory::list_directory;
use super::section::read_section;
use super::{outline, read_file};

fn capped_line_end(buf: &[u8], start_byte: usize, max_lines: u32, max_tokens: u64) -> (usize, u32) {
    let max_bytes = (max_tokens * 4) as usize;
    let mut end = start_byte;
    let mut lines = 0u32;

    while end < buf.len() && lines < max_lines && end.saturating_sub(start_byte) < max_bytes {
        if let Some(rel) = memchr::memchr(b'\n', &buf[end..]) {
            let next = end + rel + 1;
            if next.saturating_sub(start_byte) > max_bytes {
                break;
            }
            end = next;
            lines += 1;
        } else {
            let next = buf.len().min(start_byte + max_bytes);
            if next > end {
                end = next;
                lines += 1;
            }
            break;
        }
    }

    if end == start_byte && start_byte < buf.len() {
        end = buf.len().min(start_byte + max_bytes.max(1));
        lines = 1;
    }

    (end, lines)
}

/// Wrapper around `read_file` that, for `--full` requests with `--budget`,
/// degrades gracefully instead of letting the post-hoc `budget::apply`
/// truncate body bytes mid-function and leave a misleading `[full]` header.
///
/// Cascade (when `full=true` and rendered output exceeds `budget`):
///   1. full file        → if fits, return as-is.
///   2. outline           → labelled `[outline (full requested, over budget)]` + note.
///   3. signatures only   → labelled `[signatures (...)]` + note (outline still overflows).
///   4. header + advice   → file too large at any granularity for this budget.
///
/// For `section`, non-`full`, or no-budget paths, behaves identically to `read_file`
/// (caller still applies the top-level budget cap if needed).
pub(super) fn render_full_body(
    path: &Path,
    buf: &[u8],
    content: &str,
    byte_len: u64,
    line_count: u32,
    file_type: FileType,
    mtime: std::time::SystemTime,
    token_cap: u64,
    line_cap: Option<u32>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let raw_tokens = estimate_tokens(byte_len);
    let over_token_cap = raw_tokens > token_cap;
    let over_line_cap = line_cap.is_some_and(|cap| line_count > cap);
    if !over_token_cap && !over_line_cap {
        let header = format::file_header(path, byte_len, line_count, ViewMode::Full);
        let numbered = format::number_lines(content, 1);
        return Ok(format!("{header}\n\n{numbered}"));
    }

    let max_lines = line_cap.unwrap_or(u32::MAX);
    let (head_end, shown) = capped_line_end(buf, 0, max_lines, token_cap);
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let shown_tokens = estimate_tokens(head.len() as u64);
    let numbered_head = format::number_lines(&head, 1);
    let outline = cache.get_or_compute(path, mtime, || {
        outline::generate(path, file_type, content, buf, true)
    });

    let header = format::file_header(path, byte_len, line_count, ViewMode::Full);
    let next_start = shown + 1;
    let cap_text = match line_cap {
        Some(cap) => format!("{token_cap} tokens or {cap} lines"),
        None => format!("{token_cap} tokens"),
    };
    Ok(format!(
        "{header}\n\n> Caveat: full capped — tokens ~{shown_tokens}/{raw_tokens} shown (cap {cap_text}); lines {shown}/{line_count}.\n\n{numbered_head}\n\n## Outline\n\n{outline}\n\n> Next: use --section <symbol|range[,symbol|range]> for the needed parts, or retry with --budget <N>. Continue from --section {next_start}-<end>."
    ))
}

pub fn read_file_with_budget(
    path: &Path,
    section: Option<&str>,
    full: bool,
    budget: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    // Fast path: not a full-file budgeted request → defer to read_file.
    let Some(b) = budget else {
        return read_file(path, section, full, cache);
    };
    if let Some(range) = section {
        return read_section(path, range, Some(b), cache);
    }
    if !full {
        return read_file(path, section, full, cache);
    }

    let full_out = read_full_file_with_explicit_budget(path, b, cache)?;
    if estimate_tokens(full_out.len() as u64) <= b {
        return Ok(full_out);
    }

    // Step 2: outline cascade.
    let outline_out = render_outline_view(path, cache, ViewMode::OutlineCascade)?;
    let with_note = append_cascade_note(&outline_out, "full body", full_out.len(), b);
    if estimate_tokens(with_note.len() as u64) <= b {
        return Ok(with_note);
    }

    // Step 3: signatures only.
    let sig_out = render_signatures_view(path, cache)?;
    let sig_with_note = append_cascade_note(&sig_out, "outline", outline_out.len(), b);
    if estimate_tokens(sig_with_note.len() as u64) <= b {
        return Ok(sig_with_note);
    }

    // Step 4: terminal — header + advice only.
    let meta = std::fs::metadata(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let line_count =
        std::fs::read(path).map_or(0, |buf| memchr::memchr_iter(b'\n', &buf).count() as u32 + 1);
    let header = format::file_header(path, meta.len(), line_count, ViewMode::Signatures);
    Ok(format!(
        "{header}\n\n> Caveat: budget {b} tokens too small for file summary.\
         > Next: use --section <symbol|range> or --budget <N>."
    ))
}

fn read_full_file_with_explicit_budget(
    path: &Path,
    budget: u64,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let meta = fs::metadata(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    if meta.is_dir() {
        return list_directory(path);
    }
    if meta.len() == 0 {
        return Ok(format::file_header(path, 0, 0, ViewMode::Empty));
    }

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
        let mime = mime_from_ext(path);
        return Ok(format::binary_header(path, meta.len(), mime));
    }

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if crate::lang::detection::is_generated_by_name(name)
        || crate::lang::detection::is_generated_by_content(buf)
    {
        let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;
        return Ok(format::file_header(
            path,
            meta.len(),
            line_count,
            ViewMode::Generated,
        ));
    }

    let content = String::from_utf8_lossy(buf);
    let line_count = memchr::memchr_iter(b'\n', buf).count() as u32 + 1;
    let file_type = detect_file_type(path);
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    render_full_body(
        path,
        buf,
        &content,
        meta.len(),
        line_count,
        file_type,
        mtime,
        budget,
        None,
        cache,
    )
}

fn render_outline_view(
    path: &Path,
    cache: &OutlineCache,
    mode: ViewMode,
) -> Result<String, SrcwalkError> {
    let meta = std::fs::metadata(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let buf = std::fs::read(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let content = String::from_utf8_lossy(&buf);
    let line_count = memchr::memchr_iter(b'\n', &buf).count() as u32 + 1;
    let file_type = detect_file_type(path);
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let outline = cache.get_or_compute(path, mtime, || {
        outline::generate(path, file_type, &content, &buf, true)
    });
    let header = format::file_header(path, meta.len(), line_count, mode);
    Ok(format!("{header}\n\n{outline}"))
}

/// Signatures-only view: keep top-level outline lines (no nested children body).
/// Heuristic: drop indented continuation lines from the outline, preserving
/// only the first non-indented entry per block.
fn render_signatures_view(path: &Path, cache: &OutlineCache) -> Result<String, SrcwalkError> {
    let outline_full = render_outline_view(path, cache, ViewMode::Signatures)?;
    let mut lines = outline_full.lines();
    let header = lines.next().unwrap_or("");
    let mut kept: Vec<&str> = vec![header];
    for line in lines {
        // Keep blank separators and lines starting at column 0 or with one level of indent.
        if line.is_empty() {
            kept.push(line);
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ').count();
        if indent <= 2 {
            kept.push(line);
        }
    }
    Ok(kept.join("\n"))
}

fn append_cascade_note(body: &str, prev_kind: &str, prev_bytes: usize, budget: u64) -> String {
    let prev_tokens = estimate_tokens(prev_bytes as u64);
    let action = if prev_kind == "outline" {
        "compacted outline".to_string()
    } else {
        format!("downgraded {prev_kind}->outline")
    };
    format!(
        "{body}\n\n> Note: budget ~{prev_tokens}/{budget} tokens; {action}.\n> Next: use --section <symbol|range> or --budget <N>."
    )
}

/// Guess MIME type from extension for binary file headers.
pub(super) fn mime_from_ext(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz" | "tgz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("wasm") => "application/wasm",
        Some("woff" | "woff2") => "font/woff2",
        Some("ttf" | "otf") => "font/ttf",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    }
}
