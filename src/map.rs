use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::lang::detect_file_type;
use crate::read::{imports, outline};
use crate::types::{estimate_tokens, FileType, Lang};
use crate::ArtifactMode;

const MAP_HARD_TOKEN_CAP: u64 = 15_000;
const DEFAULT_MAP_DEPTH: usize = 3;
const WIDE_SCOPE_FILE_THRESHOLD: usize = 100;
const MAX_ARTIFACT_MAP_FILES: usize = 40;
const MAX_ARTIFACT_MAP_ANCHORS_PER_FILE: usize = 6;
const MAX_OUTBOUND_RELATION_GROUPS: usize = 10;

struct WalkConfig {
    hidden: bool,
    git_ignore: bool,
    git_global: bool,
    git_exclude: bool,
    ignore: bool,
    parents: bool,
}

fn default_walk_config() -> WalkConfig {
    WalkConfig {
        hidden: false,
        git_ignore: true,
        git_global: true,
        git_exclude: true,
        ignore: true,
        parents: true,
    }
}

fn map_walk_builder(
    scope: &Path,
    cfg: &WalkConfig,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<WalkBuilder, SrcwalkError> {
    let mut builder = WalkBuilder::new(scope);
    builder
        .follow_links(false)
        .hidden(cfg.hidden)
        .git_ignore(cfg.git_ignore)
        .git_global(cfg.git_global)
        .git_exclude(cfg.git_exclude)
        .ignore(cfg.ignore)
        .parents(cfg.parents)
        .filter_entry(move |entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if crate::search::io::SKIP_DIRS.contains(&name) {
                        return artifact.enabled()
                            && crate::search::io::ARTIFACT_DIRS.contains(&name);
                    }
                }
            }
            true
        });

    if let Some(pattern) = glob.filter(|p| !p.is_empty()) {
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

    Ok(builder)
}

fn choose_auto_depth(
    scope: &Path,
    cfg: &WalkConfig,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<usize, SrcwalkError> {
    let walker = map_walk_builder(scope, cfg, glob, artifact)?.build();
    let mut files = 0usize;
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_map_file(path, detect_file_type(path), artifact) {
            files += 1;
            if files > WIDE_SCOPE_FILE_THRESHOLD {
                return Ok(2);
            }
        }
    }

    Ok(DEFAULT_MAP_DEPTH)
}

fn is_map_too_large(err: &SrcwalkError) -> bool {
    matches!(
        err,
        SrcwalkError::InvalidQuery { reason, .. } if reason.contains("output too large")
    )
}

/// Build the "# Note:" header line listing which ignore sources the walker
/// honours, derived from the actual `WalkConfig` (no hardcoded copy).
fn format_walk_note(cfg: &WalkConfig, artifact: ArtifactMode) -> String {
    let mut respects: Vec<&'static str> = Vec::new();
    if cfg.git_ignore {
        respects.push(".gitignore");
    }
    if cfg.git_exclude {
        respects.push(".git/info/exclude");
    }
    if cfg.git_global {
        respects.push("core.excludesFile");
    }
    if cfg.ignore {
        respects.push(".ignore");
    }
    let scope_word = if cfg.parents {
        "+ parents"
    } else {
        "scope only"
    };

    let respects_part = if respects.is_empty() {
        "no ignore files".to_string()
    } else {
        format!("{} ({scope_word})", respects.join(", "))
    };

    let hidden_part = if cfg.hidden {
        "dotfiles excluded"
    } else {
        "dotfiles included"
    };

    let skip_part = if artifact.enabled() {
        "built-in artifact dirs included for --artifact"
    } else {
        "built-in SKIP_DIRS still apply (target, node_modules, …)"
    };

    format!(
        "# Note: respects {respects_part}; {hidden_part}; {skip_part}. Use `srcwalk <path>` to inspect an ignored file directly.\n",
    )
}

fn is_map_file(path: &Path, file_type: FileType, artifact: ArtifactMode) -> bool {
    matches!(file_type, FileType::Code(_))
        || (artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path))
}

