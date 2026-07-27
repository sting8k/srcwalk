use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{render_next_actions, NextAction};
use crate::lang::detect_file_type;
use crate::lang::detection::{is_generated_by_content, is_generated_by_name};
use crate::read::outline;
use crate::types::{estimate_tokens, FileType};
use crate::ArtifactMode;

use super::{
    compute_relations, enforce_hard_cap, extract_symbol_previews, fmt_tokens,
    format_anchored_symbols, format_walk_note, is_map_file, map_walk_builder, RelationEntry,
    SymbolPreview, WalkConfig,
};

const ORIENTATION_AREA_LIMIT: usize = 12;
const ORIENTATION_FILE_LIMIT: usize = 8;
const ORIENTATION_RELATION_LIMIT: usize = 8;
const ORIENTATION_NEXT_ACTION_LIMIT: usize = 4;
const ORIENTATION_SYMBOL_LIMIT: usize = 3;

#[derive(Clone)]
struct OrientationFile {
    path: PathBuf,
    rel: PathBuf,
    tokens: u64,
    is_test: bool,
    is_generated: bool,
}

#[derive(Default)]
struct OrientationArea {
    files: usize,
    tokens: u64,
}

pub(super) fn generate(
    scope: &Path,
    cfg: &WalkConfig,
    cache: &OutlineCache,
    include_symbols: bool,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let files = collect_orientation_files(scope, cfg, glob, artifact)?;
    if files.is_empty() {
        return Err(SrcwalkError::NoMatches {
            query: "overview".to_string(),
            scope: scope.to_path_buf(),
            suggestion: None,
            guidance: Some("No overview entries found".to_string()),
        });
    }

    let mut areas = BTreeMap::<PathBuf, OrientationArea>::new();
    let mut dirs = BTreeSet::<PathBuf>::new();
    let mut visible_files = Vec::with_capacity(files.len());
    for file in &files {
        let area = top_level_area(&file.rel);
        let summary = areas.entry(area).or_default();
        summary.files += 1;
        summary.tokens += file.tokens;
        record_parent_dirs(&file.rel, &mut dirs);
        visible_files.push(file.path.clone());
    }

    let mut areas: Vec<(PathBuf, OrientationArea)> = areas.into_iter().collect();
    areas.sort_by(|(path_a, area_a), (path_b, area_b)| {
        area_b
            .tokens
            .cmp(&area_a.tokens)
            .then_with(|| path_a.cmp(path_b))
    });

    let candidates = select_navigation_candidates(&files, &areas);

    let relations = compute_relations(scope, 1, &visible_files);
    let total_tokens: u64 = files.iter().map(|file| file.tokens).sum();
    let shown_areas = areas.len().min(ORIENTATION_AREA_LIMIT);
    let shown_files = candidates.len().min(ORIENTATION_FILE_LIMIT);

    let mut out = format!(
        "# Overview: {} (auto orientation, sizes ~= tokens)\n",
        crate::format::display_path(scope)
    );
    out.push_str("# Note: large auto overview summarized; narrow --scope for full file rows.\n");
    out.push_str(&format_walk_note(cfg, artifact));
    let _ = writeln!(
        out,
        "coverage: {} source files · {} directories · ~{}",
        files.len(),
        dirs.len(),
        fmt_tokens(total_tokens)
    );
    let _ = writeln!(
        out,
        "shown: {shown_areas} areas · {shown_files} navigation candidates"
    );
    let _ = writeln!(
        out,
        "omitted: {} areas · {} files from candidates",
        areas.len().saturating_sub(shown_areas),
        files.len().saturating_sub(shown_files)
    );

    out.push_str("\n## Areas\n");
    for (area_path, area) in areas.iter().take(ORIENTATION_AREA_LIMIT) {
        let _ = writeln!(
            out,
            "- {:<28} {:>4} files  ~{}",
            display_area(area_path),
            area.files,
            fmt_tokens(area.tokens)
        );
    }
    if areas.len() > ORIENTATION_AREA_LIMIT {
        let _ = writeln!(
            out,
            "- ... {} more areas",
            areas.len() - ORIENTATION_AREA_LIMIT
        );
    }
    out.push_str("\nEvidence: filesystem tree + source token estimates.\n");

    out.push_str("\n## Navigation candidates\n");
    for file in candidates.iter().take(ORIENTATION_FILE_LIMIT) {
        let rel = file.rel.to_string_lossy();
        let _ = writeln!(out, "- {rel}  ~{}", fmt_tokens(file.tokens));
        let reason = if file.is_generated {
            "generated source fallback"
        } else if file.is_test {
            "test source fallback"
        } else {
            "representative source file"
        };
        let _ = writeln!(
            out,
            "  reason: {reason}; selected by area coverage + token estimate"
        );
        if include_symbols {
            if let Some(symbols) = orientation_symbols(file, cache) {
                let _ = writeln!(
                    out,
                    "  symbols: {}",
                    format_anchored_symbols(&symbols, ORIENTATION_SYMBOL_LIMIT)
                );
            }
        }
    }
    if files.len() > candidates.len() {
        let _ = writeln!(
            out,
            "- ... {} more source files omitted from candidates",
            files.len() - candidates.len()
        );
    }

    if !relations.is_empty() {
        out.push_str("\n## Static relation preview\n");
        format_orientation_relations(&relations, &mut out);
    }

    append_orientation_next_actions(
        &mut out,
        scope,
        areas
            .iter()
            .map(|(path, _)| path)
            .filter(|path| !path.as_os_str().is_empty())
            .take(ORIENTATION_NEXT_ACTION_LIMIT),
        glob,
    );

    enforce_hard_cap(&out, scope, 1)?;
    Ok(out)
}

