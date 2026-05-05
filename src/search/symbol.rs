use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use super::file_metadata;
use crate::lang::treesitter::{
    definition_weight, elixir_definition_weight, extract_base_list_targets,
    extract_definition_name, extract_elixir_definition_name, extract_impl_trait, extract_impl_type,
    extract_implemented_interfaces, is_elixir_definition, DEFINITION_KINDS,
};

use crate::error::SrcwalkError;
use crate::lang::detect_file_type;
use crate::lang::outline::get_outline_entries;
use crate::lang::outline::outline_language;
use crate::search::rank;
use crate::types::{FileType, Match, OutlineEntry, OutlineKind, SearchResult};
use globset::Glob;
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;

const MAX_DEFINITION_DEPTH: usize = 8;

pub fn search_name_glob(
    pattern: &str,
    scope: &Path,
    _cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    let matcher = Glob::new(pattern).map_err(|e| SrcwalkError::InvalidQuery {
        query: pattern.to_string(),
        reason: e.to_string(),
    })?;
    let matcher = matcher.compile_matcher();
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());

    let walker = super::walker(scope, glob)?;
    walker.run(|| {
        let matches = &matches;
        let matcher = &matcher;

        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            let file_size = match std::fs::metadata(path) {
                Ok(meta) => {
                    if meta.len() > 500_000 {
                        return ignore::WalkState::Continue;
                    }
                    meta.len()
                }
                Err(_) => return ignore::WalkState::Continue,
            };
            if super::io::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }

            let Some(bytes) = super::read_file_bytes(path, file_size) else {
                return ignore::WalkState::Continue;
            };
            if file_size >= super::io::MINIFIED_CHECK_THRESHOLD && super::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };
            let FileType::Code(lang) = detect_file_type(path) else {
                return ignore::WalkState::Continue;
            };

            let (file_lines, mtime) = file_metadata(path);
            let entries = get_outline_entries(content, lang);
            if entries.is_empty() {
                return ignore::WalkState::Continue;
            }
            let lines: Vec<&str> = content.lines().collect();
            let mut file_matches = Vec::new();
            collect_name_glob_matches(
                path,
                &lines,
                file_lines,
                mtime,
                &entries,
                matcher,
                &mut file_matches,
            );
            if !file_matches.is_empty() {
                matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(file_matches);
            }
            ignore::WalkState::Continue
        })
    });

    let mut merged = matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rank::sort(&mut merged, pattern, scope, context);
    let total = merged.len();
    Ok(SearchResult {
        query: pattern.to_string(),
        scope: scope.to_path_buf(),
        matches: merged,
        total_found: total,
        definitions: total,
        usages: 0,
        comments: 0,
        has_more: false,
        offset: 0,
    })
}

fn collect_name_glob_matches(
    path: &Path,
    lines: &[&str],
    file_lines: u32,
    mtime: SystemTime,
    entries: &[OutlineEntry],
    matcher: &globset::GlobMatcher,
    out: &mut Vec<Match>,
) {
    for entry in entries {
        if matcher.is_match(&entry.name) {
            let line_idx = entry.start_line.saturating_sub(1) as usize;
            let line_text = lines.get(line_idx).unwrap_or(&"").trim_end();
            out.push(Match {
                path: path.to_path_buf(),
                line: entry.start_line,
                text: line_text.to_string(),
                is_definition: true,
                exact: false,
                file_lines,
                mtime,
                def_range: Some((entry.start_line, entry.end_line)),
                def_name: Some(entry.name.clone()),
                def_weight: outline_def_weight(entry.kind),
                impl_target: None,
                base_target: None,
                in_comment: false,
            });
        }
        collect_name_glob_matches(
            path,
            lines,
            file_lines,
            mtime,
            &entry.children,
            matcher,
            out,
        );
    }
}

