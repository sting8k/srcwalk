//! Stage-0 regex-dialect detector for `discover`.
//!
//! Real agent queries written in `rg` dialect (`parseGitUrl\(`, `a.*b`,
//! `models\.json`) must never hit a silent dead end. This detector runs
//! *before* the normal classify cascade and normalizes those queries into
//! explicit, labeled reinterpretations. It never executes a regex engine and
//! never implies one: every reinterpretation is marked `interpreted as ...`.
//!
//! Golden rule: `\` + punctuation is a regex escape, never a Windows path
//! separator. Windows drive prefixes (`C:\bin\x.exe`) and resolvable paths win
//! over regex interpretation.

use std::path::Path;

use crate::types::{QueryType, RegexCoOccurrenceQuery, RegexTextKind, RegexTextQuery};

/// Regex escape metacharacters that — when preceded by `\` — signal a
/// regex-dialect query rather than a Windows path.
const REGEX_ESCAPE_CHARS: &[char] = &['(', ')', '.', '[', '{', 'b', 'w', 's', 'd'];

/// Stage-0 detector. Returns a `QueryType` when the query is regex-dialect or
/// a `.*`/`.+` two-term co-occurrence pattern, `None` to continue the cascade.
pub fn detect(query: &str, scope: &Path) -> Option<QueryType> {
    // A Windows drive prefix or a resolvable path is a path, never a
    // regex-dialect query. `\b`/`\w`/etc. in `C:\bin` are escapes only inside
    // a regex; the drive prefix wins here.
    if looks_like_windows_drive_path(query) || resolve_exists(query, scope) {
        return None;
    }

    // `a.*b` / `minimax.*m2` / `a.+b` → bounded same-line ordered co-occurrence.
    if let Some((term1, term2)) = split_cooccurrence(query) {
        let separators = query.matches(".*").count() + query.matches(".+").count();
        return Some(QueryType::RegexCoOccurrence(RegexCoOccurrenceQuery {
            original: query.to_string(),
            term1,
            term2,
            simplified: separators > 1,
        }));
    }

    // Regex escapes: `parseGitUrl\(`, `models\.json`, `\b…`.
    if has_regex_escape(query) {
        let literal = de_escape(query);
        let symbol_core = identifier_core(&literal);
        let kind = if looks_like_bare_filename(&literal) {
            RegexTextKind::BareFilename
        } else {
            RegexTextKind::SymbolText
        };
        return Some(QueryType::RegexText(RegexTextQuery {
            original: query.to_string(),
            literal,
            symbol_core,
            kind,
        }));
    }

    None
}

/// `C:\`, `D:\`, ... — a single drive letter followed by a colon and backslash.
fn looks_like_windows_drive_path(query: &str) -> bool {
    let mut chars = query.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic() && chars.next() == Some(':') && chars.next() == Some('\\')
}

/// Does the query resolve to an existing path (file or directory)?
fn resolve_exists(query: &str, scope: &Path) -> bool {
    let path = Path::new(query);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        scope.join(path)
    };
    resolved.try_exists().unwrap_or(false)
}

/// Split a `.*`/`.+` pattern into its first two identifier-ish terms.
/// Returns `None` unless both first terms are non-empty word-like tokens, so
/// file globs like `*.rs` (which split to an empty head) are never hijacked.
pub(crate) fn split_cooccurrence(query: &str) -> Option<(String, String)> {
    for sep in [".*", ".+"] {
        let parts: Vec<&str> = query.split(sep).map(str::trim).collect();
        if parts.len() >= 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts.iter().all(|p| looks_like_word(p))
        {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }
    None
}

fn looks_like_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '$' | '@' | '.'))
}

/// Does the query contain `\` followed by a regex escape metacharacter?
fn has_regex_escape(query: &str) -> bool {
    let chars: Vec<char> = query.chars().collect();
    chars
        .windows(2)
        .any(|w| w[0] == '\\' && REGEX_ESCAPE_CHARS.contains(&w[1]))
}

/// Remove backslashes before regex escape metacharacters.
fn de_escape(query: &str) -> String {
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && REGEX_ESCAPE_CHARS.contains(&chars[i + 1]) {
            out.push(chars[i + 1]);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Identifier-ish core after stripping punctuation/escapes: `parseGitUrl\(` → `parseGitUrl`.
fn identifier_core(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '@' | '.' | '-'))
        .collect()
}

