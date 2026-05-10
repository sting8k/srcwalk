use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use streaming_iterator::StreamingIterator;

use crate::lang::treesitter::{
    extract_definition_name, is_js_function_expression_kind, js_function_context_name,
    DEFINITION_KINDS,
};

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format::rel_nonempty;
use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::session::Session;
use crate::types::FileType;
use crate::ArtifactMode;

/// Default display limit when caller does not specify one.
/// Max unique caller functions to trace for 2nd hop. Above this = wide fan-out, skip.
const IMPACT_FANOUT_THRESHOLD: usize = 10;
/// Max 2nd-hop results to display.
const IMPACT_MAX_RESULTS: usize = 15;
/// Early quit for batch caller search.
const BATCH_EARLY_QUIT: usize = 50;
const MAX_ARTIFACT_FILE_SIZE: u64 = 25_000_000;

/// Top-level sentinel used when a call site is not inside a function body.
pub(super) const TOP_LEVEL: &str = "<top-level>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixKind {
    Package,
    Variable,
}

impl PrefixKind {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Package => "pkg",
            Self::Variable => "var",
        }
    }
}

/// A single caller match — a call site of a target symbol.
#[derive(Debug)]
pub struct CallerMatch {
    pub path: PathBuf,
    pub line: u32,
    pub calling_function: String,
    pub call_text: String,
    /// Line range of the calling function (for expand).
    pub caller_range: Option<(u32, u32)>,
    /// Byte range of the exact call expression, used for artifact expand windows.
    pub call_byte_range: Option<(usize, usize)>,
    /// Selector prefix before `.method()`/`.function()` (for example `sdktranslator`).
    /// `None` for bare function calls.
    pub receiver: Option<String>,
    pub prefix_kind: Option<PrefixKind>,
    /// Number of arguments at the call site.
    pub arg_count: Option<u8>,
    /// File content, already read during `find_callers` — avoids re-reading during expand.
    /// Shared across all call sites in the same file via reference counting.
    pub content: Arc<String>,
}

/// Find all call sites of a target symbol across the codebase using tree-sitter.
pub fn find_callers(
    target: &str,
    scope: &Path,
    bloom: &crate::index::bloom::BloomFilterCache,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
) -> Result<Vec<CallerMatch>, SrcwalkError> {
    find_callers_with_artifact(target, scope, bloom, glob, cache, ArtifactMode::Source)
}

pub fn find_callers_with_artifact(
    target: &str,
    scope: &Path,
    bloom: &crate::index::bloom::BloomFilterCache,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
    artifact: ArtifactMode,
) -> Result<Vec<CallerMatch>, SrcwalkError> {
    let matches: Mutex<Vec<CallerMatch>> = Mutex::new(Vec::new());
    let found_count = AtomicUsize::new(0);
    let needle = target.as_bytes();

    let walker = if artifact.enabled() {
        crate::search::io::walker_with_artifact_dirs(scope, glob)?
    } else {
        crate::search::walker(scope, glob)?
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

            // Single metadata call: check size and capture mtime together
            let (file_len, mtime) = match std::fs::metadata(path) {
                Ok(meta) => (
                    meta.len(),
                    meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                ),
                Err(_) => return ignore::WalkState::Continue,
            };
            let is_artifact = artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path);
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }
            let max_file_size = if is_artifact {
                MAX_ARTIFACT_FILE_SIZE
            } else {
                500_000
            };
            if file_len > max_file_size {
                return ignore::WalkState::Continue;
            }
            if crate::search::io::is_minified_filename(path) && !is_artifact {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap + memchr SIMD pre-filter.
            let Some(bytes) = crate::search::read_file_bytes(path, file_len) else {
                return ignore::WalkState::Continue;
            };

            if memchr::memmem::find(&bytes, needle).is_none() {
                return ignore::WalkState::Continue;
            }

            if !is_artifact
                && file_len >= crate::search::io::MINIFIED_CHECK_THRESHOLD
                && crate::search::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }
            // Hit: validate UTF-8 only now.
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            let file_type = detect_file_type(path);

            if !is_artifact && !bloom.contains(path, mtime, content, target) {
                return ignore::WalkState::Continue;
            }

            let FileType::Code(lang) = file_type else {
                return ignore::WalkState::Continue;
            };

            let Some(ts_lang) = outline_language(lang) else {
                return ignore::WalkState::Continue;
            };

            let file_callers =
                find_callers_treesitter(path, target, &ts_lang, content, lang, mtime, cache);

            if !file_callers.is_empty() {
                found_count.fetch_add(file_callers.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_callers);
            }

            ignore::WalkState::Continue
        })
    });

    Ok(matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Tree-sitter call site detection.
fn find_callers_treesitter(
    path: &Path,
    target: &str,
    ts_lang: &tree_sitter::Language,
    content: &str,
    lang: crate::types::Lang,
    mtime: std::time::SystemTime,
    cache: Option<&crate::cache::OutlineCache>,
) -> Vec<CallerMatch> {
    // Get the query string for this language
    let Some(query_str) = crate::search::callees::callee_query_str(lang) else {
        return Vec::new();
    };

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

    let content_bytes = content.as_bytes();
    let lines: Vec<&str> = content.lines().collect();

    // One Arc per file — all call sites share the same allocation.
    let shared_content: Arc<String> = Arc::new(content.to_string());

    let Some(callers) = crate::search::callees::with_callee_query(ts_lang, query_str, |query| {
        let Some(callee_idx) = query.capture_index_for_name("callee") else {
            return Vec::new();
        };

        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content_bytes);
        let mut callers = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != callee_idx {
                    continue;
                }

                // Check if the captured text matches our target symbol
                let Ok(text) = cap.node.utf8_text(content_bytes) else {
                    continue;
                };

                if text != target {
                    continue;
                }

                // Found a call site! Now walk up to find the calling function
                let line = cap.node.start_position().row as u32 + 1;

                // Get the call text (the whole call expression, not just the callee)
                let call_node = find_call_expression_node(cap.node);
                let same_line = call_node.start_position().row == call_node.end_position().row;
                let call_text: String = if same_line {
                    let row = call_node.start_position().row;
                    if row < lines.len() {
                        lines[row].trim().to_string()
                    } else {
                        text.to_string()
                    }
                } else {
                    text.to_string()
                };

                // Extract selector prefix: walk up from callee to find `obj.method()` pattern.
                // The callee node is the method name; its parent may be a field_expression
                // (Rust), member_expression (JS/TS), or similar with an `object` field.
                let receiver = extract_receiver(cap.node, content_bytes);
                let prefix_kind = receiver
                    .as_deref()
                    .map(|prefix| classify_prefix_kind(prefix, lang, content));
                // Extract arg count from the call expression's arguments node.
                let arg_count = extract_arg_count(call_node);

                // Walk up the tree to find the enclosing function
                let (calling_function, caller_range) =
                    find_enclosing_function(cap.node, &lines, lang);

                callers.push(CallerMatch {
                    path: path.to_path_buf(),
                    line,
                    calling_function,
                    call_text,
                    caller_range,
                    call_byte_range: Some((call_node.start_byte(), call_node.end_byte())),
                    receiver,
                    prefix_kind,
                    arg_count,
                    content: Arc::clone(&shared_content),
                });
            }
        }

        callers
    }) else {
        return Vec::new();
    };

    callers
}