/// Generate a source map with static local dependency relations.
/// By default files show compact token estimates; symbol names are opt-in.
///
/// The `budget` argument is retained for public API compatibility. Map output
/// now uses a fixed hard cap and does not truncate or bypass that cap.
pub fn generate(
    scope: &Path,
    depth: usize,
    _budget: Option<u64>,
    cache: &OutlineCache,
    include_symbols: bool,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let cfg = default_walk_config();
    generate_at_depth(
        scope,
        depth,
        &depth.to_string(),
        false,
        &cfg,
        cache,
        include_symbols,
        glob,
        artifact,
    )
}

pub fn generate_for_cli(
    scope: &Path,
    depth: Option<usize>,
    cache: &OutlineCache,
    include_symbols: bool,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let cfg = default_walk_config();
    let Some(depth) = depth else {
        return generate_auto_depth(scope, &cfg, cache, include_symbols, glob, artifact);
    };

    generate_at_depth(
        scope,
        depth,
        &depth.to_string(),
        false,
        &cfg,
        cache,
        include_symbols,
        glob,
        artifact,
    )
}

fn generate_auto_depth(
    scope: &Path,
    cfg: &WalkConfig,
    cache: &OutlineCache,
    include_symbols: bool,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let initial_depth = choose_auto_depth(scope, cfg, glob, artifact)?;
    let mut last_err = None;
    for depth in (1..=initial_depth).rev() {
        let result = generate_at_depth(
            scope,
            depth,
            &format!("auto→{depth}"),
            depth < initial_depth,
            cfg,
            cache,
            include_symbols,
            glob,
            artifact,
        );
        match result {
            Ok(out) => return Ok(out),
            Err(err) if is_map_too_large(&err) => last_err = Some(err),
            Err(err) => return Err(err),
        }
    }

    Err(last_err.unwrap_or_else(|| SrcwalkError::InvalidQuery {
        query: "map".to_string(),
        reason: "output too large".to_string(),
    }))
}

fn generate_at_depth(
    scope: &Path,
    depth: usize,
    depth_label: &str,
    depth_reduced: bool,
    cfg: &WalkConfig,
    cache: &OutlineCache,
    include_symbols: bool,
    glob: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let mut tree: BTreeMap<PathBuf, Vec<FileEntry>> = BTreeMap::new();
    let mut totals: BTreeMap<PathBuf, u64> = BTreeMap::new();
    let mut visible_files = Vec::new();
    let mut artifact_files_annotated = 0usize;

    let builder = map_walk_builder(scope, cfg, glob, artifact)?;

    let walker = builder.build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let rel = path.strip_prefix(scope).unwrap_or(path);

        let file_depth = rel.components().count().saturating_sub(1);
        let parent = rel.parent().unwrap_or(Path::new("")).to_path_buf();
        let visible_at_depth = file_depth <= depth;
        let name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let file_type = detect_file_type(path);
        if !is_map_file(path, file_type, artifact) {
            continue;
        }

        let meta = std::fs::metadata(path).ok();
        let byte_len = meta.as_ref().map_or(0, std::fs::Metadata::len);
        let tokens = estimate_tokens(byte_len);

        let symbols = if include_symbols && visible_at_depth {
            match file_type {
                FileType::Code(_) => {
                    let mtime = meta
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    let outline_str = cache.get_or_compute(path, mtime, || {
                        let content = std::fs::read_to_string(path).unwrap_or_default();
                        let buf = content.as_bytes();
                        outline::generate(path, file_type, &content, buf, true)
                    });

                    Some(extract_symbol_names(&outline_str))
                }
                _ => None,
            }
        } else {
            None
        };

        let artifact_anchors = if visible_at_depth
            && artifact.enabled()
            && artifact_files_annotated < MAX_ARTIFACT_MAP_FILES
            && crate::artifact::is_artifact_js_ts_file(path)
        {
            let anchors = artifact_map_anchors(path);
            if anchors.is_some() {
                artifact_files_annotated += 1;
            }
            anchors
        } else {
            None
        };

        add_dir_rollup(&mut tree, &mut totals, &parent, depth, tokens);

        if visible_at_depth {
            tree.entry(parent.clone()).or_default().push(FileEntry {
                name,
                symbols,
                artifact_anchors,
                tokens,
            });
            visible_files.push(path.to_path_buf());
        }
    }

    let mut base = format!(
        "# Map: {} (depth {}, sizes ~= tokens)\n",
        crate::format::display_path(scope),
        depth_label
    );
    if depth_reduced {
        base.push_str("# Note: depth reduced to fit cap.\n");
    }
    base.push_str(&format_walk_note(cfg, artifact));
    format_tree(&tree, &totals, Path::new(""), 0, &mut base);

    let relations = compute_relations(scope, depth, &visible_files);
    let outbound_relations = if relations.is_empty() {
        compute_outbound_relations(scope, depth, &visible_files)
    } else {
        Vec::new()
    };
    let mut out = base.clone();
    if !relations.is_empty() {
        format_relations(&relations, &mut out);
    } else if !outbound_relations.is_empty() {
        format_outbound_relations(&outbound_relations, &mut out);
    }
    append_map_footer(
        &mut out,
        artifact,
        include_symbols,
        relations.is_empty() && outbound_relations.is_empty() && visible_files.len() > 1,
        !outbound_relations.is_empty(),
    );
    if enforce_hard_cap(&out, scope, depth).is_ok() {
        return Ok(out);
    }

    if !relations.is_empty() {
        let mut degraded = base;
        let _ = writeln!(
            degraded,
            "\n# Note: relations omitted to fit {MAP_HARD_TOKEN_CAP} token cap; narrow --scope/--depth for relations."
        );
        append_map_footer(&mut degraded, artifact, include_symbols, false, false);
        enforce_hard_cap(&degraded, scope, depth)?;
        return Ok(degraded);
    }

    enforce_hard_cap(&out, scope, depth)?;
    Ok(out)
}

