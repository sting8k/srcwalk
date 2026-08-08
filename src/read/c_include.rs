use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolve quoted C/C++ includes under bounded ancestor/include roots. Angle
/// includes are external/stdlib evidence and are intentionally not resolved.
pub(crate) fn resolve(file_dir: &Path, source: &str, scope: Option<&Path>) -> Option<PathBuf> {
    if !source.starts_with('"') {
        return None;
    }
    let clean = source.trim_matches('"');
    if clean.is_empty() {
        return None;
    }

    let boundary = scope.unwrap_or(file_dir);
    let boundary = boundary
        .canonicalize()
        .unwrap_or_else(|_| boundary.to_path_buf());
    let mut roots = Vec::new();
    let mut current = Some(file_dir);
    while let Some(dir) = current {
        roots.push(dir.to_path_buf());
        if dir == boundary {
            break;
        }
        current = dir.parent();
    }

    let mut candidates = HashSet::new();
    for root in roots {
        add_candidate(&root.join(clean), &boundary, &mut candidates);
        add_candidate(
            &root.join("include").join(clean),
            &boundary,
            &mut candidates,
        );
    }
    (candidates.len() == 1)
        .then(|| candidates.into_iter().next())
        .flatten()
}

fn add_candidate(path: &Path, boundary: &Path, candidates: &mut HashSet<PathBuf>) {
    if let Some(path) = path
        .canonicalize()
        .ok()
        .filter(|path| path.starts_with(boundary))
    {
        if path.is_file() {
            candidates.insert(path);
        }
    }
}
