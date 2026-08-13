use std::path::{Path, PathBuf};

use crate::error::SrcwalkError;
use crate::read::PathSymbolResolution;

/// Bounded candidate list size for an ambiguous same-file selector.
const MAX_AMBIGUOUS_CANDIDATES: usize = 5;

/// How a `<path>:<symbol>`-shaped target is handled by an exact-target command
/// (`show`, `context`, `trace callees`, `trace callers`).
///
/// One shared consumer keeps the four commands honest: the same explicit intent
/// for a raw `::` selector, and no silent broadening to a bare-name search when
/// the target is missing, unreadable, unresolved, or ambiguous.
#[derive(Debug)]
pub(crate) enum PathSymbolOutcome {
    /// Not a `<path>:<symbol>` form (bare symbol, `path:range`, drive-only).
    /// The command falls through to its existing behavior unchanged.
    FallThrough,
    /// A unique exact named-file target: use this path/range directly.
    Unique {
        path: PathBuf,
        symbol: String,
        start_line: usize,
        end_line: usize,
    },
    /// The named file exists and was readable, but defines no such selector.
    /// Exact-body commands turn this into an error; `context` may still fall
    /// back to named-file (never bare-name) semantics.
    UnresolvedInNamedFile(SrcwalkError),
    /// Surface verbatim and stop: raw `::` selector, missing/unreadable file,
    /// or ambiguous same-file definition.
    Error(SrcwalkError),
}

/// Resolve a `<path>:<symbol>` target into the shared command outcome.
pub(crate) fn resolve_path_symbol_outcome(target: &str, scope: &Path) -> PathSymbolOutcome {
    match crate::read::resolve_path_symbol_resolution(target, scope) {
        PathSymbolResolution::NotForm => PathSymbolOutcome::FallThrough,
        PathSymbolResolution::Unique {
            path,
            symbol,
            start_line,
            end_line,
        } => PathSymbolOutcome::Unique {
            path,
            symbol,
            start_line,
            end_line,
        },
        PathSymbolResolution::UnsupportedColonSymbol { symbol } => {
            PathSymbolOutcome::Error(unsupported_colon_symbol_error(target, &symbol))
        }
        PathSymbolResolution::NamedPathMissing { path, symbol } => {
            PathSymbolOutcome::Error(SrcwalkError::NotFound {
                path,
                suggestion: None,
                guidance: Some(format!(
                    "`path:symbol` needs an existing named file; `{symbol}` was not resolved."
                )),
            })
        }
        PathSymbolResolution::NamedFileUnreadable { path, symbol } => {
            PathSymbolOutcome::Error(named_file_error(
                &path,
                &symbol,
                "could not be read, so the exact body was not resolved",
            ))
        }
        PathSymbolResolution::NamedFileUnresolvable { path, symbol } => {
            PathSymbolOutcome::Error(named_file_error(
                &path,
                &symbol,
                "has no structural outline, so the exact body was not resolved",
            ))
        }
        PathSymbolResolution::NamedFileUnresolved { path, symbol } => {
            PathSymbolOutcome::UnresolvedInNamedFile(named_file_error(
                &path,
                &symbol,
                "does not define that selector",
            ))
        }
        PathSymbolResolution::Ambiguous {
            path,
            symbol,
            ranges,
        } => PathSymbolOutcome::Error(ambiguous_error(&path, &symbol, &ranges)),
    }
}

/// The single explicit intent every exact-target command surfaces for a raw
/// colon-bearing selector, so agents get one consistent recovery.
fn unsupported_colon_symbol_error(target: &str, symbol: &str) -> SrcwalkError {
    SrcwalkError::InvalidQuery {
        query: target.to_string(),
        reason: format!(
            "`path:symbol` takes a plain selector; `{symbol}` contains `::`. \
             Use the last segment as the selector, or a `path:start-end` range."
        ),
    }
}

fn named_file_error(path: &Path, symbol: &str, why: &str) -> SrcwalkError {
    SrcwalkError::NotFound {
        path: path.to_path_buf(),
        suggestion: None,
        guidance: Some(format!(
            "`{}` {why}; `{symbol}` was not resolved. Use a `path:start-end` range, or `srcwalk discover {symbol}`.",
            crate::format::display_path(path)
        )),
    }
}

fn ambiguous_error(path: &Path, symbol: &str, ranges: &[(usize, usize)]) -> SrcwalkError {
    let display = crate::format::display_path(path);
    let shown = ranges.len().min(MAX_AMBIGUOUS_CANDIDATES);
    let mut candidates = ranges
        .iter()
        .take(shown)
        .map(|(start, end)| format!("{display}:{start}-{end}"))
        .collect::<Vec<_>>()
        .join(", ");
    if ranges.len() > shown {
        use std::fmt::Write as _;
        let _ = write!(candidates, ", (+{} more)", ranges.len() - shown);
    }
    SrcwalkError::InvalidQuery {
        query: format!("{display}:{symbol}"),
        reason: format!(
            "`{symbol}` has {} definitions in `{display}`; pick one exact body. Candidates: {candidates}",
            ranges.len()
        ),
    }
}
