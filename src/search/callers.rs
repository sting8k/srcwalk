use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use streaming_iterator::StreamingIterator;

use crate::lang::treesitter::{extract_definition_name, DEFINITION_KINDS};

use crate::cache::OutlineCache;
use crate::error::TilthError;
use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::session::Session;
use crate::types::FileType;

/// Default display limit when caller does not specify one.
/// Max unique caller functions to trace for 2nd hop. Above this = wide fan-out, skip.
const IMPACT_FANOUT_THRESHOLD: usize = 10;
/// Max 2nd-hop results to display.
const IMPACT_MAX_RESULTS: usize = 15;
/// Early quit for batch caller search.
const BATCH_EARLY_QUIT: usize = 50;

/// Top-level sentinel used when a call site is not inside a function body.
const TOP_LEVEL: &str = "<top-level>";

/// A single caller match — a call site of a target symbol.
#[derive(Debug)]
pub struct CallerMatch {
    pub path: PathBuf,
    pub line: u32,
    pub calling_function: String,
    pub call_text: String,
    /// Line range of the calling function (for expand).
    pub caller_range: Option<(u32, u32)>,
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
) -> Result<Vec<CallerMatch>, TilthError> {
    let matches: Mutex<Vec<CallerMatch>> = Mutex::new(Vec::new());
    let found_count = AtomicUsize::new(0);
    let needle = target.as_bytes();

    let walker = super::walker(scope, glob)?;

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
            if file_len > 500_000 {
                return ignore::WalkState::Continue;
            }
            if super::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap + memchr SIMD pre-filter.
            let Some(bytes) = super::read_file_bytes(path, file_len) else {
                return ignore::WalkState::Continue;
            };

            if memchr::memmem::find(&bytes, needle).is_none() {
                return ignore::WalkState::Continue;
            }

            if file_len >= super::MINIFIED_CHECK_THRESHOLD && super::looks_minified(&bytes) {
                return ignore::WalkState::Continue;
            }

            // Hit: validate UTF-8 only now.
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            // Bloom pre-filter: skip if target is definitely not in file
            if !bloom.contains(path, mtime, content, target) {
                return ignore::WalkState::Continue;
            }

            // Only process files with tree-sitter grammars
            let file_type = detect_file_type(path);
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
    let Some(query_str) = super::callees::callee_query_str(lang) else {
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

    let Some(callers) = super::callees::with_callee_query(ts_lang, query_str, |query| {
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
                let call_node = cap.node.parent().unwrap_or(cap.node);
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

                // Walk up the tree to find the enclosing function
                let (calling_function, caller_range) =
                    find_enclosing_function(cap.node, &lines, lang);

                callers.push(CallerMatch {
                    path: path.to_path_buf(),
                    line,
                    calling_function,
                    call_text,
                    caller_range,
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
) -> Result<Vec<(String, CallerMatch)>, TilthError> {
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

    let walker = super::walker(scope, glob)?;

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
            if file_len > 500_000 {
                return ignore::WalkState::Continue;
            }
            if super::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap + multi-pattern pre-filter.
            let Some(bytes) = super::read_file_bytes(path, file_len) else {
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

            if file_len >= super::MINIFIED_CHECK_THRESHOLD && super::looks_minified(&bytes) {
                return ignore::WalkState::Continue;
            }

            // Hit: validate UTF-8 only now.
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            // Bloom pre-filter: skip if none of the targets are definitely in the file
            if !targets
                .iter()
                .any(|t| bloom.contains(path, mtime, content, t))
            {
                return ignore::WalkState::Continue;
            }

            // Only process files with tree-sitter grammars
            let file_type = detect_file_type(path);
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
    let Some(query_str) = super::callees::callee_query_str(lang) else {
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

    let Some(callers) = super::callees::with_callee_query(ts_lang, query_str, |query| {
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
                let call_node = cap.node.parent().unwrap_or(cap.node);
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

                callers.push((
                    matched_target,
                    CallerMatch {
                        path: path.to_path_buf(),
                        line,
                        calling_function,
                        call_text,
                        caller_range,
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

        // Check standard definition kinds, or Elixir call-node definitions
        let def_name = if DEFINITION_KINDS.contains(&kind) {
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
pub fn search_callers_expanded(
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
) -> Result<String, TilthError> {
    let max_matches = limit.unwrap_or(usize::MAX);
    let callers = find_callers(target, scope, bloom, glob, Some(cache))?;

    if callers.is_empty() {
        return Ok(format!(
            "# Callers of \"{}\" in {} — no call sites found\n\n\
             Tip: tilth detects only direct, by-name call sites. The symbol may still be invoked via:\n\
               - Rust trait objects (`dyn Trait`) or generic bounds\n\
               - Go interface dispatch or function values stored in structs\n\
               - Java/Kotlin interface or abstract methods, reflection\n\
               - TypeScript/JS class hierarchies, callbacks, or dynamic property access\n\
               - Python duck typing, `getattr`, decorators\n\n\
             Try `tilth(\"{}\")` (symbol search) to find the declaring interface/trait, \
             then run `callers` on that name, or search for implementors.",
            target,
            scope.display(),
            target,
        ));
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

    // Format the output
    let mut output = format!(
        "# Callers of \"{}\" in {} — {} call site{}\n",
        target,
        scope.display(),
        total,
        if total == 1 { "" } else { "s" }
    );

    for (i, caller) in sorted_callers.iter().enumerate() {
        // Header: file:line [caller: calling_function]
        let _ = write!(
            output,
            "\n## {}:{} [caller: {}]\n",
            caller
                .path
                .strip_prefix(scope)
                .unwrap_or(&caller.path)
                .display(),
            caller.line,
            caller.calling_function
        );

        // Show the call text
        let _ = writeln!(output, "→ {}", caller.call_text);

        // Expand if requested and we have the range
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

                output.push('\n');
                output.push_str("```\n");

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

    if total > effective_offset + shown {
        let omitted = total - effective_offset - shown;
        let next_offset = effective_offset + shown;
        let _ = write!(
            output,
            "\n... and {omitted} more call sites. Next page: --offset {next_offset}."
        );
    } else if effective_offset > 0 {
        let _ = write!(output, "\n(end of results, offset={effective_offset})");
    }

    // ── Adaptive 2nd-hop impact analysis ──
    // Use all_caller_names (pre-truncation) for the fan-out threshold check,
    // but search for callers of the full set to capture transitive impact.
    if !all_caller_names.is_empty() && all_caller_names.len() <= IMPACT_FANOUT_THRESHOLD {
        if let Ok(hop2) = find_callers_batch(&all_caller_names, scope, bloom, glob, Some(cache), Some(BATCH_EARLY_QUIT)) {
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

                    let rel_path = m.path.strip_prefix(scope).unwrap_or(&m.path).display();
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
    Ok(output)
}

/// Simple ranking: context file first, then by path length (proximity heuristic).
fn rank_callers(callers: &mut [CallerMatch], scope: &Path, context: Option<&Path>) {
    callers.sort_by(|a, b| {
        // Context file wins
        if let Some(ctx) = context {
            match (a.path == ctx, b.path == ctx) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
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

// ═══════════════════════════════════════════════════════════════════════════
// Graphwalk-style BFS over call graph (v1 — safe, reuses find_callers_batch)
// ═══════════════════════════════════════════════════════════════════════════

/// A single directed edge in the caller graph: `from` is the enclosing
/// function that contains a call to `to` at `from_loc`.
#[derive(Debug, Clone)]
pub struct BfsEdge {
    pub hop: usize,
    pub from: String,
    pub from_file: PathBuf,
    pub from_line: u32,
    pub to: String,
}

/// Aggregated BFS result.
#[derive(Debug, Default)]
pub struct BfsStats {
    pub depth_reached: usize,
    pub edges_total: usize,
    pub frontier_cut_hops: Vec<(usize, usize)>, // (hop, frontier_size_before_cut)
    pub edges_cut_at_hop: Option<usize>,
    pub top_level_terminal: usize,
    pub unresolved_symbols: usize, // frontier symbols that produced zero callers
    pub hubs_skipped: Vec<String>, // hub symbols dropped from frontier (user override)
    pub auto_hubs_skipped: Vec<String>, // auto-promoted hub drops on later hops
    pub auto_hubs_promoted: Vec<(String, usize)>, // (symbol, edge_count) promoted this run
    pub per_hop: Vec<HopStats>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct HopStats {
    pub hop: usize,
    pub frontier_size: usize, // after hub-skip + cap
    pub edges: usize,
}

/// Threshold for auto-hub promotion: if a single symbol produces this many
/// edges in one hop, treat it as a hub and drop from the next frontier.
/// Data-driven, language-agnostic — no hard-coded hub list.
const AUTO_HUB_THRESHOLD: usize = 200;

/// Parse user-provided hub overrides. Returns empty set by default;
/// `--skip-hubs "A,B,C"` adds explicit skips on top of auto-promotion.
fn parse_hubs(skip_hubs: Option<&str>) -> HashSet<String> {
    match skip_hubs {
        None | Some("") => HashSet::new(),
        Some(csv) => csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
    }
}

/// BFS backward over callers up to `max_depth` hops.
/// Reuses `find_callers_batch` per hop — one walk per hop, Aho-Corasick multi-pattern.
///
/// Guards:
/// - `skip_hubs`: explicit user override for symbols to drop from frontier.
///   Auto-hub promotion (>= `AUTO_HUB_THRESHOLD` edges/hop) is always on.
/// - `max_frontier`: cap symbols explored per hop (drop lowest-priority excess).
/// - `max_edges`: hard stop on total edges.
/// - Visited set on `(symbol, path)` avoids revisits and loops.
/// - Unsupported-lang files become leaf nodes (inherited from `find_callers_batch`).
#[allow(clippy::too_many_arguments)]
pub fn search_callers_bfs(
    target: &str,
    scope: &Path,
    cache: &OutlineCache,
    bloom: &crate::index::bloom::BloomFilterCache,
    max_depth: usize,
    max_frontier: usize,
    max_edges: usize,
    glob: Option<&str>,
    skip_hubs: Option<&str>,
    json: bool,
) -> Result<String, TilthError> {
    let t_start = std::time::Instant::now();
    let user_hubs = parse_hubs(skip_hubs);
    // Auto-promoted hubs: symbols producing >= AUTO_HUB_THRESHOLD edges per hop.
    // Dropped from frontier in subsequent hops. Data-driven, language-agnostic.
    let mut auto_hubs: HashSet<String> = HashSet::new();

    let mut edges: Vec<BfsEdge> = Vec::new();
    let mut stats = BfsStats::default();
    // Visited: (calling_function, path) — a function body in a given file
    // already enumerated as source of edges at some hop.
    let mut visited: HashSet<(String, PathBuf)> = HashSet::new();

    // Frontier at hop k: set of symbol names whose callers we want next.
    let mut frontier: HashSet<String> = HashSet::from([target.to_string()]);

    'outer: for hop in 1..=max_depth {
        if frontier.is_empty() {
            break;
        }

        // Drop hub symbols from frontier BEFORE the cap — they dominate fan-out.
        // Two sources: explicit user override (`--skip-hubs`) and auto-promoted
        // hubs detected in previous hops (>= AUTO_HUB_THRESHOLD edges/hop).
        // Root symbol is always explored even if it's a hub.
        let mut frontier_vec: Vec<String> = frontier
            .iter()
            .filter(|s| {
                if hop == 1 {
                    true
                } else if user_hubs.contains(s.as_str()) {
                    stats.hubs_skipped.push((*s).clone());
                    false
                } else if auto_hubs.contains(s.as_str()) {
                    stats.auto_hubs_skipped.push((*s).clone());
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        frontier_vec.sort();
        if frontier_vec.len() > max_frontier {
            stats.frontier_cut_hops.push((hop, frontier_vec.len()));
            frontier_vec.truncate(max_frontier);
        }
        let frontier_this_hop: HashSet<String> = frontier_vec.into_iter().collect();
        let frontier_size = frontier_this_hop.len();

        if frontier_this_hop.is_empty() {
            break;
        }

        let mut hop_matches =
            find_callers_batch(&frontier_this_hop, scope, bloom, glob, Some(cache), None)?;
        // Parallel walker returns matches in thread-scheduling order. Sort
        // deterministically so `--max-edges` truncation is reproducible across runs.
        // Key matches the final edge sort below: (from_file, from_line, callee, caller).
        hop_matches.sort_by(|(a_to, a), (b_to, b)| {
            a.path
                .cmp(&b.path)
                .then(a.line.cmp(&b.line))
                .then(a_to.cmp(b_to))
                .then(a.calling_function.cmp(&b.calling_function))
        });

        let mut next_frontier: HashSet<String> = HashSet::new();
        let mut hit_targets_this_hop: HashSet<String> = HashSet::new();
        let edges_before_hop = edges.len();
        // Count edges per callee in this hop — used for auto-hub promotion.
        let mut per_callee_count: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for (callee, m) in hop_matches {
            hit_targets_this_hop.insert(callee.clone());
            *per_callee_count.entry(callee.clone()).or_insert(0) += 1;

            let key = (m.calling_function.clone(), m.path.clone());

            if m.calling_function == TOP_LEVEL {
                stats.top_level_terminal += 1;
            } else if visited.insert(key) {
                next_frontier.insert(m.calling_function.clone());
            }

            edges.push(BfsEdge {
                hop,
                from: m.calling_function,
                from_file: m.path,
                from_line: m.line,
                to: callee,
            });

            if edges.len() >= max_edges {
                stats.edges_cut_at_hop = Some(hop);
                stats.depth_reached = hop;
                stats.per_hop.push(HopStats {
                    hop,
                    frontier_size,
                    edges: edges.len() - edges_before_hop,
                });
                break 'outer;
            }
        }

        // Promote high-fan-out symbols to auto-hubs for subsequent hops.
        for (sym, count) in &per_callee_count {
            if *count >= AUTO_HUB_THRESHOLD && auto_hubs.insert(sym.clone()) {
                stats.auto_hubs_promoted.push((sym.clone(), *count));
            }
        }

        // Unresolved = frontier symbols nobody matched in the repo (by-name miss,
        // unsupported-lang file-only, indirect dispatch, etc).
        for s in &frontier_this_hop {
            if !hit_targets_this_hop.contains(s) {
                stats.unresolved_symbols += 1;
            }
        }

        stats.per_hop.push(HopStats {
            hop,
            frontier_size,
            edges: edges.len() - edges_before_hop,
        });
        stats.depth_reached = hop;
        frontier = next_frontier;
    }

    stats.edges_total = edges.len();
    stats.elapsed_ms = t_start.elapsed().as_millis();

    // Deterministic sort across ALL edges (stable for both text + JSON).
    edges.sort_by(|a, b| {
        a.hop
            .cmp(&b.hop)
            .then_with(|| a.from_file.cmp(&b.from_file))
            .then_with(|| a.from_line.cmp(&b.from_line))
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.from.cmp(&b.from))
    });

    if json {
        Ok(format_bfs_json(target, scope, &edges, &stats, max_depth))
    } else {
        Ok(format_bfs(target, scope, &edges, &stats, max_depth))
    }
}

/// Deterministic sort + pretty-print BFS edges grouped by hop.
fn format_bfs(
    target: &str,
    scope: &Path,
    edges: &[BfsEdge],
    stats: &BfsStats,
    max_depth: usize,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# BFS callers of \"{}\" in {} — depth={}/{}, {} edge{}, {} ms",
        target,
        scope.display(),
        stats.depth_reached,
        max_depth,
        stats.edges_total,
        if stats.edges_total == 1 { "" } else { "s" },
        stats.elapsed_ms
    );

    if edges.is_empty() {
        let _ = writeln!(
            out,
            "\nNo call sites found. Symbol may be invoked via indirect dispatch \
             (trait objects, interfaces, callbacks, reflection, macros) — tilth \
             only sees direct by-name calls."
        );
        return out;
    }

    // Group edges by hop, sort deterministically within each group.
    let mut by_hop: std::collections::BTreeMap<usize, Vec<&BfsEdge>> =
        std::collections::BTreeMap::new();
    for e in edges {
        by_hop.entry(e.hop).or_default().push(e);
    }

    for (hop, mut list) in by_hop {
        list.sort_by(|a, b| {
            a.from_file
                .cmp(&b.from_file)
                .then_with(|| a.from_line.cmp(&b.from_line))
                .then_with(|| a.to.cmp(&b.to))
                .then_with(|| a.from.cmp(&b.from))
        });
        let _ = writeln!(out, "\n── hop {} ({} edge{}) ──", hop, list.len(),
            if list.len() == 1 { "" } else { "s" });
        for e in list {
            let rel = e.from_file.strip_prefix(scope).unwrap_or(&e.from_file);
            let _ = writeln!(
                out,
                "  {:<28} {}:{}  → {}",
                e.from,
                rel.display(),
                e.from_line,
                e.to
            );
        }
    }

    // Banner — truthfulness about what was cut.
    let mut notes: Vec<String> = Vec::new();
    if let Some(h) = stats.edges_cut_at_hop {
        notes.push(format!("edges capped at hop {h}"));
    }
    for (h, n) in &stats.frontier_cut_hops {
        notes.push(format!("frontier at hop {h} truncated from {n}"));
    }
    if !stats.auto_hubs_promoted.is_empty() {
        let mut parts: Vec<String> = stats
            .auto_hubs_promoted
            .iter()
            .map(|(s, c)| format!("{s}({c})"))
            .collect();
        parts.sort();
        let preview: Vec<String> = parts.iter().take(6).cloned().collect();
        let more = parts.len().saturating_sub(preview.len());
        let suffix = if more > 0 { format!(", +{more} more") } else { String::new() };
        notes.push(format!(
            "{} symbol(s) auto-promoted to hub (≥{} edges/hop): {}{}",
            parts.len(),
            AUTO_HUB_THRESHOLD,
            preview.join(", "),
            suffix
        ));
    }
    if !stats.hubs_skipped.is_empty() {
        let mut uniq: Vec<&String> = stats.hubs_skipped.iter().collect();
        uniq.sort();
        uniq.dedup();
        let preview: Vec<String> = uniq.iter().take(6).map(|s| (*s).clone()).collect();
        let more = uniq.len().saturating_sub(preview.len());
        let suffix = if more > 0 { format!(", +{more} more") } else { String::new() };
        notes.push(format!(
            "{} hub symbol(s) skipped: {}{}",
            uniq.len(),
            preview.join(","),
            suffix
        ));
    }
    if stats.top_level_terminal > 0 {
        notes.push(format!("{} top-level terminal edge(s)", stats.top_level_terminal));
    }
    if stats.unresolved_symbols > 0 {
        notes.push(format!(
            "{} frontier symbol(s) unresolved (unsupported lang / indirect / orphan)",
            stats.unresolved_symbols
        ));
    }
    if !notes.is_empty() {
        let _ = writeln!(out, "\n── budget ──");
        for n in notes {
            let _ = writeln!(out, "  • {n}");
        }
    }
    let _ = writeln!(
        out,
        "\nStatic by-name call graph only. May miss indirect dispatch, reflection, macros, \
         and calls from files > 500KB or from languages without a tree-sitter call query."
    );

    let tokens = crate::types::estimate_tokens(out.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(out, "\n({token_str} tokens)");
    out
}

/// Emit the BFS result as JSON (edge-list schema compatible with GraphWalks-style consumers).
fn format_bfs_json(
    target: &str,
    scope: &Path,
    edges: &[BfsEdge],
    stats: &BfsStats,
    max_depth: usize,
) -> String {
    let edges_json: Vec<serde_json::Value> = edges
        .iter()
        .map(|e| {
            let rel = e
                .from_file
                .strip_prefix(scope)
                .unwrap_or(&e.from_file)
                .display()
                .to_string();
            serde_json::json!({
                "hop": e.hop,
                "from": e.from,
                "from_file": rel,
                "from_line": e.from_line,
                "to": e.to,
            })
        })
        .collect();

    let per_hop: Vec<serde_json::Value> = stats
        .per_hop
        .iter()
        .map(|h| {
            serde_json::json!({
                "hop": h.hop,
                "frontier_size": h.frontier_size,
                "edges": h.edges,
            })
        })
        .collect();

    let frontier_cuts: Vec<serde_json::Value> = stats
        .frontier_cut_hops
        .iter()
        .map(|(h, n)| serde_json::json!({"hop": h, "frontier_size_before_cut": n}))
        .collect();

    let mut hubs_sorted: Vec<String> = stats.hubs_skipped.clone();
    hubs_sorted.sort();
    hubs_sorted.dedup();

    let auto_hubs_json: Vec<serde_json::Value> = stats
        .auto_hubs_promoted
        .iter()
        .map(|(s, c)| serde_json::json!({"symbol": s, "edges": c}))
        .collect();

    let payload = serde_json::json!({
        "root": target,
        "scope": scope.display().to_string(),
        "max_depth": max_depth,
        "depth_reached": stats.depth_reached,
        "edges_total": stats.edges_total,
        "elapsed_ms": stats.elapsed_ms,
        "edges": edges_json,
        "stats": {
            "per_hop": per_hop,
            "top_level_terminal": stats.top_level_terminal,
            "unresolved_symbols": stats.unresolved_symbols,
        },
        "elided": {
            "edges_cut_at_hop": stats.edges_cut_at_hop,
            "frontier_cuts": frontier_cuts,
            "hubs_skipped": hubs_sorted,
            "auto_hubs_promoted": auto_hubs_json,
            "auto_hub_threshold": AUTO_HUB_THRESHOLD,
        },
        "disclaimer": "Static by-name call graph only. May miss indirect dispatch, reflection, macros, and calls from files > 500KB or from languages without a tree-sitter call query.",
    });

    serde_json::to_string_pretty(&payload)
        .expect("serde_json::Value is always serializable")
}