fn append_map_footer(
    out: &mut String,
    artifact: ArtifactMode,
    include_symbols: bool,
    show_no_relations_hint: bool,
    has_outbound_relations: bool,
) {
    if artifact.enabled() {
        out.push_str("> Artifact mode: JS/TS anchors, binaries skipped, AST cap 25MB.\n");
    }

    if artifact.enabled() {
        out.push_str("\n\n> Next: drill into artifact files with `srcwalk <path> --artifact`, or search anchors with `srcwalk find <name> --artifact`.\n");
    } else if show_no_relations_hint {
        out.push_str("\n\n> Next: no cross-group relations shown. Use `srcwalk deps <file>` for file-level deps, or adjust --scope/--depth.\n");
    } else if has_outbound_relations {
        // Outbound preview already includes the relevant next action.
    } else if include_symbols {
        out.push_str("\n\n> Next: narrow with --scope <dir>.\n");
    } else {
        out.push_str("\n\n> Next: add --symbols, or narrow with --scope <dir>.\n");
    }
}

fn enforce_hard_cap(out: &str, scope: &Path, depth: usize) -> Result<(), SrcwalkError> {
    let estimated = estimate_tokens(out.len() as u64);
    if estimated <= MAP_HARD_TOKEN_CAP {
        return Ok(());
    }

    Err(SrcwalkError::InvalidQuery {
        query: "map".to_string(),
        reason: format!(
            "output too large (~{estimated} tokens; hard cap {MAP_HARD_TOKEN_CAP}). \
             Narrow with `srcwalk map --scope <dir>`, lower `--depth` below {depth}, \
             or inspect specific relations with `srcwalk deps <file>`. Scope: {}",
            crate::format::display_path(scope)
        ),
    })
}

#[derive(Debug, Eq, PartialEq)]
struct RelationEntry {
    from: String,
    to: String,
    count: usize,
}

