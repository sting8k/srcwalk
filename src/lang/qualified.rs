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

/// The terminal bare callable name from a possibly-qualified callable label
/// (`Class.method`, `mod::func`, or bare `method`). This is the single shared
/// source used by direct callers, BFS frontiers, and recovery for relation
/// lookup keys. `Class.method` -> `method`, `mod::func` -> `func`, bare name
/// unchanged. Commands must use this helper and never re-split on `.` or `::`.
pub(crate) fn terminal_callable_key(qualified: &str) -> &str {
    // Terminal key = the segment after the RIGHTMOST supported separator of
    // either kind (`.` or `::`), so `A.B::method` -> `method` and
    // `A::B.method` -> `method` regardless of separator order/subtype/width.
    // `(start_byte, width)` of the rightmost separator.
    let mut sep: Option<(usize, usize)> = None;
    let bytes = qualified.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'.' {
            sep = Some((idx, 1));
            idx += 1;
        } else if bytes[idx] == b':' && idx + 1 < bytes.len() && bytes[idx + 1] == b':' {
            sep = Some((idx, 2));
            idx += 2;
        } else {
            idx += 1;
        }
    }
    match sep {
        // Rightmost separator is not trailing: terminal = suffix after it.
        Some((start, width)) if start + width < qualified.len() => &qualified[start + width..],
        // Trailing separator (`method.` / `method::`): the key is the prefix up
        // to the separator, so it is never an empty lookup string.
        Some((start, _)) if start > 0 => &qualified[..start],
        // `.` / `::` alone -> explicitly empty (no invented value).
        Some(_) => "",
        None => qualified,
    }
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
    resolve_selector_matches(entries, lang, selector)
        .into_iter()
        .next()
}

/// Resolve a symbol selector against a file's outline, returning every distinct
/// matching `(start_line, end_line)` range in deterministic order. This is the
/// shared cardinality primitive for canonical `path:symbol` targets.
///
/// Precedence mirrors US-064 exact selection first (distinct sorted), then
/// `Q.N` qualifier + plain resolution (distinct sorted). Duplicate outline rows
/// covering the same `(start,end)` range collapse to one entry, so N>1 here
/// means genuinely distinct same-file definitions (e.g. overloads).
pub(crate) fn resolve_selector_matches(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    selector: &str,
) -> Vec<(u32, u32)> {
    let exact = find_exact_ranges(entries, selector);
    if !exact.is_empty() {
        return exact;
    }
    if let Some((qualifier, plain)) = split_dot_symbol_query(selector) {
        return qualified_distinct_ranges(entries, lang, qualifier, plain);
    }
    Vec::new()
}

/// Collect every distinct exact-match range (name/signature/CSS/document
/// dotted name) in sorted order, collapsing duplicate outline rows.
fn find_exact_ranges(entries: &[OutlineEntry], symbol: &str) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    collect_exact_ranges(entries, symbol, &mut ranges);
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

fn collect_exact_ranges(entries: &[OutlineEntry], symbol: &str, out: &mut Vec<(u32, u32)>) {
    for entry in entries {
        if entry.name == symbol
            || entry.signature.as_deref() == Some(symbol)
            || crate::lang::css::outline_name_matches(entry.kind, &entry.name, symbol)
            || crate::lang::document::outline_name_matches(entry.kind, &entry.name, symbol)
        {
            out.push((entry.start_line, entry.end_line));
        }
        collect_exact_ranges(&entry.children, symbol, out);
    }
}

/// Build the canonical `Container.Member` selector for the definition occupying
/// `(start_line, end_line)` named `member`, from outline primitives only (never
/// the display-qualified name). The owning container is the deepest container
/// threaded on the path to the member, using the SAME threading rule as
/// `collect_qualified`, so the selector the resolver accepts is the one we
/// emit. A class/struct/interface deeper than a namespace naturally wins,
/// dropping the namespace prefix (C# `System.Text.Json` ->
/// `JsonSerializerOptions.GetTypeInfoInternal`). Go methods are top-level but
/// their receiver type is the owning container, so `Batch.Set` is emitted.
/// Returns `None` only when a body has no resolvable owning container (a
/// top-level non-Go function).
pub(crate) fn selector_from_outline(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    start_line: u32,
    end_line: u32,
    member: &str,
) -> Option<String> {
    let mut out = None;
    build_outline_selector(entries, lang, start_line, end_line, member, None, &mut out);
    out
}