/// Find all call sites of any symbol in `targets` across the codebase using a single walk.
/// Returns tuples of (`target_name`, match) so callers know which symbol was matched.
pub(crate) fn find_callers_batch(
    targets: &HashSet<String>,
    scope: &Path,
    bloom: &crate::index::bloom::BloomFilterCache,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
    early_quit: Option<usize>,
) -> Result<Vec<(String, CallerMatch)>, SrcwalkError> {
    find_callers_batch_with_artifact(
        targets,
        scope,
        bloom,
        glob,
        cache,
        early_quit,
        ArtifactMode::Source,
    )
}

pub(crate) fn find_callers_batch_with_artifact(
    targets: &HashSet<String>,
    scope: &Path,
    bloom: &crate::index::bloom::BloomFilterCache,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
    early_quit: Option<usize>,
    artifact: ArtifactMode,
) -> Result<Vec<(String, CallerMatch)>, SrcwalkError> {
    let matches: Mutex<Vec<(String, CallerMatch)>> = Mutex::new(Vec::new());
    let found_count = AtomicUsize::new(0);

    // Build Aho-Corasick automaton once for all targets — single-pass multi-pattern
    // search. Faster than N independent memchr calls when targets.len() >= 3.
    // For 1-2 targets, use length-sorted memchr (still beats unsorted).
    let target_vec: Vec<&str> = targets.iter().map(String::as_str).collect();
    let ac = if target_vec.len() >= 3 {
        aho_corasick::AhoCorasick::new(&target_vec).ok()
    } else {
        None
    };
    // Sort fallback memchr targets longest-first: rare/specific names give
    // quick misses on most files; common short names match too aggressively.
    let mut sorted_targets: Vec<&str> = target_vec.clone();
    sorted_targets.sort_by_key(|t| std::cmp::Reverse(t.len()));

    let walker = crate::search::walker(scope, glob)?;

    walker.run(|| {
        let matches = &matches;
        let found_count = &found_count;
        let ac = ac.as_ref();
        let sorted_targets = &sorted_targets;

        Box::new(move |entry| {
            // Early termination: enough callers found (UI preview only).
            if let Some(cap) = early_quit {
                if found_count.load(Ordering::Relaxed) >= cap {
                    return ignore::WalkState::Quit;
                }
            }

            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();

            // Single metadata call: check size and capture mtime together
            let (file_len, mtime) = match std::fs::metadata(path) {
                Ok(meta) => (
                    meta.len(),
                    meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                ),
                Err(_) => return ignore::WalkState::Continue,
            };
            let is_artifact = artifact.enabled() && crate::artifact::is_artifact_js_ts_file(path);
            if artifact.enabled() && !crate::artifact::is_artifact_search_file(path) {
                return ignore::WalkState::Continue;
            }
            let max_file_size = if is_artifact {
                MAX_ARTIFACT_FILE_SIZE
            } else {
                500_000
            };
            if file_len > max_file_size {
                return ignore::WalkState::Continue;
            }
            if crate::search::io::is_minified_filename(path) && !is_artifact {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap + multi-pattern pre-filter.
            let Some(bytes) = crate::search::read_file_bytes(path, file_len) else {
                return ignore::WalkState::Continue;
            };

            let any_match = if let Some(ac) = ac {
                ac.is_match(&*bytes)
            } else {
                sorted_targets
                    .iter()
                    .any(|t| memchr::memmem::find(&bytes, t.as_bytes()).is_some())
            };
            if !any_match {
                return ignore::WalkState::Continue;
            }

            if !is_artifact
                && file_len >= crate::search::io::MINIFIED_CHECK_THRESHOLD
                && crate::search::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }
            // Hit: validate UTF-8 only now.
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            let file_type = detect_file_type(path);

            if !is_artifact
                && !targets
                    .iter()
                    .any(|t| bloom.contains(path, mtime, content, t))
            {
                return ignore::WalkState::Continue;
            }

            let FileType::Code(lang) = file_type else {
                return ignore::WalkState::Continue;
            };

            let Some(ts_lang) = outline_language(lang) else {
                return ignore::WalkState::Continue;
            };

            let file_callers =
                find_callers_treesitter_batch(path, targets, &ts_lang, content, lang, mtime, cache);

            if !file_callers.is_empty() {
                found_count.fetch_add(file_callers.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_callers);
            }

            ignore::WalkState::Continue
        })
    });

    Ok(matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Tree-sitter call site detection for a set of target symbols.
/// Returns tuples of (`matched_target_name`, `CallerMatch`).
fn find_callers_treesitter_batch(
    path: &Path,
    targets: &HashSet<String>,
    ts_lang: &tree_sitter::Language,
    content: &str,
    lang: crate::types::Lang,
    mtime: std::time::SystemTime,
    cache: Option<&crate::cache::OutlineCache>,
) -> Vec<(String, CallerMatch)> {
    // Get the query string for this language
    let Some(query_str) = crate::search::callees::callee_query_str(lang) else {
        return Vec::new();
    };

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

    let content_bytes = content.as_bytes();
    let lines: Vec<&str> = content.lines().collect();

    // One Arc per file — all call sites share the same allocation.
    let shared_content: Arc<String> = Arc::new(content.to_string());

    let Some(callers) = crate::search::callees::with_callee_query(ts_lang, query_str, |query| {
        let Some(callee_idx) = query.capture_index_for_name("callee") else {
            return Vec::new();
        };

        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content_bytes);
        let mut callers = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != callee_idx {
                    continue;
                }

                // Check if the captured text matches any of our target symbols
                let Ok(text) = cap.node.utf8_text(content_bytes) else {
                    continue;
                };

                if !targets.contains(text) {
                    continue;
                }

                let matched_target = text.to_string();

                // Found a call site! Now walk up to find the calling function
                let line = cap.node.start_position().row as u32 + 1;

                // Get the call text (the whole call expression, not just the callee)
                let call_node = find_call_expression_node(cap.node);
                let same_line = call_node.start_position().row == call_node.end_position().row;
                let call_text: String = if same_line {
                    let row = call_node.start_position().row;
                    if row < lines.len() {
                        lines[row].trim().to_string()
                    } else {
                        matched_target.clone()
                    }
                } else {
                    matched_target.clone()
                };

                // Walk up the tree to find the enclosing function
                let (calling_function, caller_range) =
                    find_enclosing_function(cap.node, &lines, lang);

                let receiver = extract_receiver(cap.node, content_bytes);
                let prefix_kind = receiver
                    .as_deref()
                    .map(|prefix| classify_prefix_kind(prefix, lang, content));
                let arg_count = extract_arg_count(call_node);

                callers.push((
                    matched_target,
                    CallerMatch {
                        path: path.to_path_buf(),
                        line,
                        calling_function,
                        call_text,
                        caller_range,
                        call_byte_range: Some((call_node.start_byte(), call_node.end_byte())),
                        receiver,
                        prefix_kind,
                        arg_count,
                        content: Arc::clone(&shared_content),
                    },
                ));
            }
        }

        callers
    }) else {
        return Vec::new();
    };

    callers
}