/// Does the de-escaped literal look like a bare filename (has an extension or
/// is a known extensionless filename)? Routes `models\.json` to the glob branch.
fn looks_like_bare_filename(literal: &str) -> bool {
    if literal.contains(' ') || literal.contains('/') || literal.contains('\\') {
        return false;
    }
    if let Some(dot_pos) = literal.rfind('.') {
        if dot_pos > 0 && dot_pos < literal.len() - 1 {
            return true;
        }
    }
    matches!(
        literal,
        "README"
            | "LICENSE"
            | "Makefile"
            | "GNUmakefile"
            | "Dockerfile"
            | "Containerfile"
            | "Vagrantfile"
            | "Rakefile"
            | "Gemfile"
            | "Procfile"
            | "Justfile"
            | "Taskfile"
            | "CHANGELOG"
            | "CONTRIBUTING"
            | "AUTHORS"
            | "CODEOWNERS"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::QueryType;
    use std::path::PathBuf;

    fn scope() -> PathBuf {
        PathBuf::from(".")
    }

    #[test]
    fn regex_escape_queries_detected() {
        let s = scope();
        assert!(matches!(
            detect(r"parseGitUrl\(", &s),
            Some(QueryType::RegexText(_))
        ));
        assert!(matches!(
            detect(r"models\.json", &s),
            Some(QueryType::RegexText(_))
        ));
    }

    #[test]
    fn cooccurrence_patterns_detected() {
        let s = scope();
        assert!(matches!(
            detect(r"a.*b", &s),
            Some(QueryType::RegexCoOccurrence(_))
        ));
        assert!(matches!(
            detect(r"minimax.*m2", &s),
            Some(QueryType::RegexCoOccurrence(_))
        ));
        match detect(r"budget.*truncat", &s) {
            Some(QueryType::RegexCoOccurrence(q)) => {
                assert_eq!(q.term1, "budget");
                assert_eq!(q.term2, "truncat");
            }
            other => panic!("expected co-occurrence, got {other:?}"),
        }
    }

    #[test]
    fn windows_paths_are_not_regex_dialect() {
        let s = scope();
        // `\l` is not a recognized escape → not regex dialect.
        assert!(detect(r"src\lib.rs", &s).is_none());
        // Drive prefix `C:\` wins over `\b`/`\x`.
        assert!(detect(r"C:\bin\x.exe", &s).is_none());
    }

    #[test]
    fn file_glob_star_dot_is_not_cooccurrence() {
        let s = scope();
        // `*.rs` splits to an empty head → not co-occurrence, falls through.
        assert!(detect(r"*.rs", &s).is_none());
    }

    #[test]
    fn de_escape_removes_escape_backslashes() {
        assert_eq!(de_escape(r"parseGitUrl\("), "parseGitUrl(");
        assert_eq!(de_escape(r"models\.json"), "models.json");
        assert_eq!(de_escape(r"a\.b\(c\)"), "a.b(c)");
    }

    #[test]
    fn identifier_core_strips_punctuation() {
        assert_eq!(identifier_core("parseGitUrl("), "parseGitUrl");
        assert_eq!(identifier_core("models.json"), "models.json");
    }

    #[test]
    fn bare_filename_escape_routes_to_glob_kind() {
        let s = scope();
        match detect(r"models\.json", &s) {
            Some(QueryType::RegexText(q)) => {
                assert!(matches!(q.kind, RegexTextKind::BareFilename));
            }
            other => panic!("expected regex text, got {other:?}"),
        }
        match detect(r"parseGitUrl\(", &s) {
            Some(QueryType::RegexText(q)) => {
                assert!(matches!(q.kind, RegexTextKind::SymbolText));
            }
            other => panic!("expected regex text, got {other:?}"),
        }
    }

    #[test]
    fn simplified_note_for_three_plus_terms() {
        let s = scope();
        match detect(r"a.*b.*c", &s) {
            Some(QueryType::RegexCoOccurrence(q)) => {
                assert_eq!(q.term1, "a");
                assert_eq!(q.term2, "b");
                assert!(q.simplified, "3+ terms should be marked simplified");
            }
            other => panic!("expected co-occurrence, got {other:?}"),
        }
    }
}
