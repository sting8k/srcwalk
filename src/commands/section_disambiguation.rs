use std::path::Path;

use crate::error::SrcwalkError;
use crate::{format, search};

/// Test/vendor/build directories that we de-prioritize when picking a single
/// file for a bare-filename + `--section` request.
const NON_PROD_DIR_SEGMENTS: &[&str] = &[
    "tests",
    "test",
    "spec",
    "specs",
    "__tests__",
    "vendor",
    "node_modules",
    "override",
    "overrides",
    "fixtures",
    "examples",
    "docs",
    "build",
    "dist",
    "target",
];

fn is_non_prod(path: &Path, scope: &Path) -> bool {
    let rel = path.strip_prefix(scope).unwrap_or(path);
    rel.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| NON_PROD_DIR_SEGMENTS.contains(&s))
    })
}

/// Build a set of files visible to a .gitignore-respecting walk of `scope`.
/// Anything NOT in this set (e.g. build artifacts, benchmark fixtures, caches,
/// egg-info, venvs) is treated as non-primary — this lets us avoid hardcoding
/// every repo's ignore patterns and naturally adapts to whatever conventions
/// a project uses (`.gitignore` + `.ignore` + `.git/info/exclude`).
fn build_visible_set(scope: &Path) -> std::collections::HashSet<std::path::PathBuf> {
    let walker = ignore::WalkBuilder::new(scope)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .build();
    let mut out = std::collections::HashSet::new();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            out.insert(entry.path().to_path_buf());
        }
    }
    out
}

/// Rank by path-depth from scope (shallower = more primary). Used as a
/// tiebreaker when gitignore + hardcoded filters still leave >1 candidate:
/// an `index.ts` or `Program.cs` at the workspace root is almost always the
/// one the agent wants, vs. nested test harness copies.
fn depth_from_scope(path: &Path, scope: &Path) -> usize {
    path.strip_prefix(scope)
        .unwrap_or(path)
        .components()
        .count()
}

/// Resolve a glob pattern produced from a bare filename to a single file when
/// `--section` is supplied. Returns:
/// - `Some((picked, Some(note)))` when exactly one prod-path candidate exists
///   and other candidates were skipped.
/// - `Some((picked, None))` when there's a single match overall.
/// - Returns an `Err(InvalidQuery)` listing candidates when the choice is
///   ambiguous (>1 prod paths or >1 total with no prod/non-prod split).
/// - `Ok(None)` when the glob matched nothing — caller falls back to the
///   normal Glob handler so existing 0-match UX is preserved.
pub(crate) fn disambiguate_glob_for_section(
    pattern: &str,
    scope: &Path,
    original_query: &str,
) -> Result<Option<(std::path::PathBuf, Option<String>)>, SrcwalkError> {
    let result = search::glob::search(pattern, scope, Some(200), 0)?;
    if result.files.is_empty() {
        return Ok(None);
    }

    let total = result.files.len();
    if total == 1 {
        return Ok(Some((result.files[0].path.clone(), None)));
    }

    // .gitignore-aware "primary" set — a file is primary iff it is visible
    // to a standard gitignore-respecting walk AND not inside one of the
    // hardcoded test/vendor segments (which stay around even in repos
    // without a .gitignore).
    let visible = build_visible_set(scope);
    let primary: Vec<&std::path::PathBuf> = result
        .files
        .iter()
        .map(|e| &e.path)
        .filter(|p| visible.contains(*p) && !is_non_prod(p, scope))
        .collect();

    // Picker: single primary → done. Multiple primary → break tie by
    // min depth-from-scope if unique, otherwise fail loud.
    let picked_opt: Option<std::path::PathBuf> = match primary.len().cmp(&1) {
        std::cmp::Ordering::Equal => Some(primary[0].clone()),
        std::cmp::Ordering::Greater => {
            let min_depth = primary
                .iter()
                .map(|p| depth_from_scope(p, scope))
                .min()
                .unwrap_or(0);
            let shallowest: Vec<&std::path::PathBuf> = primary
                .iter()
                .copied()
                .filter(|p| depth_from_scope(p, scope) == min_depth)
                .collect();
            if shallowest.len() == 1 {
                Some(shallowest[0].clone())
            } else {
                None
            }
        }
        std::cmp::Ordering::Less => None,
    };

    if let Some(picked) = picked_opt {
        let skipped_count = total - 1;
        // Preview up to 3 of the skipped non-primary paths so the agent
        // knows what got filtered (helps when the pick is wrong).
        let skipped_preview: Vec<String> = result
            .files
            .iter()
            .map(|e| &e.path)
            .filter(|p| **p != picked)
            .take(3)
            .map(|p| format::rel_nonempty(p, scope))
            .collect();
        let skipped_str = if skipped_preview.is_empty() {
            String::new()
        } else {
            let joined = skipped_preview.join(", ");
            let more = if skipped_count > skipped_preview.len() {
                format!(", +{} more", skipped_count - skipped_preview.len())
            } else {
                String::new()
            };
            format!(" [{joined}{more}]")
        };
        let note = format!(
            "Resolved '{original_query}' → {} (skipped {skipped_count} non-primary {}{skipped_str}). Pass full path to override.",
            format::rel_nonempty(&picked, scope),
            if skipped_count == 1 { "copy" } else { "copies" },
        );
        return Ok(Some((picked, Some(note))));
    }

    // Ambiguous — fail loud with top-5 candidates (prefer primary set).
    let candidates: Vec<&std::path::PathBuf> = if primary.is_empty() {
        result.files.iter().take(5).map(|e| &e.path).collect()
    } else {
        primary
    };
    let listing = candidates
        .iter()
        .take(5)
        .map(|p| format!("  - {}", format::rel_nonempty(p, scope)))
        .collect::<Vec<_>>()
        .join("\n");
    let more = if candidates.len() > 5 {
        format!("\n  ... and {} more", candidates.len() - 5)
    } else {
        String::new()
    };
    Err(SrcwalkError::InvalidQuery {
        query: original_query.to_string(),
        reason: format!(
            "matches {total} files; --section needs exactly one. Candidates:\n{listing}{more}\nPass full path or narrow --scope."
        ),
    })
}
