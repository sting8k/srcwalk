use std::path::Path;

use crate::classify::{self, classify};
use crate::commands::context::{
    with_artifact_note, with_artifact_read_label, ArtifactMode, ExpandedCtx,
};
use crate::commands::multi_scope::{
    parse_multi_symbol_query, unsupported_find_syntax_error, use_files_error,
};
use crate::commands::section_disambiguation::disambiguate_glob_for_section;
use crate::types::QueryType;
use crate::OutlineCache;
use crate::SrcwalkError;
use crate::{artifact, budget, format, index, read, search, session};

/// classify → match on query type → return formatted string.
pub(crate) fn run(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

pub(crate) fn run_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        false,
        cache,
    )
}

pub(crate) fn run_filtered_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        false,
        0,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::from(artifact),
        cache,
    )
}

/// Full variant — forces full file output, bypassing smart views.
pub(crate) fn run_full(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_full_filtered(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

pub(crate) fn run_full_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_full_filtered_with_artifact(
        query,
        scope,
        section,
        budget_tokens,
        limit,
        offset,
        glob,
        filter,
        false,
        cache,
    )
}

pub(crate) fn run_full_filtered_with_artifact(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: bool,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        true,
        0,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::from(artifact),
        cache,
    )
}

/// Run with expanded search — inline source for top N matches.
pub(crate) fn run_expanded(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_expanded_filtered(
        query,
        scope,
        section,
        budget_tokens,
        full,
        expand,
        limit,
        offset,
        glob,
        None,
        cache,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_expanded_filtered(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    run_inner(
        query,
        scope,
        section,
        budget_tokens,
        full,
        expand,
        limit,
        offset,
        glob,
        filter,
        ArtifactMode::Source,
        cache,
    )
}

pub(crate) fn run_files(
    pattern: &str,
    scope: &Path,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
) -> Result<String, SrcwalkError> {
    let output = search::search_files_glob(pattern, scope, limit, offset)?;
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}

fn run_inner(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    if let Some(err) = unsupported_find_syntax_error(query) {
        return Err(err);
    }

    let query_type = classify(query, scope);

    // P1.2 — disambiguate bare-filename + --section.
    // Glob classification swallows `--section` silently for bare filenames like
    // `Cart.php`. When section is set, resolve the glob now: pick the prod
    // candidate if exactly one survives test/vendor filtering, else fail loud.
    let mut resolution_note: Option<String> = None;
    let query_type = if section.is_some() {
        if let QueryType::Glob(pattern) = &query_type {
            match disambiguate_glob_for_section(pattern, scope, query)? {
                Some((picked, note)) => {
                    resolution_note = note;
                    QueryType::FilePath(picked)
                }
                None => query_type,
            }
        } else {
            query_type
        }
    } else {
        query_type
    };

    if !artifact.enabled()
        && classify::looks_like_path_with_separator(query)
        && !matches!(
            query_type,
            QueryType::FilePath(_)
                | QueryType::FilePathLine(_, _)
                | QueryType::FilePathSection(_, _)
        )
    {
        return Err(SrcwalkError::PathLikeNotFound {
            path: scope.join(query),
            scope: scope.to_path_buf(),
            basename: std::path::Path::new(query)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        });
    }

    let use_expanded = expand > 0
        && !matches!(
            query_type,
            QueryType::FilePath(_)
                | QueryType::FilePathLine(_, _)
                | QueryType::FilePathSection(_, _)
                | QueryType::Glob(_)
        );

    // Multi-symbol: comma-separated identifiers, 2..=5 items.
    // Check before main dispatch. Only activate when all parts look like identifiers
    // to avoid hijacking regex (/foo,bar/) or glob (*.{rs,ts}) queries.
    if !matches!(
        query_type,
        QueryType::Glob(_)
            | QueryType::SymbolGlob(_)
            | QueryType::FilePath(_)
            | QueryType::FilePathLine(_, _)
            | QueryType::FilePathSection(_, _)
    ) {
        if let Some(parts) = parse_multi_symbol_query(query)? {
            let session = session::Session::new();
            let sym_index = index::SymbolIndex::new();
            let bloom = index::bloom::BloomFilterCache::new();
            let expand = if expand > 0 { expand } else { 2 };
            let output = search::search_multi_symbol_expanded(
                &parts,
                scope,
                cache,
                &session,
                &sym_index,
                &bloom,
                expand,
                None,
                limit,
                offset,
                glob,
                filter,
                budget_tokens,
            )?;
            return match budget_tokens {
                Some(b) => Ok(budget::apply_preserving_footer(&output, b)),
                None => Ok(output),
            };
        }
    }

    // FilePath and Glob are read operations, not search — handle before expanded dispatch
    let output_result = match query_type {
        QueryType::FilePath(_) | QueryType::FilePathLine(_, _) | QueryType::Glob(_)
            if filter.is_some() =>
        {
            Err(SrcwalkError::InvalidQuery {
                query: query.to_string(),
                reason:
                    "--filter applies to search results and direct --callers, not file/glob reads"
                        .to_string(),
            })
        }
        QueryType::FilePath(path) => {
            let mut out = read::read_file_with_budget(&path, section, full, budget_tokens, cache)?;
            out = with_artifact_read_label(out, artifact);
            if section.is_none() && !full {
                out = artifact::add_anchors(out, &path, artifact);
            }
            if section.is_none() && !full && read::would_outline(&path) && !artifact.enabled() {
                let related = read::imports::resolve_related_files(&path);
                if !related.is_empty() {
                    let hints: Vec<String> = related
                        .iter()
                        .map(|p| format::rel_nonempty(p, scope))
                        .collect();
                    out.push_str("\n\n> Related: ");
                    out.push_str(&hints.join(", "));
                }
                out.push_str("\n> Next: use `srcwalk deps <file>` to see imports and dependents");
            }
            Ok(out)
        }
        QueryType::FilePathLine(path, line) => {
            let line_section = line.to_string();
            let effective_section = section.unwrap_or(&line_section);
            read::read_file_with_budget(&path, Some(effective_section), full, budget_tokens, cache)
                .map(|out| with_artifact_read_label(out, artifact))
        }
        QueryType::FilePathSection(path, path_section) => {
            let effective_section = section.unwrap_or(&path_section);
            read::read_file_with_budget(&path, Some(effective_section), full, budget_tokens, cache)
                .map(|out| with_artifact_read_label(out, artifact))
        }
        QueryType::Glob(_) if classify::has_glob_chars(query) => Err(use_files_error(query)),
        QueryType::Glob(pattern) => search::search_files_glob(&pattern, scope, limit, offset),
        _ if use_expanded => {
            let ctx = ExpandedCtx {
                session: session::Session::new(),
                sym_index: index::SymbolIndex::new(),
                bloom: index::bloom::BloomFilterCache::new(),
                expand,
                budget_tokens,
            };
            run_query_expanded(&query_type, scope, cache, &ctx, limit, offset, glob, filter)
        }
        _ => run_query_basic(
            &query_type,
            scope,
            cache,
            limit,
            offset,
            glob,
            filter,
            artifact,
        ),
    };

    let output = match output_result {
        Ok(output) => output,
        Err(err) => {
            return Err(match resolution_note {
                Some(note) => SrcwalkError::WithNote {
                    note,
                    source: Box::new(err),
                },
                None => err,
            });
        }
    };
    let output = with_artifact_note(output, artifact);

    let final_out = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    };
    Ok(match resolution_note {
        Some(note) => format!("{note}\n\n{final_out}"),
        None => final_out,
    })
}

/// Dispatch search queries in expanded mode (inline source for top N matches).
/// Only called for search query types — FilePath/Glob are handled before this.
fn run_query_expanded(
    query_type: &QueryType,
    scope: &Path,
    cache: &OutlineCache,
    ctx: &ExpandedCtx,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) => search::search_symbol_expanded(
            name,
            scope,
            cache,
            &ctx.session,
            &ctx.sym_index,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::SymbolGlob(pattern) => search::search_symbol_glob_expanded(
            pattern,
            scope,
            cache,
            &ctx.session,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::Concept(text) if text.contains(' ') => search::search_content_expanded(
            text,
            scope,
            cache,
            &ctx.session,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        QueryType::Concept(text) | QueryType::Fallthrough(text) => search::search_symbol_expanded(
            text,
            scope,
            cache,
            &ctx.session,
            &ctx.sym_index,
            &ctx.bloom,
            ctx.expand,
            None,
            limit,
            offset,
            glob,
            filter,
            ctx.budget_tokens,
        ),
        // FilePath/Glob/Glob never reach here (gated by use_expanded)
        QueryType::FilePath(_)
        | QueryType::FilePathLine(_, _)
        | QueryType::FilePathSection(_, _)
        | QueryType::Glob(_) => {
            unreachable!("non-search query type in expanded path")
        }
    }
}

/// Dispatch search queries in basic mode (no expansion).
/// Only called for search query types — FilePath/Glob are handled before this.
fn run_query_basic(
    query_type: &QueryType,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) if artifact.enabled() => single_query_search(
            name, scope, cache, true, limit, offset, glob, filter, artifact,
        ),
        QueryType::Symbol(name) => search::search_symbol_with_artifact(
            name, scope, cache, limit, offset, glob, filter, artifact,
        ),
        QueryType::SymbolGlob(pattern) => search::search_symbol_glob_with_artifact(
            pattern, scope, cache, limit, offset, glob, filter, artifact,
        ),
        QueryType::Concept(text) if text.contains(' ') => {
            multi_word_concept_search(text, scope, cache, limit, offset, glob, filter, artifact)
        }
        QueryType::Concept(text) => single_query_search(
            text, scope, cache, true, limit, offset, glob, filter, artifact,
        ),
        QueryType::Fallthrough(text) => single_query_search(
            text, scope, cache, false, limit, offset, glob, filter, artifact,
        ),
        QueryType::FilePath(_)
        | QueryType::FilePathLine(_, _)
        | QueryType::FilePathSection(_, _)
        | QueryType::Glob(_) => {
            unreachable!("non-search query type in basic path")
        }
    }
}

