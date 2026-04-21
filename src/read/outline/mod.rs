pub mod code;
pub mod fallback;
pub mod markdown;
pub mod structured;
pub mod tabular;
pub mod test_file;

use std::path::Path;

use crate::types::FileType;

const OUTLINE_CAP: usize = 100; // max outline lines for huge files

/// Generate a smart view based on file type.
pub fn generate(
    path: &Path,
    file_type: FileType,
    content: &str,
    buf: &[u8],
    capped: bool,
) -> String {
    let max_lines = if capped { OUTLINE_CAP } else { usize::MAX };

    // Test files get special treatment regardless of language
    if crate::types::is_test_file(path) {
        if let FileType::Code(lang) = file_type {
            if let Some(outline) = test_file::outline(content, lang, max_lines) {
                return with_omission_note(outline, max_lines);
            }
        }
    }

    let out = match file_type {
        FileType::Code(lang) => code::outline(content, lang, max_lines),
        FileType::Markdown => markdown::outline(buf, max_lines),
        FileType::StructuredData => structured::outline(path, content, max_lines),
        FileType::Tabular => tabular::outline(content, max_lines),
        FileType::Log => fallback::log_view(content),
        FileType::Other => fallback::head_tail(content),
    };
    with_omission_note(out, max_lines)
}

/// Append a hint when the outline was capped, so the agent knows more symbols
/// exist beyond what's shown. Heuristic: outline body line count >= `max_lines`.
fn with_omission_note(outline: String, max_lines: usize) -> String {
    if max_lines == usize::MAX {
        return outline;
    }
    let body_lines = outline.lines().count();
    if body_lines < max_lines {
        return outline;
    }
    format!(
        "{outline}\n\n> _outline capped at {max_lines} lines — more symbols exist. \
         Use `section=\"start-end\"` or query a specific symbol to see the rest._"
    )
}