fn collect_orientation_files(
    scope: &Path,
    cfg: &WalkConfig,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<Vec<OrientationFile>, SrcwalkError> {
    let mut files = Vec::new();
    let walker = map_walk_builder(scope, cfg, glob, artifact)?.build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let file_type = detect_file_type(path);
        if !is_map_file(path, file_type, artifact) {
            continue;
        }
        let rel = path.strip_prefix(scope).unwrap_or(path).to_path_buf();
        let tokens = std::fs::metadata(path)
            .ok()
            .map_or(0, |meta| estimate_tokens(meta.len()));
        files.push(OrientationFile {
            path: path.to_path_buf(),
            is_test: is_test_path(&rel),
            is_generated: is_generated_path(path),
            rel,
            tokens,
        });
    }
    Ok(files)
}

fn top_level_area(rel: &Path) -> PathBuf {
    if rel
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return PathBuf::new();
    }

    rel.components()
        .next()
        .map_or_else(PathBuf::new, |component| {
            PathBuf::from(component.as_os_str())
        })
}

fn record_parent_dirs(rel: &Path, dirs: &mut BTreeSet<PathBuf>) {
    let Some(parent) = rel.parent() else {
        return;
    };
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        if !current.as_os_str().is_empty() {
            dirs.insert(current.clone());
        }
    }
}

fn display_area(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "(root files)".to_string()
    } else {
        format!("{}/", path.to_string_lossy())
    }
}

fn is_test_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(name.as_ref(), "test" | "tests" | "testing")
            || name.ends_with("_test.go")
            || name.contains("_test.")
            || name.contains(".test.")
            || name.contains(".spec.")
    })
}

fn is_generated_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if is_generated_by_name(name)
        || name.starts_with("zz_generated")
        || name.contains(".generated.")
    {
        return true;
    }

    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut prefix = [0_u8; 512];
    let Ok(read) = file.read(&mut prefix) else {
        return false;
    };
    is_generated_by_content(&prefix[..read])
}

fn select_navigation_candidates<'a>(
    files: &'a [OrientationFile],
    areas: &[(PathBuf, OrientationArea)],
) -> Vec<&'a OrientationFile> {
    let mut selected = Vec::new();
    for (area, _) in areas {
        let best = files
            .iter()
            .filter(|file| top_level_area(&file.rel) == *area)
            .min_by(|a, b| compare_candidates(a, b));
        if let Some(file) = best {
            selected.push(file);
        }
        if selected.len() == ORIENTATION_FILE_LIMIT {
            return selected;
        }
    }

    let mut remaining: Vec<&OrientationFile> = files
        .iter()
        .filter(|file| !selected.iter().any(|chosen| chosen.rel == file.rel))
        .collect();
    remaining.sort_by(|a, b| compare_candidates(a, b));
    selected.extend(
        remaining
            .into_iter()
            .take(ORIENTATION_FILE_LIMIT - selected.len()),
    );
    selected
}

fn compare_candidates(a: &OrientationFile, b: &OrientationFile) -> std::cmp::Ordering {
    a.is_generated
        .cmp(&b.is_generated)
        .then_with(|| a.is_test.cmp(&b.is_test))
        .then_with(|| b.tokens.cmp(&a.tokens))
        .then_with(|| a.rel.cmp(&b.rel))
}