fn normalize_existing_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn compute_relations(scope: &Path, depth: usize, visible_files: &[PathBuf]) -> Vec<RelationEntry> {
    let scope = normalize_existing_path(scope.to_path_buf());
    let visible_files: Vec<PathBuf> = visible_files
        .iter()
        .cloned()
        .map(normalize_existing_path)
        .collect();
    let visible: HashSet<PathBuf> = visible_files.iter().cloned().collect();
    let mut go_modules = BTreeMap::<PathBuf, Option<(String, PathBuf)>>::new();
    let mut php_autoloads = BTreeMap::<PathBuf, Vec<(String, PathBuf)>>::new();
    let relation_depth = depth.clamp(1, 2);
    let mut edges = BTreeSet::<(String, String, PathBuf, PathBuf)>::new();

    for source in &visible_files {
        let Ok(content) = std::fs::read_to_string(source) else {
            continue;
        };

        for target in imports::resolve_all_related_files_with_content(source, &content) {
            let target = normalize_existing_path(target);
            if target == *source || !visible.contains(&target) {
                continue;
            }
            add_relation_edge(scope.as_path(), relation_depth, source, &target, &mut edges);
        }

        let source_dir = source.parent().unwrap_or(scope.as_path()).to_path_buf();
        let go_module = go_modules
            .entry(source_dir.clone())
            .or_insert_with(|| find_go_module(&source_dir));
        if let Some((module_name, module_root)) = go_module.as_ref() {
            for target_dir in go_import_dirs(source, &content, module_name, module_root) {
                let target_dir = normalize_existing_path(target_dir);
                if !target_dir.starts_with(&scope)
                    || !has_visible_file_under(&target_dir, &visible_files)
                {
                    continue;
                }
                let relation_base = if module_root.starts_with(&scope) {
                    module_root.as_path()
                } else {
                    scope.as_path()
                };
                add_relation_edge_with_base(
                    scope.as_path(),
                    relation_base,
                    relation_depth,
                    source,
                    &target_dir,
                    &mut edges,
                );
            }
        }
        if matches!(detect_file_type(source), FileType::Code(Lang::Php)) {
            let source_dir = source.parent().unwrap_or(scope.as_path()).to_path_buf();
            let php_autoload = php_autoloads
                .entry(source_dir.clone())
                .or_insert_with(|| find_php_psr4_autoload(&source_dir));
            for target in php_import_paths(&content, php_autoload) {
                let target = normalize_existing_path(target);
                if target == *source || !visible.contains(&target) {
                    continue;
                }
                add_relation_edge(scope.as_path(), relation_depth, source, &target, &mut edges);
            }
        }
    }

    relation_entries_from_edges(edges)
}

fn compute_outbound_relations(
    scope: &Path,
    depth: usize,
    visible_files: &[PathBuf],
) -> Vec<RelationEntry> {
    let scope = normalize_existing_path(scope.to_path_buf());
    let visible_files: Vec<PathBuf> = visible_files
        .iter()
        .cloned()
        .map(normalize_existing_path)
        .collect();
    let visible: HashSet<PathBuf> = visible_files.iter().cloned().collect();
    let mut go_modules = BTreeMap::<PathBuf, Option<(String, PathBuf)>>::new();
    let outbound_depth = depth.clamp(1, 2);
    let mut edges = BTreeSet::<(String, String, PathBuf, PathBuf)>::new();

    for source in &visible_files {
        let Ok(content) = std::fs::read_to_string(source) else {
            continue;
        };

        for target in imports::resolve_all_related_files_with_content(source, &content) {
            let target = normalize_existing_path(target);
            if target == *source || visible.contains(&target) || target.starts_with(&scope) {
                continue;
            }
            let relation_base = outbound_relation_base(scope.as_path(), source, &target);
            add_relation_edge_with_base(
                scope.as_path(),
                &relation_base,
                outbound_depth,
                source,
                &target,
                &mut edges,
            );
        }

        let source_dir = source.parent().unwrap_or(scope.as_path()).to_path_buf();
        let go_module = go_modules
            .entry(source_dir.clone())
            .or_insert_with(|| find_go_module(&source_dir));
        if let Some((module_name, module_root)) = go_module.as_ref() {
            for target_dir in go_import_dirs(source, &content, module_name, module_root) {
                let target_dir = normalize_existing_path(target_dir);
                if target_dir.starts_with(&scope) {
                    continue;
                }
                add_relation_edge_with_base(
                    scope.as_path(),
                    module_root,
                    outbound_depth,
                    source,
                    &target_dir,
                    &mut edges,
                );
            }
        }
    }

    relation_entries_from_edges(edges)
}

fn outbound_relation_base(scope: &Path, source: &Path, target: &Path) -> PathBuf {
    if let Some(root) = crate::lang::package_root(source.parent().unwrap_or(source)) {
        if source.starts_with(root) && target.starts_with(root) {
            return root.to_path_buf();
        }
    }

    if let Some(parent) = scope.parent() {
        if source.starts_with(parent) && target.starts_with(parent) {
            return parent.to_path_buf();
        }
    }

    scope.to_path_buf()
}