/// Extract receiver from a call like `obj.method()` → `Some("obj")`.
/// Returns `None` for bare calls like `method()`.
fn extract_receiver(callee_node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let parent = callee_node.parent()?;
    let kind = parent.kind();

    match kind {
        // obj.method / obj.field — Rust, JS/TS, Go, Python, C#, C/C++, PHP
        "field_expression"
        | "member_expression"
        | "selector_expression"
        | "attribute"
        | "member_access_expression"
        | "scoped_call_expression"
        | "member_call_expression" => {
            let obj = parent
                .child_by_field_name("object")
                .or_else(|| parent.child_by_field_name("receiver"))
                .or_else(|| parent.child_by_field_name("expression"));
            let obj = obj.or_else(|| {
                // Fallback: first named child that isn't the callee itself
                (0..parent.named_child_count())
                    .filter_map(|i| parent.named_child(i))
                    .find(|c| c.id() != callee_node.id())
            });
            let text = obj?.utf8_text(source).ok()?;
            Some(if text.len() > 40 {
                format!("{}…", &text[..37])
            } else {
                text.to_string()
            })
        }
        // Java: method_invocation has "object" field
        "method_invocation" => {
            let text = parent
                .child_by_field_name("object")?
                .utf8_text(source)
                .ok()?;
            Some(if text.len() > 40 {
                format!("{}…", &text[..37])
            } else {
                text.to_string()
            })
        }
        // Rust Mod::func, C++ ns::func
        "scoped_identifier" | "qualified_identifier" => {
            let mut cursor = parent.walk();
            let first = parent
                .named_children(&mut cursor)
                .find(|c| c.id() != callee_node.id())?;
            Some(first.utf8_text(source).ok()?.to_string())
        }
        // Ruby: call node has "receiver" field directly
        "call" => {
            let text = parent
                .child_by_field_name("receiver")?
                .utf8_text(source)
                .ok()?;
            Some(if text.len() > 40 {
                format!("{}…", &text[..37])
            } else {
                text.to_string()
            })
        }
        // Kotlin: navigation_expression (logger.info)
        "navigation_expression" => {
            // First named child is the object, callee is the second
            (0..parent.named_child_count())
                .filter_map(|i| parent.named_child(i))
                .find(|c| c.id() != callee_node.id())
                .and_then(|obj| {
                    let text = obj.utf8_text(source).ok()?;
                    Some(if text.len() > 40 {
                        format!("{}…", &text[..37])
                    } else {
                        text.to_string()
                    })
                })
        }
        // Swift: navigation_suffix → walk up to navigation_expression
        "navigation_suffix" => {
            let nav = parent.parent()?;
            if nav.kind() != "navigation_expression" {
                return None;
            }
            (0..nav.named_child_count())
                .filter_map(|i| nav.named_child(i))
                .find(|c| c.kind() != "navigation_suffix")
                .and_then(|obj| {
                    let text = obj.utf8_text(source).ok()?;
                    Some(if text.len() > 40 {
                        format!("{}…", &text[..37])
                    } else {
                        text.to_string()
                    })
                })
        }
        _ => None,
    }
}
fn classify_prefix_kind(prefix: &str, lang: crate::types::Lang, content: &str) -> PrefixKind {
    if lang == crate::types::Lang::Go && go_import_prefixes(content).contains(prefix) {
        PrefixKind::Package
    } else {
        PrefixKind::Variable
    }
}