/// Shared cascade for single-word queries: symbol → content → not found.
///
/// When `prefer_definitions` is true (Concept path), only accept symbol results
/// that contain actual definitions; fall back to content otherwise.
/// When false (Fallthrough path), accept any symbol match immediately.
fn single_query_search(
    text: &str,
    scope: &Path,
    cache: &OutlineCache,
    prefer_definitions: bool,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    let mut sym_result = search::search_symbol_raw_with_artifact(text, scope, glob, artifact)?;
    search::apply_general_filter(&mut sym_result, scope, cache, filter)?;
    let accept_sym = if prefer_definitions {
        sym_result.definitions > 0
    } else {
        sym_result.total_found > 0
    };

    if accept_sym {
        search::pagination::paginate(&mut sym_result, limit, offset);
        search::compact_artifact_snippets(&mut sym_result, artifact);
        return search::format_raw_result(&sym_result, cache);
    }

    let mut content_result = search::search_content_raw_with_artifact(text, scope, glob, artifact)?;
    search::apply_general_filter(&mut content_result, scope, cache, filter)?;
    if content_result.total_found > 0 {
        search::pagination::paginate(&mut content_result, limit, offset);
        search::compact_artifact_snippets(&mut content_result, artifact);
        return search::format_raw_result(&content_result, cache);
    }

    // For concept queries: if symbol had usages but no definitions, show those
    if prefer_definitions && sym_result.total_found > 0 {
        search::pagination::paginate(&mut sym_result, limit, offset);
        search::compact_artifact_snippets(&mut sym_result, artifact);
        return search::format_raw_result(&sym_result, cache);
    }

    Err(SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, text, glob),
    })
}

