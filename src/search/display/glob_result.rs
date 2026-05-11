use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use crate::error::SrcwalkError;
use crate::format::rel_nonempty;
use crate::search::glob;

fn append_grouped_files(out: &mut String, files: &[glob::GlobFileEntry], scope: &Path) {
    if files.is_empty() {
        return;
    }

    let mut groups: BTreeMap<String, Vec<(String, Option<&str>)>> = BTreeMap::new();
    for file in files {
        let display = rel_nonempty(&file.path, scope);
        let (dir, name) = match display.rsplit_once('/') {
            Some((dir, name)) if !dir.is_empty() => (format!("{dir}/"), name.to_string()),
            _ => ("./".to_string(), display),
        };
        groups
            .entry(dir)
            .or_default()
            .push((name, file.preview.as_deref()));
    }

    for (dir, entries) in groups {
        let _ = write!(out, "\n\n{dir} ({})", entries.len());
        for (name, preview) in entries {
            let _ = write!(out, "\n  {name}");
            if let Some(preview) = preview {
                let _ = write!(out, "  ({preview})");
            }
        }
    }
}

/// Format glob search results (file list with previews + pagination hint).
pub(super) fn format_glob_result(
    result: &glob::GlobResult,
    scope: &Path,
    label: &str,
) -> Result<String, SrcwalkError> {
    let header = format!(
        "# {label}: \"{}\" in {} — {} of {} files (offset {})",
        result.pattern,
        crate::format::display_path(scope),
        result.files.len(),
        result.total_found,
        result.offset,
    );

    let mut out = header;
    if result.oversized {
        let _ = write!(
            out,
            "\n\n> ⚠ Large match set ({} files). Pagination is stable but \
             walks may be slow. Consider narrowing `--scope` or refining the pattern.",
            result.total_found,
        );
    }

    append_grouped_files(&mut out, &result.files, scope);

    let shown_end = result.offset + result.files.len();
    if result.total_found > shown_end {
        let omitted = result.total_found - shown_end;
        let _ = write!(
            out,
            "\n\n> Next: {omitted} more files available. Continue with --offset {shown_end} --limit {limit}.",
            limit = result.limit,
        );
    } else if result.offset > 0 {
        let _ = write!(out, "\n> Note: end of results.");
    }

    if result.files.is_empty() && !result.available_extensions.is_empty() {
        let _ = write!(
            out,
            "\n\nNo matches. Available extensions in scope: {}",
            result.available_extensions.join(", ")
        );
    }

    Ok(out)
}