fn build_outline_selector(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    start_line: u32,
    end_line: u32,
    member: &str,
    container: Option<&str>,
    out: &mut Option<String>,
) {
    for entry in entries {
        if entry.start_line == start_line
            && entry.end_line == end_line
            && entry.name == member
            && entry.kind == OutlineKind::Function
        {
            let qualifier = if lang == Some(Lang::Go) {
                entry
                    .signature
                    .as_deref()
                    .and_then(go_receiver_from_signature)
            } else {
                container.map(str::to_string)
            };
            if let Some(q) = qualifier {
                *out = Some(format!("{q}.{member}"));
            }
            return;
        }
        let child_container = if outline_container_kind(entry.kind) {
            Some(normalize_outline_container(&entry.name))
        } else {
            container.map(str::to_string)
        };
        build_outline_selector(
            &entry.children,
            lang,
            start_line,
            end_line,
            member,
            child_container.as_deref(),
            out,
        );
        if out.is_some() {
            return;
        }
    }
}

/// Resolve `Q.N` against outline entries, returning distinct sorted ranges.
/// Same container/receiver threading as `resolve_selector_first`, but collapses
/// duplicate outline rows and sorts deterministically for cardinality checks.
fn qualified_distinct_ranges(
    entries: &[OutlineEntry],
    lang: Option<Lang>,
    qualifier: &str,
    plain: &str,
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    collect_qualified(entries, lang, qualifier, plain, None, &mut out);
    out.sort_unstable();
    out.dedup();
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
    fn terminal_callable_key_returns_bare_terminal_name() {
        assert_eq!(terminal_callable_key("Class.method"), "method");
        assert_eq!(terminal_callable_key("mod::func"), "func");
        assert_eq!(terminal_callable_key("plain"), "plain");
        assert_eq!(terminal_callable_key("A::b::c"), "c");
        assert_eq!(terminal_callable_key(""), "");
    }

    #[test]
    fn terminal_callable_key_uses_rightmost_separator_across_both_forms() {
        // BLOCKER 2 regression: any separator must not win over a later one of
        // the other kind. Terminal is after the RIGHTMOST `.` or `::`.
        assert_eq!(terminal_callable_key("A.B::method"), "method");
        assert_eq!(terminal_callable_key("A::B.method"), "method");
        assert_eq!(terminal_callable_key("A.B.C"), "C");
        assert_eq!(terminal_callable_key("A::B::C"), "C");
        assert_eq!(terminal_callable_key("a.b::c.d"), "d");
        assert_eq!(terminal_callable_key("a::b.c::d"), "d");
        // Leading separators: suffix is non-empty.
        assert_eq!(terminal_callable_key(".method"), "method");
        assert_eq!(terminal_callable_key("::method"), "method");
        // Trailing separators yield the prefix, never an empty key.
        assert_eq!(terminal_callable_key("method."), "method");
        assert_eq!(terminal_callable_key("method::"), "method");
        assert_eq!(terminal_callable_key("A.B."), "A.B");
        // Bare / degenerate.
        assert_eq!(terminal_callable_key("plain"), "plain");
        assert_eq!(terminal_callable_key("A...B"), "B");
        // Empty input stays empty (explicit, not invented).
        assert_eq!(terminal_callable_key(""), "");
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

    fn fn_entry(name: &str, start: u32, end: u32, signature: Option<&str>) -> OutlineEntry {
        OutlineEntry {
            kind: OutlineKind::Function,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: signature.map(str::to_string),
            children: Vec::new(),
            doc: None,
        }
    }

    fn container(
        kind: OutlineKind,
        name: &str,
        start: u32,
        end: u32,
        children: Vec<OutlineEntry>,
    ) -> OutlineEntry {
        OutlineEntry {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: None,
            children,
            doc: None,
        }
    }

    // C#: namespace is a Module wrapper; the owning container is the class.
    // Emit must drop the namespace and produce `JsonSerializerOptions.GetTypeInfoInternal`.
    #[test]
    fn selector_from_outline_drops_namespace_uses_owning_class() {
        let entries = vec![container(
            OutlineKind::Module,
            "System.Text.Json",
            1,
            120,
            vec![container(
                OutlineKind::Class,
                "JsonSerializerOptions",
                2,
                118,
                vec![fn_entry("GetTypeInfoInternal", 55, 65, None)],
            )],
        )];
        let lang = Some(Lang::CSharp);
        let selector = selector_from_outline(&entries, lang, 55, 65, "GetTypeInfoInternal");
        assert_eq!(
            selector.as_deref(),
            Some("JsonSerializerOptions.GetTypeInfoInternal")
        );
        // And the emitted selector resolves UNIQUELY to exactly this body through
        // the production resolver.
        let matches = resolve_selector_matches(&entries, lang, selector.as_deref().unwrap());
        assert_eq!(matches, vec![(55, 65)]);
    }

    // N>1 same-file distinct definitions (e.g. overloads / multiple impl blocks)
    // => Ambiguous, never canonical emit (symbol_backed false).
    #[test]
    fn same_file_distinct_matches_report_cardinality_no_canonical_emit() {
        let entries = vec![container(
            OutlineKind::Class,
            "JsonSerializerContext",
            1,
            120,
            vec![
                fn_entry("GetTypeInfo", 108, 108, None),
                fn_entry("GetTypeInfo", 110, 118, None),
            ],
        )];
        let lang = Some(Lang::CSharp);
        let selector = selector_from_outline(&entries, lang, 108, 108, "GetTypeInfo");
        assert_eq!(
            selector.as_deref(),
            Some("JsonSerializerContext.GetTypeInfo")
        );
        // Two distinct bodies => cardinality 2: this selector must NOT be treated
        // as uniquely resolving to either body.
        let matches = resolve_selector_matches(&entries, lang, selector.as_deref().unwrap());
        assert_eq!(matches, vec![(108, 108), (110, 118)]);
    }

    // Duplicate outline rows covering the SAME (start,end) collapse to one entry;
    // N>1 means genuinely distinct ranges only.
    #[test]
    fn duplicate_rows_same_range_are_deduped_not_false_overload() {
        let entries = vec![container(
            OutlineKind::Struct,
            "Thing",
            1,
            50,
            vec![fn_entry("Run", 10, 20, None), fn_entry("Run", 10, 20, None)],
        )];
        let lang = Some(Lang::CSharp);
        let selector = selector_from_outline(&entries, lang, 10, 20, "Run");
        assert_eq!(selector.as_deref(), Some("Thing.Run"));
        let matches = resolve_selector_matches(&entries, lang, selector.as_deref().unwrap());
        assert_eq!(matches, vec![(10, 20)]);
    }

    // Go: method receiver is the owning container; emit `Batch.Set`.
    #[test]
    fn selector_from_outline_go_uses_receiver_type() {
        let entries = vec![fn_entry("Set", 10, 15, Some("func (b *Batch) Set(v int)"))];
        let lang = Some(Lang::Go);
        let selector = selector_from_outline(&entries, lang, 10, 15, "Set");
        assert_eq!(selector.as_deref(), Some("Batch.Set"));
        let matches = resolve_selector_matches(&entries, lang, selector.as_deref().unwrap());
        assert_eq!(matches, vec![(10, 15)]);
    }

    // A top-level non-Go function has no owning container => no canonical selector.
    #[test]
    fn top_level_non_go_function_has_no_canonical_selector() {
        let entries = vec![fn_entry("helper", 3, 8, None)];
        let selector = selector_from_outline(&entries, Some(Lang::Rust), 3, 8, "helper");
        assert_eq!(selector, None);
    }
}
