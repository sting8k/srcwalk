//! JavaScript owner-region extraction (US-067 slice 3B).
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
//!     `A.x` (direct class-field binding in a structurally named class wins).
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
//! property callable) are sticky for descendants whose qualified identity would
//! depend on the missing/unsupported container: traversal recurses exhaustively
//! but every dependent nested callable is classified as a barrier. Anonymous
//! callbacks (arrow/IIFE/member-value) preserve the previously valid outer
//! prefix, so a named declaration inside them may remain eligible per the core
//! barrier rule. Binding/container nodes never emit duplicate callable regions.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerRegion,
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
///
/// TypeScript-only categories (type-only non-callables and body-less callable
/// declarations) are added by the slice-3C manifest.
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

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-javascript` `NODE_TYPES`
/// JSON (tree-sitter-javascript 0.23.1). Pinned so any grammar metadata change
/// fails the manifest gate and forces a re-audit of the inventories above.
#[cfg(test)]
const JS_NODE_TYPES_FINGERPRINT: u64 = 0x372f_ecd5_b5f8_67c2;

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

/// Parse a JavaScript file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn js_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::JavaScript)?;
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

/// Attribute a JavaScript hit line to a named owner, honoring local errors.
pub(crate) fn js_owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> Option<&'a OwnerAnchor> {
    attribute_line(regions, errors, line)
}

/// Walk a JavaScript tree, emitting `Named` regions for supported callables and
/// `AnonymousBarrier` regions for every other callable. `prefixes` carries
/// `.`-suffixed path segments (classes, binding-backed callables, nested
/// functions) so nested callables get their full qualified name.
///
/// `sticky` is true inside an invalid naming container (an anonymous class or an
/// unsupported object method/property callable) whose descendants must all be
/// classified as barriers rather than emit a partial/guessed name. Traversal
/// still recurses exhaustively so EVERY callable node is classified.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            walk_declaration(node, bytes, path, prefixes, errors, regions, sticky);
        }
        "function_expression" | "generator_function" => {
            walk_expression(node, bytes, path, prefixes, errors, regions, sticky);
        }
        "arrow_function" => walk_arrow(node, bytes, path, prefixes, errors, regions, sticky),
        "method_definition" => walk_method(node, bytes, path, prefixes, errors, regions, sticky),
        "class_declaration" => {
            // A named class is a valid `.`-prefix container; it is not itself a
            // callable region (a hit on the class header line abstains).
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                prefixes.push(format!("{}.", name.trim()));
                recurse(node, bytes, path, prefixes, errors, regions, false);
                prefixes.pop();
            } else {
                recurse(node, bytes, path, prefixes, errors, regions, sticky);
            }
        }
        "class" => {
            // Class expression: an explicitly named class uses its own class
            // name; an anonymous class is an invalid naming container (sticky).
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                prefixes.push(format!("{}.", name.trim()));
                recurse(node, bytes, path, prefixes, errors, regions, false);
                prefixes.pop();
            } else {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                recurse(node, bytes, path, prefixes, errors, regions, true);
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
            recurse(node, bytes, path, prefixes, errors, regions, sticky);
        }
        // Binding/container/wrapper nodes: traversal context only, never emit
        // duplicate callable regions. Their callable values/children are handled
        // by the callable arms above.
        _ => recurse(node, bytes, path, prefixes, errors, regions, sticky),
    }
}

/// A `function_declaration` / `generator_function_declaration` is always a
/// named callable (it carries a structural `name`); under a valid context it
/// emits a `Named` region and becomes a `.`-prefix for nested declarations.
fn walk_declaration(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
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
        recurse(node, bytes, path, prefixes, errors, regions, false);
        return;
    };
    let name = name.trim().to_string();
    let qualified = join_prefixes(prefixes, &name);
    emit_named(node, path, &name, &qualified, errors, regions);
    prefixes.push(format!("{name}."));
    recurse(node, bytes, path, prefixes, errors, regions, false);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    // Rule 1: direct simple variable binding wins.
    if let Some(binding) = variable_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &binding);
        emit_named(node, path, &binding, &qualified, errors, regions);
        prefixes.push(format!("{binding}."));
        recurse(node, bytes, path, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    // Rule 2: direct class-field binding in a structurally named class wins.
    if let Some(field) = field_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &field);
        emit_named(node, path, &field, &qualified, errors, regions);
        prefixes.push(format!("{field}."));
        recurse(node, bytes, path, prefixes, errors, regions, false);
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
        emit_named(node, path, &own, &qualified, errors, regions);
        prefixes.push(format!("{own}."));
        recurse(node, bytes, path, prefixes, errors, regions, false);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    // Unsupported direct binding context (computed field, destructuring/
    // pattern declarator): sticky barrier for the callable and its descendants.
    if unsupported_field_direct_context(node) || unsupported_declarator_direct_context(node) {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    if let Some(binding) = variable_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &binding);
        emit_named(node, path, &binding, &qualified, errors, regions);
        prefixes.push(format!("{binding}."));
        recurse(node, bytes, path, prefixes, errors, regions, false);
        prefixes.pop();
        return;
    }
    if let Some(field) = field_binding_name(node, bytes) {
        let qualified = join_prefixes(prefixes, &field);
        emit_named(node, path, &field, &qualified, errors, regions);
        prefixes.push(format!("{field}."));
        recurse(node, bytes, path, prefixes, errors, regions, false);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    }
    if !in_named_class(node) {
        // Anonymous class: invalid naming container, sticky.
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line,
            end_line,
        });
        recurse(node, bytes, path, prefixes, errors, regions, true);
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
        recurse(node, bytes, path, prefixes, errors, regions, true);
        return;
    };
    let qualified = join_prefixes(prefixes, &name);
    emit_named(node, path, &name, &qualified, errors, regions);
    prefixes.push(format!("{name}."));
    recurse(node, bytes, path, prefixes, errors, regions, false);
    prefixes.pop();
}

/// Whether a class-body member's enclosing class has a structural name.
/// `node` is a `method_definition` or `field_definition` whose parent is a
/// `class_body`; walk up to the `class_declaration`/`class` and check its name.
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

/// Whether `node` is the direct `value` of a `field_definition` inside a
/// structurally named class whose field name is a simple/private identifier.
/// Returns the field name (the class prefix is already on the stack).
fn field_binding_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    if parent.kind() != "field_definition" {
        return None;
    }
    if parent.child_by_field_name("value")?.id() != node.id() {
        return None;
    }
    if !in_named_class(parent) {
        return None;
    }
    let field = parent.child_by_field_name("property")?;
    if !matches!(
        field.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return None;
    }
    field.utf8_text(bytes).ok().map(|s| s.trim().to_string())
}

/// Whether `node` is the direct `value` of a `field_definition` whose field
/// name is NOT a simple/private identifier (computed/unreadable). Such a direct
/// class-field callable is an unsupported naming context: it is a barrier and
/// its descendants must be sticky barriers rather than leak a partial name.
fn unsupported_field_direct_context(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "field_definition" {
        return false;
    }
    let Some(value) = parent.child_by_field_name("value") else {
        return false;
    };
    if value.id() != node.id() {
        return false;
    }
    let Some(field) = parent.child_by_field_name("property") else {
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
/// full `.`-qualified display name.
fn emit_named(
    node: Node<'_>,
    path: &Path,
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
        language: Lang::JavaScript,
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
    prefixes: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
    sticky: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, bytes, path, prefixes, errors, regions, sticky);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.js")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        js_regions(&path(), src).expect("should parse")
    }

    fn assert_owner_exact(
        regions: &[OwnerRegion],
        errors: &[ErrorRange],
        hit_line: u32,
        name: &str,
        start: u32,
        end: u32,
    ) {
        let owner = js_owner_for(regions, errors, hit_line)
            .unwrap_or_else(|| panic!("line {hit_line} should attribute to {name}"));
        assert_eq!(owner.qualified_name(), name, "line {hit_line} display name");
        assert_eq!(owner.start_line, start, "line {hit_line} start line");
        assert_eq!(owner.end_line, end, "line {hit_line} end line");
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            js_owner_for(regions, errors, line).is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn manifest_gate_verifies_kinds_exist_and_fingerprint_is_pinned() {
        let v: serde_json::Value =
            serde_json::from_str(tree_sitter_javascript::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty());

        let mut all: Vec<&str> = Vec::new();
        all.extend(JS_EXECUTABLE_CALLABLES);
        all.extend(JS_BINDING_CONTAINER_KINDS);
        all.extend(JS_NON_CALLABLE_SAFETY_BARRIERS);
        for k in &all {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );

        assert_eq!(
            fnv1a(tree_sitter_javascript::NODE_TYPES.as_bytes()),
            JS_NODE_TYPES_FINGERPRINT,
            "tree_sitter_javascript NODE_TYPES changed; re-pin fingerprint and re-verify the matrix"
        );
    }

    #[test]
    fn all_six_executable_callable_kinds_are_disposed() {
        // Every kind in JS_EXECUTABLE_CALLABLES must be exercised as Named (or
        // Barrier) so a grammar reclassification cannot silently drop coverage.
        let (r, e) = parse(
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
    fn quality_matrix() {
        struct Case {
            src: &'static str,
            owners: &'static [(u32, &'static str, u32, u32)],
            abstain: &'static [u32],
        }
        let cases: &[Case] = &[
            Case {
                src: "function alpha() {\n    return 1;\n}\n",
                owners: &[(1, "alpha", 1, 3), (2, "alpha", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "function* gen() {\n    yield 1;\n}\n",
                owners: &[(1, "gen", 1, 3), (2, "gen", 1, 3)],
                abstain: &[],
            },
            Case {
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
                src: "class A {\n    get x() { return 1; }\n    set x(v) { this._x = v; }\n}\n",
                owners: &[(2, "A.x", 2, 2), (3, "A.x", 3, 3)],
                abstain: &[],
            },
            Case {
                src: "class A {\n    #priv() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (2, "A.#priv", 2, 4),
                    (3, "A.#priv", 2, 4),
                    (4, "A.#priv", 2, 4),
                ],
                abstain: &[],
            },
            Case {
                src: "const f = () => {\n    return 1;\n};\n",
                owners: &[(1, "f", 1, 3), (2, "f", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "const g = function() {\n    return 2;\n};\n",
                owners: &[(1, "g", 1, 3), (2, "g", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "const h = function* () {\n    yield 1;\n};\n",
                owners: &[(1, "h", 1, 3), (2, "h", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "const f = function named() {\n    return 1;\n};\n",
                owners: &[(1, "f", 1, 3), (2, "f", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "class A {\n    x = () => {\n        return 1;\n    };\n}\n",
                owners: &[(2, "A.x", 2, 4), (3, "A.x", 2, 4), (4, "A.x", 2, 4)],
                abstain: &[],
            },
            Case {
                src: "class A {\n    y = function named() {\n        return 2;\n    };\n}\n",
                owners: &[(2, "A.y", 2, 4), (3, "A.y", 2, 4)],
                abstain: &[],
            },
            Case {
                src: "(function named() {\n    return 1;\n})();\n",
                owners: &[(1, "named", 1, 3), (2, "named", 1, 3)],
                abstain: &[],
            },
            Case {
                src: "(function* gen() {\n    yield 1;\n})();\n",
                owners: &[(1, "gen", 1, 3), (2, "gen", 1, 3)],
                abstain: &[],
            },
            Case {
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
                src: "(function() {\n    function helper() {\n        return 1;\n    }\n})();\n",
                owners: &[
                    (2, "helper", 2, 4),
                    (3, "helper", 2, 4),
                    (4, "helper", 2, 4),
                ],
                abstain: &[1],
            },
            Case {
                src: "function outer() {\n    const f = () => {\n        return 1;\n    };\n}\n",
                owners: &[
                    (1, "outer", 1, 5),
                    (2, "outer.f", 2, 4),
                    (3, "outer.f", 2, 4),
                    (4, "outer.f", 2, 4),
                ],
                abstain: &[],
            },
            // Intentional abstentions (safety barriers / unsupported phase-1 contexts).
            Case {
                src: "const o = {\n    foo() {\n        return 1;\n    },\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                src: "const o = {\n    bar: function() {\n        return 1;\n    },\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                src: "const o = {\n    bar: function named() {\n        return 1;\n    },\n};\n",
                owners: &[(2, "named", 2, 4), (3, "named", 2, 4)],
                abstain: &[],
            },
            Case {
                src: "const A = class {\n    foo() {\n        return 1;\n    }\n};\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                src: "(function() {\n    let x = 1;\n})();\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                src: "Foo.prototype.bar = function() {\n    return 1;\n};\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                src: "button.onClick = () => {\n    handle();\n};\n",
                owners: &[],
                abstain: &[1, 2],
            },
            Case {
                src: "class A {\n    ['foo']() {\n        return 1;\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3],
            },
            Case {
                src: "class A {\n    static {\n        let x = 1;\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3],
            },
        ];

        let mut positives = 0usize;
        let mut abstentions = 0usize;
        for c in cases {
            let (r, e) = parse(c.src);
            for &(line, name, start, end) in c.owners {
                assert_owner_exact(&r, &e, line, name, start, end);
                positives += 1;
            }
            for &line in c.abstain {
                assert_abstain(&r, &e, line);
                abstentions += 1;
            }
        }
        assert!(
            positives >= 30,
            "need >=30 positive attributions, got {positives}"
        );
        assert!(
            abstentions >= 10,
            "need >=10 intentional abstentions, got {abstentions}"
        );
    }

    #[test]
    fn invalid_naming_contexts_are_sticky_for_nested_declarations() {
        // Computed class method: recurse sticky so a nested fn cannot leak as
        // `A.inner`.
        let (r, e) =
            parse("class A {\n    ['foo']() {\n        function inner() { return 1; }\n    }\n}\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        // Direct class-field callable with a computed/unreadable field name:
        // unsupported direct field context recurses sticky.
        let (r, e) = parse(
            "class A {\n    ['x'] = () => {\n        function inner() { return 1; }\n    };\n}\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);

        // Direct declarator with a destructuring/patter name: unsupported
        // direct binding context recurses sticky.
        let (r, e) = parse("const { a } = function() {\n    function inner() { return 1; }\n};\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn ordinary_anonymous_callback_keeps_nested_declarations_eligible() {
        // A non-sticky anonymous callback (IIFE / member-value arrow) preserves
        // the outer prefix, so an independently named nested declaration stays
        // eligible.
        let (r, e) = parse(
            "const xs = [1].map(() => {\n    function inner() { return 1; }\n    return inner();\n});\n",
        );
        assert_owner_exact(&r, &e, 2, "inner", 2, 2);
        // Line 3 is inside the anonymous arrow (outside `inner`'s span): abstain.
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn object_get_set_are_barriers_but_named_container_allows_binding() {
        let (r, e) = parse("const o = {\n    get x() { return 1; }\n};\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn malformed_callable_full_region_barrier_but_clean_adjacent_eligible() {
        // A malformed statement inside a callable overlaps its span, degrading
        // the WHOLE named region to a barrier (every line abstains). A clean
        // adjacent function (no error overlap) remains eligible.
        let (r, e) = parse(
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
            language: crate::types::Lang::JavaScript,
            display_name: "outer.inner".into(),
        };
        assert_eq!(a.qualified_name(), "outer.inner");
    }
}
