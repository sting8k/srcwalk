use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format;
use crate::format::rel_nonempty;
use crate::read;
use crate::session::Session;
use crate::types::{estimate_tokens, FileType, Match, SearchResult};

use super::{facets, glob};

mod basename;
mod expand;
mod glob_result;
mod match_item;
mod semantic;

pub(super) use expand::{append_expand_budget_note, ExpandBudget};
#[cfg(test)]
pub(super) use semantic::best_semantic_candidate;

pub(super) fn match_kind_label(m: &Match, cache: &OutlineCache) -> Option<&'static str> {
    if m.in_comment {
        return Some("comment");
    }
    if !m.is_definition {
        return Some(non_definition_label(m));
    }
    if m.impl_target.is_some() {
        return Some("impl");
    }
    if m.base_target.is_some() {
        return Some("base");
    }
    semantic::semantic_candidate_for_match(m, cache)
        .map(|candidate| semantic::outline_kind_label(candidate.kind))
}

pub fn format_raw_result(
    result: &SearchResult,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    format_search_result(result, cache, None, &bloom, 0, None)
}

pub fn format_raw_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    header: String,
) -> Result<String, SrcwalkError> {
    let bloom = crate::index::bloom::BloomFilterCache::new();
    format_search_result_with_header(result, cache, None, &bloom, 0, None, header)
}

pub fn search_files_glob(
    pattern: &str,
    scope: &Path,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    let result = glob::search(pattern, scope, limit, offset)?;
    glob_result::format_glob_result(&result, scope, "Files")
}

/// Format match entries with optional expansion.
fn format_compact_facet_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    out: &mut String,
) {
    let mut grouped: IndexMap<&Path, Vec<&Match>> = IndexMap::new();
    for m in matches {
        if m.is_definition {
            semantic::format_definition_semantic_match(m, scope, cache, out);
        } else {
            grouped.entry(m.path.as_path()).or_default().push(m);
        }
    }

    for (path, group) in grouped {
        if group.len() == 1 {
            format_compact_non_definition_match(group[0], scope, out);
            continue;
        }
        let noun = non_definition_group_noun(path);
        let _ = write!(
            out,
            "\n  {} [{} {noun}]",
            rel_nonempty(path, scope),
            group.len()
        );
        for m in group {
            let kind = if m.in_comment {
                "comment"
            } else {
                non_definition_label(m)
            };
            let _ = write!(out, "\n    [{kind}] :{} | {}", m.line, m.text.trim());
        }
    }
}

fn format_compact_non_definition_match(m: &Match, scope: &Path, out: &mut String) {
    let kind = if m.in_comment {
        "comment"
    } else {
        non_definition_label(m)
    };
    let _ = write!(
        out,
        "\n  [{kind}] {}:{} | {}",
        rel_nonempty(&m.path, scope),
        m.line,
        m.text.trim()
    );
}

/// Groups consecutive usage matches in the same enclosing function to reduce token noise.
/// Shared expand state enables cross-query dedup in multi-symbol search.
pub(super) fn format_matches(
    matches: &[Match],
    scope: &Path,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand_remaining: &mut usize,
    expand_budget: &mut ExpandBudget,
    expanded_files: &mut HashSet<PathBuf>,
    context_shown_files: &mut HashSet<PathBuf>,
    smart_truncated: &mut bool,
    out: &mut String,
) {
    // Multi-file: one expand per unique file. Single-file: sequential per-match.
    // expanded_files may contain entries from prior queries (cross-query dedup).
    let multi_file = matches
        .first()
        .is_some_and(|first| matches.iter().any(|m| m.path != first.path));

    let groups = group_matches(matches, cache);

    for group in &groups {
        match group {
            MatchGroup::Single(m) => {
                match_item::format_single_match(
                    m,
                    scope,
                    cache,
                    session,
                    bloom,
                    expand_remaining,
                    expand_budget,
                    expanded_files,
                    context_shown_files,
                    smart_truncated,
                    multi_file,
                    out,
                );
            }
            MatchGroup::FileGroup(usages) => {
                format_file_group(usages, scope, cache, context_shown_files, out);
            }
        }
    }
}