fn go_import_prefixes(content: &str) -> HashSet<String> {
    content
        .lines()
        .filter_map(go_import_prefix_from_line)
        .collect()
}

fn go_import_prefix_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let quote_start = trimmed.find('"')?;
    let quote_end = trimmed[quote_start + 1..].find('"')? + quote_start + 1;
    let import_path = &trimmed[quote_start + 1..quote_end];
    let before_quote = trimmed[..quote_start].trim();
    let alias = before_quote
        .strip_prefix("import")
        .unwrap_or(before_quote)
        .split_whitespace()
        .next();

    match alias {
        Some("_" | ".") => None,
        Some(name) if !name.is_empty() => Some(name.to_string()),
        _ => import_path
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string),
    }
}

/// Count arguments at a call site.
fn find_call_expression_node(mut node: tree_sitter::Node) -> tree_sitter::Node {
    let original = node;
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "call_expression" | "new_expression" => return parent,
            "member_expression" | "field_expression" | "scoped_identifier" => {
                node = parent;
            }
            _ => break,
        }
    }
    node.parent().unwrap_or(original)
}

fn extract_arg_count(call_node: tree_sitter::Node) -> Option<u8> {
    // Try the node itself, then its parent (for languages where the callee is captured
    // inside a member_access/field_expression that is a child of the actual call node).
    for node in [Some(call_node), call_node.parent()] {
        let node = node?;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "arguments" | "argument_list" | "actual_parameters" | "method_arguments"
                | "value_arguments" | "call_suffix" => {
                    let mut arg_cursor = child.walk();
                    let count = child.named_children(&mut arg_cursor).count();
                    return Some(count.min(255) as u8);
                }
                _ => {}
            }
        }
    }
    None
}

/// Walk up the AST from a node to find the enclosing function definition.
/// Returns (`function_name`, `line_range`).
/// Type-like node kinds that can enclose a function definition.
const TYPE_KINDS: &[&str] = &[
    "class_declaration",
    "class_definition",
    "struct_item",
    "impl_item",
    "interface_declaration",
    "trait_item",
    "trait_declaration",
    "type_declaration",
    "enum_item",
    "enum_declaration",
    "module",
    "mod_item",
    "namespace_definition",
];

fn find_enclosing_function(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: crate::types::Lang,
) -> (String, Option<(u32, u32)>) {
    // Walk up the tree until we find a definition node
    let mut current = Some(node);

    while let Some(n) = current {
        let kind = n.kind();

        // Check JS/TS function expressions, standard definition kinds, or Elixir call-node definitions.
        let js_function_name = || {
            if matches!(
                lang,
                crate::types::Lang::JavaScript
                    | crate::types::Lang::TypeScript
                    | crate::types::Lang::Tsx
            ) && is_js_function_expression_kind(kind)
            {
                js_function_context_name(n, lines)
            } else {
                None
            }
        };
        let def_name = if let Some(name) = js_function_name() {
            Some(name)
        } else if DEFINITION_KINDS.contains(&kind)
            && !matches!(kind, "lexical_declaration" | "variable_declaration")
        {
            extract_definition_name(n, lines)
        } else if lang == crate::types::Lang::Elixir
            && crate::lang::treesitter::is_elixir_definition(n, lines)
        {
            crate::lang::treesitter::extract_elixir_definition_name(n, lines)
        } else {
            None
        };

        if let Some(name) = def_name {
            let range = Some((
                n.start_position().row as u32 + 1,
                n.end_position().row as u32 + 1,
            ));

            // Walk further up to find an enclosing type and qualify the name
            let mut parent = n.parent();
            while let Some(p) = parent {
                if TYPE_KINDS.contains(&p.kind()) {
                    if let Some(type_name) = extract_definition_name(p, lines) {
                        return (format!("{type_name}.{name}"), range);
                    }
                }
                // Elixir: `defmodule` is a `call` node, not in TYPE_KINDS, so it
                // needs a separate check to qualify function names as Module.func.
                if lang == crate::types::Lang::Elixir
                    && crate::lang::treesitter::is_elixir_definition(p, lines)
                {
                    if let Some(type_name) =
                        crate::lang::treesitter::extract_elixir_definition_name(p, lines)
                    {
                        return (format!("{type_name}.{name}"), range);
                    }
                }
                parent = p.parent();
            }

            return (name, range);
        }

        current = n.parent();
    }

    // No enclosing function found — top-level call
    ("<top-level>".to_string(), None)
}