fn relation_entries_from_edges(
    edges: BTreeSet<(String, String, PathBuf, PathBuf)>,
) -> Vec<RelationEntry> {
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for (from, to, _, _) in edges {
        if from != to {
            *counts.entry((from, to)).or_insert(0) += 1;
        }
    }

    let mut relations: Vec<RelationEntry> = counts
        .into_iter()
        .map(|((from, to), count)| RelationEntry { from, to, count })
        .collect();
    relations.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    relations
}

fn has_visible_file_under(dir: &Path, visible_files: &[PathBuf]) -> bool {
    visible_files.iter().any(|file| file.starts_with(dir))
}

fn add_relation_edge(
    scope: &Path,
    relation_depth: usize,
    source: &Path,
    target: &Path,
    edges: &mut BTreeSet<(String, String, PathBuf, PathBuf)>,
) {
    add_relation_edge_with_base(scope, scope, relation_depth, source, target, edges);
}

fn add_relation_edge_with_base(
    scope: &Path,
    relation_base: &Path,
    relation_depth: usize,
    source: &Path,
    target: &Path,
    edges: &mut BTreeSet<(String, String, PathBuf, PathBuf)>,
) {
    let from = relation_key(scope, relation_base, source, relation_depth);
    let to = relation_key(scope, relation_base, target, relation_depth);
    if from != to {
        edges.insert((from, to, source.to_path_buf(), target.to_path_buf()));
    }
}

fn relation_key(scope: &Path, relation_base: &Path, path: &Path, relation_depth: usize) -> String {
    let mut dirs = relation_dirs(relation_base, path);
    let mut prefix: Vec<String> = if relation_base != scope && relation_base.starts_with(scope) {
        relation_base
            .strip_prefix(scope)
            .unwrap_or(relation_base)
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect()
    } else {
        Vec::new()
    };

    if dirs.is_empty() {
        if prefix.is_empty() {
            return "(root)".to_string();
        }
        return prefix.join("/");
    }

    dirs.truncate(relation_depth);
    prefix.extend(dirs);
    prefix.join("/")
}

fn relation_dirs(relation_base: &Path, path: &Path) -> Vec<String> {
    let rel = path.strip_prefix(relation_base).unwrap_or(path);
    let dir = if path.is_dir() {
        rel
    } else {
        rel.parent().unwrap_or(Path::new(""))
    };
    dir.components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect()
}

fn find_go_module(scope: &Path) -> Option<(String, PathBuf)> {
    for dir in scope.ancestors() {
        let go_mod = dir.join("go.mod");
        let Ok(content) = std::fs::read_to_string(&go_mod) else {
            continue;
        };
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(module) = trimmed.strip_prefix("module ") {
                return Some((module.trim().to_string(), dir.to_path_buf()));
            }
        }
    }
    None
}

