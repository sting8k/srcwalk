use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::classify::{self, classify};
use crate::error::SrcwalkError;
use crate::types::QueryType;
use crate::{budget, format, search, types};

use crate::commands::find::symbol_or_file_suggestion;

pub(crate) fn run_multi_scope_find_filtered(
    query: &str,
    scopes: &[PathBuf],
    budget_tokens: Option<u64>,
    expand: usize,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    if let Some(err) = unsupported_find_syntax_error(query) {
        return Err(err);
    }

    if expand > 0 {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "multi-scope find does not support --expand yet; try one --scope".to_string(),
        });
    }

    let first_scope = scopes.first().ok_or_else(|| SrcwalkError::InvalidQuery {
        query: query.to_string(),
        reason: "at least one --scope is required".to_string(),
    })?;
    let query_type = classify(query, first_scope);
    if matches!(query_type, QueryType::Glob(_)) && classify::has_glob_chars(query) {
        return Err(use_files_error(query));
    }
    if matches!(
        query_type,
        QueryType::FilePath(_) | QueryType::FilePathLine(_, _) | QueryType::FilePathSection(_, _)
    ) {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "multi-scope find supports symbol/text/name-glob queries, not file reads"
                .to_string(),
        });
    }
    if let Some(parts) = parse_multi_symbol_query(query)? {
        return run_multi_scope_multi_symbol_find(
            query,
            &parts,
            scopes,
            budget_tokens,
            limit,
            offset,
            glob,
            filter,
            cache,
        );
    }

    let mut result = multi_scope_search_result(query, &query_type, scopes, cache, glob, filter)?;
    if result.total_found == 0 {
        return Err(SrcwalkError::NoMatches {
            query: query.to_string(),
            scope: common_scope(scopes),
            suggestion: scopes
                .iter()
                .find_map(|scope| symbol_or_file_suggestion(scope, query, glob)),
        });
    }

    search::rank::sort(&mut result.matches, query, &result.scope, None);
    search::pagination::paginate(&mut result, limit, offset);
    let header = multi_scope_search_header(&result, scopes);
    let mut output = search::format_raw_result_with_header(&result, cache, header)?;
    if scopes_overlap(scopes) {
        output.push_str("\n\n> Note: overlapping scopes were deduplicated.");
    }
    let final_out = match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    };
    Ok(final_out)
}

#[allow(clippy::too_many_arguments)]
fn run_multi_scope_multi_symbol_find(
    original_query: &str,
    parts: &[&str],
    scopes: &[PathBuf],
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
    cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let first_scope = scopes.first().expect("caller ensures at least one scope");
    let mut sections = Vec::with_capacity(parts.len());
    let mut total_found = 0;

    for part in parts {
        let query_type = classify(part, first_scope);
        if matches!(query_type, QueryType::Glob(_)) && classify::has_glob_chars(part) {
            return Err(use_files_error(part));
        }
        if matches!(
            query_type,
            QueryType::FilePath(_)
                | QueryType::FilePathLine(_, _)
                | QueryType::FilePathSection(_, _)
        ) {
            return Err(SrcwalkError::InvalidQuery {
                query: (*part).to_string(),
                reason: "multi-scope find supports symbol/text/name-glob queries, not file reads"
                    .to_string(),
            });
        }

        let mut result = multi_scope_search_result(part, &query_type, scopes, cache, glob, filter)?;
        total_found += result.total_found;
        search::rank::sort(&mut result.matches, part, &result.scope, None);
        search::pagination::paginate(&mut result, limit, offset);
        let header = multi_scope_search_header(&result, scopes);
        sections.push(search::format_raw_result_with_header(
            &result, cache, header,
        )?);
    }

    if total_found == 0 {
        return Err(SrcwalkError::NoMatches {
            query: original_query.to_string(),
            scope: common_scope(scopes),
            suggestion: parts.iter().find_map(|part| {
                scopes
                    .iter()
                    .find_map(|scope| symbol_or_file_suggestion(scope, part, glob))
            }),
        });
    }

    let mut output = sections.join("\n\n---\n");
    if scopes_overlap(scopes) {
        output.push_str("\n\n> Note: overlapping scopes were deduplicated.");
    }
    Ok(match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    })
}