/// Group consecutive non-definition matches by (path, enclosing outline entry).
/// Dedup key for definition matches: (path, line, `def_range`, `def_name`, `impl_target`).
type DefKey<'a> = (
    &'a Path,
    u32,
    Option<(u32, u32)>,
    Option<&'a str>,
    Option<&'a str>,
);

/// Returns a Vec of groups, where each group is a slice of matches.
/// Definitions and impl matches are always singleton groups.
enum MatchGroup<'a> {
    Single(&'a Match),
    FileGroup(Vec<&'a Match>),
}

/// Group matches for rendering: definitions/impls stay individual, usages grouped by file.
fn group_matches<'a>(matches: &'a [Match], _cache: &OutlineCache) -> Vec<MatchGroup<'a>> {
    let mut groups: Vec<MatchGroup<'a>> = Vec::new();
    let mut seen_defs: HashSet<DefKey<'_>> = HashSet::new();
    // Collect usages per file (preserving order of first occurrence)
    let mut file_usages: IndexMap<&Path, Vec<&'a Match>> = IndexMap::new();

    for m in matches {
        if m.is_definition || m.impl_target.is_some() {
            let key = (
                m.path.as_path(),
                m.line,
                m.def_range,
                m.def_name.as_deref(),
                m.impl_target.as_deref(),
            );
            if !seen_defs.insert(key) {
                continue;
            }
            groups.push(MatchGroup::Single(m));
        } else {
            file_usages.entry(m.path.as_path()).or_default().push(m);
        }
    }

    // Emit file-grouped usages after definitions
    for (_path, usages) in file_usages {
        if usages.len() == 1 {
            groups.push(MatchGroup::Single(usages[0]));
        } else {
            groups.push(MatchGroup::FileGroup(usages));
        }
    }

    groups
}

/// Format a file-level group of usages: one header, outline once, compact list with fn names.
pub(super) fn non_definition_label(m: &Match) -> &'static str {
    if m.impl_target.is_some() {
        return "impl";
    }
    match crate::lang::detect_file_type(&m.path) {
        crate::types::FileType::Other | crate::types::FileType::Log => "text",
        _ => "usage",
    }
}

fn non_definition_group_noun(path: &Path) -> &'static str {
    match crate::lang::detect_file_type(path) {
        crate::types::FileType::Other | crate::types::FileType::Log => "matches",
        _ => "usages",
    }
}
fn format_file_group(
    group: &[&Match],
    scope: &Path,
    cache: &OutlineCache,
    context_shown_files: &mut HashSet<PathBuf>,
    out: &mut String,
) {
    let first = group[0];
    let path_str = rel_nonempty(&first.path, scope);

    let noun = non_definition_group_noun(&first.path);
    let _ = write!(out, "\n\n## {path_str} [{} {noun}]", group.len());

    // Show outline context once per file
    if context_shown_files.insert(first.path.clone()) {
        if let Some(context) = outline_context_for_match(&first.path, first.line, cache) {
            out.push_str(&context);
        }
    }

    // Compact list: one line per hit with enclosing fn annotation
    for m in group {
        let fn_name = semantic::enclosing_fn_name(&m.path, m.line, cache);
        if let Some(name) = fn_name {
            let _ = write!(out, "\n- :{:<6} {} ← {name}", m.line, m.text.trim());
        } else {
            let _ = write!(out, "\n- :{:<6} {}", m.line, m.text.trim());
        }
    }
}

/// Format a symbol/content search result.
/// When an outline cache is available, wraps each match in the file's outline context.
/// When `expand > 0`, the top N matches inline actual code (def body or ±10 lines).
/// When there are >5 matches, groups them into facets for easier navigation.
pub(super) fn format_search_result(
    result: &SearchResult,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    budget_tokens: Option<u64>,
) -> Result<String, SrcwalkError> {
    let header = format::search_header(
        &result.query,
        &result.scope,
        result.matches.len(),
        result.definitions,
        result.usages,
        result.comments,
    );
    format_search_result_with_header(result, cache, session, bloom, expand, budget_tokens, header)
}

