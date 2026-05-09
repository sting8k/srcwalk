use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::SrcwalkError;
use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::lang::treesitter::{
    definition_weight, elixir_definition_weight, extract_base_list_targets,
    extract_definition_name, extract_elixir_definition_name, extract_impl_trait, extract_impl_type,
    extract_implemented_interfaces, is_elixir_definition, DEFINITION_KINDS,
};
use crate::types::{FileType, Match, OutlineKind};
use crate::ArtifactMode;

use super::super::{file_metadata, read_file_bytes, walker};
use super::{MAX_ARTIFACT_DEFINITION_DEPTH, MAX_ARTIFACT_FILE_SIZE, MAX_DEFINITION_DEPTH};

pub(super) fn outline_def_weight(kind: OutlineKind) -> u16 {
    match kind {
        OutlineKind::Class | OutlineKind::Struct | OutlineKind::Interface | OutlineKind::Enum => {
            100
        }
        OutlineKind::Function => 90,
        OutlineKind::TypeAlias => 80,
        OutlineKind::Constant | OutlineKind::Variable | OutlineKind::ImmutableVariable => 60,
        _ => 40,
    }
}

/// Find definitions using tree-sitter structural detection.
/// For each file containing the query string, parse with tree-sitter and walk
/// definition nodes to see if any declare the queried symbol.
/// Falls back to keyword heuristic for files without grammars.
///
/// Single-read design: reads each file once, checks for symbol via
/// `memchr::memmem` (SIMD), then reuses the buffer for tree-sitter parsing.
/// Early termination: quits the parallel walker once enough defs are found.
pub(super) fn find_definitions_with_artifact(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
    artifact: ArtifactMode,
) -> Result<Vec<Match>, SrcwalkError> {
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    // Relaxed is correct: walker.run() joins all threads before we read the final value.
    // Early-quit checks are approximate by design — one extra iteration is harmless.
    let found_count = AtomicUsize::new(0);
    let needle = query.as_bytes();

    let walker = if artifact.enabled() {
        super::super::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        walker(scope, glob)?
    };

    walker.run(|| {
        let matches = &matches;
        let found_count = &found_count;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            let is_artifact = artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path);
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }

            // Skip oversized source files, but allow explicit artifact-mode JS/TS
            // bundles up to the artifact AST cap.
            let file_size = match std::fs::metadata(path) {
                Ok(meta) => {
                    let max_size = if is_artifact {
                        MAX_ARTIFACT_FILE_SIZE
                    } else {
                        500_000
                    };
                    if meta.len() > max_size {
                        return ignore::WalkState::Continue;
                    }
                    meta.len()
                }
                Err(_) => return ignore::WalkState::Continue,
            };

            // Skip minified/bundled assets by filename unless explicitly in artifact mode.
            if super::super::io::is_minified_filename(path) && !is_artifact {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap (or heap-read for tiny files) +
            // memchr SIMD search. Skips UTF-8 validation on ~90% of files
            // that don't contain the symbol.
            let Some(bytes) = read_file_bytes(path, file_size) else {
                return ignore::WalkState::Continue;
            };

            if memchr::memmem::find(&bytes, needle).is_none() {
                return ignore::WalkState::Continue;
            }

            // Content-based minified detection for large files that slipped
            // through filename check (e.g. `app.js` actually minified).
            if !is_artifact
                && file_size >= super::super::io::MINIFIED_CHECK_THRESHOLD
                && super::super::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }

            // Hit: validate UTF-8 only now (matched files are <10% in typical search)
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            // Get file metadata once per file
            let (file_lines, mtime) = file_metadata(path);

            // Try tree-sitter structural detection
            let file_type = detect_file_type(path);
            let lang = match file_type {
                FileType::Code(l) => Some(l),
                _ => None,
            };

            let ts_language = lang.and_then(outline_language);

            let mut file_defs = if let Some(ref ts_lang) = ts_language {
                find_defs_treesitter_with_depth(
                    path,
                    query,
                    ts_lang,
                    lang,
                    content,
                    file_lines,
                    mtime,
                    cache,
                    if is_artifact {
                        MAX_ARTIFACT_DEFINITION_DEPTH
                    } else {
                        MAX_DEFINITION_DEPTH
                    },
                )
            } else {
                Vec::new()
            };

            if is_artifact {
                file_defs.extend(find_artifact_anchor_defs(
                    path, query, content, file_lines, mtime,
                ));
            }

            // Fallback: keyword heuristic for files without grammars
            if file_defs.is_empty() && ts_language.is_none() {
                file_defs = find_defs_heuristic_buf(path, query, content, file_lines, mtime);
            }

            if !file_defs.is_empty() {
                found_count.fetch_add(file_defs.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_defs);
            }

            ignore::WalkState::Continue
        })
    });

    Ok(matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Tree-sitter structural definition detection.
/// Accepts pre-read content — no redundant file read.
pub(super) fn find_defs_treesitter(
    path: &Path,
    query: &str,
    ts_lang: &tree_sitter::Language,
    lang: Option<crate::types::Lang>,
    content: &str,
    file_lines: u32,
    mtime: SystemTime,
    cache: Option<&crate::cache::OutlineCache>,
) -> Vec<Match> {
    find_defs_treesitter_with_depth(
        path,
        query,
        ts_lang,
        lang,
        content,
        file_lines,
        mtime,
        cache,
        MAX_DEFINITION_DEPTH,
    )
}

fn find_defs_treesitter_with_depth(
    path: &Path,
    query: &str,
    ts_lang: &tree_sitter::Language,
    lang: Option<crate::types::Lang>,
    content: &str,
    file_lines: u32,
    mtime: SystemTime,
    cache: Option<&crate::cache::OutlineCache>,
    max_depth: usize,
) -> Vec<Match> {
    let tree = if let Some(c) = cache {
        let Some(tree) = c.get_or_parse(path, mtime, content, ts_lang) else {
            return Vec::new();
        };
        tree
    } else {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(ts_lang).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(content, None) else {
            return Vec::new();
        };
        tree
    };

    let lines: Vec<&str> = content.lines().collect();
    let root = tree.root_node();
    let mut defs = Vec::new();

    walk_for_definitions(
        root, query, path, &lines, file_lines, mtime, &mut defs, lang, 0, max_depth,
    );

    defs
}

/// Recursively walk AST nodes looking for definitions of the queried symbol.
fn walk_for_definitions(
    node: tree_sitter::Node,
    query: &str,
    path: &Path,
    lines: &[&str],
    file_lines: u32,
    mtime: SystemTime,
    defs: &mut Vec<Match>,
    lang: Option<crate::types::Lang>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }

    let kind = node.kind();

    if DEFINITION_KINDS.contains(&kind) {
        // Check if this node defines the queried symbol
        if let Some(name) = extract_definition_name(node, lines) {
            if name == query {
                let line_num = node.start_position().row as u32 + 1;
                let line_text = lines
                    .get(node.start_position().row)
                    .unwrap_or(&"")
                    .trim_end();
                defs.push(Match {
                    path: path.to_path_buf(),
                    line: line_num,
                    text: line_text.to_string(),
                    is_definition: true,
                    exact: true,
                    file_lines,
                    mtime,
                    def_range: Some((
                        node.start_position().row as u32 + 1,
                        node.end_position().row as u32 + 1,
                    )),
                    def_name: Some(query.to_string()),
                    def_weight: definition_weight(node.kind()),
                    impl_target: None,
                    base_target: None,
                    in_comment: false,
                });
            }
        }

        // Impl/interface detection: surface `impl Trait for Type` and
        // `class X implements Interface` blocks when searching for the trait/interface.
        if kind == "impl_item" {
            if let Some(trait_name) = extract_impl_trait(node, lines) {
                if trait_name == query {
                    let impl_type =
                        extract_impl_type(node, lines).unwrap_or_else(|| "<unknown>".to_string());
                    let line_num = node.start_position().row as u32 + 1;
                    let line_text = lines
                        .get(node.start_position().row)
                        .unwrap_or(&"")
                        .trim_end();
                    defs.push(Match {
                        path: path.to_path_buf(),
                        line: line_num,
                        text: line_text.to_string(),
                        is_definition: true,
                        exact: true,
                        file_lines,
                        mtime,
                        def_range: Some((
                            node.start_position().row as u32 + 1,
                            node.end_position().row as u32 + 1,
                        )),
                        def_name: Some(format!("impl {query} for {impl_type}")),
                        def_weight: 80,
                        impl_target: Some(query.to_string()),
                        base_target: None,
                        in_comment: false,
                    });
                }
            }
        } else if kind == "class_declaration" || kind == "class_definition" {
            let class_name =
                extract_definition_name(node, lines).unwrap_or_else(|| "<anonymous>".to_string());
            let line_num = node.start_position().row as u32 + 1;
            let line_text = lines
                .get(node.start_position().row)
                .unwrap_or(&"")
                .trim_end();

            let interfaces = extract_implemented_interfaces(node, lines);
            if interfaces.iter().any(|i| i == query) {
                defs.push(Match {
                    path: path.to_path_buf(),
                    line: line_num,
                    text: line_text.to_string(),
                    is_definition: true,
                    exact: true,
                    file_lines,
                    mtime,
                    def_range: Some((
                        node.start_position().row as u32 + 1,
                        node.end_position().row as u32 + 1,
                    )),
                    def_name: Some(format!("{class_name} implements {query}")),
                    def_weight: 80,
                    impl_target: Some(query.to_string()),
                    base_target: None,
                    in_comment: false,
                });
            }

            let base_targets = extract_base_list_targets(node, lines);
            if base_targets.iter().any(|i| i == query) {
                defs.push(Match {
                    path: path.to_path_buf(),
                    line: line_num,
                    text: line_text.to_string(),
                    is_definition: true,
                    exact: true,
                    file_lines,
                    mtime,
                    def_range: Some((
                        node.start_position().row as u32 + 1,
                        node.end_position().row as u32 + 1,
                    )),
                    def_name: Some(format!("{class_name} : {query}")),
                    def_weight: 70,
                    impl_target: None,
                    base_target: Some(query.to_string()),
                    in_comment: false,
                });
            }
        }
    } else if lang == Some(crate::types::Lang::Elixir) && is_elixir_definition(node, lines) {
        // Elixir: definitions are `call` nodes — check separately
        if let Some(name) = extract_elixir_definition_name(node, lines) {
            if name == query {
                let line_num = node.start_position().row as u32 + 1;
                let line_text = lines
                    .get(node.start_position().row)
                    .unwrap_or(&"")
                    .trim_end();
                defs.push(Match {
                    path: path.to_path_buf(),
                    line: line_num,
                    text: line_text.to_string(),
                    is_definition: true,
                    exact: true,
                    file_lines,
                    mtime,
                    def_range: Some((
                        node.start_position().row as u32 + 1,
                        node.end_position().row as u32 + 1,
                    )),
                    def_name: Some(query.to_string()),
                    def_weight: elixir_definition_weight(node, lines),
                    impl_target: None,
                    base_target: None,
                    in_comment: false,
                });
            }
        }
    }

    // Recurse into children (for nested definitions, class bodies, impl blocks, etc.)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_for_definitions(
            child,
            query,
            path,
            lines,
            file_lines,
            mtime,
            defs,
            lang,
            depth + 1,
            max_depth,
        );
    }
}

