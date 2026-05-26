use std::fs;
use std::path::Path;

use crate::error::SrcwalkError;
use crate::evidence::{render_next_actions, NextAction};
use crate::format;
use crate::types::estimate_tokens;

/// List directory contents — treat as glob on dir/*.
pub(super) fn list_directory(path: &Path) -> Result<String, SrcwalkError> {
    let mut entries: Vec<String> = Vec::new();
    let read_dir = fs::read_dir(path).map_err(|e| SrcwalkError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let mut items: Vec<_> = read_dir.filter_map(std::result::Result::ok).collect();
    items.sort_by_key(std::fs::DirEntry::file_name);

    for entry in &items {
        let ft = entry.file_type().ok();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let meta = entry.metadata().ok();

        let suffix = match ft {
            Some(t) if t.is_dir() => "/".to_string(),
            Some(t) if t.is_symlink() => " →".to_string(),
            _ => match meta {
                Some(m) => format!("  ~{}", fmt_tokens(estimate_tokens(m.len()))),
                None => String::new(),
            },
        };
        entries.push(format!("  {name}{suffix}"));
    }

    let display_path = format::display_path(path);
    let header = format!(
        "# {} ({} items, sizes ~= tokens)",
        display_path,
        items.len()
    );
    let mut out = format!("{header}\n\n{}", entries.join("\n"));
    let next_actions = render_next_actions(&[
        NextAction::guidance(
            format!("srcwalk overview --scope {display_path} --symbols"),
            "directory code structure drilldown",
            40,
        ),
        NextAction::guidance(
            format!("srcwalk discover <symbol> --scope {display_path}"),
            "directory symbol discovery drilldown",
            50,
        ),
    ]);
    if !next_actions.is_empty() {
        out.push_str("\n\n");
        out.push_str(&next_actions);
    }
    Ok(out)
}

fn fmt_tokens(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display-only; mantissa loss is fine for summaries
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1_000.0)
    } else {
        n.to_string()
    }
}
