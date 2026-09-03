//! Rust owner-region extraction (US-067 phase 2).
//!
//! Supported named owners: top-level and body-bearing `function_item` nodes,
//! nested module/function `::` paths, inherent `Type::method`, trait default
//! `Trait::method`, and trait impl exactly `<Type as Trait>::method`.
//!
//! Abstractions/barriers: every `closure_expression` is an `AnonymousBarrier`;
//! body-less `function_signature_item` (trait/extern declarations) abstain so
//! they cannot fall through; an `impl_item` whose identity is missing/unreadable
//! is a full-callable `AnonymousBarrier` (never an unqualified fallback); a
//! `mod_item` with no structural name, and any callable reached through an
//! anonymous collapsed-module block, barrier so guessed child names never leak;
//! `macro_definition` regions barrier so macro-generated owners are not guessed.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    any_error_overlaps_bytes, attribute_line, collect_error_ranges, degrade_named_on_error,
    ErrorRange, OwnerAttribution, OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned Rust callable manifest (anti-drift contract).
///
/// Inventories are kept distinct so a grammar change to one category cannot
/// silently reclassify a node into another:
///
/// * (a) executable callable kinds that MUST be classified `Named`/`Barrier`;
/// * (b) binding/container/wrapper kinds used only for naming/context;
/// * (c) body-less declaration kinds that abstain/barrier (no owner region).
#[cfg(test)]
const RUST_EXECUTABLE_CALLABLES: &[&str] = &["function_item", "closure_expression"];
#[cfg(test)]
const RUST_CONTAINER_KINDS: &[&str] = &["mod_item", "impl_item", "trait_item", "foreign_mod_item"];
#[cfg(test)]
const RUST_BODILESS_DECLARATIONS: &[&str] = &["function_signature_item"];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-rust` `NODE_TYPES` JSON
/// (tree-sitter-rust 0.24.2). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit of the inventories above.
#[cfg(test)]
const RUST_NODE_TYPES_FINGERPRINT: u64 = 0x50a2_cce6_3015_8590;

/// Stable FNV-1a 64-bit hash, dependency-free and reproducible across builds.
#[cfg(test)]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Parse a Rust file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn rust_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Rust)?;
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return None;
    }
    let tree = parser.parse(content, None)?;
    if tree.root_node().kind() == "ERROR" {
        return None;
    }
    let bytes = content.as_bytes();
    let errors = collect_error_ranges(&tree, bytes);
    let mut regions = Vec::new();
    let mut prefixes: Vec<String> = Vec::new();
    walk(
        tree.root_node(),
        bytes,
        path,
        &mut prefixes,
        &errors,
        &mut regions,
        false,
    );
    Some((regions, errors))
}

/// Walk a Rust tree, emitting `Named` regions for body-bearing `function_item`
/// nodes and `AnonymousBarrier` regions for every other callable (closures,
/// body-less signatures, and callables whose enclosing container identity is
/// unknown). A `prefixes` stack carries `::`-suffixed path segments (modules,
/// functions, inherent/trait impls, traits) so nested callables get their full
/// qualified name. `impl` identity text is preserved from source tokens with
/// whitespace collapsed; an unreadable impl becomes a full-callable barrier.
///
/// `recovery` is true when walking inside a collapsed malformed container
/// (`mod <missing-name> {`, `impl <unreadable> {`, pointer-less `trait {`),
/// which tree-sitter recovers as an `expression_statement`-wrapped block with
/// an immediately-preceding ERROR sibling. Under `recovery` every contained
/// callable is classified as an `AnonymousBarrier` (never a guessed name), but
/// traversal still continues so EVERY callable node is classified — omission
/// is a contract violation. A structurally named callable inside an ordinary
/// anonymous closure barrier remains eligible when its naming context is valid.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    recovery: bool,
) {
    match node.kind() {
        "function_item" => walk_function(node, bytes, path, prefixes, errors, regions, recovery),
        "closure_expression" => {
            // Anonymous barrier: a hit inside a closure must not fall through
            // to an enclosing named owner. Recurse so a named item nested
            // inside a closure still gets classified; under a valid naming
            // context it remains eligible within its narrower range.
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
            recurse(node, bytes, path, prefixes, errors, regions, recovery);
        }
        "function_signature_item" => {
            // Body-less trait/extern signature: abstain so it cannot fall
            // through to an enclosing owner. No body to recurse into.
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
        }
        "impl_item" => walk_impl(node, bytes, path, prefixes, errors, regions, recovery),
        "trait_item" => {
            if recovery {
                // Invalid-identity context: barrier the whole trait and keep
                // traversing so every contained callable is classified.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, prefixes, errors, regions, true);
            } else if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                prefixes.push(format!("{}::", name.trim()));
                recurse(node, bytes, path, prefixes, errors, regions, false);
                prefixes.pop();
            } else {
                // Nameless trait: unknown lexical container identity.
                // Barrier and continue so contained callables (usually
                // signatures) are classified, never guessed.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, prefixes, errors, regions, true);
            }
        }
        "mod_item" => {
            if recovery {
                // Invalid-identity context: barrier the whole module and keep
                // traversing so every contained callable is classified.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, prefixes, errors, regions, true);
            } else if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                prefixes.push(format!("{}::", name.trim()));
                recurse(node, bytes, path, prefixes, errors, regions, false);
                prefixes.pop();
            } else {
                // Nameless module: unknown lexical container identity.
                // Barrier and continue so every contained callable is
                // classified, never exposed with a guessed name.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, prefixes, errors, regions, true);
            }
        }
        "foreign_mod_item" => {
            // Extern block: body-less signatures inside are barriers; the
            // block itself is a transparent container (no name to qualify).
            recurse(node, bytes, path, prefixes, errors, regions, recovery);
        }
        "macro_definition" => {
            // macro_rules! is not a callable; its rule bodies are not code to
            // recurse into. Barrier so generated-owner lines never leak.
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
        }
        "expression_statement" => {
            if is_recovery_wrapper(node) {
                // A collapsed malformed container (mod/impl/trait with no valid
                // identity) recovers here: an expression_statement wrapping a
                // block of declarations with an immediately-preceding ERROR. Its
                // contained callables have unknown identity, so the whole block
                // is a barrier and traversal continues under `recovery`.
                if let Some(block) = node.named_child(0) {
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line: block.start_position().row as u32 + 1,
                        end_line: block.end_position().row as u32 + 1,
                    });
                }
                recurse(node, bytes, path, prefixes, errors, regions, true);
            } else {
                recurse(node, bytes, path, prefixes, errors, regions, recovery);
            }
        }
        // `macro_invocation` is a statement/expression, not a callable; do not
        // guess owners from its generated token tree. Skip (enclosing owner
        // still owns the line).
        _ => recurse(node, bytes, path, prefixes, errors, regions, recovery),
    }
}

/// Whether `node` is the recovery wrapper of a collapsed malformed container:
/// an `expression_statement` whose first named child is a `block` and which has
/// an immediately-preceding ERROR sibling. This is the structural fingerprint
/// of `mod <missing-name> {`, an unreadable `impl {`, or name-less `trait
/// {` (tree-sitter parses the stray `{ ... }` as a statement block). It is NOT
/// a valid nested block: those (standalone block, if/loop/match bodies) have no
/// ERROR sibling, so a lexically nested `function_item` inside them stays
/// eligible. The preceding ERROR must itself announce a collapsed mod/impl/trait
/// container (its subtree contains the relevant item-introducer keyword token),
/// so an unrelated local ERROR before a valid standalone block is rejected.
///
/// NOTE: the keyword may be an unnamed token, so all ERROR children are walked.
fn is_recovery_wrapper(node: Node<'_>) -> bool {
    if node.kind() != "expression_statement" {
        return false;
    }
    if node
        .named_child(0)
        .is_none_or(|first| first.kind() != "block")
    {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    let mut cursor = parent.walk();
    let siblings: Vec<Node<'_>> = parent.named_children(&mut cursor).collect();
    let Some(idx) = siblings.iter().position(|s| s.id() == node.id()) else {
        return false;
    };
    idx > 0
        && siblings[idx - 1].kind() == "ERROR"
        && error_is_collapsed_container(siblings[idx - 1])
}

/// Whether an ERROR node announces a collapsed `mod`/`impl`/`trait` container:
/// its subtree contains an anonymous grammar token of kind `mod`, `impl`, or
/// `trait`. Traversal is purely structural (`Node::kind()`, no source text/bytes,
/// no whole-ERROR fallback) and recurses through ALL children because the token
/// may sit below the immediate child. This rejects unrelated local ERRORs (e.g.
/// a stray `struct`/`enum`/`foo(`) that happen to be immediately followed by a
/// valid standalone block, so a clean nested `function_item` with no ERROR
/// overlap stays eligible.
fn error_is_collapsed_container(error: Node<'_>) -> bool {
    fn contains_item_keyword(node: Node<'_>) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() && matches!(child.kind(), "mod" | "impl" | "trait") {
                return true;
            }
            if contains_item_keyword(child) {
                return true;
            }
        }
        false
    }
    contains_item_keyword(error)
}

/// Emit a `Named` region for a body-bearing callable, or a full-callable
/// `AnonymousBarrier` when the callable is under an invalid-identity/recovery
/// context or has no structural name. Traversal always continues so every
/// nested callable is classified (never omitted).
fn walk_function(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    recovery: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if recovery {
        // Under a collapsed malformed container the enclosing identity is
        // unknown; classify this callable as a barrier and keep traversing so
        // deeper callables are also classified (never a guessed partial name).
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    if let Some(name) = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
    {
        let name = name.to_string();
        let qualified = join_prefixes(prefixes, &name);
        let anchor = OwnerAnchor {
            path: path.to_path_buf(),
            name: name.clone(),
            receiver_var: None,
            receiver_type: None,
            package_dir: std::path::PathBuf::from("."),
            start_line,
            end_line,
            language: Lang::Rust,
            display_name: qualified,
        };
        let region = OwnerRegion::Named(anchor);
        let region = degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
        regions.push(region);
        prefixes.push(format!("{name}::"));
        recurse(node, bytes, path, prefixes, errors, regions, false);
        prefixes.pop();
    } else {
        // Nameless function: unknown identity; barrier and keep traversing so
        // deeper callables are classified (never guessed).
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
    }
}

/// Walk an `impl_item`: inherent (`impl Type`) or trait (`impl Trait for
/// Type`). Pushes the correctly-qualified prefix so methods become
/// `Type::method` or `<Type as Trait>::method`. An unreadable/missing impl
/// identity becomes a full-callable barrier over the whole impl (never an
/// unqualified fallback) while traversal continues under an invalid-identity
/// context so every contained callable is classified.
fn walk_impl(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    recovery: bool,
) {
    let body_byte = node
        .child_by_field_name("body")
        .map_or(node.end_byte(), |b| b.start_byte());
    // The signature region is from the `impl` keyword to the body. An ERROR
    // there means the identity text is unreadable -> full barrier.
    let identity_unreadable = any_error_overlaps_bytes(errors, node.start_byte(), body_byte);
    let type_text = node
        .child_by_field_name("type")
        .and_then(|n| n.utf8_text(bytes).ok())
        .map(normalize_impl_text);
    let trait_text = node
        .child_by_field_name("trait")
        .and_then(|n| n.utf8_text(bytes).ok())
        .map(normalize_impl_text);
    let prefix = match (&type_text, &trait_text) {
        (Some(ty), None) => Some(format!("{ty}::")),
        (Some(ty), Some(tr)) => Some(format!("<{ty} as {tr}>::")),
        _ => None,
    };
    if recovery {
        // Under a collapsed malformed container: barrier the whole impl and
        // keep traversing so every contained callable is classified.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    if let (Some(prefix), false) = (prefix, identity_unreadable) {
        prefixes.push(prefix);
        recurse(node, bytes, path, prefixes, errors, regions, false);
        prefixes.pop();
    } else {
        // Unreadable/missing impl identity: full-callable barrier; do not
        // emit unqualified method names, but keep traversing so every
        // contained callable is classified (never omitted).
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
    }
}

#[allow(clippy::too_many_arguments)]
fn recurse(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    recovery: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, bytes, path, prefixes, errors, regions, recovery);
    }
}
fn join_prefixes(prefixes: &[String], name: &str) -> String {
    if prefixes.is_empty() {
        name.to_string()
    } else {
        let mut joined = prefixes.concat();
        joined.push_str(name);
        joined
    }
}

/// Preserve an impl type/trait node's source tokens and punctuation, trimming
/// and collapsing each `split_whitespace` run to one ASCII space. No semantic
/// resolution (e.g. `Foo<T>` stays `Foo<T>`, `Vec < T >` collapses to
/// `Vec < T >`).
fn normalize_impl_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Attribute a Rust hit line to a named owner, honoring local errors.
pub(crate) fn rust_owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> OwnerAttribution<'a> {
    attribute_line(regions, errors, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.rs")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        rust_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = rust_owner_for(regions, errors, line).named();
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            rust_owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn top_level_function_and_inherent_method() {
        let (r, e) = parse("struct Foo {}\nimpl Foo {\n    fn a() {}\n    fn b(&self) {}\n}\n");
        assert_owner(&r, &e, 3, "Foo::a");
        assert_owner(&r, &e, 4, "Foo::b");
        assert_abstain(&r, &e, 2); // impl header line
    }

    #[test]
    fn trait_impl_uses_type_as_trait_identity() {
        let (r, e) = parse("impl Display for Foo {\n    fn fmt(&self) {}\n}\n");
        assert_owner(&r, &e, 2, "<Foo as Display>::fmt");
        assert_abstain(&r, &e, 1); // impl header line
    }

    #[test]
    fn trait_default_body_some_and_body_less_signature_barriers() {
        let (r, e) =
            parse("trait Greet {\n    fn hi();\n    fn bye() {\n        let x = 1;\n    }\n}\n");
        assert_owner(&r, &e, 3, "Greet::bye");
        assert_abstain(&r, &e, 2); // body-less signature
        assert_abstain(&r, &e, 1); // trait header line
    }

    #[test]
    fn nested_module_and_function_scopes_qualify_with_double_colon() {
        let (r, e) = parse(
            "mod a {\n    mod b {\n        fn f() {\n            let x = 1;\n        }\n    }\n}\n",
        );
        assert_owner(&r, &e, 3, "a::b::f");
        assert_abstain(&r, &e, 1); // mod a header
        assert_abstain(&r, &e, 2); // mod b header
    }

    #[test]
    fn nested_function_inside_function_qualifies() {
        let (r, e) =
            parse("fn outer() {\n    fn inner() {\n        let x = 1;\n    }\n    inner();\n}\n");
        assert_owner(&r, &e, 1, "outer");
        assert_owner(&r, &e, 2, "outer::inner");
        assert_owner(&r, &e, 5, "outer");
    }

    #[test]
    fn closure_expression_is_barrier_preventing_outer_fallthrough() {
        let (r, e) = parse("fn outer() {\n    let f = |x| {\n        x + 1\n    };\n}\n");
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2); // closure line
        assert_abstain(&r, &e, 3); // closure body
    }

    #[test]
    fn nested_closure_abstains_without_named_item() {
        let (r, e) = parse("fn factory() {\n    || {\n        |x| x + 1\n    }\n}\n");
        assert_owner(&r, &e, 1, "factory");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn generic_inherent_impl_whitespace_normalized() {
        // `Stack<T>` keeps its tokens; the identity is `Stack<T>::push`.
        let (r, e) = parse(
            "impl<T: Default> Stack<T> {\n    fn push(self) {\n        let x = 1;\n    }\n}\n",
        );
        assert_owner(&r, &e, 2, "Stack<T>::push");
        assert_abstain(&r, &e, 1);
    }

    #[test]
    fn impl_identity_whitespace_runs_collapsed_to_single_space() {
        let (r, e) = parse(
            "impl<T: Bound> Foo < T > for Bar {\n    fn m() {\n        let x = 1;\n    }\n}\n",
        );
        // Source tokens preserved with whitespace collapsed: `<Bar as Foo < T >>::m`.
        assert_owner(&r, &e, 2, "<Bar as Foo < T >>::m");
        assert_abstain(&r, &e, 1);
    }

    #[test]
    fn missing_impl_identity_is_full_barrier_never_unqualified() {
        // `impl Foo for {` has an ERROR in its signature region: the whole
        // impl becomes a barrier; `m` must NOT fall back to an unqualified name.
        let (r, e) = parse("impl Foo for {\n    fn m() {\n        let x = 1;\n    }\n}\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2); // method inside unreadable impl
    }

    #[test]
    fn collapsed_impl_no_type_is_barrier_not_unqualified() {
        // Bare `impl {` collapses to ERROR + statement block; the re-parsed
        // `fn m` must not leak as an unqualified owner.
        let (r, e) = parse("impl {\n    fn m() {\n        let x = 1;\n    }\n}\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn malformed_module_no_name_is_barrier_not_leaking_to_outer() {
        // `mod {` has no structural module name; tree-sitter collapses it to an
        // ERROR plus an `expression_statement`-wrapped anonymous block. Nested
        // inside a named function, the contained fn must NOT leak as an
        // outer-qualified owner (`outer::inner`); the collapsed module's ERROR
        // also degrades the enclosing function, so nothing inside may attribute.
        let (r, e) = parse(
            "fn outer() {\n    mod {\n        fn inner() {\n            let x = 1;\n        }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 1); // outer def: degraded by the collapsed-mod ERROR
        assert_abstain(&r, &e, 2); // collapsed mod header (ERROR)
        assert_abstain(&r, &e, 3); // inner def: no `outer::inner` leak
        assert_abstain(&r, &e, 4); // inner body
    }

    #[test]
    fn malformed_module_top_level_is_barrier() {
        let (r, e) = parse("mod {\n    fn inner() {\n        let x = 1;\n    }\n}\n");
        assert_abstain(&r, &e, 1); // collapsed mod header (ERROR)
        assert_abstain(&r, &e, 2); // inner def line
        assert_abstain(&r, &e, 3); // inner body
    }

    #[test]
    fn malformed_module_nested_function_does_not_leak_at_depth() {
        // A function nested deeper inside the collapsed module must also not
        // leak: no guessed child owner is emitted at any depth.
        let (r, e) = parse(
            "fn outer() {\n    mod {\n        fn a() {\n            fn b() {\n                let x = 1;\n            }\n        }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 1); // outer degraded by the collapsed-mod ERROR
        assert_abstain(&r, &e, 3); // a (collapsed) def line
        assert_abstain(&r, &e, 4); // b def line: no deeper leak
        assert_abstain(&r, &e, 5); // b body
    }

    #[test]
    fn nested_function_in_clean_blocks_stays_eligible() {
        // Valid lexically nested function items inside clean control-flow/standalone
        // blocks remain eligible: the enclosing named function is a valid naming
        // context, so they are `outer::inner` (NOT mistaken for collapsed-mod artifacts).
        let (r, e) = parse(
            "fn outer() {\n    { fn in_block() { let x = 1; } }\n    if cond { fn in_if() { } }\n    loop { fn in_loop() { } }\n    match x { _ => { fn in_match() { } } }\n}\n",
        );
        assert_owner(&r, &e, 2, "outer::in_block");
        assert_owner(&r, &e, 3, "outer::in_if");
        assert_owner(&r, &e, 4, "outer::in_loop");
        assert_owner(&r, &e, 5, "outer::in_match");
    }

    #[test]
    fn malformed_container_nested_callables_classified_as_barriers() {
        // Exhaustive extraction: every callable nested under a collapsed malformed
        // mod/impl/trait must be individually classified as an AnonymousBarrier
        // (never omitted, never given a guessed name). Prove classification by
        // asserting a barrier region exists covering each nested function.
        let (r, _) = parse(
            "fn outer() {\n    mod {\n        fn a() {\n            let x = 1;\n        }\n        fn b() {\n            let y = 2;\n        }\n    }\n}\n",
        );
        let barriers = r
            .iter()
            .filter_map(|region| match region {
                OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                } => Some((*start_line, *end_line)),
                OwnerRegion::Named(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            barriers.contains(&(3, 5)),
            "fn a must be individually barriered, got {barriers:?}"
        );
        assert!(
            barriers.contains(&(6, 8)),
            "fn b must be individually barriered, got {barriers:?}"
        );
        // No named owner may leak from the malformed module.
        assert!(r
            .iter()
            .all(|region| !matches!(region, OwnerRegion::Named(_))));
    }

    #[test]
    fn malformed_impl_and_trait_nested_callables_classified_as_barriers() {
        // `impl {` and `trait {` (no valid identity) recover to the same kind of
        // statement-level block; their contained methods must barrier, not leak.
        let (r1, _) = parse("impl {\n    fn m() {\n        let x = 1;\n    }\n}\n");
        let b1 = r1
            .iter()
            .filter_map(|region| match region {
                OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                } => Some((*start_line, *end_line)),
                OwnerRegion::Named(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            b1.contains(&(2, 4)),
            "impl fn m must be barriered, got {b1:?}"
        );
        assert!(r1
            .iter()
            .all(|region| !matches!(region, OwnerRegion::Named(_))));

        let (r2, _) = parse("trait {\n    fn t() {\n        let x = 1;\n    }\n}\n");
        let b2 = r2
            .iter()
            .filter_map(|region| match region {
                OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                } => Some((*start_line, *end_line)),
                OwnerRegion::Named(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            b2.contains(&(2, 4)),
            "trait fn t must be barriered, got {b2:?}"
        );
        assert!(r2
            .iter()
            .all(|region| !matches!(region, OwnerRegion::Named(_))));
    }

    #[test]
    fn unrelated_local_error_followed_by_clean_block_keeps_inner_eligible() {
        // `foo(;` is an unrelated local ERROR (its ERROR is nested inside its own
        // expression_statement, not a sibling of the block). The following valid
        // standalone block is preserved, so `fn inner` (no ERROR overlap) stays
        // eligible as `outer::inner` even though `outer` degrades from the local
        // ERROR — the pinned local-error rule must not be contradicted.
        let (r, e) =
            parse("fn outer() {\n    foo(;\n    { fn inner() {\n        let x = 1;\n    } }\n}\n");
        assert_abstain(&r, &e, 1); // outer def degrades (local ERROR overlaps)
        assert_owner(&r, &e, 3, "outer::inner");
        assert_owner(&r, &e, 4, "outer::inner");
    }

    #[test]
    fn struct_error_is_not_recovery_leaves_nested_fn_eligible() {
        // `struct {` has the SAME ERROR-sibling shape as a collapsed mod/impl/trait
        // but its keyword is not a mod/impl/trait item-introducer. The keyword gate
        // must reject it, so the clean nested `fn a` stays eligible as `outer::a`
        // (rather than incorrectly abstaining as recovery-invalid).
        let (r, e) = parse(
            "fn outer() {\n    struct {\n        fn a() {\n            let x = 1;\n        }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 1); // outer degrades from the struct ERROR
        assert_owner(&r, &e, 3, "outer::a");
        assert_owner(&r, &e, 4, "outer::a");
    }

    #[test]
    fn error_is_collapsed_container_accepts_only_item_introducers() {
        // Direct proof of the keyword gate: a collapsed mod/impl/trait ERROR is
        // accepted, but a stray struct/enum ERROR (same shape, different keyword)
        // is rejected.
        fn check(src: &str) -> bool {
            fn find(n: tree_sitter::Node, found: &mut bool) {
                if n.is_error() {
                    *found = error_is_collapsed_container(n);
                    return;
                }
                let mut c = n.walk();
                for ch in n.named_children(&mut c) {
                    find(ch, found);
                }
            }
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&outline_language(Lang::Rust).unwrap())
                .unwrap();
            let tree = parser.parse(src, None).unwrap();
            let mut found = false;
            find(tree.root_node(), &mut found);
            found
        }
        assert!(check(
            "fn outer() {\n    mod {\n        fn a() {}\n    }\n}\n"
        ));
        assert!(check(
            "fn outer() {\n    impl {\n        fn m() {}\n    }\n}\n"
        ));
        assert!(check(
            "fn outer() {\n    trait {\n        fn t() {}\n    }\n}\n"
        ));
        assert!(!check(
            "fn outer() {\n    struct {\n        fn a() {}\n    }\n}\n"
        ));
        assert!(!check(
            "fn outer() {\n    enum {\n        fn a() {}\n    }\n}\n"
        ));
    }

    #[test]
    fn extern_body_less_signatures_barrier() {
        let (r, e) = parse("extern \"C\" {\n    fn c_func(x: i32);\n    fn d_func() -> i32;\n}\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn macro_definition_barriers_and_macro_invocation_not_guessed() {
        let (r, e) = parse("macro_rules! make {\n    () => { fn generated() {} };\n}\nfn real() {\n    make!();\n}\n");
        // The macro-rules body must not produce a named `generated` owner.
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 4, "real");
        assert_owner(&r, &e, 5, "real"); // macro invocation line still owned by `real`
    }

    #[test]
    fn local_error_degrades_overlapping_callable_and_clean_elsewhere_eligible() {
        let (r, e) = parse("fn good() {\n}\nfn bad() {\n    x = (\n    ;\n}\n");
        assert_owner(&r, &e, 1, "good");
        assert_abstain(&r, &e, 3); // bad def line degrades
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn distinct_top_level_functions_each_unique_narrowest() {
        let (r, e) = parse("fn a() {\n    let x = 1;\n}\nfn b() {\n}\n");
        assert_owner(&r, &e, 1, "a");
        assert_owner(&r, &e, 4, "b");
    }

    #[test]
    fn empty_content_yields_no_regions() {
        let (r, e) = parse("let x = 1;\n");
        assert!(r.is_empty());
        assert!(e.is_empty());
    }

    #[test]
    fn module_level_statement_and_comment_abstain() {
        let (r, e) = parse("fn one() {\n}\nlet x = 1;\n// c\nfn two() {\n}\n");
        assert_owner(&r, &e, 1, "one");
        assert_abstain(&r, &e, 3); // module-level let
        assert_abstain(&r, &e, 4); // comment
        assert_owner(&r, &e, 5, "two");
    }

    /// A single independent fixture, parsed on its own with 1-based local line
    /// numbers. `owners` lists `(hit_line, name, start_line, end_line)` that
    /// must attribute to the exact owner; `abstain` lists lines that must
    /// abstain and count toward the intentional-abstention floor; `incidental`
    /// lists lines that must abstain but do NOT count (module-level blanks).
    struct Case {
        label: &'static str,
        source: &'static str,
        owners: &'static [(u32, &'static str, u32, u32)],
        abstain: &'static [u32],
        incidental: &'static [u32],
    }

    /// Slice-2 binding quality gate: a table of independent, varied fixtures.
    /// Each case is parsed separately with local line numbers (no shared buffer,
    /// no offset math). Counts assertions programmatically and asserts the floor
    /// (`>=60` positive hits, `>=20` intentional abstentions); mechanically repeated
    /// `blank`/`separator` lines never count toward the abstention floor. Every
    /// positive row asserts the exact owner (`qualified_name`, `start_line`,
    /// `end_line`); every abstention row asserts no attribution (0 mismatches).

    #[test]
    fn rust_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level simple",
                source: "fn alpha() {\n    let x = 1;\n    x\n}\n",
                owners: &[
                    (1, "alpha", 1, 4),
                    (2, "alpha", 1, 4),
                    (3, "alpha", 1, 4),
                    (4, "alpha", 1, 4),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level params + return",
                source: "fn beta(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
                owners: &[(1, "beta", 1, 3), (2, "beta", 1, 3), (3, "beta", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level multiline body",
                source: "fn gamma() {\n    let s = String::new();\n    for c in s.chars() {\n        s.push(c);\n    }\n    s\n}\n",
                owners: &[
                    (1, "gamma", 1, 7),
                    (2, "gamma", 1, 7),
                    (3, "gamma", 1, 7),
                    (4, "gamma", 1, 7),
                    (5, "gamma", 1, 7),
                    (6, "gamma", 1, 7),
                    (7, "gamma", 1, 7),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "nested function",
                source: "fn outer() {\n    fn inner() {\n        let x = 1;\n    }\n    inner()\n}\n",
                owners: &[
                    (1, "outer", 1, 6),
                    (2, "outer::inner", 2, 4),
                    (3, "outer::inner", 2, 4),
                    (4, "outer::inner", 2, 4),
                    (5, "outer", 1, 6),
                    (6, "outer", 1, 6),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level async",
                source: "async fn fetch() {\n    let data = load().await;\n    data\n}\n",
                owners: &[
                    (1, "fetch", 1, 4),
                    (2, "fetch", 1, 4),
                    (3, "fetch", 1, 4),
                    (4, "fetch", 1, 4),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level unsafe",
                source: "unsafe fn raw() {\n    let x = 1;\n}\n",
                owners: &[(1, "raw", 1, 3), (2, "raw", 1, 3), (3, "raw", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "inherent impl methods",
                source: "struct Foo {}\nimpl Foo {\n    fn a() {\n        let x = 1;\n    }\n    fn b(&self) {\n        let y = 2;\n    }\n}\n\n",
                owners: &[
                    (3, "Foo::a", 3, 5),
                    (4, "Foo::a", 3, 5),
                    (5, "Foo::a", 3, 5),
                    (6, "Foo::b", 6, 8),
                    (7, "Foo::b", 6, 8),
                    (8, "Foo::b", 6, 8),
                ],
                abstain: &[1, 2],
                incidental: &[10],
            },
            Case {
                label: "trait impl",
                source: "impl Display for Foo {\n    fn fmt(&self) {\n        let x = 1;\n    }\n}\n",
                owners: &[(2, "<Foo as Display>::fmt", 2, 4), (3, "<Foo as Display>::fmt", 2, 4), (4, "<Foo as Display>::fmt", 2, 4)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "trait default + body-less signature",
                source: "trait Greet {\n    fn hi();\n    fn bye() {\n        let x = 1;\n    }\n}\n",
                owners: &[(3, "Greet::bye", 3, 5), (4, "Greet::bye", 3, 5), (5, "Greet::bye", 3, 5)],
                abstain: &[1, 2],
                incidental: &[],
            },
            Case {
                label: "nested modules",
                source: "mod a {\n    mod b {\n        fn f() {\n            let x = 1;\n        }\n    }\n}\n",
                owners: &[(3, "a::b::f", 3, 5), (4, "a::b::f", 3, 5), (5, "a::b::f", 3, 5)],
                abstain: &[1, 2],
                incidental: &[],
            },
            Case {
                label: "closure barrier",
                source: "fn outer() {\n    let f = |x| {\n        x + 1\n    };\n}\n",
                owners: &[(1, "outer", 1, 5), (5, "outer", 1, 5)],
                abstain: &[2, 3, 4],
                incidental: &[],
            },
            Case {
                label: "nested closure abstains",
                source: "fn factory() {\n    || {\n        |x| x + 1\n    }\n}\n",
                owners: &[(1, "factory", 1, 5), (5, "factory", 1, 5)],
                abstain: &[2, 3, 4],
                incidental: &[],
            },
            Case {
                label: "generic inherent impl",
                source: "impl<T: Bound> Stack<T> {\n    fn push(self) {\n        let x = 1;\n    }\n}\n",
                owners: &[(2, "Stack<T>::push", 2, 4), (3, "Stack<T>::push", 2, 4), (4, "Stack<T>::push", 2, 4)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "extern body-less signatures",
                source: "extern \"C\" {\n    fn c_func(x: i32);\n    fn d_func() -> i32;\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "macro_definition barrier + macro_invocation owned",
                source: "macro_rules! make {\n    () => { fn generated() {} };\n}\nfn real() {\n    make!();\n}\n",
                owners: &[(4, "real", 4, 6), (5, "real", 4, 6), (6, "real", 4, 6)],
                abstain: &[1, 2],
                incidental: &[],
            },
            Case {
                label: "module-level statement + comment",
                source: "fn one() {\n}\nlet x = 1;\n// c\nfn two() {\n}\n",
                owners: &[(1, "one", 1, 2), (2, "one", 1, 2), (5, "two", 5, 6), (6, "two", 5, 6)],
                abstain: &[3, 4],
                incidental: &[],
            },
            Case {
                label: "module-level blank separation",
                source: "fn one() {\n}\n\nfn two() {\n}\n",
                owners: &[(1, "one", 1, 2), (2, "one", 1, 2), (4, "two", 4, 5), (5, "two", 4, 5)],
                abstain: &[],
                incidental: &[3],
            },
            Case {
                label: "top-level match body",
                source: "fn classify(n: i32) -> &'static str {\n    match n {\n        0 => \"zero\",\n        _ => \"other\",\n    }\n}\n",
                owners: &[
                    (1, "classify", 1, 6),
                    (2, "classify", 1, 6),
                    (3, "classify", 1, 6),
                    (4, "classify", 1, 6),
                    (5, "classify", 1, 6),
                    (6, "classify", 1, 6),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level generic fn",
                source: "fn identity<T>(v: T) -> T {\n    v\n}\n",
                owners: &[(1, "identity", 1, 3), (2, "identity", 1, 3), (3, "identity", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "local error degrades callable",
                source: "fn broken() {\n    x = (\n    return x;\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "clean callable elsewhere in partial tree",
                source: "fn good() {\n}\nfn bad() {\n    x = (\n    ;\n}\n",
                owners: &[(1, "good", 1, 2), (2, "good", 1, 2)],
                abstain: &[3, 4],
                incidental: &[],
            },
            Case {
                label: "missing impl identity is full barrier",
                source: "impl Foo for {\n    fn m() {\n        let x = 1;\n    }\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "collapsed impl no type is barrier",
                source: "impl {\n    fn m() {\n        let x = 1;\n    }\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "malformed module no name is barrier",
                source: "fn outer() {\n    mod {\n        fn inner() {\n            let x = 1;\n        }\n    }\n}\n",
                owners: &[],
                abstain: &[1, 2, 3, 4, 5, 6],
                incidental: &[],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = rust_owner_for(&regions, &errors, hit_line).named();
                assert!(
                    owner.is_some(),
                    "[{}] line {hit_line}: expected {name}@{start}-{end}, got abstain",
                    case.label
                );
                let owner = owner.unwrap();
                assert_eq!(
                    owner.qualified_name(),
                    name,
                    "[{}] line {hit_line} name",
                    case.label
                );
                assert_eq!(
                    owner.start_line, start,
                    "[{}] line {hit_line} start",
                    case.label
                );
                assert_eq!(owner.end_line, end, "[{}] line {hit_line} end", case.label);
            }
            for &line in case.abstain {
                abstentions += 1;
                assert_abstain(&regions, &errors, line);
            }
            for &line in case.incidental {
                assert_abstain(&regions, &errors, line);
            }
        }
        // Binding quality floor; cannot silently regress. Separation/blank lines
        // live in `incidental` and never count toward the abstention floor.
        assert!(
            positives >= 60,
            "positive floor: only {positives} assertions"
        );
        assert!(
            abstentions >= 20,
            "intentional-abstention floor: only {abstentions} assertions"
        );
    }

    #[test]
    fn manifest_gate_verifies_kinds_exist_and_fingerprint_is_pinned() {
        let v: serde_json::Value = serde_json::from_str(tree_sitter_rust::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        // Every executable/config/bodyless kind must exist in the grammar.
        for k in RUST_EXECUTABLE_CALLABLES
            .iter()
            .chain(RUST_CONTAINER_KINDS)
            .chain(RUST_BODILESS_DECLARATIONS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        // Inventories must be disjoint (a node cannot be two categories).
        let mut all: Vec<&str> = Vec::new();
        all.extend(RUST_EXECUTABLE_CALLABLES);
        all.extend(RUST_CONTAINER_KINDS);
        all.extend(RUST_BODILESS_DECLARATIONS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        // Fingerprint is pinned so grammar metadata changes fail here.
        assert_eq!(
            fnv1a(tree_sitter_rust::NODE_TYPES.as_bytes()),
            RUST_NODE_TYPES_FINGERPRINT,
            "tree-sitter-rust NODE_TYPES changed; re-audit the manifest"
        );
    }
}