fn outline_def_weight(kind: OutlineKind) -> u16 {
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

/// Multi-symbol batch search.
/// Single-walk: each file is opened/parsed once; `AhoCorasick` gates by any-query hit;
/// tree-sitter AST walked once with per-query buckets. Same for usages.
/// Returns one `SearchResult` per query in input order.
pub fn search_batch(
    queries: &[&str],
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<Vec<SearchResult>, SrcwalkError> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    if queries.len() == 1 {
        return Ok(vec![search(queries[0], scope, cache, context, glob)?]);
    }

    // Build aho-corasick automaton for byte-level any-of gate.
    let ac = aho_corasick::AhoCorasick::new(queries).map_err(|e| SrcwalkError::InvalidQuery {
        query: queries.join(","),
        reason: e.to_string(),
    })?;

    // Build single regex \b(q1|q2|...)\b for usages.
    let alt = queries
        .iter()
        .map(|q| regex_syntax::escape(q))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = format!(r"\b(?:{alt})\b");
    let matcher = RegexMatcher::new(&pattern).map_err(|e| SrcwalkError::InvalidQuery {
        query: queries.join(","),
        reason: e.to_string(),
    })?;

    let (defs_by_q, usages_by_q) = rayon::join(
        || find_definitions_batch(queries, &ac, scope, glob, cache),
        || find_usages_batch(queries, &matcher, scope, glob),
    );

    let defs_by_q = defs_by_q?;
    let usages_by_q = usages_by_q?;

    let mut out = Vec::with_capacity(queries.len());
    for (i, query) in queries.iter().enumerate() {
        let defs = defs_by_q[i].clone();
        let usages = usages_by_q[i].clone();
        let mut merged: Vec<Match> = defs;
        let def_count = merged.len();
        for m in usages {
            let dominated = merged[..def_count]
                .iter()
                .any(|d| d.path == m.path && d.line == m.line);
            if !dominated {
                merged.push(m);
            }
        }
        let total = merged.len();
        let comment_count = merged.iter().filter(|m| m.in_comment).count();
        let usage_count = total - def_count - comment_count;
        rank::sort(&mut merged, query, scope, context);
        out.push(SearchResult {
            query: (*query).to_string(),
            scope: scope.to_path_buf(),
            matches: merged,
            total_found: total,
            definitions: def_count,
            usages: usage_count,
            comments: comment_count,
            has_more: false,
            offset: 0,
        });
    }
    Ok(out)
}

fn find_definitions_batch(
    queries: &[&str],
    ac: &aho_corasick::AhoCorasick,
    scope: &Path,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
) -> Result<Vec<Vec<Match>>, SrcwalkError> {
    let buckets: Mutex<Vec<Vec<Match>>> = Mutex::new(vec![Vec::new(); queries.len()]);
    let walker = super::walker(scope, glob)?;

    walker.run(|| {
        let buckets = &buckets;
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            let file_size = match std::fs::metadata(path) {
                Ok(meta) => {
                    if meta.len() > 500_000 {
                        return ignore::WalkState::Continue;
                    }
                    meta.len()
                }
                Err(_) => return ignore::WalkState::Continue,
            };
            if super::io::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }
            let Some(bytes) = super::read_file_bytes(path, file_size) else {
                return ignore::WalkState::Continue;
            };

            // Single-pass any-of gate: find which queries hit this file.
            let mut hit_mask = vec![false; queries.len()];
            let mut any_hit = false;
            for m in ac.find_iter(&bytes[..]) {
                hit_mask[m.pattern().as_usize()] = true;
                any_hit = true;
            }
            if !any_hit {
                return ignore::WalkState::Continue;
            }

            if file_size >= super::io::MINIFIED_CHECK_THRESHOLD && super::io::looks_minified(&bytes)
            {
                return ignore::WalkState::Continue;
            }
            let Ok(content) = std::str::from_utf8(&bytes) else {
                return ignore::WalkState::Continue;
            };

            let (file_lines, mtime) = file_metadata(path);
            let file_type = detect_file_type(path);
            let lang = match file_type {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            let ts_language = lang.and_then(outline_language);

            // Per-file local buckets so we lock global mutex once.
            let mut local: Vec<Vec<Match>> = vec![Vec::new(); queries.len()];

            if let Some(ref ts_lang) = ts_language {
                // Parse once, walk once per query that hit (cheap: walk is fast vs parse).
                for (i, q) in queries.iter().enumerate() {
                    if !hit_mask[i] {
                        continue;
                    }
                    let defs = find_defs_treesitter(
                        path, q, ts_lang, lang, content, file_lines, mtime, cache,
                    );
                    if !defs.is_empty() {
                        local[i] = defs;
                    }
                }
            } else {
                for (i, q) in queries.iter().enumerate() {
                    if !hit_mask[i] {
                        continue;
                    }
                    let defs = find_defs_heuristic_buf(path, q, content, file_lines, mtime);
                    if !defs.is_empty() {
                        local[i] = defs;
                    }
                }
            }

            let any_local = local.iter().any(|v| !v.is_empty());
            if any_local {
                let mut all = buckets
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (i, v) in local.into_iter().enumerate() {
                    if !v.is_empty() {
                        all[i].extend(v);
                    }
                }
            }

            ignore::WalkState::Continue
        })
    });

    Ok(buckets
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn find_usages_batch(
    queries: &[&str],
    matcher: &RegexMatcher,
    scope: &Path,
    glob: Option<&str>,
) -> Result<Vec<Vec<Match>>, SrcwalkError> {
    let buckets: Mutex<Vec<Vec<Match>>> = Mutex::new(vec![Vec::new(); queries.len()]);
    let walker = super::walker(scope, glob)?;

    walker.run(|| {
        let buckets = &buckets;
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 500_000 {
                    return ignore::WalkState::Continue;
                }
            }
            let (file_lines, mtime) = file_metadata(path);

            // Per-file local buckets.
            let mut local: Vec<Vec<Match>> = vec![Vec::new(); queries.len()];
            let mut searcher = Searcher::new();
            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|line_num, line| {
                    // Dispatch line to whichever query has a word-boundary match.
                    // The regex already guaranteed at least one query matches; we
                    // re-check per-query so substrings inside larger words don't
                    // leak into the wrong bucket (e.g. "parseFoo" must not count
                    // toward "parse" when only "format" actually matched).
                    let bytes = line.as_bytes();
                    for (i, q) in queries.iter().enumerate() {
                        let qb = q.as_bytes();
                        let mut start = 0;
                        let mut hit = false;
                        while let Some(pos) = memchr::memmem::find(&bytes[start..], qb) {
                            let abs = start + pos;
                            let before_ok = abs == 0 || !is_word_byte(bytes[abs - 1]);
                            let after = abs + qb.len();
                            let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
                            if before_ok && after_ok {
                                hit = true;
                                break;
                            }
                            start = abs + 1;
                        }
                        if hit {
                            local[i].push(Match {
                                path: path.to_path_buf(),
                                line: line_num as u32,
                                text: line.trim_end().to_string(),
                                is_definition: false,
                                exact: true,
                                file_lines,
                                mtime,
                                def_range: None,
                                def_name: None,
                                def_weight: 0,
                                impl_target: None,
                                base_target: None,
                                in_comment: false,
                            });
                        }
                    }
                    Ok(true)
                }),
            );

            let any_local = local.iter().any(|v| !v.is_empty());
            if any_local {
                let mut all = buckets
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (i, v) in local.into_iter().enumerate() {
                    if !v.is_empty() {
                        all[i].extend(v);
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    let mut buckets = buckets
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tag_comment_matches(&mut buckets);
    Ok(buckets)
}

/// Symbol search: find definitions via tree-sitter, usages via ripgrep, concurrently.
/// Merge results, deduplicate, definitions first.
pub fn search(
    query: &str,
    scope: &Path,
    cache: Option<&crate::cache::OutlineCache>,
    context: Option<&Path>,
    glob: Option<&str>,
) -> Result<SearchResult, SrcwalkError> {
    // Compile regex once, share across both arms
    let word_pattern = format!(r"\b{}\b", regex_syntax::escape(query));
    let matcher = RegexMatcher::new(&word_pattern).map_err(|e| SrcwalkError::InvalidQuery {
        query: query.to_string(),
        reason: e.to_string(),
    })?;

    let (defs, usages) = rayon::join(
        || find_definitions(query, scope, glob, cache),
        || find_usages(query, &matcher, scope, glob),
    );

    let defs = defs?;
    let mut usages_vec = vec![usages?];
    tag_comment_matches(&mut usages_vec);
    let usages = usages_vec.into_iter().next().unwrap();

    // Deduplicate: remove usage matches that overlap with definition matches.
    // Linear scan — max ~30 defs from EARLY_QUIT_THRESHOLD, no allocation needed.
    let mut merged: Vec<Match> = defs;
    let def_count = merged.len();

    for m in usages {
        let dominated = merged[..def_count]
            .iter()
            .any(|d| d.path == m.path && d.line == m.line);
        if !dominated {
            merged.push(m);
        }
    }

    let total = merged.len();
    let comment_count = merged.iter().filter(|m| m.in_comment).count();
    let usage_count = total - def_count - comment_count;

    rank::sort(&mut merged, query, scope, context);

    Ok(SearchResult {
        query: query.to_string(),
        scope: scope.to_path_buf(),
        matches: merged,
        total_found: total,
        definitions: def_count,
        usages: usage_count,
        comments: comment_count,
        has_more: false,
        offset: 0,
    })
}

/// Find definitions using tree-sitter structural detection.
/// For each file containing the query string, parse with tree-sitter and walk
/// definition nodes to see if any declare the queried symbol.
/// Falls back to keyword heuristic for files without grammars.
///
/// Single-read design: reads each file once, checks for symbol via
/// `memchr::memmem` (SIMD), then reuses the buffer for tree-sitter parsing.
/// Early termination: quits the parallel walker once enough defs are found.
fn find_definitions(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    cache: Option<&crate::cache::OutlineCache>,
) -> Result<Vec<Match>, SrcwalkError> {
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    // Relaxed is correct: walker.run() joins all threads before we read the final value.
    // Early-quit checks are approximate by design — one extra iteration is harmless.
    let found_count = AtomicUsize::new(0);
    let needle = query.as_bytes();

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

            // Skip oversized files — avoid tree-sitter parsing multi-MB minified bundles
            let file_size = match std::fs::metadata(path) {
                Ok(meta) => {
                    if meta.len() > 500_000 {
                        return ignore::WalkState::Continue;
                    }
                    meta.len()
                }
                Err(_) => return ignore::WalkState::Continue,
            };

            // Skip minified/bundled assets by filename — they're parseable but
            // tree-sitter costs 100-500ms on them with zero useful defs.
            if super::io::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }

            // Fast byte-level scan: mmap (or heap-read for tiny files) +
            // memchr SIMD search. Skips UTF-8 validation on ~90% of files
            // that don't contain the symbol.
            let Some(bytes) = super::read_file_bytes(path, file_size) else {
                return ignore::WalkState::Continue;
            };

            if memchr::memmem::find(&bytes, needle).is_none() {
                return ignore::WalkState::Continue;
            }

            // Content-based minified detection for large files that slipped
            // through filename check (e.g. `app.js` actually minified).
            if file_size >= super::io::MINIFIED_CHECK_THRESHOLD && super::io::looks_minified(&bytes)
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
                find_defs_treesitter(
                    path, query, ts_lang, lang, content, file_lines, mtime, cache,
                )
            } else {
                Vec::new()
            };

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
fn find_defs_treesitter(
    path: &Path,
    query: &str,
    ts_lang: &tree_sitter::Language,
    lang: Option<crate::types::Lang>,
    content: &str,
    file_lines: u32,
    mtime: SystemTime,
    cache: Option<&crate::cache::OutlineCache>,
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
        root, query, path, &lines, file_lines, mtime, &mut defs, lang, 0,
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
) {
    if depth > MAX_DEFINITION_DEPTH {
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
        );
    }
}

/// Keyword heuristic fallback for files without tree-sitter grammars.
/// Operates on pre-read buffer — no redundant file read.
fn find_defs_heuristic_buf(
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

/// Find all usages via ripgrep (word-boundary matching).
/// Collects per-file, locks once per file (not per line).
/// Early termination once enough usages found.
fn find_usages(
    query: &str,
    matcher: &RegexMatcher,
    scope: &Path,
    glob: Option<&str>,
) -> Result<Vec<Match>, SrcwalkError> {
    let matches: Mutex<Vec<Match>> = Mutex::new(Vec::new());
    // Relaxed: same reasoning as find_definitions — approximate early-quit, joined before read
    let found_count = AtomicUsize::new(0);

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

            // Skip oversized files
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 500_000 {
                    return ignore::WalkState::Continue;
                }
            }

            let (file_lines, mtime) = file_metadata(path);

            let mut file_matches = Vec::new();
            let mut searcher = Searcher::new();

            let _ = searcher.search_path(
                matcher,
                path,
                UTF8(|line_num, line| {
                    file_matches.push(Match {
                        path: path.to_path_buf(),
                        line: line_num as u32,
                        text: line.trim_end().to_string(),
                        is_definition: false,
                        exact: line.contains(query),
                        file_lines,
                        mtime,
                        def_range: None,
                        def_name: None,
                        def_weight: 0,
                        impl_target: None,
                        base_target: None,
                        in_comment: false,
                    });
                    Ok(true)
                }),
            );

            if !file_matches.is_empty() {
                found_count.fetch_add(file_matches.len(), Ordering::Relaxed);
                let mut all = matches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_matches);
            }

            ignore::WalkState::Continue
        })
    });

    Ok(matches
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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

/// Find spelling-similar symbols for a query that produced 0 hits.
///
/// Strategy: case-insensitive `\bquery\b` regex sweep over the same scope used
/// by the failed search. Collects each distinct **actual spelling** found in
/// source, with its first location, then ranks by `edit_distance(query_lower`,
/// `hit_lower`). Returns up to `top_n` suggestions.
///
/// Cheap because it only fires on the 0-hit path. Uses ripgrep's `\b…\b`
/// matcher with `(?i)` flag — same engine as `find_usages`.
/// Normalize an identifier for fuzzy comparison: lowercase + strip underscores.
/// Lets `searchSymbol` ↔ `search_symbol` ↔ `SearchSymbol` all collapse to
/// the same canonical string, so a single `edit_distance` over normalized
/// forms covers both naming-convention mismatches and typos uniformly.
fn normalize_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b == b'_' {
            continue;
        }
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// Sweep `scope` for identifiers whose normalized form is close to the
/// normalized `query` (edit distance ≤ threshold). Only considers source
/// files (JSON / markdown / lockfiles excluded) to avoid noise from i18n
/// bundles, build manifests, etc.
pub fn suggest(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
    top_n: usize,
) -> Vec<(String, std::path::PathBuf, u32)> {
    if query.is_empty() {
        return Vec::new();
    }
    let q_norm = normalize_ident(query);
    if q_norm.is_empty() {
        return Vec::new();
    }
    // Threshold scales with query length: allow 1 edit for short queries,
    // up to 2 for ≥6 normalized chars. Keeps recall for typos without
    // matching unrelated identifiers.
    let max_dist: usize = if q_norm.len() >= 6 { 2 } else { 1 };

    let Ok(walker) = super::walker(scope, glob) else {
        return Vec::new();
    };

    // spelling → (path, line)
    let hits: Mutex<std::collections::HashMap<String, (std::path::PathBuf, u32)>> =
        Mutex::new(std::collections::HashMap::new());

    walker.run(|| {
        let hits = &hits;
        let q_norm = q_norm.clone();
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            // Only sweep real source files — drops i18n JSON, SOURCES.txt,
            // lockfiles, markdown, etc. that would otherwise pollute
            // suggestions with non-identifier text.
            if !matches!(
                crate::lang::detect_file_type(path),
                crate::types::FileType::Code(_)
            ) {
                return ignore::WalkState::Continue;
            }
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > 500_000 {
                    return ignore::WalkState::Continue;
                }
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return ignore::WalkState::Continue;
            };
            let mut local: Vec<(String, u32)> = Vec::new();
            let mut seen_on_path: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (line_idx, line) in content.lines().enumerate() {
                let bytes = line.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    let b = bytes[i];
                    if !(b.is_ascii_alphabetic() || b == b'_') {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let word = &line[start..i];
                    // Cheap length prefilter — keeps the inner Levenshtein
                    // call off >90% of tokens.
                    let w_norm_len = word.bytes().filter(|&c| c != b'_').count();
                    if w_norm_len == 0
                        || w_norm_len + max_dist < q_norm.len()
                        || w_norm_len > q_norm.len() + max_dist
                    {
                        continue;
                    }
                    if seen_on_path.contains(word) {
                        continue;
                    }
                    let w_norm = normalize_ident(word);
                    if w_norm == q_norm && word == query {
                        // Exact match — caller already handles hit path.
                        continue;
                    }
                    let d = crate::read::edit_distance(&q_norm, &w_norm);
                    if d <= max_dist {
                        seen_on_path.insert(word.to_string());
                        local.push((word.to_string(), line_idx as u32 + 1));
                    }
                }
            }
            if !local.is_empty() {
                let mut h = hits
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (spelling, line) in local {
                    h.entry(spelling)
                        .or_insert_with(|| (path.to_path_buf(), line));
                }
            }
            ignore::WalkState::Continue
        })
    });

    let mut all: Vec<(String, std::path::PathBuf, u32)> = hits
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .into_iter()
        .filter(|(s, _)| s != query)
        .map(|(s, (p, l))| (s, p, l))
        .collect();
    // Rank by distance on normalized form; prefer same-case exact normalized
    // match over mere typo; then alphabetical for stability.
    all.sort_by(|a, b| {
        let an = normalize_ident(&a.0);
        let bn = normalize_ident(&b.0);
        let da = crate::read::edit_distance(&q_norm, &an);
        let db = crate::read::edit_distance(&q_norm, &bn);
        da.cmp(&db).then_with(|| a.0.cmp(&b.0))
    });
    all.truncate(top_n);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn rust_definitions_detected() {
        let code = r#"pub fn hello(name: &str) -> String {
    format!("Hello, {}", name)
}

