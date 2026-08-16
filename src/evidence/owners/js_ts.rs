//! JavaScript/TypeScript/TSX owner-region extraction (US-067 slices 3B/3C).
//!
//! Supported named owners (`.`-qualified, per spec):
//! * `function_declaration` / `generator_function_declaration` -> `name`
//!   (top-level) or `outer.inner` when nested in a function body.
//! * `method_definition` in a structurally named class -> `Class.method`
//!   (get/set/async/generator all collapse to this kind; simple/private names
//!   only, computed/string/number names barrier).
//! * A directly-bound arrow/function/generator expression:
//!   - `const f = () => {}` and `const f = function named(){}` -> `f`
//!     (direct simple variable binding wins).
//!   - `class A { x = () => {} }` / `class A { x = function named(){} }` ->
//!     `A.x` (direct class-field binding in a structurally named class wins;
//!     JS uses `field_definition`, TS uses `public_field_definition`).
//! * An explicit internal function/generator name, when no supported direct
//!   binding/property owner applies (e.g. a named IIFE) -> that own name.
//!
//! Everything else is an `AnonymousBarrier` (never omission, never a guessed
//! name): anonymous arrows/function expressions/IIFEs, object-literal callables
//! (`method_definition` under `object`, `pair { key: fn }`), anonymous classes
//! (invalid naming container), member/prototype assignments, computed names,
//! and `class_static_block` (a non-callable safety fallthrough guard).
//!
//! Invalid naming containers (anonymous class; unsupported object method/
//! property callable; computed/unreadable class field or module name) are
//! sticky for descendants whose qualified identity would depend on the
//! missing/unsupported container: traversal recurses exhaustively but every
//! dependent nested callable is classified as a barrier. Anonymous callbacks
//! (arrow/IIFE/member-value) preserve the previously valid outer prefix, so a
//! named declaration inside them may remain eligible per the core barrier rule.
//! Binding/container nodes never emit duplicate callable regions.
//!
//! TypeScript additions (slice 3C):
//! * Body-less callable declarations (`function_signature`,
//!   `abstract_method_signature`, `method_signature`, `call_signature`,
//!   `construct_signature`) are barriers over their exact node spans; only a
//!   body-bearing concrete node emits a `Named` region.
//! * Type-only non-callables (`property_signature`, `index_signature`,
//!   `function_type`, `constructor_type`) are transparent: no barrier, no name.
//! * Valid prefix containers add named/abstract classes and TypeScript
//!   `namespace`/`internal_module`/`module` (readable identifier or
//!   nested-identifier segments only; string/computed module names barrier).
//! * `ambient_declaration`, decorators, export/default and type annotations
//!   are transparent wrappers, not labels. Interface/type-alias/enum are
//!   traversal containers that never become name prefixes.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerAttribution,
    OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned JavaScript callable manifest (anti-drift contract).
///
/// Inventories are kept distinct so a grammar change to one category cannot
/// silently reclassify a node into another:
///
/// * (a) executable callable kinds that MUST be classified `Named`/`Barrier`;
/// * (b) binding/container/wrapper kinds used only for naming/context;
/// * (c) non-callable safety barriers (fallthrough guard, no callable coverage).
#[cfg(test)]
const JS_EXECUTABLE_CALLABLES: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
];
#[cfg(test)]
const JS_BINDING_CONTAINER_KINDS: &[&str] = &[
    "class_declaration",
    "class",
    "class_body",
    "object",
    "pair",
    "field_definition",
    "variable_declarator",
    "statement_block",
    "program",
    "export_statement",
];
#[cfg(test)]
const JS_NON_CALLABLE_SAFETY_BARRIERS: &[&str] = &["class_static_block"];

/// Version-pinned TypeScript callable manifest (slice 3C). Separate from the
/// JS inventories because the TypeScript grammar reuses JS callable kinds but
/// adds body-less signatures and a distinct class-field node.
#[cfg(test)]
const TS_EXECUTABLE_CALLABLES: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
];
#[cfg(test)]
const TS_BODY_LESS_SIGNATURES: &[&str] = &[
    "abstract_method_signature",
    "method_signature",
    "function_signature",
    "call_signature",
    "construct_signature",
];
#[cfg(test)]
const TS_BINDING_CONTAINER_KINDS: &[&str] = &[
    "class_declaration",
    "abstract_class_declaration",
    "class",
    "class_body",
    "object",
    "pair",
    "public_field_definition",
    "variable_declarator",
    "statement_block",
    "program",
    "export_statement",
    "internal_module",
    "module",
    "interface_declaration",
    "interface_body",
    "type_alias_declaration",
    "enum_declaration",
    "enum_body",
    "ambient_declaration",
    "decorator",
    "expression_statement",
];
#[cfg(test)]
const TS_TYPE_ONLY_TRANSPARENT: &[&str] = &[
    "property_signature",
    "index_signature",
    "function_type",
    "constructor_type",
];
#[cfg(test)]
const TS_NON_CALLABLE_SAFETY_BARRIERS: &[&str] = &["class_static_block"];

/// TSX-only routing/wrapper kinds (JSX). These are NOT a callable inventory:
/// they are traversal/wrapper sentinels proving the TSX grammar routes JSX that
/// contains callables. Kept disjoint from the reusable TS callable/signature/
/// binding/type-only inventories.
#[cfg(test)]
const TSX_ROUTING_WRAPPER_KINDS: &[&str] = &[
    "jsx_element",
    "jsx_self_closing_element",
    "jsx_expression",
    "jsx_attribute",
];