/// Multi-word concept search: exact phrase first, then relaxed word proximity.
fn multi_word_concept_search(
    text: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    artifact: ArtifactMode,
) -> Result<String, SrcwalkError> {
    // Try exact phrase match first
    let mut content_result = search::search_content_raw_with_artifact(text, scope, glob, artifact)?;
    search::apply_general_filter(&mut content_result, scope, cache, filter)?;
    content_result.query = text.to_string();
    if content_result.total_found > 0 {
        search::pagination::paginate(&mut content_result, limit, offset);
        return search::format_raw_result(&content_result, cache);
    }

    // Relaxed: match all words in any order
    let words: Vec<&str> = text.split_whitespace().collect();
    let relaxed = if words.len() == 2 {
        format!(
            "{}.*{}|{}.*{}",
            regex_syntax::escape(words[0]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[0]),
        )
    } else {
        // 3+ words: match any word (OR), rely on multi_word_boost in ranking
        words
            .iter()
            .map(|w| regex_syntax::escape(w))
            .collect::<Vec<_>>()
            .join("|")
    };

    let mut relaxed_result =
        search::search_regex_raw_with_artifact(&relaxed, scope, glob, artifact)?;
    search::apply_general_filter(&mut relaxed_result, scope, cache, filter)?;
    relaxed_result.query = text.to_string();
    if relaxed_result.total_found > 0 {
        search::pagination::paginate(&mut relaxed_result, limit, offset);
        return search::format_raw_result(&relaxed_result, cache);
    }

    let first_word = words.first().copied().unwrap_or(text);
    Err(SrcwalkError::NoMatches {
        query: text.to_string(),
        scope: scope.to_path_buf(),
        suggestion: symbol_or_file_suggestion(scope, first_word, glob),
    })
}

/// Cross-convention symbol suggest first (P1.3 infra), then file-name fallback.
/// Used by symbol→content miss paths so users get a useful "Did you mean: ...".
pub(crate) fn symbol_or_file_suggestion(
    scope: &Path,
    query: &str,
    glob: Option<&str>,
) -> Option<String> {
    let hits = search::symbol::suggest(query, scope, glob, 1);
    if let Some((name, path, line)) = hits.into_iter().next() {
        // Skip case-only variants to avoid suggest loops (foo→Foo→foo).
        let q_low: String = query
            .chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        let n_low: String = name
            .chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect();
        if q_low == n_low {
            return None;
        }
        let rel = format::rel_nonempty(&path, scope);
        return Some(format!("{name} ({rel}:{line})"));
    }
    read::suggest_similar_file(scope, query)
}