pub struct Foo {
    bar: i32,
}

pub(crate) fn dispatch_tool(tool: &str) -> Result<String, String> {
    match tool {
        "read" => Ok("read".to_string()),
        _ => Err("unknown".to_string()),
    }
}
"#;
        let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::Rust).unwrap();

        let defs = find_defs_treesitter(
            std::path::Path::new("test.rs"),
            "hello",
            &ts_lang,
            Some(crate::types::Lang::Rust),
            code,
            15,
            SystemTime::now(),
            None,
        );
        assert!(!defs.is_empty(), "should find 'hello' definition");
        assert!(defs[0].is_definition);
        assert!(defs[0].def_range.is_some());

        let defs = find_defs_treesitter(
            std::path::Path::new("test.rs"),
            "Foo",
            &ts_lang,
            Some(crate::types::Lang::Rust),
            code,
            15,
            SystemTime::now(),
            None,
        );
        assert!(!defs.is_empty(), "should find 'Foo' definition");

        let defs = find_defs_treesitter(
            std::path::Path::new("test.rs"),
            "dispatch_tool",
            &ts_lang,
            Some(crate::types::Lang::Rust),
            code,
            15,
            SystemTime::now(),
            None,
        );
        assert!(!defs.is_empty(), "should find 'dispatch_tool' definition");
    }

    /// Helper: search for an Elixir definition by name in a code snippet.
    fn elixir_find(code: &str, name: &str) -> Vec<Match> {
        let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::Elixir).unwrap();
        let lines = code.lines().count() as u32;
        find_defs_treesitter(
            std::path::Path::new("test.ex"),
            name,
            &ts_lang,
            Some(crate::types::Lang::Elixir),
            code,
            lines,
            SystemTime::now(),
            None,
        )
    }

    #[test]
    fn elixir_definitions_detected() {
        let code = r#"defmodule MyApp.Greeter do
  @type t :: %{name: String.t()}

  def hello(name) do
    "Hello, #{name}!"
  end

  defp private_helper(x), do: x + 1

  defmacro my_macro(expr) do
    quote do: unquote(expr)
  end
end
"#;
        // Dotted module name
        let defs = elixir_find(code, "MyApp.Greeter");
        assert!(!defs.is_empty(), "should find 'MyApp.Greeter' module def");
        assert!(defs[0].is_definition);

        // Public function (block form with parens)
        assert!(
            !elixir_find(code, "hello").is_empty(),
            "should find 'hello'"
        );

        // Private function (keyword form: `, do:`)
        assert!(
            !elixir_find(code, "private_helper").is_empty(),
            "should find 'private_helper'"
        );

        // Macro
        assert!(
            !elixir_find(code, "my_macro").is_empty(),
            "should find 'my_macro'"
        );
    }

    #[test]
    fn elixir_guard_clause_definitions() {
        let code = r#"defmodule Guards do
  def safe_div(a, b) when b != 0 do
    a / b
  end

  defp checked(x) when is_integer(x), do: x

  defguard is_positive(x) when x > 0
end
"#;
        // Guard clause with `when` — block form
        assert!(
            !elixir_find(code, "safe_div").is_empty(),
            "should find 'safe_div' with guard clause"
        );

        // Guard clause with `when` — keyword form
        assert!(
            !elixir_find(code, "checked").is_empty(),
            "should find 'checked' with guard clause"
        );

        // defguard
        assert!(
            !elixir_find(code, "is_positive").is_empty(),
            "should find 'is_positive' defguard"
        );
    }

    #[test]
    fn elixir_multi_clause_and_no_arg() {
        let code = r#"defmodule Dispatch do
  def handle(:ok), do: :success
  def handle(:error), do: :failure

  def version, do: "1.0"
end
"#;
        // Multi-clause: both clauses should be found
        let defs = elixir_find(code, "handle");
        assert!(
            defs.len() >= 2,
            "should find both 'handle' clauses, got {}: {defs:?}",
            defs.len()
        );

        // No-arg function (bare identifier, no parens)
        assert!(
            !elixir_find(code, "version").is_empty(),
            "should find no-arg 'version'"
        );
    }

    #[test]
    fn elixir_protocol_impl_exception() {
        let code = r#"defprotocol Printable do
  @callback format(t) :: String.t()
  def to_string(data)
end

defimpl Printable, for: User do
  def to_string(user), do: user.name
end

defmodule MyError do
  defexception [:message, :code]
end
"#;
        // Protocol + defimpl: both indexed under the protocol name "Printable"
        let defs = elixir_find(code, "Printable");
        assert!(
            defs.len() >= 2,
            "should find both defprotocol and defimpl for 'Printable', got {}",
            defs.len()
        );

        // defexception
        assert!(
            !elixir_find(code, "defexception").is_empty(),
            "should find 'defexception'"
        );

        // Module containing exception
        assert!(
            !elixir_find(code, "MyError").is_empty(),
            "should find 'MyError' module"
        );
    }

    #[test]
    fn elixir_delegate_and_nested_modules() {
        let code = r#"defmodule Outer do
  defdelegate count(list), to: Enum

  defmodule Inner do
    def nested_func, do: :ok
  end
end
"#;
        // defdelegate
        assert!(
            !elixir_find(code, "count").is_empty(),
            "should find 'count' defdelegate"
        );

        // Nested module
        assert!(
            !elixir_find(code, "Inner").is_empty(),
            "should find nested 'Inner' module"
        );
    }

    #[test]
    fn suggest_finds_case_variant() {
        let dir = std::env::temp_dir().join(format!("srcwalk_p13_suggest_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foo.rs");
        std::fs::write(
            &path,
            "pub fn orderExists() -> bool { true }\nfn other() {}\n",
        )
        .unwrap();

        let hits = suggest("OrderExists", &dir, None, 3);
        assert!(
            hits.iter().any(|(s, _, _)| s == "orderExists"),
            "expected case-variant suggestion, got: {hits:?}"
        );

        let no_match = suggest("CompletelyUnrelatedXyz", &dir, None, 3);
        assert!(
            no_match.is_empty(),
            "no fuzzy hit expected, got: {no_match:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggest_crosses_naming_convention() {
        let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_conv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foo.rs");
        std::fs::write(
            &path,
            "pub fn search_symbol() -> bool { true }\npub fn HotReloadProcessor() {}\n",
        )
        .unwrap();

        // camelCase query → snake_case symbol
        let hits = suggest("searchSymbol", &dir, None, 3);
        assert!(
            hits.iter().any(|(s, _, _)| s == "search_symbol"),
            "expected snake_case suggestion for camelCase query, got: {hits:?}"
        );

        // lowercase query → PascalCase symbol
        let hits2 = suggest("hotreloadprocessor", &dir, None, 3);
        assert!(
            hits2.iter().any(|(s, _, _)| s == "HotReloadProcessor"),
            "expected PascalCase suggestion for lowercase query, got: {hits2:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggest_handles_lev1_typo() {
        let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_typo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foo.rs");
        std::fs::write(&path, "pub fn run_inner() {}\n").unwrap();

        let hits = suggest("run_iner", &dir, None, 3);
        assert!(
            hits.iter().any(|(s, _, _)| s == "run_inner"),
            "expected typo-tolerant suggestion, got: {hits:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggest_skips_non_source_files() {
        let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_skip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // JSON i18n bundle — must NOT match
        std::fs::write(dir.join("es.json"), r#"{"sesion": "iniciar"}"#).unwrap();
        // SOURCES.txt build artifact — must NOT match
        std::fs::write(dir.join("SOURCES.txt"), "src/foo/sesion.py\n").unwrap();
        // Real source — should match
        std::fs::write(dir.join("real.py"), "def session(): pass\n").unwrap();

        let hits = suggest("Sesion", &dir, None, 5);
        assert!(
            hits.iter().all(|(_, p, _)| {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                n != "es.json" && n != "SOURCES.txt"
            }),
            "expected non-source files filtered out, got: {hits:?}"
        );
        assert!(
            hits.iter().any(|(s, _, _)| s == "session"),
            "expected real .py hit, got: {hits:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Collect sorted byte-offset ranges of all comment nodes in a tree-sitter tree.
/// Works across all supported languages — tree-sitter grammars universally use
/// node kinds containing "comment" for line, block, and doc comments.
fn collect_comment_ranges(root: tree_sitter::Node) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    collect_comment_ranges_recursive(&mut cursor, &mut ranges);
    ranges
}

fn collect_comment_ranges_recursive(
    cursor: &mut tree_sitter::TreeCursor,
    ranges: &mut Vec<(usize, usize)>,
) {
    loop {
        let node = cursor.node();
        if node.kind().contains("comment") {
            ranges.push((node.start_byte(), node.end_byte()));
        } else if cursor.goto_first_child() {
            collect_comment_ranges_recursive(cursor, ranges);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Check whether a byte offset falls inside any comment range (binary search).
fn is_in_comment(offset: usize, comment_ranges: &[(usize, usize)]) -> bool {
    comment_ranges
        .binary_search_by(|&(start, end)| {
            if offset < start {
                std::cmp::Ordering::Greater
            } else if offset >= end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Tag `in_comment` on usage matches by parsing each file with tree-sitter.
/// Only files that have at least one usage match are parsed.
fn tag_comment_matches(buckets: &mut [Vec<Match>]) {
    use std::collections::HashMap;

    // Collect all unique file paths that need comment-checking.
    let mut file_paths: HashMap<std::path::PathBuf, Vec<(usize, usize)>> = HashMap::new();
    for bucket in buckets.iter() {
        for m in bucket {
            if !m.is_definition {
                file_paths.entry(m.path.clone()).or_default();
            }
        }
    }

    // Parse each file once, collect comment ranges.
    for (path, ranges) in &mut file_paths {
        let lang = detect_file_type(path);
        let ts_lang = match lang {
            FileType::Code(l) => outline_language(l),
            _ => None,
        };
        let Some(ts_lang) = ts_lang else { continue };
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&ts_lang).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&content, None) else {
            continue;
        };
        *ranges = collect_comment_ranges(tree.root_node());
    }

    // Tag each usage match.
    for bucket in buckets.iter_mut() {
        for m in bucket.iter_mut() {
            if m.is_definition {
                continue;
            }
            if let Some(ranges) = file_paths.get(&m.path) {
                if ranges.is_empty() {
                    continue;
                }
                // Convert line number to byte offset: read file and find line start.
                // We need the byte offset of the match line to check against comment ranges.
                // Since we already read the file above, re-read is cached by OS.
                if let Ok(content) = std::fs::read_to_string(&m.path) {
                    if let Some(byte_offset) = line_to_byte_offset(&content, m.line as usize) {
                        m.in_comment = is_in_comment(byte_offset, ranges);
                    }
                }
            }
        }
    }
}

/// Convert 1-based line number to byte offset of that line's start.
fn line_to_byte_offset(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    let mut current_line = 1usize;
    if line == 1 {
        return Some(0);
    }
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(i + 1);
            }
        }
    }
    None
}