/// FNV-1a-64 fingerprints of the bundled grammar `NODE_TYPES` JSON. Pinned so
/// any grammar metadata change fails the manifest gate and forces a re-audit.
#[cfg(test)]
const JS_NODE_TYPES_FINGERPRINT: u64 = 0x372f_ecd5_b5f8_67c2;
#[cfg(test)]
const TYPESCRIPT_NODE_TYPES_FINGERPRINT: u64 = 0x4c9c_a118_1310_63de;
#[cfg(test)]
const TSX_NODE_TYPES_FINGERPRINT: u64 = 0xda35_b73f_4bdc_e75b;

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

/// Parse a JS/TS/TSX file (grammar selected by `lang`) and produce its owner
/// regions and local error ranges. Returns `None` when the file cannot be
/// read, has no tree, or its root node itself is an `ERROR` (preserve raw
/// hits, emit no owner evidence).
pub(crate) fn regions_for(
    lang: Lang,
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(lang)?;
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
        lang,
        &mut prefixes,
        &errors,
        &mut regions,
        false,
    );
    Some((regions, errors))
}

/// Attribute a JS/TS/TSX hit line to a named owner, honoring local errors.
pub(crate) fn owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> OwnerAttribution<'a> {
    attribute_line(regions, errors, line)
}