/// Keyword heuristic fallback for files without tree-sitter grammars.
/// Operates on pre-read buffer — no redundant file read.
pub(super) fn find_defs_heuristic_buf(
    path: &Path,
    query: &str,
    content: &str,
    file_lines: u32,
    mtime: SystemTime,
) -> Vec<Match> {
    let mut defs = Vec::new();

    for (i, line) in content.lines().enumerate() {
        if line.contains(query) && is_definition_line(line) {
            defs.push(Match {
                path: path.to_path_buf(),
                line: (i + 1) as u32,
                text: line.trim_end().to_string(),
                is_definition: true,
                exact: true,
                file_lines,
                mtime,
                def_range: None,
                def_name: Some(query.to_string()),
                def_weight: 60,
                impl_target: None,
                base_target: None,
                in_comment: false,
            });
        }
    }

    defs
}

pub(super) fn find_artifact_anchor_defs(
    path: &Path,
    query: &str,
    content: &str,
    file_lines: u32,
    mtime: SystemTime,
) -> Vec<Match> {
    crate::artifact::search_anchor_matches(content, query)
        .into_iter()
        .map(|anchor| Match {
            path: path.to_path_buf(),
            line: anchor.line,
            text: format!("artifact anchor {} {}", anchor.kind, anchor.name),
            is_definition: true,
            exact: anchor.name == query || format!("{} {}", anchor.kind, anchor.name) == query,
            file_lines,
            mtime,
            def_range: None,
            def_name: Some(format!("{} {}", anchor.kind, anchor.name)),
            def_weight: 95,
            impl_target: None,
            base_target: None,
            in_comment: false,
        })
        .collect()
}

/// Keyword heuristic fallback — only used when tree-sitter grammar unavailable.
fn is_definition_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub(crate) fn ")
        || trimmed.starts_with("async fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("export function ")
        || trimmed.starts_with("export default function ")
        || trimmed.starts_with("export async function ")
        || trimmed.starts_with("async function ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("export const ")
        || trimmed.starts_with("let ")
        || trimmed.starts_with("export let ")
        || trimmed.starts_with("var ")
        || trimmed.starts_with("export var ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("export class ")
        || trimmed.starts_with("interface ")
        || trimmed.starts_with("export interface ")
        || trimmed.starts_with("type ")
        || trimmed.starts_with("export type ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("pub struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("pub enum ")
        || trimmed.starts_with("trait ")
        || trimmed.starts_with("pub trait ")
        || trimmed.starts_with("impl ")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("async def ")
        || trimmed.starts_with("func ")
}