fn multi_scope_search_result(
    query: &str,
    query_type: &QueryType,
    scopes: &[PathBuf],
    cache: &OutlineCache,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<types::SearchResult, SrcwalkError> {
    match query_type {
        QueryType::Symbol(name) => merge_scope_results(
            query,
            scopes,
            scopes
                .iter()
                .map(|scope| search_result_symbol(name, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        QueryType::SymbolGlob(pattern) => merge_scope_results(
            query,
            scopes,
            scopes
                .iter()
                .map(|scope| search_result_symbol_glob(pattern, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        QueryType::Concept(text) if text.contains(' ') => {
            let exact = scopes
                .iter()
                .map(|scope| search_result_content(text, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?;
            let exact_total: usize = exact.iter().map(|r| r.total_found).sum();
            if exact_total > 0 {
                return merge_scope_results(query, scopes, exact);
            }
            let words: Vec<&str> = text.split_whitespace().collect();
            let relaxed = relaxed_multi_word_pattern(&words);
            merge_scope_results(
                query,
                scopes,
                scopes
                    .iter()
                    .map(|scope| search_result_regex(&relaxed, scope, cache, glob, filter))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        QueryType::Concept(text) => {
            let symbols = scopes
                .iter()
                .map(|scope| search_result_symbol(text, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?;
            if symbols.iter().any(|r| r.definitions > 0) {
                return merge_scope_results(query, scopes, symbols);
            }
            let content = scopes
                .iter()
                .map(|scope| search_result_content(text, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?;
            if content.iter().any(|r| r.total_found > 0) {
                return merge_scope_results(query, scopes, content);
            }
            merge_scope_results(query, scopes, symbols)
        }
        QueryType::Fallthrough(text) => {
            let symbols = scopes
                .iter()
                .map(|scope| search_result_symbol(text, scope, cache, glob, filter))
                .collect::<Result<Vec<_>, _>>()?;
            if symbols.iter().any(|r| r.total_found > 0) {
                return merge_scope_results(query, scopes, symbols);
            }
            merge_scope_results(
                query,
                scopes,
                scopes
                    .iter()
                    .map(|scope| search_result_content(text, scope, cache, glob, filter))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        QueryType::FilePath(_)
        | QueryType::FilePathLine(_, _)
        | QueryType::FilePathSection(_, _)
        | QueryType::Glob(_) => {
            unreachable!("file query rejected before multi-scope search")
        }
    }
}

pub(crate) fn use_files_error(query: &str) -> SrcwalkError {
    SrcwalkError::unsupported_syntax(query, "srcwalk find", &find_supported_forms(None))
}

pub(crate) fn unsupported_find_syntax_error(query: &str) -> Option<SrcwalkError> {
    let parts = parse_pipe_separated_identifiers(query)?;
    let batch = format!("srcwalk find \"{}\" --scope <dir>", parts.join(", "));
    Some(SrcwalkError::unsupported_syntax(
        query,
        "srcwalk find",
        &find_supported_forms(Some(batch)),
    ))
}

fn find_supported_forms(batch: Option<String>) -> Vec<String> {
    let mut forms = Vec::new();
    if let Some(batch) = batch {
        forms.push(batch);
    }
    forms.extend([
        "srcwalk find <query> --scope <dir>".to_string(),
        "srcwalk find \"A, B, C\" --scope <dir>".to_string(),
        "srcwalk find '*Name*' --scope <dir>".to_string(),
        "srcwalk find <query> --filter kind:fn --scope <dir>".to_string(),
        "srcwalk files '<glob>' --scope <dir>  # filenames".to_string(),
        "rg '<regex>' <dir>  # raw regex".to_string(),
    ]);
    forms
}

fn parse_pipe_separated_identifiers(query: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = if query.contains("\\|") {
        query.split("\\|").map(str::trim).collect()
    } else if query.contains('|') {
        query.split('|').map(str::trim).collect()
    } else {
        return None;
    };

    let parts: Vec<&str> = parts.into_iter().filter(|part| !part.is_empty()).collect();
    if (2..=5).contains(&parts.len()) && parts.iter().all(|part| classify::is_identifier(part)) {
        Some(parts)
    } else {
        None
    }
}

pub(crate) fn parse_multi_symbol_query(query: &str) -> Result<Option<Vec<&str>>, SrcwalkError> {
    if !query.contains(',') {
        return Ok(None);
    }

    let parts: Vec<&str> = query
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let all_identifiers = parts.iter().all(|p| classify::is_identifier(p));
    if parts.len() > 5 && all_identifiers {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "multi-symbol search supports 2-5 symbols".to_string(),
        });
    }
    if parts.len() >= 2 && parts.len() <= 5 && all_identifiers {
        return Ok(Some(parts));
    }
    Ok(None)
}

fn search_result_symbol(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<types::SearchResult, SrcwalkError> {
    let mut result = search::search_symbol_raw(query, scope, glob)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    Ok(result)
}

fn search_result_symbol_glob(
    pattern: &str,
    scope: &Path,
    cache: &OutlineCache,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<types::SearchResult, SrcwalkError> {
    let mut result = search::search_symbol_glob_raw(pattern, scope, glob)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    Ok(result)
}

fn search_result_content(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<types::SearchResult, SrcwalkError> {
    let mut result = search::search_content_raw(query, scope, glob)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    Ok(result)
}

fn search_result_regex(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<types::SearchResult, SrcwalkError> {
    let mut result = search::search_regex_raw(query, scope, glob)?;
    search::apply_general_filter(&mut result, scope, cache, filter)?;
    Ok(result)
}

fn merge_scope_results(
    query: &str,
    scopes: &[PathBuf],
    results: Vec<types::SearchResult>,
) -> Result<types::SearchResult, SrcwalkError> {
    let mut seen = HashSet::new();
    let mut matches = Vec::new();
    for result in results {
        for m in result.matches {
            let key = (
                m.path.clone(),
                m.line,
                m.def_range,
                m.text.clone(),
                m.is_definition,
                m.in_comment,
            );
            if seen.insert(key) {
                matches.push(m);
            }
        }
    }
    let definitions = matches.iter().filter(|m| m.is_definition).count();
    let comments = matches.iter().filter(|m| m.in_comment).count();
    let total_found = matches.len();
    Ok(types::SearchResult {
        query: query.to_string(),
        scope: common_scope(scopes),
        matches,
        total_found,
        definitions,
        usages: total_found.saturating_sub(definitions + comments),
        comments,
        has_more: false,
        offset: 0,
    })
}

fn multi_scope_search_header(result: &types::SearchResult, scopes: &[PathBuf]) -> String {
    let total = result.matches.len();
    let parts = match (result.definitions, result.usages, result.comments) {
        (0, _, 0) => format!("{total} matches"),
        (0, _, c) => format!("{total} matches ({c} in comments)"),
        (d, u, 0) => format!("{total} matches ({d} definitions, {u} usages)"),
        (d, u, c) => format!("{total} matches ({d} definitions, {u} usages, {c} in comments)"),
    };
    let scope_list = if scopes_overlap(scopes) {
        scopes
            .iter()
            .map(|scope| format::display_path(scope))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        scopes
            .iter()
            .map(|scope| {
                let count = result
                    .matches
                    .iter()
                    .filter(|m| m.path.starts_with(scope))
                    .count();
                format!("{} ({count})", format::display_path(scope))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let scope_label = if result.offset > 0 || result.matches.len() != result.total_found {
        "Scopes on this page"
    } else {
        "Scopes"
    };
    format!(
        "# Search: \"{}\" in {} scopes — {parts}\n{scope_label}: {scope_list}",
        result.query,
        scopes.len()
    )
}

pub(crate) fn relaxed_multi_word_pattern(words: &[&str]) -> String {
    if words.len() == 2 {
        format!(
            "{}.*{}|{}.*{}",
            regex_syntax::escape(words[0]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[1]),
            regex_syntax::escape(words[0]),
        )
    } else {
        words
            .iter()
            .map(|word| regex_syntax::escape(word))
            .collect::<Vec<_>>()
            .join("|")
    }
}

fn common_scope(scopes: &[PathBuf]) -> PathBuf {
    let Some(first) = scopes.first() else {
        return PathBuf::from(".");
    };
    let mut prefix: Vec<_> = first.components().collect();
    for scope in &scopes[1..] {
        let components: Vec<_> = scope.components().collect();
        let shared = prefix
            .iter()
            .zip(components.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(shared);
    }
    if prefix.is_empty() {
        PathBuf::from(".")
    } else {
        prefix.iter().collect()
    }
}

fn scopes_overlap(scopes: &[PathBuf]) -> bool {
    scopes.iter().enumerate().any(|(i, a)| {
        scopes
            .iter()
            .enumerate()
            .any(|(j, b)| i != j && (a.starts_with(b) || b.starts_with(a)))
    })
}