fn go_import_dirs(
    source: &Path,
    content: &str,
    module_name: &str,
    module_root: &Path,
) -> Vec<PathBuf> {
    if source.extension().is_none_or(|ext| ext != "go") {
        return Vec::new();
    }

    let mut dirs = Vec::new();
    let mut in_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import (") {
            in_block = true;
            continue;
        }
        if in_block && trimmed == ")" {
            in_block = false;
            continue;
        }

        let source = if in_block {
            extract_go_import_path(trimmed)
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            extract_go_import_path(rest.trim())
        } else {
            None
        };

        let Some(import_path) = source else {
            continue;
        };
        let Some(rest) = import_path.strip_prefix(module_name) else {
            continue;
        };
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            continue;
        }
        let dir = module_root.join(rest);
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn extract_go_import_path(line: &str) -> Option<String> {
    let first = line.find(['"', '`'])?;
    let quote = line.as_bytes()[first] as char;
    let rest = &line[first + 1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn find_php_psr4_autoload(start: &Path) -> Vec<(String, PathBuf)> {
    for ancestor in start.ancestors() {
        let composer = ancestor.join("composer.json");
        let Ok(content) = std::fs::read_to_string(&composer) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };

        let mut mappings = Vec::new();
        for section in ["autoload", "autoload-dev"] {
            let Some(psr4) = json
                .get(section)
                .and_then(|v| v.get("psr-4"))
                .and_then(|v| v.as_object())
            else {
                continue;
            };

            for (prefix, dirs) in psr4 {
                if let Some(dir) = dirs.as_str() {
                    mappings.push((prefix.clone(), ancestor.join(dir)));
                } else if let Some(items) = dirs.as_array() {
                    for dir in items.iter().filter_map(|v| v.as_str()) {
                        mappings.push((prefix.clone(), ancestor.join(dir)));
                    }
                }
            }
        }

        if !mappings.is_empty() {
            return mappings;
        }
    }

    Vec::new()
}

fn php_import_paths(content: &str, mappings: &[(String, PathBuf)]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("use ") else {
            continue;
        };
        let imported = rest
            .split([';', '{'])
            .next()
            .unwrap_or(rest)
            .split(" as ")
            .next()
            .unwrap_or(rest)
            .trim();
        if imported.is_empty() {
            continue;
        }

        for (prefix, base) in mappings {
            let Some(suffix) = imported.strip_prefix(prefix) else {
                continue;
            };
            let relative = suffix.replace('\\', "/");
            let candidate = base.join(relative).with_extension("php");
            if candidate.exists() && !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
    }

    targets
}

fn format_relations(relations: &[RelationEntry], out: &mut String) {
    let group_word = if relations.len() == 1 {
        "group"
    } else {
        "groups"
    };
    let _ = writeln!(out, "\n[relations] {} {}", relations.len(), group_word);
    format_relation_blocks(relations.iter(), out);
    out.push_str("> Relations: static local deps; not runtime calls.\n");
}
fn format_relation_blocks<'a>(
    relations: impl IntoIterator<Item = &'a RelationEntry>,
    out: &mut String,
) {
    let mut by_source = BTreeMap::<&str, Vec<&RelationEntry>>::new();
    for relation in relations {
        by_source
            .entry(relation.from.as_str())
            .or_default()
            .push(relation);
    }

    let mut by_source: Vec<(&str, Vec<&RelationEntry>)> = by_source.into_iter().collect();
    by_source.sort_by(|(source_a, relations_a), (source_b, relations_b)| {
        let count_a: usize = relations_a.iter().map(|relation| relation.count).sum();
        let count_b: usize = relations_b.iter().map(|relation| relation.count).sum();
        count_b.cmp(&count_a).then_with(|| source_a.cmp(source_b))
    });

    for (source, source_relations) in by_source {
        let source_count: usize = source_relations.iter().map(|relation| relation.count).sum();
        let _ = writeln!(out, "{source} deps:{source_count}");
        for relation in source_relations {
            let _ = writeln!(out, "  -> {} deps:{}", relation.to, relation.count);
        }
    }
}
fn format_outbound_relations(relations: &[RelationEntry], out: &mut String) {
    let group_word = if relations.len() == 1 {
        "group"
    } else {
        "groups"
    };
    let _ = writeln!(out, "\n[relations] 0 in-scope groups\n");
    let _ = writeln!(
        out,
        "[outbound deps] {} {} (targets outside scope)",
        relations.len(),
        group_word
    );
    format_relation_blocks(relations.iter().take(MAX_OUTBOUND_RELATION_GROUPS), out);

    if relations.len() > MAX_OUTBOUND_RELATION_GROUPS {
        let _ = writeln!(
            out,
            "> Outbound: showing top {} of {} groups; deps point outside --scope. Use `srcwalk deps <file>` for details, or widen --scope to include targets.",
            MAX_OUTBOUND_RELATION_GROUPS,
            relations.len()
        );
    } else {
        out.push_str("> Outbound: deps point outside --scope. Use `srcwalk deps <file>` for details, or widen --scope to include targets.\n");
    }
}

/// Add a file's tokens to rendered directory rollups up to the requested depth.
fn add_dir_rollup(
    tree: &mut BTreeMap<PathBuf, Vec<FileEntry>>,
    totals: &mut BTreeMap<PathBuf, u64>,
    parent: &Path,
    depth: usize,
    tokens: u64,
) {
    *totals.entry(PathBuf::new()).or_insert(0) += tokens;

    let max_dirs = parent.components().count().min(depth);
    let mut dir = PathBuf::new();
    for component in parent.components().take(max_dirs) {
        dir.push(component.as_os_str());
        tree.entry(dir.clone()).or_default();
        *totals.entry(dir.clone()).or_insert(0) += tokens;
    }
}

struct FileEntry {
    name: String,
    symbols: Option<Vec<String>>,
    artifact_anchors: Option<ArtifactMapAnchors>,
    tokens: u64,
}

