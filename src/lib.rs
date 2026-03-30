#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,  // line numbers as u32, token counts — we target 64-bit
    clippy::cast_sign_loss,            // same
    clippy::cast_possible_wrap,        // u32→i32 for tree-sitter APIs
    clippy::module_name_repetitions,   // Rust naming conventions
    clippy::similar_names,             // common in parser/search code
    clippy::too_many_lines,            // one complex function (find_definitions)
    clippy::too_many_arguments,        // internal recursive AST walker
    clippy::unnecessary_wraps,         // Result return for API consistency
    clippy::struct_excessive_bools,    // CLI struct derives clap
    clippy::missing_errors_doc,        // internal pub(crate) fns don't need error docs
    clippy::missing_panics_doc,        // same
)]

pub(crate) mod budget;
pub mod cache;
pub(crate) mod classify;
pub(crate) mod edit;
pub mod error;
pub(crate) mod format;
pub mod index;
pub mod install;
pub mod map;
pub mod mcp;
pub(crate) mod read;
pub(crate) mod search;
pub(crate) mod session;
pub(crate) mod types;

use std::path::Path;

use cache::OutlineCache;
use classify::classify;
use error::TilthError;
use types::QueryType;

/// The single public API. Everything flows through here:
/// classify → match on query type → return formatted string.
pub fn run(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, TilthError> {
    run_inner(query, scope, section, budget_tokens, false, cache)
}

/// Full variant — forces full file output, bypassing smart views.
pub fn run_full(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    cache: &OutlineCache,
) -> Result<String, TilthError> {
    run_inner(query, scope, section, budget_tokens, true, cache)
}

fn run_inner(
    query: &str,
    scope: &Path,
    section: Option<&str>,
    budget_tokens: Option<u64>,
    full: bool,
    cache: &OutlineCache,
) -> Result<String, TilthError> {
    let query_type = classify(query, scope);

    let output = match query_type {
        QueryType::FilePath(path) => read::read_file(&path, section, full, cache, false)?,

        QueryType::Glob(pattern) => search::search_glob(&pattern, scope, cache)?,

        QueryType::Symbol(name) => search::search_symbol(&name, scope, cache)?,

        QueryType::Concept(text) => {
            let is_multi_word = text.contains(' ');

            if is_multi_word {
                multi_word_concept_search(&text, scope, cache)?
            } else {
                // Single-word concept: prefer definitions, then content, then any match.
                // Differs from Fallthrough which accepts any match immediately.
                single_query_search(&text, scope, cache, true)?
            }
        }

        QueryType::Content(text) => search::search_content(&text, scope, cache)?,

        QueryType::Regex(pattern) => search::search_regex(&pattern, scope, cache)?,

        QueryType::Fallthrough(text) => single_query_search(&text, scope, cache, false)?,
    };

    match budget_tokens {
        Some(b) => Ok(budget::apply(&output, b)),
        None => Ok(output),
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
    cache: &cache::OutlineCache,
    prefer_definitions: bool,
) -> Result<String, error::TilthError> {
    let sym_result = search::search_symbol_raw(text, scope)?;
    let accept_sym = if prefer_definitions {
        sym_result.definitions > 0
    } else {
        sym_result.total_found > 0
    };

    if accept_sym {
        return search::format_raw_result(&sym_result, cache);
    }

    let content_result = search::search_content_raw(text, scope)?;
    if content_result.total_found > 0 {
        return search::format_raw_result(&content_result, cache);
    }

    // For concept queries: if symbol had usages but no definitions, show those
    if prefer_definitions && sym_result.total_found > 0 {
        return search::format_raw_result(&sym_result, cache);
    }

    Err(error::TilthError::NotFound {
        path: scope.join(text),
        suggestion: read::suggest_similar_file(scope, text),
    })
}

/// Multi-word concept search: exact phrase first, then relaxed word proximity.
fn multi_word_concept_search(
    text: &str,
    scope: &Path,
    cache: &cache::OutlineCache,
) -> Result<String, error::TilthError> {
    // Try exact phrase match first
    let mut content_result = search::search_content_raw(text, scope)?;
    content_result.query = text.to_string();
    if content_result.total_found > 0 {
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

    let mut relaxed_result = search::search_regex_raw(&relaxed, scope)?;
    relaxed_result.query = text.to_string();
    if relaxed_result.total_found > 0 {
        return search::format_raw_result(&relaxed_result, cache);
    }

    let first_word = words.first().copied().unwrap_or(text);
    Err(error::TilthError::NotFound {
        path: scope.join(text),
        suggestion: read::suggest_similar_file(scope, first_word),
    })
}