fn orientation_symbols(file: &OrientationFile, cache: &OutlineCache) -> Option<Vec<SymbolPreview>> {
    let file_type = detect_file_type(&file.path);
    let FileType::Code(_) = file_type else {
        return None;
    };
    let meta = std::fs::metadata(&file.path).ok()?;
    let mtime = meta.modified().ok()?;
    let outline_str = cache.get_or_compute(&file.path, mtime, || {
        let content = std::fs::read_to_string(&file.path).unwrap_or_default();
        let buf = content.as_bytes();
        outline::generate(&file.path, file_type, &content, buf, true)
    });
    let symbols = extract_symbol_previews(&outline_str);
    (!symbols.is_empty()).then_some(symbols)
}

fn format_orientation_relations(relations: &[RelationEntry], out: &mut String) {
    let mut relations: Vec<&RelationEntry> = relations.iter().collect();
    relations.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    for relation in relations.iter().take(ORIENTATION_RELATION_LIMIT) {
        let _ = writeln!(
            out,
            "- {} -> {} deps:{}",
            relation.from, relation.to, relation.count
        );
    }
    if relations.len() > ORIENTATION_RELATION_LIMIT {
        let _ = writeln!(
            out,
            "- ... {} more static relation groups omitted",
            relations.len() - ORIENTATION_RELATION_LIMIT
        );
    }
    out.push_str("caveat: static local deps; not runtime calls.\n");
}

fn append_orientation_next_actions<'a>(
    out: &mut String,
    scope: &Path,
    area_paths: impl IntoIterator<Item = &'a PathBuf>,
    glob: Option<&str>,
) {
    let mut actions = Vec::new();
    for area in area_paths {
        if area.as_os_str().is_empty() {
            continue;
        }
        let display = crate::format::display_path(&scope.join(area));
        let Some(quoted) = crate::format::shell_quote_arg(&display) else {
            continue;
        };
        let command = if let Some(pattern) = glob.filter(|pattern| !pattern.is_empty()) {
            let Some(quoted_pattern) = crate::format::shell_quote_arg(pattern) else {
                continue;
            };
            format!("srcwalk overview --scope {quoted} --symbols --glob {quoted_pattern}")
        } else {
            format!("srcwalk overview --scope {quoted} --symbols")
        };
        actions.push(NextAction::guidance(
            command,
            "overview scoped drilldown",
            40,
        ));
    }
    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        let _ = write!(out, "\n## Next\n{rendered}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, tokens: u64, is_test: bool, is_generated: bool) -> OrientationFile {
        OrientationFile {
            path: PathBuf::from(path),
            rel: PathBuf::from(path),
            tokens,
            is_test,
            is_generated,
        }
    }

    #[test]
    fn test_path_detection_avoids_spec_substring_false_positives() {
        assert!(is_test_path(Path::new("src/foo.spec.ts")));
        assert!(is_test_path(Path::new("tests/foo.rs")));
        assert!(is_test_path(Path::new("src/foo_test.go")));
        assert!(!is_test_path(Path::new("src/spec.rs")));
        assert!(!is_test_path(Path::new("src/typespec.ts")));
    }

    #[test]
    fn candidates_cover_areas_before_repeating_and_deprioritize_generated_files() {
        let files = vec![
            candidate("alpha/generated.rs", 10_000, false, true),
            candidate("alpha/real.rs", 100, false, false),
            candidate("alpha/second.rs", 9_000, false, false),
            candidate("beta/entry.rs", 80, false, false),
            candidate("gamma/entry.rs", 70, false, false),
        ];
        let areas = vec![
            (PathBuf::from("alpha"), OrientationArea::default()),
            (PathBuf::from("beta"), OrientationArea::default()),
            (PathBuf::from("gamma"), OrientationArea::default()),
        ];

        let selected = select_navigation_candidates(&files, &areas);
        let paths: Vec<&Path> = selected.iter().map(|file| file.rel.as_path()).collect();

        assert_eq!(
            &paths[..3],
            &[
                Path::new("alpha/second.rs"),
                Path::new("beta/entry.rs"),
                Path::new("gamma/entry.rs"),
            ]
        );
        assert_eq!(paths[3], Path::new("alpha/real.rs"));
        assert_eq!(paths[4], Path::new("alpha/generated.rs"));
    }
}