struct ArtifactMapAnchors {
    anchors: Vec<String>,
    omitted: usize,
}

fn artifact_map_anchors(path: &Path) -> Option<ArtifactMapAnchors> {
    let content = std::fs::read_to_string(path).ok()?;
    let (anchors, omitted) =
        crate::artifact::capped_anchors(&content, MAX_ARTIFACT_MAP_ANCHORS_PER_FILE);
    if anchors.is_empty() {
        return None;
    }
    Some(ArtifactMapAnchors {
        anchors: anchors
            .into_iter()
            .map(|anchor| format!("{} {}", anchor.kind, anchor.name))
            .collect(),
        omitted,
    })
}

/// Extract symbol names from an outline string.
/// Outline lines look like: `[7-57]       fn classify`
/// We extract the last word(s) after the kind keyword.
fn extract_symbol_names(outline: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in outline.lines() {
        let trimmed = line.trim();
        // Skip import lines and empty lines
        if trimmed.starts_with('[') {
            // Find the symbol name after kind keywords
            if let Some(sig_start) = find_symbol_start(trimmed) {
                let sig = &trimmed[sig_start..];
                // Take just the name (up to first paren or space after name)
                let name = extract_name_from_sig(sig);
                if !name.is_empty() && name != "imports" {
                    names.push(name);
                }
            }
        }
    }
    names
}

fn find_symbol_start(line: &str) -> Option<usize> {
    let kinds = [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "mod ",
        "class ",
        "interface ",
        "type ",
        "const ",
        "static ",
        "function ",
        "method ",
        "def ",
    ];
    for kind in &kinds {
        if let Some(pos) = line.find(kind) {
            return Some(pos + kind.len());
        }
    }
    None
}

fn extract_name_from_sig(sig: &str) -> String {
    // Take characters until we hit a non-identifier char
    sig.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect()
}

fn format_tree(
    tree: &BTreeMap<PathBuf, Vec<FileEntry>>,
    totals: &BTreeMap<PathBuf, u64>,
    dir: &Path,
    indent: usize,
    out: &mut String,
) {
    // Show directories first, largest first, so truncated maps keep the
    // highest-signal navigation scaffold near the top.
    let mut subdirs: Vec<&PathBuf> = tree
        .keys()
        .filter(|k| k.parent() == Some(dir) && *k != dir)
        .collect();
    subdirs.sort_by(|a, b| {
        let a_total = totals.get(*a).copied().unwrap_or(0);
        let b_total = totals.get(*b).copied().unwrap_or(0);
        b_total.cmp(&a_total).then_with(|| a.cmp(b))
    });

    let prefix = "  ".repeat(indent);

    for subdir in subdirs {
        let dir_name = subdir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let total = totals.get(subdir).copied().unwrap_or(0);
        let _ = writeln!(out, "{prefix}{dir_name}/  ~{}", fmt_tokens(total));
        format_tree(tree, totals, subdir, indent + 1, out);
    }

    if let Some(files) = tree.get(dir) {
        let mut files: Vec<&FileEntry> = files.iter().collect();
        files.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));

        for f in files {
            if let Some(ref symbols) = f.symbols {
                if symbols.is_empty() {
                    let _ = writeln!(out, "{prefix}{}  ~{}", f.name, fmt_tokens(f.tokens));
                } else {
                    let syms = symbols.join(", ");
                    let truncated = if syms.len() > 80 {
                        format!("{}...", crate::types::truncate_str(&syms, 77))
                    } else {
                        syms
                    };
                    let _ = writeln!(out, "{prefix}{}: {truncated}", f.name);
                }
            } else {
                let _ = writeln!(out, "{prefix}{}  ~{}", f.name, fmt_tokens(f.tokens));
            }
            if let Some(ref artifact_anchors) = f.artifact_anchors {
                let anchors = artifact_anchors.anchors.join(", ");
                let suffix = if artifact_anchors.omitted > 0 {
                    format!(", ... +{}", artifact_anchors.omitted)
                } else {
                    String::new()
                };
                let _ = writeln!(out, "{prefix}  anchors: {anchors}{suffix}");
            }
        }
    }
}

/// Compact token count for directory rollups (e.g. "12.3k", "1.2M").
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