pub(super) fn format_search_result_with_header(
    result: &SearchResult,
    cache: &OutlineCache,
    session: Option<&Session>,
    bloom: &crate::index::bloom::BloomFilterCache,
    expand: usize,
    budget_tokens: Option<u64>,
    header: String,
) -> Result<String, SrcwalkError> {
    let mut out = header;
    let mut expand_remaining = expand;
    let mut expand_budget = ExpandBudget::new(expand, budget_tokens);
    let mut expanded_files = HashSet::new();
    let mut context_shown_files = HashSet::new();
    let mut smart_truncated = false;

    let compact_facets = result.matches.len() > 5 && expand == 0;

    // File-level retrieval: when a file basename matches the query exactly,
    // prepend a compact outline so the agent gets file-level context first.
    // Semantic-compact facets render kind/parent/children inline, so the
    // basename outline would duplicate the same facts and cost tokens.
    if !compact_facets {
        if let Some(file_outline) =
            basename::basename_file_outline(&result.query, &result.matches, &result.scope, cache)
        {
            let _ = write!(out, "\n\n{file_outline}");
        }
    }

    // Apply faceting when there are many matches (>5)
    if result.matches.len() > 5 {
        let faceted = facets::facet_matches(result.matches.clone(), &result.scope);

        // Format each non-empty facet with section headers
        if !faceted.definitions.is_empty() {
            let _ = write!(out, "\n\n### Definitions ({})", faceted.definitions.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.definitions, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.definitions,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.implementations.is_empty() {
            let _ = write!(
                out,
                "\n\n### Implementations ({})",
                faceted.implementations.len()
            );
            if compact_facets {
                format_compact_facet_matches(
                    &faceted.implementations,
                    &result.scope,
                    cache,
                    &mut out,
                );
            } else {
                format_matches(
                    &faceted.implementations,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.bases.is_empty() {
            let _ = write!(out, "\n\n### Base relationships ({})", faceted.bases.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.bases, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.bases,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.tests.is_empty() {
            let _ = write!(out, "\n\n### Tests ({})", faceted.tests.len());
            // Compact test format — one line per match, no expand budget consumed
            for m in &faceted.tests {
                let _ = write!(
                    out,
                    "\n  {}:{} — {}",
                    rel_nonempty(&m.path, &result.scope),
                    m.line,
                    m.text.trim()
                );
            }
        }

        if !faceted.comments.is_empty() {
            let _ = write!(out, "\n\n### Comments ({})", faceted.comments.len());
            for m in &faceted.comments {
                let _ = write!(
                    out,
                    "\n  {}:{} — {}",
                    rel_nonempty(&m.path, &result.scope),
                    m.line,
                    m.text.trim()
                );
            }
        }

        if !faceted.usages_local.is_empty() {
            let header = if faceted
                .usages_local
                .iter()
                .all(|m| non_definition_label(m) == "text")
            {
                "Text matches — same package"
            } else {
                "Usages — same package"
            };
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_local.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.usages_local, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.usages_local,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }

        if !faceted.usages_cross.is_empty() {
            let header = if faceted
                .usages_cross
                .iter()
                .all(|m| non_definition_label(m) == "text")
            {
                "Text matches — other"
            } else {
                "Usages — other"
            };
            let _ = write!(out, "\n\n### {header} ({})", faceted.usages_cross.len());
            if compact_facets {
                format_compact_facet_matches(&faceted.usages_cross, &result.scope, cache, &mut out);
            } else {
                format_matches(
                    &faceted.usages_cross,
                    &result.scope,
                    cache,
                    session,
                    bloom,
                    &mut expand_remaining,
                    &mut expand_budget,
                    &mut expanded_files,
                    &mut context_shown_files,
                    &mut smart_truncated,
                    &mut out,
                );
            }
        }
    } else {
        // Linear display for ≤5 matches
        format_matches(
            &result.matches,
            &result.scope,
            cache,
            session,
            bloom,
            &mut expand_remaining,
            &mut expand_budget,
            &mut expanded_files,
            &mut context_shown_files,
            &mut smart_truncated,
            &mut out,
        );
    }

    let mut footer = String::new();
    if result.has_more {
        let omitted = result.total_found - result.matches.len() - result.offset;
        let next_offset = result.offset + result.matches.len();
        let page_size = result.matches.len().max(1);
        let _ = write!(
            footer,
            "> Next: {omitted} more matches available. Continue with --offset {next_offset} --limit {page_size}."
        );
    } else if result.offset > 0 {
        let _ = write!(
            footer,
            "> Note: end of results at offset {}.",
            result.offset
        );
    } else if result.total_found > result.matches.len() {
        let omitted = result.total_found - result.matches.len();
        let _ = write!(
            footer,
            "> Next: {omitted} more matches hidden by display limits. Narrow with --scope <dir> or --glob <pattern>."
        );
    }

    if result.total_found > 0 {
        if !footer.is_empty() {
            footer.push('\n');
        }
        footer.push_str("> Next: drill into any hit with `srcwalk <path>:<line>`.");
    }

    if smart_truncated {
        if !footer.is_empty() {
            footer.push('\n');
        }
        footer.push_str("> Caveat: expanded source truncated.\n> Next: use shown line range with --section <start-end>.");
    }

    if expand_budget.omitted > 0 {
        if !footer.is_empty() {
            footer.push('\n');
        }
        let expanded = expand_budget.expanded;
        let omitted = expand_budget.omitted;
        let used = expand_budget
            .cap_tokens
            .saturating_sub(expand_budget.remaining_tokens);
        let cap = expand_budget.cap_tokens;
        let _ = write!(
            footer,
            "> Note: expand cap ~{used}/{cap} tokens; expanded {expanded}, omitted {omitted}.\n> Next: drill into omitted hits with `srcwalk <path>:<line>` or `srcwalk <path> --section <symbol|range>`."
        );
    }

    let tokens = estimate_tokens(out.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(out, "\n\n({token_str} tokens)");
    if !footer.is_empty() {
        let _ = write!(out, "\n\n{footer}");
    }

    Ok(out)
}

/// Get cached outline string for a file. Returns None for non-code or huge files.
fn get_outline_str(path: &std::path::Path, cache: &OutlineCache) -> Option<std::sync::Arc<str>> {
    let file_type = crate::lang::detect_file_type(path);
    if !matches!(file_type, FileType::Code(_)) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    if meta.len() > 500_000 {
        return None;
    }
    Some(cache.get_or_compute(path, mtime, || {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let buf = content.as_bytes();
        read::outline::generate(path, file_type, &content, buf, false)
    }))
}

/// Build outline context around a match — ±2 entries around the enclosing one.
fn outline_context_for_match(
    path: &std::path::Path,
    match_line: u32,
    cache: &OutlineCache,
) -> Option<String> {
    let outline_str = get_outline_str(path, cache)?;
    let outline_lines: Vec<&str> = outline_str.lines().collect();
    if outline_lines.is_empty() {
        return None;
    }

    let match_idx = outline_lines.iter().position(|line| {
        extract_line_range(line).is_some_and(|(s, e)| match_line >= s && match_line <= e)
    })?;

    let start = match_idx.saturating_sub(2);
    let end = (match_idx + 3).min(outline_lines.len());

    let mut context = String::new();
    for (i, line) in outline_lines.iter().enumerate().take(end).skip(start) {
        if i == match_idx {
            let _ = write!(context, "\n→ {line}");
        } else {
            let _ = write!(context, "\n  {line}");
        }
    }
    Some(context)
}

/// Extract (`start_line`, `end_line`) from an outline entry like "[20-115]" or "[16]".
fn extract_line_range(line: &str) -> Option<(u32, u32)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let range_str = &trimmed[1..end];
    if let Some((a, b)) = range_str.split_once('-') {
        let start: u32 = a.trim().parse().ok()?;
        // Handle import ranges like "[1-]"
        let end: u32 = if b.trim().is_empty() {
            start
        } else {
            b.trim().parse().ok()?
        };
        Some((start, end))
    } else {
        let n: u32 = range_str.trim().parse().ok()?;
        Some((n, n))
    }
}