/// Format and rank caller search results with optional expand.
#[allow(dead_code)]
pub fn search_callers_expanded(
    target: &str,
    scope: &Path,
    cache: &OutlineCache,
    session: &Session,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    count_by: Option<&str>,
) -> Result<String, SrcwalkError> {
    search_callers_expanded_with_artifact(
        target,
        scope,
        cache,
        session,
        bloom,
        expand,
        context,
        limit,
        offset,
        glob,
        filter,
        count_by,
        ArtifactMode::Source,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn search_callers_expanded_with_artifact(
    target: &str,
    scope: &Path,
    cache: &OutlineCache,
    _session: &Session,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    context: Option<&Path>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    count_by: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let max_matches = limit.unwrap_or(usize::MAX);
    let group_limit = limit.unwrap_or(50);
    let mut callers =
        find_callers_with_artifact(target, scope, bloom, glob, Some(cache), artifact)?;
    let filters = parse_callsite_filters(filter)?;
    let unfiltered_total = callers.len();
    if !filters.is_empty() {
        callers.retain(|caller| filters.iter().all(|f| f.matches(caller, scope)));
    }

    if callers.is_empty() {
        return Ok(format!(
            "# Callers of \"{}\" in {} — no call sites found\n\n\
             > Caveat: direct by-name search only; misses dynamic dispatch, reflection, macros.\n\
             > Next: use `srcwalk find {}` or search interface/trait/implementor names.",
            target,
            crate::format::display_path(scope),
            target,
        ));
    }

    if let Some(field) = count_by {
        return format_callsite_counts(target, scope, &callers, field, filter, group_limit, offset);
    }

    // Sort by relevance (context file first, then by proximity)
    let mut sorted_callers = callers;
    rank_callers(&mut sorted_callers, scope, context);

    let total = sorted_callers.len();

    // Collect unique caller names BEFORE pagination for accurate fan-out threshold
    let all_caller_names: HashSet<String> = sorted_callers
        .iter()
        .filter(|c| c.calling_function != "<top-level>")
        .map(|c| c.calling_function.clone())
        .collect();

    // Apply offset then limit (pagination)
    let effective_offset = offset.min(total);
    if effective_offset > 0 {
        sorted_callers.drain(..effective_offset);
    }
    sorted_callers.truncate(max_matches);
    let shown = sorted_callers.len();

    let js_ts_artifact_callers = artifact.enabled()
        && sorted_callers
            .iter()
            .any(|caller| crate::artifact::is_artifact_js_ts_file(&caller.path));
    // Format the output as semantic-compact call edges.
    let mut output = format!(
        "# Slice: {target} — {total} call site{}\n\n[symbol] {target}\n<- calls\n",
        if total == 1 { "" } else { "s" }
    );

    if js_ts_artifact_callers {
        append_artifact_callers_grouped(&mut output, &sorted_callers, scope, expand);
    } else {
        for (i, caller) in sorted_callers.iter().enumerate() {
            let caller_kind = "fn";

            let _ = write!(
                output,
                "  [{caller_kind}] {} {}:{}",
                caller.calling_function,
                rel_nonempty(&caller.path, scope),
                caller.line,
            );
            if let Some(ref prefix) = caller.receiver {
                if let Some(kind) = caller.prefix_kind {
                    let _ = write!(output, " prefix={prefix}({})", kind.suffix());
                } else {
                    let _ = write!(output, " prefix={prefix}");
                }
            }
            if let Some(argc) = caller.arg_count {
                let _ = write!(output, " args={argc}");
            }
            let _ = writeln!(output);

            // Expand only when explicitly requested and we have the range.
            if i < expand {
                if let Some((start, end)) = caller.caller_range {
                    // Use cached content — no re-read needed.
                    // Show a compact window around the callsite (±2 lines)
                    // bounded by the enclosing function range.
                    let lines: Vec<&str> = caller.content.lines().collect();
                    let window_start = caller.line.saturating_sub(2).max(start);
                    let window_end = (caller.line + 2).min(end);
                    let start_idx = (window_start as usize).saturating_sub(1);
                    let end_idx = (window_end as usize).min(lines.len());

                    output.push_str("\n```\n");

                    for (idx, line) in lines[start_idx..end_idx].iter().enumerate() {
                        let line_num = start_idx + idx + 1;
                        let prefix = if line_num == caller.line as usize {
                            "► "
                        } else {
                            "  "
                        };
                        let _ = writeln!(output, "{prefix}{line_num:4} │ {line}");
                    }

                    output.push_str("```\n");
                }
            }
        }
    }

    let mut footer = String::new();
    if total > effective_offset + shown {
        let omitted = total - effective_offset - shown;
        let next_offset = effective_offset + shown;
        let page_size = shown.max(1);
        let _ = write!(
            footer,
            "> Next: {omitted} more call sites available. Continue with --offset {next_offset} --limit {page_size}."
        );
    } else if effective_offset > 0 {
        let _ = write!(
            footer,
            "> Note: end of results at offset {effective_offset}."
        );
    }
    if !footer.is_empty() {
        footer.push('\n');
    }
    if js_ts_artifact_callers {
        footer.push_str(
            "> Next: --expand[=N] for byte-window evidence | --count-by args|path | --filter 'args:N prefix:NAME'.",
        );
    } else {
        footer.push_str(
            "> Next: <path>:<line> | --expand[=N] | --count-by args|path | --filter 'args:N prefix:NAME' | --depth N.",
        );
    }
    if artifact.enabled() {
        if let Some(note) = artifact.callers_note() {
            footer.push_str("\n> ");
            footer.push_str(note);
        }
    }
    if !filters.is_empty() {
        let _ = write!(
            footer,
            "\n> Note: filter matched {total}/{unfiltered_total} call sites. Qualifiers: args:N prefix:NAME (or receiver:NAME) caller:NAME path:TEXT text:TEXT."
        );
    }

    // ── Adaptive 2nd-hop impact analysis ──
    // Use all_caller_names (pre-truncation) for the fan-out threshold check,
    // but search for callers of the full set to capture transitive impact.
    if !artifact.enabled()
        && !all_caller_names.is_empty()
        && all_caller_names.len() <= IMPACT_FANOUT_THRESHOLD
    {
        if let Ok(hop2) = find_callers_batch(
            &all_caller_names,
            scope,
            bloom,
            glob,
            Some(cache),
            Some(BATCH_EARLY_QUIT),
        ) {
            // Filter out hop-1 matches (same file+line = same call site)
            let hop1_locations: HashSet<(PathBuf, u32)> = sorted_callers
                .iter()
                .map(|c| (c.path.clone(), c.line))
                .collect();

            let hop2_filtered: Vec<_> = hop2
                .into_iter()
                .filter(|(_, m)| !hop1_locations.contains(&(m.path.clone(), m.line)))
                .collect();

            if !hop2_filtered.is_empty() {
                output.push_str("\n── impact (2nd hop) ──\n");

                let mut seen: HashSet<(String, PathBuf)> = HashSet::new();
                let mut count = 0;
                for (via, m) in &hop2_filtered {
                    let key = (m.calling_function.clone(), m.path.clone());
                    if !seen.insert(key) {
                        continue;
                    }
                    if count >= IMPACT_MAX_RESULTS {
                        break;
                    }

                    let rel_path = rel_nonempty(&m.path, scope);
                    let _ = writeln!(
                        output,
                        "  {:<20} {}:{}  \u{2192} {}",
                        m.calling_function, rel_path, m.line, via
                    );
                    count += 1;
                }

                let unique_total = hop2_filtered
                    .iter()
                    .map(|(_, m)| (&m.calling_function, &m.path))
                    .collect::<HashSet<_>>()
                    .len();
                if unique_total > IMPACT_MAX_RESULTS {
                    let _ = writeln!(
                        output,
                        "  ... and {} more",
                        unique_total - IMPACT_MAX_RESULTS
                    );
                    if !footer.is_empty() {
                        footer.push('\n');
                    }
                    footer.push_str(
                        "> Caveat: impact list was capped. Use `srcwalk callers <symbol> --depth 2` for the full 2-hop graph.",
                    );
                }

                let _ = writeln!(
                    output,
                    "\n{} functions affected across 2 hops.",
                    sorted_callers.len() + count
                );
            }
        }
    }

    let tokens = crate::types::estimate_tokens(output.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(output, "\n\n({token_str} tokens)");
    if !footer.is_empty() {
        let _ = write!(output, "\n\n{footer}");
    }
    Ok(output)
}

fn append_artifact_callers_grouped(
    output: &mut String,
    callers: &[CallerMatch],
    scope: &Path,
    expand: usize,
) {
    let groups = artifact_caller_groups(callers);
    let mut current_path: Option<String> = None;
    for (idx, group) in groups.iter().enumerate() {
        let rel = rel_nonempty(&group.path, scope);
        if current_path.as_deref() != Some(rel.as_str()) {
            current_path = Some(rel.clone());
            let _ = writeln!(output, "  {rel}");
        }
        append_artifact_caller_group(output, group);
        if idx < expand {
            if let Some((start, end)) = group.callers[0].call_byte_range {
                output.push_str(&format_artifact_call_window(group.callers[0], start, end));
            }
        }
    }
}

struct ArtifactCallerGroup<'a> {
    path: PathBuf,
    calling_function: String,
    line: u32,
    receiver: Option<String>,
    arg_count: Option<u8>,
    callers: Vec<&'a CallerMatch>,
}

fn artifact_caller_groups(callers: &[CallerMatch]) -> Vec<ArtifactCallerGroup<'_>> {
    let mut groups: Vec<ArtifactCallerGroup<'_>> = Vec::new();
    for caller in callers {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| same_artifact_caller_group(group.callers[0], caller))
        {
            group.callers.push(caller);
        } else {
            groups.push(ArtifactCallerGroup {
                path: caller.path.clone(),
                calling_function: caller.calling_function.clone(),
                line: caller.line,
                receiver: caller.receiver.clone(),
                arg_count: caller.arg_count,
                callers: vec![caller],
            });
        }
    }
    groups
}

