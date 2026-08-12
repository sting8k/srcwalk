//! US-064 shared qualified-symbol primitives and outline resolution.
//!
//! `Type.method` (exactly one dot with both sides non-empty) resolves a
//! member `method` defined under container / on receiver `Type`. These rules
//! are shared between the search pipeline (`src/search/symbol/definitions.rs`)
//! and the `--section` reader (`src/read/section.rs`) so receiver/container
//! interpretation never diverges between discovery and read.

use crate::types::{Lang, OutlineEntry, OutlineKind};

/// Split a `Q.N` symbol query — exactly one dot, both sides non-empty.
/// Multi-dot queries (`a.b.c`) and trailing/leading dots return `None`.
pub(crate) fn split_dot_symbol_query(query: &str) -> Option<(&str, &str)> {
    let mut parts = query.split('.');
    let qualifier = parts.next()?;
    let plain = parts.next()?;
    if parts.next().is_some() || qualifier.is_empty() || plain.is_empty() {
        return None;
    }
    Some((qualifier, plain))
}

/// Normalize a Go receiver type — strip leading `*` and generic params:
/// `*Batch` -> `Batch`, `syncQueue[T]` -> `syncQueue`. Single source of truth
/// shared by search and the `--section` reader.
pub(crate) fn normalize_receiver_type(receiver: &str) -> String {
    let stripped = receiver.trim().trim_start_matches('*').trim();
    stripped
        .chars()
        .take_while(|&c| c != '[')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Normalize an outline container name — Rust `impl X` entries count as `X`.
pub(crate) fn normalize_outline_container(name: &str) -> String {
    name.strip_prefix("impl ")
        .unwrap_or(name)
        .trim()
        .to_string()
}

/// Is this outline kind a container that qualifies its members?
pub(crate) fn outline_container_kind(kind: OutlineKind) -> bool {
    matches!(
        kind,
        OutlineKind::Class
            | OutlineKind::Struct
            | OutlineKind::Module
            | OutlineKind::Interface
            | OutlineKind::Enum
    )
}

/// Extract a Go receiver type from a method's outline signature. Signatures
/// look like `func (b *Batch) Set(...)`. Only real methods (receiver group
/// immediately after `func`) qualify; a plain `func Set(...)` returns `None`.
/// Normalization (pointer/value/generic) flows through `normalize_receiver_type`.
fn go_receiver_from_signature(signature: &str) -> Option<String> {
    let sig = signature.trim().strip_prefix("func")?.trim_start();
    if !sig.starts_with('(') {
        return None;
    }
    let close = sig.find(')')?;
    let receiver = &sig[1..close];
    let type_text = receiver.split_whitespace().last()?;
    Some(normalize_receiver_type(type_text))
}

/// Resolve a symbol selector (bare name or `Q.N`) against a file's outline
/// entries, returning the first matching `(start_line, end_line)` inclusive.
///
/// Exact dotted-name precedence is preserved: a legitimate exact entry.name /
/// signature / CSS / document dotted name is resolved before `Q.N` is
/// interpreted as qualifier + plain name. Bare names resolve through the exact
/// path only.
pub(crate) fn resolve_selector_first(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    selector: &str,
) -> Option<(u32, u32)> {
    if let Some((start, end)) = find_exact_in_entries(entries, selector) {
        return Some((start, end));
    }
    if let Some((qualifier, plain)) = split_dot_symbol_query(selector) {
        if let Some((start, end)) = qualified_outline_ranges(entries, lang, qualifier, plain)
            .into_iter()
            .next()
        {
            return Some((start, end));
        }
    }
    None
}

/// Recursively find an exact entry.name / signature / CSS / document match.
fn find_exact_in_entries(entries: &[OutlineEntry], symbol: &str) -> Option<(u32, u32)> {
    for entry in entries {
        if entry.name == symbol
            || entry.signature.as_deref() == Some(symbol)
            || crate::lang::css::outline_name_matches(entry.kind, &entry.name, symbol)
            || crate::lang::document::outline_name_matches(entry.kind, &entry.name, symbol)
        {
            return Some((entry.start_line, entry.end_line));
        }
        if let Some((start, end)) = find_exact_in_entries(&entry.children, symbol) {
            return Some((start, end));
        }
    }
    None
}

/// Resolve `Q.N` against outline entries, threading container names down into
/// children (class/struct/module/interface/enum) and matching Go methods by
/// receiver type parsed from the signature. Returns every matching range.
fn qualified_outline_ranges(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    qualifier: &str,
    plain: &str,
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    collect_qualified(entries, lang, qualifier, plain, None, &mut out);
    out
}

fn collect_qualified(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    qualifier: &str,
    plain: &str,
    container: Option<&str>,
    out: &mut Vec<(u32, u32)>,
) {
    for entry in entries {
        if entry.name == plain {
            let container_match = container == Some(qualifier);
            let receiver_match = lang == Some(Lang::Go)
                && entry.kind == OutlineKind::Function
                && entry
                    .signature
                    .as_deref()
                    .and_then(go_receiver_from_signature)
                    .as_deref()
                    == Some(qualifier);
            if container_match || receiver_match {
                out.push((entry.start_line, entry.end_line));
            }
        }
        let child_container = if outline_container_kind(entry.kind) {
            Some(normalize_outline_container(&entry.name))
        } else {
            container.map(str::to_string)
        };
        collect_qualified(
            &entry.children,
            lang,
            qualifier,
            plain,
            child_container.as_deref(),
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_normalization_covers_pointer_value_and_generic() {
        assert_eq!(normalize_receiver_type("*Batch"), "Batch");
        assert_eq!(normalize_receiver_type("*syncQueue[T]"), "syncQueue");
        assert_eq!(normalize_receiver_type("syncQueue[T]"), "syncQueue");
        assert_eq!(normalize_receiver_type("Queue"), "Queue");
        assert_eq!(normalize_receiver_type("*pkg[K,V]"), "pkg");
        assert_eq!(normalize_receiver_type("  *Batch  "), "Batch");
    }

    #[test]
    fn go_receiver_from_signature_distinguishes_method_from_function() {
        assert_eq!(
            go_receiver_from_signature("func (b *Batch) Set(v int)"),
            Some("Batch".to_string())
        );
        assert_eq!(
            go_receiver_from_signature("func (b Batch) Value()"),
            Some("Batch".to_string())
        );
        assert_eq!(
            go_receiver_from_signature("func (q *syncQueue[T]) Push(x T)"),
            Some("syncQueue".to_string())
        );
        // A plain function has no receiver group right after `func`.
        assert_eq!(go_receiver_from_signature("func Set(x int) int"), None);
    }
}