/// Walk a JS/TS/TSX tree, emitting `Named` regions for supported callables and
/// `AnonymousBarrier` regions for every other callable. `prefixes` carries
/// `.`-suffixed path segments (classes, namespaces, namespaces/modules,
/// binding-backed callables, nested functions) so nested callables get their
/// full qualified name. `lang` sets the `OwnerAnchor.language` identity.
///
/// `sticky` is true inside an invalid naming container (an anonymous class, an
/// unsupported object method/property callable, a computed/unreadable class
/// field or module name) whose descendants must all be classified as barriers
/// rather than emit a partial/guessed name. Traversal still recurses
/// exhaustively so EVERY callable node is classified.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            walk_declaration(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        "function_expression" | "generator_function" => {
            walk_expression(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        "arrow_function" => walk_arrow(node, bytes, path, lang, prefixes, errors, regions, sticky),
        "method_definition" => {
            walk_method(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        // TS body-less callable declarations: barrier over their exact span.
        "function_signature"
        | "abstract_method_signature"
        | "method_signature"
        | "call_signature"
        | "construct_signature" => {
            walk_bodyless_signature(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        // TS type-only non-callables and traversal-only containers/wrappers:
        // transparent (no barrier, no name, never a prefix).
        "property_signature"
        | "index_signature"
        | "function_type"
        | "constructor_type"
        | "interface_declaration"
        | "interface_body"
        | "type_alias_declaration"
        | "enum_declaration"
        | "enum_body"
        | "ambient_declaration"
        | "decorator" => {
            recurse(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        // Named class containers (regular or abstract) -> `.`-prefix.
        "class_declaration" | "abstract_class_declaration" => {
            walk_class_container(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        "class" => {
            // Class expression: an explicitly named class uses its own class
            // name; an anonymous class is an invalid naming container (sticky).
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                prefixes.push(format!("{}.", name.trim()));
                recurse(node, bytes, path, lang, prefixes, errors, regions, false);
                prefixes.pop();
            } else {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, lang, prefixes, errors, regions, true);
            }
        }
        "class_static_block" => {
            // Non-callable safety barrier: guards fallthrough but does NOT claim
            // callable coverage. Recurse preserving the outer naming context so a
            // named declaration inside the static block stays eligible.
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
            recurse(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        // TS internal_module/module: `.`-prefix containers (readable
        // identifier/nested-identifier segments only; string -> sticky barrier).
        // (`namespace` is only a keyword token in NODE_TYPES and is never
        // visited via named_children traversal.)
        "internal_module" | "module" => {
            walk_module_container(node, bytes, path, lang, prefixes, errors, regions, sticky);
        }
        // Binding/container/wrapper nodes: traversal context only, never emit
        // duplicate callable regions. Their callable values/children are handled
        // by the callable arms above.
        _ => recurse(node, bytes, path, lang, prefixes, errors, regions, sticky),
    }
}

/// A `function_declaration` / `generator_function_declaration` is always a
/// named callable (it carries a structural `name`); under a valid context it
/// emits a `Named` region and becomes a `.`-prefix for nested declarations.
fn walk_declaration(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if sticky {
        // Descendant of an invalid naming container: barrier, keep classifying.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    let Some(name) = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
    else {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        return;
    };
    let name = name.trim().to_string();
    let qualified = join_prefixes(prefixes, &name);
    emit_named(node, path, lang, &name, &qualified, errors, regions);
    prefixes.push(format!("{name}."));
    recurse(node, bytes, path, lang, prefixes, errors, regions, false);
    prefixes.pop();
}

/// A `function_expression` / `generator_function` is classified by binding
/// precedence: (1) direct simple variable binding, (2) direct class-field
/// binding in a structurally named class, (3) explicit internal name, else (4)
/// barrier. An anonymous object-property callable is sticky for descendants.
fn walk_expression(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if sticky {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    // Unsupported direct binding context (computed field, destructuring/
    // pattern declarator): the callable cannot take a valid simple identity, so
    // it is a barrier and its descendants are sticky barriers (no partial name).
    if unsupported_field_direct_context(node) || unsupported_declarator_direct_context(node) {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    // Rule 1: direct simple variable binding wins.
    if let Some(binding) = variable_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &binding);
        emit_named(node, path, lang, &binding, &qualified, errors, regions);
        prefixes.push(format!("{binding}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    // Rule 2: direct class-field binding in a structurally named class wins.
    if let Some(field) = field_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &field);
        emit_named(node, path, lang, &field, &qualified, errors, regions);
        prefixes.push(format!("{field}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    // Rule 3: explicit internal name may render its own name.
    if let Some(own) = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
    {
        let own = own.trim().to_string();
        let qualified = join_prefixes(prefixes, &own);
        emit_named(node, path, lang, &own, &qualified, errors, regions);
        prefixes.push(format!("{own}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    // Rule 4: anonymous. An object-property callable is an unsupported sticky
    // container; any other anonymous callable (IIFE, member value, plain expr)
    // is a non-sticky barrier preserving the outer prefix.
    let object_property = node.parent().is_some_and(|p| p.kind() == "pair");
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line,
        end_line,
    });
    recurse(
        node,
        bytes,
        path,
        lang,
        prefixes,
        errors,
        regions,
        sticky || object_property,
    );
}

/// An `arrow_function` is intrinsically anonymous: it is only `Named` when
/// promoted by a supported direct binding context (rule 1 variable or rule 2
/// class-field); otherwise it is a barrier.
fn walk_arrow(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if sticky {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    // Unsupported direct binding context (computed field, destructuring/
    // pattern declarator): sticky barrier for the callable and its descendants.
    if unsupported_field_direct_context(node) || unsupported_declarator_direct_context(node) {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    if let Some(binding) = variable_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &binding);
        emit_named(node, path, lang, &binding, &qualified, errors, regions);
        prefixes.push(format!("{binding}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    if let Some(field) = field_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &field);
        emit_named(node, path, lang, &field, &qualified, errors, regions);
        prefixes.push(format!("{field}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    let object_property = node.parent().is_some_and(|p| p.kind() == "pair");
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line,
        end_line,
    });
    recurse(
        node,
        bytes,
        path,
        lang,
        prefixes,
        errors,
        regions,
        sticky || object_property,
    );
}

/// A `method_definition` in a structurally named class is `Class.method`; in an
/// object literal it is an unsupported sticky barrier; in an anonymous class it
/// is a sticky barrier. Computed/string/number names barrier.
fn walk_method(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if sticky {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    let parent_kind = node.parent().map(|p| p.kind().to_string());
    if parent_kind.as_deref() == Some("object") {
        // Object literal method (incl. getters/setters/shorthand): unsupported
        // in phase 1. Sticky for descendants.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    if !in_named_class(node) {
        // Anonymous class: invalid naming container, sticky.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    let Some(name) = simple_member_name(node, bytes) else {
        // Computed/string/number method name: incapable of a simple identity.
        // Sticky: a nested declaration must not leak a partial `A.<nested>`
        // name from an unreadable computed method.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    };
    let qualified = join_prefixes(prefixes, &name);
    emit_named(node, path, lang, &name, &qualified, errors, regions);
    prefixes.push(format!("{name}."));
    recurse(node, bytes, path, lang, prefixes, errors, regions, false);
    prefixes.pop();
}

/// A TS body-less callable declaration is a barrier over its exact node span:
/// it has no runtime body so it can never be a concrete owner, and a nested
/// declaration must not fall through to an enclosing function.
fn walk_bodyless_signature(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line,
        end_line,
    });
    recurse(node, bytes, path, lang, prefixes, errors, regions, sticky);
}

/// A named `class_declaration` / `abstract_class_declaration` is a valid
/// `.`-prefix container; it is not itself a callable region (a hit on the
/// class header line abstains).
fn walk_class_container(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    if let Some(name) = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
    {
        prefixes.push(format!("{}.", name.trim()));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
    } else {
        recurse(node, bytes, path, lang, prefixes, errors, regions, sticky);
    }
}

/// A TS `namespace`/`internal_module`/`module` is a `.`-prefix container only
/// when its name is a structurally readable identifier or nested-identifier
/// (e.g. `A.B`). String/computed/unreadable module names are sticky barriers.
fn walk_module_container(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    if sticky {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
        return;
    };
    let readable = matches!(name_node.kind(), "identifier" | "nested_identifier")
        && !name_node
            .utf8_text(bytes)
            .map_or(true, |t| t.trim().is_empty());
    if readable {
        let name = name_node.utf8_text(bytes).unwrap().trim().to_string();
        prefixes.push(format!("{name}."));
        recurse(node, bytes, path, lang, prefixes, errors, regions, false);
        prefixes.pop();
    } else {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, lang, prefixes, errors, regions, true);
    }
}

/// Whether a class-body member's enclosing class has a structural name.
/// `node` is a `method_definition`, `field_definition`, or
/// `public_field_definition` whose parent is a `class_body`; walk up to the
/// `class_declaration`/`abstract_class_declaration`/`class` and check its name.
fn in_named_class(node: Node<'_>) -> bool {
    let Some(body) = node.parent() else {
        return false;
    };
    if body.kind() != "class_body" {
        return false;
    }
    let Some(class) = body.parent() else {
        return false;
    };
    class.child_by_field_name("name").is_some()
}

/// The class-field name node for a JS `field_definition` or TS
/// `public_field_definition` (the two grammars store it under different
/// field names: JS `property`, TS `name`). Returns `None` for other kinds.
fn field_name_node(parent: Node<'_>) -> Option<Node<'_>> {
    match parent.kind() {
        "field_definition" => parent.child_by_field_name("property"),
        "public_field_definition" => parent.child_by_field_name("name"),
        _ => None,
    }
}

/// Whether `node` is the direct `value` of a `variable_declarator` whose `name`
/// is a simple `identifier` (not a destructuring/pattern). Returns the binding
/// name.
fn variable_binding_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    if parent.kind() != "variable_declarator" {
        return None;
    }
    if parent.child_by_field_name("value")?.id() != node.id() {
        return None;
    }
    let name = parent.child_by_field_name("name")?;
    if name.kind() != "identifier" {
        return None;
    }
    name.utf8_text(bytes).ok().map(|s| s.trim().to_string())
}

/// Whether `node` is the direct `value` of a class field (`field_definition` in
/// JS, `public_field_definition` in TS) inside a structurally named class whose
/// field name is a simple/private identifier. Returns the field name (the
/// class prefix is already on the stack).
fn field_binding_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    if !matches!(
        parent.kind(),
        "field_definition" | "public_field_definition"
    ) {
        return None;
    }
    if parent.child_by_field_name("value")?.id() != node.id() {
        return None;
    }
    if !in_named_class(parent) {
        return None;
    }
    let field = field_name_node(parent)?;
    if !matches!(
        field.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return None;
    }
    field.utf8_text(bytes).ok().map(|s| s.trim().to_string())
}

/// Whether `node` is the direct `value` of a class field (`field_definition` in
/// JS, `public_field_definition` in TS) whose field name is NOT a simple/private
/// identifier (computed/unreadable). Such a direct class-field callable is an
/// unsupported naming context: it is a barrier and its descendants must be
/// sticky barriers rather than leak a partial name.
fn unsupported_field_direct_context(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if !matches!(
        parent.kind(),
        "field_definition" | "public_field_definition"
    ) {
        return false;
    }
    let Some(value) = parent.child_by_field_name("value") else {
        return false;
    };
    if value.id() != node.id() {
        return false;
    }
    let Some(field) = field_name_node(parent) else {
        return false;
    };
    !matches!(
        field.kind(),
        "property_identifier" | "private_property_identifier"
    )
}

/// Whether `node` is the direct `value` of a `variable_declarator` whose name
/// is a destructuring/pattern (not a simple `identifier`). Such a direct
/// callable value is an unsupported binding context: it is a barrier and its
/// descendants must be sticky barriers.
fn unsupported_declarator_direct_context(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "variable_declarator" {
        return false;
    }
    let Some(value) = parent.child_by_field_name("value") else {
        return false;
    };
    if value.id() != node.id() {
        return false;
    }
    let Some(name) = parent.child_by_field_name("name") else {
        return false;
    };
    name.kind() != "identifier"
}

/// A simple/private member name for a `method_definition`. Computed, string,
/// and number names return `None` (they barrier).
fn simple_member_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    if !matches!(
        name.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return None;
    }
    name.utf8_text(bytes).ok().map(|s| s.trim().to_string())
}

/// Concatenate the `.`-suffixed prefix stack with `name` to form the qualified
/// display name.
fn join_prefixes(prefixes: &[String], name: &str) -> String {
    if prefixes.is_empty() {
        name.to_string()
    } else {
        let mut joined = prefixes.concat();
        joined.push_str(name);
        joined
    }
}

/// Emit a `Named` region for `node`, degrading to a barrier if its byte span
/// overlaps a local error range. `name` is the simple name; `qualified` is the
/// full `.`-qualified display name; `lang` is the actual source language.
fn emit_named(
    node: Node<'_>,
    path: &Path,
    lang: Lang,
    name: &str,
    qualified: &str,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let anchor = OwnerAnchor {
        path: path.to_path_buf(),
        name: name.to_string(),
        receiver_var: None,
        receiver_type: None,
        package_dir: std::path::PathBuf::from("."),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        language: lang,
        display_name: qualified.to_string(),
    };
    let region = OwnerRegion::Named(anchor);
    let region = degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
    regions.push(region);
}

#[allow(clippy::too_many_arguments)]
fn recurse(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, bytes, path, lang, prefixes, errors, regions, sticky);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.ts")
    }

    fn parse(lang: Lang, src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        regions_for(lang, &path(), src).expect("should parse")
    }

    fn assert_owner_exact(
        regions: &[OwnerRegion],
        errors: &[ErrorRange],
        hit_line: u32,
        name: &str,
        start: u32,
        end: u32,
    ) {
        let owner = owner_for(regions, errors, hit_line)
            .named()
            .unwrap_or_else(|| panic!("line {hit_line} should attribute to {name}"));
        assert_eq!(owner.qualified_name(), name, "line {hit_line} display name");
        assert_eq!(owner.start_line, start, "line {hit_line} start line");
        assert_eq!(owner.end_line, end, "line {hit_line} end line");
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    /// Number of `AnonymousBarrier` regions whose span covers `line`.
    fn barrier_regions_at(regions: &[OwnerRegion], line: u32) -> usize {
        regions
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    OwnerRegion::AnonymousBarrier { start_line, end_line }
                        if *start_line <= line && line <= *end_line
                )
            })
            .count()
    }
    /// Number of `AnonymousBarrier` regions that occupy exactly the one-line
    /// span `(line, line)`. Exact match: an oversized barrier that merely
    /// covers `line` does NOT count.
    fn exact_one_line_barriers_at(regions: &[OwnerRegion], line: u32) -> usize {
        regions
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    OwnerRegion::AnonymousBarrier { start_line, end_line }
                        if *start_line == line && *end_line == line
                )
            })
            .count()
    }

    /// Total number of `AnonymousBarrier` regions anywhere.
    fn total_barrier_regions(regions: &[OwnerRegion]) -> usize {
        regions
            .iter()
            .filter(|r| matches!(r, OwnerRegion::AnonymousBarrier { .. }))
            .count()
    }

    /// Number of `Named` regions whose span covers `line`.
    fn named_regions_at(regions: &[OwnerRegion], line: u32) -> usize {
        regions
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    OwnerRegion::Named(a) if a.start_line <= line && line <= a.end_line
                )
            })
            .count()
    }

    #[test]
    fn js_manifest_gate_verifies_kinds_exist_and_fingerprint_is_pinned() {
        let kinds = node_types(tree_sitter_javascript::NODE_TYPES);
        let mut all: Vec<&str> = Vec::new();
        all.extend(JS_EXECUTABLE_CALLABLES);
        all.extend(JS_BINDING_CONTAINER_KINDS);
        all.extend(JS_NON_CALLABLE_SAFETY_BARRIERS);
        assert_kinds_exist(&kinds, &all);
        assert_disjoint(&all);
        assert_eq!(
            fnv1a(tree_sitter_javascript::NODE_TYPES.as_bytes()),
            JS_NODE_TYPES_FINGERPRINT,
            "tree_sitter_javascript NODE_TYPES changed; re-pin fingerprint and re-verify the matrix"
        );
    }

    #[test]
    fn ts_manifest_gate_verifies_typescript_kinds_and_fingerprint() {
        let kinds = node_types(tree_sitter_typescript::TYPESCRIPT_NODE_TYPES);
        let mut all: Vec<&str> = Vec::new();
        all.extend(TS_EXECUTABLE_CALLABLES);
        all.extend(TS_BODY_LESS_SIGNATURES);
        all.extend(TS_BINDING_CONTAINER_KINDS);
        all.extend(TS_TYPE_ONLY_TRANSPARENT);
        all.extend(TS_NON_CALLABLE_SAFETY_BARRIERS);
        assert_kinds_exist(&kinds, &all);
        assert_disjoint(&all);
        assert_eq!(
            fnv1a(tree_sitter_typescript::TYPESCRIPT_NODE_TYPES.as_bytes()),
            TYPESCRIPT_NODE_TYPES_FINGERPRINT,
            "typescript NODE_TYPES changed; re-pin fingerprint and re-verify the matrix"
        );
    }

    #[test]
    fn tsx_manifest_gate_verifies_tsx_kinds_and_fingerprint() {
        let kinds = node_types(tree_sitter_typescript::TSX_NODE_TYPES);
        // TSX reuses ALL reusable TS inventories (callables, body-less
        // signatures, binding/container kinds, type-only transparent, safety
        // barriers) and adds its own JSX routing/wrapper sentinels.
        let mut all: Vec<&str> = Vec::new();
        all.extend(TS_EXECUTABLE_CALLABLES);
        all.extend(TS_BODY_LESS_SIGNATURES);
        all.extend(TS_BINDING_CONTAINER_KINDS);
        all.extend(TS_TYPE_ONLY_TRANSPARENT);
        all.extend(TS_NON_CALLABLE_SAFETY_BARRIERS);
        assert_kinds_exist(&kinds, &all);
        assert_disjoint(&all);
        // JSX routing/wrapper sentinels (not a callable inventory) also exist.
        for k in TSX_ROUTING_WRAPPER_KINDS {
            assert!(kinds.contains(*k), "tsx kind {k} missing from NODE_TYPES");
        }
        // The JSX sentinels must not collide with any reusable TS category.
        let mut with_jsx: Vec<&str> = all.clone();
        with_jsx.extend(TSX_ROUTING_WRAPPER_KINDS);
        assert_disjoint(&with_jsx);
        assert_eq!(
            fnv1a(tree_sitter_typescript::TSX_NODE_TYPES.as_bytes()),
            TSX_NODE_TYPES_FINGERPRINT,
            "tsx NODE_TYPES changed; re-pin fingerprint and re-verify the matrix"
        );
    }

    fn node_types(json: &str) -> HashSet<String> {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect()
    }

    fn assert_kinds_exist(kinds: &HashSet<String>, all: &[&str]) {
        assert!(!kinds.is_empty());
        for k in all {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
    }

    fn assert_disjoint(all: &[&str]) {
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
    }

    #[test]
    fn all_six_js_executable_callable_kinds_are_disposed() {
        let (r, e) = parse(
            Lang::JavaScript,
            "function fdecl() {}\n\
             function* gdecl() { yield 1; }\n\
             const fexpr = function() {};\n\
             const gexpr = function* () { yield 1; };\n\
             const arrow = () => {};\n\
             class A { method() {} }\n",
        );
        assert_owner_exact(&r, &e, 1, "fdecl", 1, 1); // function_declaration
        assert_owner_exact(&r, &e, 2, "gdecl", 2, 2); // generator_function_declaration
        assert_owner_exact(&r, &e, 3, "fexpr", 3, 3); // function_expression
        assert_owner_exact(&r, &e, 4, "gexpr", 4, 4); // generator_function
        assert_owner_exact(&r, &e, 5, "arrow", 5, 5); // arrow_function
        assert_owner_exact(&r, &e, 6, "A.method", 6, 6); // method_definition
    }

    #[test]
    fn all_ts_callable_and_signature_kinds_are_disposed() {
        // Every TS executable callable kind is Named; every body-less signature
        // kind is a barrier over its exact span.
        let (r, e) = parse(
            Lang::TypeScript,
            "function fdecl() {}\n\
             function* gdecl() { yield 1; }\n\
             const fexpr = function() {};\n\
             const gexpr = function* () { yield 1; };\n\
             const arrow = () => {};\n\
             abstract class A {\n\
               method() {}\n\
               abstract abs(): void;\n\
               sig(): void;\n\
             }\n\
             declare function amb(): void;\n\
             interface I {\n\
               run(): void;\n\
               new (): I;\n\
               (): void;\n\
             }\n",
        );
        assert_owner_exact(&r, &e, 1, "fdecl", 1, 1); // function_declaration
        assert_owner_exact(&r, &e, 2, "gdecl", 2, 2); // generator_function_declaration
        assert_owner_exact(&r, &e, 3, "fexpr", 3, 3); // function_expression
        assert_owner_exact(&r, &e, 4, "gexpr", 4, 4); // generator_function
        assert_owner_exact(&r, &e, 5, "arrow", 5, 5); // arrow_function
        assert_owner_exact(&r, &e, 7, "A.method", 7, 7); // method_definition (Named)
                                                         // Body-less signature kinds must emit EXACTLY ONE AnonymousBarrier over
                                                         // their exact one-line span and never a Named region (the "exactly one
                                                         // region classification" contract; a silent omission or an oversized
                                                         // enclosing barrier would not pass).
        for (line, kind) in [
            (8, "abstract_method_signature"),
            (9, "method_signature"),
            (11, "function_signature"),
            (13, "method_signature"),
            (14, "construct_signature"),
            (15, "call_signature"),
        ] {
            assert_eq!(
                exact_one_line_barriers_at(&r, line),
                1,
                "{kind} at line {line} must emit exactly one exact (line,line) barrier"
            );
            assert_eq!(
                named_regions_at(&r, line),
                0,
                "{kind} at line {line} must not emit a Named region"
            );
            assert_abstain(&r, &e, line);
        }
        // The whole fixture has exactly six one-line signature barriers and no
        // other barrier, so no oversized/extra barrier can hide the omission.
        assert_eq!(
            total_barrier_regions(&r),
            6,
            "this valid fixture must classify exactly the six body-less signatures as barriers"
        );
    }
    #[test]
    fn ts_type_only_kinds_add_zero_regions_and_stay_transparent() {
        // property_signature, index_signature, function_type, and
        // constructor_type are transparent: they add neither a Named nor a
        // Barrier region and never shadow an enclosing owner. They appear
        // inside a valid outer callable (`load`) and inside an interface
        // (traversal-only) so both transparency directions are proven.
        let (r, e) = parse(
            Lang::TypeScript,
            "interface Config {\n    url: string;\n    [key: string]: number;\n}\nfunction load(cfg: { url: string; [key: string]: number }, cb: (x: number) => string, ctor: new () => number) {\n    return null;\n}\n",
        );
        // Exactly one Named region (load) and zero barriers: the type-only
        // kinds added no classifications anywhere.
        assert_eq!(r.len(), 1, "type-only kinds must add exactly zero regions");
        assert_eq!(named_regions_at(&r, 5), 1);
        assert_eq!(barrier_regions_at(&r, 5), 0);
        // Hit lines inside `load` resolve to load; the type-only kinds on line
        // 5 do not shadow it with a barrier.
        assert_owner_exact(&r, &e, 5, "load", 5, 7);
        assert_owner_exact(&r, &e, 6, "load", 5, 7);
        assert_owner_exact(&r, &e, 7, "load", 5, 7);
        // Inside the interface (traversal-only, no owner) the type-only kinds
        // are transparent: their lines abstain rather than emit a bogus
        // owner/barrier.
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn family_quality_matrix() {
        struct Case {
            lang: Lang,
            src: &'static str,
            owners: &'static [(u32, &'static str, u32, u32)],
            abstain: &'static [u32],
        }
        let cases: &[Case] = &[
            // ---- JavaScript positive cases ----
            Case {
                lang: Lang::JavaScript,
                src: "function alpha() {\n    return 1;\n}\n",
                owners: &[(1, "alpha", 1, 3), (2, "alpha", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "function* gen() {\n    yield 1;\n}\n",
                owners: &[(1, "gen", 1, 3), (2, "gen", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "function outer() {\n    function inner() {\n        let x = 1;\n    }\n}\n",
                owners: &[
                    (1, "outer", 1, 5),
                    (2, "outer.inner", 2, 4),
                    (3, "outer.inner", 2, 4),
                    (4, "outer.inner", 2, 4),
                ],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    foo() {\n        return 1;\n    }\n    static bar() {}\n}\n",
                owners: &[
                    (2, "A.foo", 2, 4),
                    (3, "A.foo", 2, 4),
                    (4, "A.foo", 2, 4),
                    (5, "A.bar", 5, 5),
                ],
                abstain: &[1],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    get x() { return 1; }\n    set x(v) { this._x = v; }\n}\n",
                owners: &[(2, "A.x", 2, 2), (3, "A.x", 3, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    #priv() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (2, "A.#priv", 2, 4),
                    (3, "A.#priv", 2, 4),
                    (4, "A.#priv", 2, 4),
                ],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const f = () => {\n    return 1;\n};\n",
                owners: &[(1, "f", 1, 3), (2, "f", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const g = function() {\n    return 2;\n};\n",
                owners: &[(1, "g", 1, 3), (2, "g", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const h = function* () {\n    yield 1;\n};\n",
                owners: &[(1, "h", 1, 3), (2, "h", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const f = function named() {\n    return 1;\n};\n",
                owners: &[(1, "f", 1, 3), (2, "f", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    x = () => {\n        return 1;\n    };\n}\n",
                owners: &[(2, "A.x", 2, 4), (3, "A.x", 2, 4), (4, "A.x", 2, 4)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    y = function named() {\n        return 2;\n    };\n}\n",
                owners: &[(2, "A.y", 2, 4), (3, "A.y", 2, 4)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "(function named() {\n    return 1;\n})();\n",
                owners: &[(1, "named", 1, 3), (2, "named", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "(function* gen() {\n    yield 1;\n})();\n",
                owners: &[(1, "gen", 1, 3), (2, "gen", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const f = () => {\n    function inner() {\n        return 1;\n    }\n};\n",
                owners: &[
                    (1, "f", 1, 5),
                    (2, "f.inner", 2, 4),
                    (3, "f.inner", 2, 4),
                    (4, "f.inner", 2, 4),
                ],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "(function() {\n    function helper() {\n        return 1;\n    }\n})();\n",
                owners: &[(2, "helper", 2, 4), (3, "helper", 2, 4), (4, "helper", 2, 4)],
                abstain: &[1],
            },
            Case {
                lang: Lang::JavaScript,
                src: "function outer() {\n    const f = () => {\n        return 1;\n    };\n}\n",
                owners: &[
                    (1, "outer", 1, 5),
                    (2, "outer.f", 2, 4),
                    (3, "outer.f", 2, 4),
                    (4, "outer.f", 2, 4),
                ],
                abstain: &[],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const o = {\n    bar: function named() {\n        return 1;\n    },\n};\n",
                owners: &[(2, "named", 2, 4), (3, "named", 2, 4)],
                abstain: &[],
            },
            // ---- JavaScript intentional abstentions ----
            Case {
                lang: Lang::JavaScript,
                src: "const o = {\n    foo() {\n        return 1;\n    },\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const o = {\n    bar: function() {\n        return 1;\n    },\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                lang: Lang::JavaScript,
                src: "const A = class {\n    foo() {\n        return 1;\n    }\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                lang: Lang::JavaScript,
                src: "(function() {\n    let x = 1;\n})();\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                lang: Lang::JavaScript,
                src: "Foo.prototype.bar = function() {\n    return 1;\n};\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                lang: Lang::JavaScript,
                src: "button.onClick = () => {\n    handle();\n};\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    ['foo']() {\n        return 1;\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                lang: Lang::JavaScript,
                src: "class A {\n    static {\n        let x = 1;\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3],
            },
            // ---- TypeScript positive cases ----
            Case {
                lang: Lang::TypeScript,
                src: "namespace A.B {\n    export function foo() {}\n    export class C {\n        bar() {}\n    }\n}\n",
                owners: &[(2, "A.B.foo", 2, 2), (4, "A.B.C.bar", 4, 4)],
                abstain: &[],
            },
            Case {
                lang: Lang::TypeScript,
                src: "abstract class Shape {\n    abstract area(): number;\n    area() {}\n    abstract name(): string;\n    describe() {}\n}\n",
                owners: &[(3, "Shape.area", 3, 3), (5, "Shape.describe", 5, 5)],
                abstain: &[2, 4],
            },
            Case {
                lang: Lang::TypeScript,
                src: "class Fmt {\n    format(): string;\n    format(x: number): string;\n    format(): string { return \"\"; }\n}\n",
                owners: &[(4, "Fmt.format", 4, 4)],
                abstain: &[2, 3],
            },
            Case {
                lang: Lang::TypeScript,
                src: "function transform(cb: (x: number) => string, ctor: new () => number) {\n    return null;\n}\n",
                owners: &[(1, "transform", 1, 3), (2, "transform", 1, 3), (3, "transform", 1, 3)],
                abstain: &[],
            },
            Case {
                lang: Lang::TypeScript,
                src: "class A {\n    x = () => {};\n    private y = function() {};\n    static z = () => {};\n}\n",
                owners: &[(2, "A.x", 2, 2), (3, "A.y", 3, 3), (4, "A.z", 4, 4)],
                abstain: &[],
            },
            Case {
                lang: Lang::TypeScript,
                src: "function good() {\n    return 1;\n}\n",
                owners: &[(1, "good", 1, 3), (2, "good", 1, 3)],
                abstain: &[],
            },
            // ---- TypeScript intentional abstentions ----
            Case {
                lang: Lang::TypeScript,
                src: "declare function fetch(): void;\ndeclare class Store {\n    get(): string;\n}\n",
                owners: &[],
                abstain: &[1, 3],
            },
            Case {
                lang: Lang::TypeScript,
                src: "interface I {\n    run(): void;\n    new (): I;\n    (): void;\n}\n",
                owners: &[],
                abstain: &[2, 3, 4],
            },
            Case {
                lang: Lang::TypeScript,
                src: "class A {\n    ['x'] = () => {\n        function inner() {}\n    };\n}\nconst { a } = function() {\n    function inner() {}\n};\n",
                owners: &[],
                abstain: &[2, 3, 6, 7],
            },
            Case {
                lang: Lang::TypeScript,
                src: "function bad() {\n    const x: number = ;\n    return 1;\n}\nfunction good() {\n    return 1;\n}\n",
                owners: &[(5, "good", 5, 7), (6, "good", 5, 7)],
                abstain: &[1, 2, 3, 4],
            },
        ];

        let mut positives = 0usize;
        let mut abstentions = 0usize;
        for c in cases {
            let (r, e) = parse(c.lang, c.src);
            for &(line, name, start, end) in c.owners {
                assert_owner_exact(&r, &e, line, name, start, end);
                positives += 1;
            }
            for &line in c.abstain {
                assert_abstain(&r, &e, line);
                abstentions += 1;
            }
        }
        // Combined JS+TS family gate (substantial, not padded).
        assert!(
            positives >= 60,
            "need >=60 combined positive owner checks, got {positives}"
        );
        assert!(
            abstentions >= 20,
            "need >=20 combined intentional abstention lines, got {abstentions}"
        );
    }

    #[test]
    fn ts_invalid_naming_contexts_are_sticky_for_nested_declarations() {
        // Computed class method: recurse sticky so a nested fn cannot leak as
        // `A.inner`.
        let (r, e) = parse(
            Lang::TypeScript,
            "abstract class A {\n    ['foo'](): void {\n        function inner() {}\n    }\n}\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        // Computed public field: sticky.
        let (r, e) = parse(
            Lang::TypeScript,
            "class A {\n    ['x'] = () => {\n        function inner() {}\n    };\n}\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        // Destructuring declarator: sticky.
        let (r, e) = parse(
            Lang::TypeScript,
            "const { a } = function() {\n    function inner() {}\n};\n",
        );
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn ts_string_module_name_is_sticky_barrier() {
        // A string-named ambient module cannot be a readable prefix: descendants
        // are sticky barriers, and a nested named declaration does not leak.
        let (r, e) = parse(
            Lang::TypeScript,
            "declare module \"x\" {\n    export function inner() {}\n}\n",
        );
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn ts_module_prefix_joins_with_dots() {
        let (r, e) = parse(
            Lang::TypeScript,
            "namespace A {\n    export namespace B {\n        export function foo() {}\n    }\n}\n",
        );
        assert_owner_exact(&r, &e, 3, "A.B.foo", 3, 3);
    }

    #[test]
    fn tsx_jsx_callback_keeps_nested_declarations_eligible() {
        // TSX routes through the TSX grammar; an inline JSX arrow callback is an
        // ordinary non-sticky barrier, so a named declaration inside stays
        // eligible, and the component itself is a Named owner.
        let (r, e) = parse(
            Lang::Tsx,
            "function App() {\n  return <div onClick={() => { inner(); }}>x</div>;\n}\nfunction inner() {\n  return 1;\n}\n",
        );
        assert_owner_exact(&r, &e, 1, "App", 1, 3);
        assert_owner_exact(&r, &e, 4, "inner", 4, 6);
        assert_owner_exact(&r, &e, 5, "inner", 4, 6);
    }

    #[test]
    fn invalid_naming_contexts_are_sticky_for_nested_declarations_js() {
        let (r, e) = parse(
            Lang::JavaScript,
            "class A {\n    ['foo']() {\n        function inner() { return 1; }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        let (r, e) = parse(
            Lang::JavaScript,
            "class A {\n    ['x'] = () => {\n        function inner() { return 1; }\n    };\n}\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        let (r, e) = parse(
            Lang::JavaScript,
            "const { a } = function() {\n    function inner() { return 1; }\n};\n",
        );
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn ordinary_anonymous_callback_keeps_nested_declarations_eligible() {
        let (r, e) = parse(
            Lang::JavaScript,
            "const xs = [1].map(() => {\n    function inner() { return 1; }\n    return inner();\n});\n",
        );
        assert_owner_exact(&r, &e, 2, "inner", 2, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn object_get_set_are_barriers_but_named_container_allows_binding() {
        let (r, e) = parse(
            Lang::JavaScript,
            "const o = {\n    get x() { return 1; }\n};\n",
        );
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn malformed_callable_full_region_barrier_but_clean_adjacent_eligible() {
        let (r, e) = parse(
            Lang::JavaScript,
            "function bad() {\n    const x = ;\n    return 1;\n}\nfunction good() {\n    return 1;\n}\n",
        );
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_owner_exact(&r, &e, 5, "good", 5, 7);
        assert_owner_exact(&r, &e, 6, "good", 5, 7);
    }

    #[test]
    fn owner_anchor_qualified_name_uses_display_name() {
        let a = OwnerAnchor {
            path: path(),
            name: "inner".into(),
            receiver_var: None,
            receiver_type: None,
            package_dir: PathBuf::from("pkg"),
            start_line: 1,
            end_line: 1,
            language: crate::types::Lang::TypeScript,
            display_name: "outer.inner".into(),
        };
        assert_eq!(a.qualified_name(), "outer.inner");
    }
}