fn same_artifact_caller_group(a: &CallerMatch, b: &CallerMatch) -> bool {
    a.path == b.path
        && a.calling_function == b.calling_function
        && a.line == b.line
        && a.receiver == b.receiver
        && a.arg_count == b.arg_count
}

fn append_artifact_caller_group(output: &mut String, group: &ArtifactCallerGroup<'_>) {
    let _ = write!(output, "    [fn] {}:{}", group.calling_function, group.line);
    if group.callers.len() > 1 {
        let _ = write!(output, " [{} calls]", group.callers.len());
    }
    if let Some(ref receiver) = group.receiver {
        let _ = write!(output, " prefix={receiver}");
    }
    if let Some(argc) = group.arg_count {
        let _ = write!(output, " args={argc}");
    }

    let ranges: Vec<_> = group
        .callers
        .iter()
        .filter_map(|caller| caller.call_byte_range)
        .collect();
    if group.callers.len() == 1 {
        if let Some((start, end)) = ranges.first() {
            let _ = write!(output, " bytes:{start}-{end}");
        }
        let _ = writeln!(output);
        return;
    }

    let _ = writeln!(output);
    for (start, end) in ranges.iter().take(6) {
        let _ = writeln!(output, "      bytes:{start}-{end}");
    }
    if ranges.len() > 6 {
        let _ = writeln!(output, "      ... {} more byte ranges", ranges.len() - 6);
    }
}

