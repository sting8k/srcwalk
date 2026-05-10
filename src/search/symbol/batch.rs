use std::path::Path;
use std::sync::Mutex;

use crate::error::SrcwalkError;
use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::search::rank;
use crate::types::{FileType, Match, SearchResult};
use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;

use super::super::{file_metadata, read_file_bytes, walker};
use super::comments::tag_comment_matches;
use super::definitions::{find_defs_heuristic_buf, find_defs_treesitter};
use super::usages::is_word_byte;

/// Multi-symbol batch search.
/// Single-walk: each file is opened/parsed once; `AhoCorasick` gates by any-query hit;
/// tree-sitter AST walked once with per-query buckets. Same for usages.
/// Returns one `SearchResult` per query in input order.
pub(super) fn search_batch(
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
        return Ok(vec![super::search(
            queries[0], scope, cache, context, glob,
        )?]);
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
    let walker = walker(scope, glob)?;

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
            if super::super::io::is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }
            let Some(bytes) = read_file_bytes(path, file_size) else {
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

            if file_size >= super::super::io::MINIFIED_CHECK_THRESHOLD
                && super::super::io::looks_minified(&bytes)
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
    let walker = walker(scope, glob)?;

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
                                text: crate::search::truncate::compact_match_line(
                                    line.trim_end(),
                                    q,
                                    false,
                                ),
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