fn format_artifact_call_window(caller: &CallerMatch, start_byte: usize, end_byte: usize) -> String {
    const CONTEXT: usize = 180;
    const MAX_WINDOW: usize = 560;

    let content = caller.content.as_str();
    if start_byte >= end_byte || start_byte >= content.len() {
        return String::new();
    }
    let end_byte = end_byte.min(content.len());
    let mut window_start = start_byte.saturating_sub(CONTEXT);
    let mut window_end = (end_byte + CONTEXT).min(content.len());
    if window_end.saturating_sub(window_start) > MAX_WINDOW {
        window_start = start_byte.saturating_sub(MAX_WINDOW / 3);
        window_end = (end_byte + MAX_WINDOW / 3).min(content.len());
    }
    window_start = floor_char_boundary(content, window_start);
    window_end = ceil_char_boundary(content, window_end);

    let prefix = if window_start > 0 { "…" } else { "" };
    let suffix = if window_end < content.len() {
        "…"
    } else {
        ""
    };
    let snippet = content[window_start..window_end].trim();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n```js\n// line {}, bytes {}-{}\n{prefix}{snippet}{suffix}\n```",
        caller.line, start_byte, end_byte
    );
    out
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(text: &str, mut idx: usize) -> usize {
    idx = idx.min(text.len());
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

fn format_callsite_counts(
    target: &str,
    scope: &Path,
    callers: &[CallerMatch],
    field: &str,
    filter: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<String, SrcwalkError> {
    let field = normalize_count_field(field)?;
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for caller in callers {
        let key = callsite_field_value(caller, scope, field);
        *counts.entry(key).or_insert(0) += 1;
    }

    let total = callers.len();
    let filter_suffix = filter.map_or(String::new(), |f| format!(" matching `{f}`"));
    let mut output = format!(
        "# Slice: {target} — {total} call site{} grouped by {field}{}\n\n[symbol] {target}\n<- calls\n",
        if total == 1 { "" } else { "s" },
        filter_suffix,
    );

    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total_groups = rows.len();
    let effective_offset = offset.min(total_groups);
    let page_size = limit.max(1);
    for (key, count) in rows.into_iter().skip(effective_offset).take(page_size) {
        let _ = writeln!(output, "  [group] {field}={key} count={count}");
    }

    let shown_end = (effective_offset + page_size).min(total_groups);
    let mut footer = String::from(
        "> Next: narrow with --filter 'args:N prefix:NAME caller:NAME path:TEXT text:TEXT'; group with --count-by args|caller|path|file|prefix.",
    );
    if total_groups > shown_end {
        let omitted = total_groups - shown_end;
        let _ = write!(
            footer,
            "\n> Next: {omitted} more groups available. Continue with --offset {shown_end} --limit {page_size}."
        );
    } else if effective_offset > 0 {
        let _ = write!(
            footer,
            "\n> Note: end of groups at offset {effective_offset}."
        );
    }
    let _ = write!(output, "\n{footer}");
    Ok(output)
}

fn normalize_count_field(field: &str) -> Result<&'static str, SrcwalkError> {
    match field {
        "args" => Ok("args"),
        "caller" => Ok("caller"),
        "receiver" | "recv" | "prefix" | "qual" => Ok("receiver"),
        "path" => Ok("path"),
        "file" => Ok("file"),
        _ => Err(SrcwalkError::InvalidQuery {
            query: field.to_string(),
            reason: "unsupported count field; use args, caller, path, file, or prefix".to_string(),
        }),
    }
}

fn callsite_field_value(caller: &CallerMatch, scope: &Path, field: &str) -> String {
    match field {
        "args" => caller
            .arg_count
            .map_or_else(|| "?".to_string(), |argc| argc.to_string()),
        "caller" => caller.calling_function.clone(),
        "receiver" => caller
            .receiver
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
        "path" => rel_nonempty(&caller.path, scope),
        "file" => caller
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string(),
        _ => "<unknown>".to_string(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CallsiteFilter {
    field: String,
    value: String,
}

fn parse_callsite_filters(filter: Option<&str>) -> Result<Vec<CallsiteFilter>, SrcwalkError> {
    let Some(filter) = filter else {
        return Ok(Vec::new());
    };
    let mut filters = Vec::new();
    for part in filter.split_whitespace() {
        let Some((field, value)) = part.split_once(':') else {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filters must use field:value qualifiers".to_string(),
            });
        };
        let field = field.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if field.is_empty() || value.is_empty() {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filter field and value cannot be empty".to_string(),
            });
        }
        match field.as_str() {
            "args" | "caller" | "path" | "file" | "text" => {
                filters.push(CallsiteFilter { field, value });
            }
            "receiver" | "recv" | "prefix" | "qual" => {
                filters.push(CallsiteFilter {
                    field: "receiver".to_string(),
                    value,
                });
            }
            _ => {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: format!(
                        "unsupported filter field `{field}`; use args, prefix, caller, path, or text"
                    ),
                });
            }
        }
    }
    Ok(filters)
}

impl CallsiteFilter {
    fn matches(&self, caller: &CallerMatch, scope: &Path) -> bool {
        match self.field.as_str() {
            "args" => caller
                .arg_count
                .is_some_and(|argc| self.value.parse::<u8>().is_ok_and(|wanted| argc == wanted)),
            "receiver" => caller.receiver.as_deref() == Some(self.value.as_str()),
            "caller" => caller.calling_function == self.value,
            "path" | "file" => rel_nonempty(&caller.path, scope).contains(&self.value),
            "text" => caller.call_text.contains(&self.value),
            _ => false,
        }
    }
}

/// Simple ranking: context file first, then actionable/context-rich callsites.
fn receiver_specificity_score(receiver: Option<&str>) -> u8 {
    let Some(receiver) = receiver.map(str::trim).filter(|r| !r.is_empty()) else {
        return 2;
    };

    let normalized = receiver.trim_matches(|c: char| c == '&' || c == '*' || c == '(' || c == ')');
    match normalized {
        "this" | "$this" | "self" | "Self" | "static" | "parent" | "super" => 1,
        _ => 0,
    }
}

fn is_duplicate_context_callsite(
    caller: &CallerMatch,
    first_line_by_context: &HashMap<(PathBuf, String), u32>,
) -> bool {
    let key = (caller.path.clone(), caller.calling_function.clone());
    first_line_by_context
        .get(&key)
        .is_some_and(|first_line| caller.line > *first_line)
}

fn rank_callers(callers: &mut [CallerMatch], scope: &Path, context: Option<&Path>) {
    let mut first_line_by_context: HashMap<(PathBuf, String), u32> = HashMap::new();
    for caller in callers.iter() {
        let key = (caller.path.clone(), caller.calling_function.clone());
        first_line_by_context
            .entry(key)
            .and_modify(|line| *line = (*line).min(caller.line))
            .or_insert(caller.line);
    }

    callers.sort_by(|a, b| {
        // Context file wins
        if let Some(ctx) = context {
            match (a.path == ctx, b.path == ctx) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
        }

        // Named caller contexts are usually more actionable than module top-level matches.
        match (
            a.calling_function == TOP_LEVEL,
            b.calling_function == TOP_LEVEL,
        ) {
            (false, true) => return std::cmp::Ordering::Less,
            (true, false) => return std::cmp::Ordering::Greater,
            _ => {}
        }

        // Show the first callsite per caller context before repeated calls in the same function.
        let a_duplicate = is_duplicate_context_callsite(a, &first_line_by_context);
        let b_duplicate = is_duplicate_context_callsite(b, &first_line_by_context);
        match (a_duplicate, b_duplicate) {
            (false, true) => return std::cmp::Ordering::Less,
            (true, false) => return std::cmp::Ordering::Greater,
            _ => {}
        }

        // Explicit receivers (e.g. $kernel->getCacheDir()) are often more disambiguating
        // than self/no receiver for common method names. This only reranks; it never filters.
        let a_receiver_score = receiver_specificity_score(a.receiver.as_deref());
        let b_receiver_score = receiver_specificity_score(b.receiver.as_deref());
        match a_receiver_score.cmp(&b_receiver_score) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }

        // Shorter paths (more similar to scope) rank higher
        let a_rel = a.path.strip_prefix(scope).unwrap_or(&a.path);
        let b_rel = b.path.strip_prefix(scope).unwrap_or(&b.path);
        a_rel
            .components()
            .count()
            .cmp(&b_rel.components().count())
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
}

#[cfg(test)]
mod callsite_filter_tests;
