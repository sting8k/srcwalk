//! PHP owner-region extraction (US-068 Wave 2A).
//!
//! Supported named owners: a body-bearing `function_definition` (its declared
//! name), a body-bearing `method_declaration` under its class/interface/trait/
//! enum container (`Service::load`), a declared namespace as a structural
//! lexical container using PHP `\` spelling (`Acme\Api\load`,
//! `Acme\Api\Service::load`; braced and unbraced forms yield the same
//! identity), and a body-bearing named property hook (`Container::$prop::get`
//! / `.set`) when the pinned grammar exposes a complete property + hook
//! identity. The owner range is the callable declaration node itself;
//! attributes/docblocks outside it are not owned.
//!
//! Abstentions/barriers: every `anonymous_function` (closure) and
//! `arrow_function` is an `AnonymousBarrier`; an `anonymous_class` is an
//! identity barrier whose methods/hooks abstain; a named function declared
//! lexically inside another callable body (`in_function`) is an identity
//! barrier and abstains (PHP registers it under its global name at execution;
//! neither `outer::inner` nor `outer.inner` is honest); interface/abstract/
//! otherwise body-less methods and hooks are not owners; a callable with a
//! missing/unreadable name/container abstains. `.phtml` inline text and
//! top-level statements remain raw hits without a synthesized owner.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerAttribution,
    OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned PHP callable manifest (anti-drift contract), holding the
/// pinned tree-sitter-php 0.24.2 (`php` grammar) callable kinds.
#[cfg(test)]
const PHP_EXECUTABLE_CALLABLES: &[&str] =
    &["function_definition", "method_declaration", "property_hook"];
#[cfg(test)]
const PHP_CONTAINER_KINDS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "trait_declaration",
    "enum_declaration",
    "namespace_definition",
];
#[cfg(test)]
const PHP_ANONYMOUS_OR_BARRIER_KINDS: &[&str] =
    &["anonymous_function", "arrow_function", "anonymous_class"];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-php` `PHP_NODE_TYPES` JSON
/// (tree-sitter-php 0.24.2, `php` grammar). Pinned so any grammar metadata
/// change fails the manifest gate and forces a re-audit.
#[cfg(test)]
const PHP_NODE_TYPES_FINGERPRINT: u64 = 0x1297_dd3f_8fd2_f5d3;

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

/// Parse a PHP file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn php_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Php)?;
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
    let mut namespace: Option<String> = None;
    let mut containers: Vec<String> = Vec::new();
    walk_children(
        tree.root_node(),
        bytes,
        path,
        false,
        false,
        &mut namespace,
        &mut containers,
        &errors,
        &mut regions,
    );
    Some((regions, errors))
}

/// Read a node field's trimmed source text, returning `None` when the field is
/// missing, unreadable, or empty after trimming. This is fail-closed: an empty
/// structural name must never become an owner or a nameless container that a
/// nested callable could fall through.
fn field_nonempty<'a>(node: Node<'_>, field: &str, bytes: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name(field)?
        .utf8_text(bytes)
        .ok()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Emit a full-range `AnonymousBarrier` for a node and descend `in_anonymous`.
fn barrier_and_descend(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_function: bool,
    namespace: &mut Option<String>,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
    });
    walk_children(
        node,
        bytes,
        path,
        true,
        in_function,
        namespace,
        containers,
        errors,
        regions,
    );
}

/// Walk a PHP tree, emitting `Named` regions for body-bearing callables and
/// `AnonymousBarrier` regions for anonymous/identity barriers. `in_anonymous`
/// is true inside an anonymous identity barrier (closure, arrow function,
/// anonymous class); `in_function` is true inside a callable body, so a nested
/// named function becomes an identity barrier rather than a dishonest owner.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    in_function: bool,
    namespace: &mut Option<String>,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    // Anonymous identity barriers (no structural name).
    if matches!(node.kind(), "anonymous_function" | "arrow_function") {
        barrier_and_descend(
            node, bytes, path, true, namespace, containers, errors, regions,
        );
        return;
    }
    if node.kind() == "anonymous_class" {
        barrier_and_descend(
            node, bytes, path, true, namespace, containers, errors, regions,
        );
        return;
    }

    match node.kind() {
        "function_definition" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
            if in_anonymous || in_function {
                // Anonymous context, or a named function nested inside another
                // callable body: an identity barrier, never `outer::inner` or
                // `outer.inner`.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                walk_children(
                    node, bytes, path, true, true, namespace, containers, errors, regions,
                );
            } else if let Some(name) = field_nonempty(node, "name", bytes) {
                let name = name.to_string();
                let qualified = join_qualified(namespace.as_deref(), containers, &name);
                let anchor = OwnerAnchor {
                    path: path.to_path_buf(),
                    name,
                    receiver_var: None,
                    receiver_type: None,
                    package_dir: std::path::PathBuf::from("."),
                    start_line,
                    end_line,
                    language: Lang::Php,
                    display_name: qualified,
                };
                let region = OwnerRegion::Named(anchor);
                let region =
                    degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                regions.push(region);
                // A function is a callable body: nested named functions must
                // become identity barriers, so descend with in_function = true.
                // Functions are never lexical containers in PHP.
                walk_children(
                    node, bytes, path, false, true, namespace, containers, errors, regions,
                );
            } else {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                walk_children(
                    node, bytes, path, true, true, namespace, containers, errors, regions,
                );
            }
        }
        "method_declaration" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if in_anonymous {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                walk_children(
                    node, bytes, path, true, true, namespace, containers, errors, regions,
                );
            } else if has_body {
                if let Some(name) = field_nonempty(node, "name", bytes) {
                    let name = name.to_string();
                    let qualified = join_qualified(namespace.as_deref(), containers, &name);
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: name.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: std::path::PathBuf::from("."),
                        start_line,
                        end_line,
                        language: Lang::Php,
                        display_name: qualified,
                    };
                    let region = OwnerRegion::Named(anchor);
                    let region =
                        degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                    regions.push(region);
                    walk_children(
                        node, bytes, path, false, true, namespace, containers, errors, regions,
                    );
                } else {
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line,
                        end_line,
                    });
                    walk_children(
                        node, bytes, path, true, true, namespace, containers, errors, regions,
                    );
                }
            } else {
                // Body-less method (interface/abstract): not an owner.
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
            }
        }
        "class_declaration"
        | "interface_declaration"
        | "trait_declaration"
        | "enum_declaration" => {
            if in_anonymous {
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
            } else if let Some(name) = field_nonempty(node, "name", bytes) {
                containers.push(name.to_string());
                walk_children(
                    node,
                    bytes,
                    path,
                    false,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
                containers.pop();
            } else {
                // Unnamed/malformed container: conservative barrier so nested
                // regions cannot leak to an outer callable.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
            }
        }
        "property_declaration" => {
            let mut cur = node.walk();
            let kids: Vec<Node> = node.named_children(&mut cur).collect();
            let prop_names: Vec<String> = kids
                .iter()
                .filter(|c| c.kind() == "property_element")
                .filter_map(|pe| field_nonempty(*pe, "name", bytes).map(String::from))
                .collect();
            // A property hook owner requires a unique structural property
            // identity. When several properties share one hook list their
            // physical ranges coincide, so the per-property readings would
            // form an equal-width tie and violate the unique-narrowest rule;
            // that ambiguous form abstains instead.
            let single_prop = (prop_names.len() == 1).then(|| prop_names[0].clone());
            if let (Some(hl), Some(prop)) = (
                kids.iter().find(|c| c.kind() == "property_hook_list"),
                single_prop.as_ref(),
            ) {
                let prop = prop.as_str();
                let mut hc = hl.walk();
                for hook in hl
                    .named_children(&mut hc)
                    .filter(|h| h.kind() == "property_hook")
                {
                    let start_line = hook.start_position().row as u32 + 1;
                    let end_line = hook.end_position().row as u32 + 1;
                    let hook_name = {
                        let mut hcur = hook.walk();
                        let found = hook
                            .named_children(&mut hcur)
                            .find(|c| c.kind() == "name")
                            .and_then(|n| n.utf8_text(bytes).ok())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from);
                        found
                    };
                    let has_body = hook.child_by_field_name("body").is_some();
                    if in_anonymous {
                        regions.push(OwnerRegion::AnonymousBarrier {
                            start_line,
                            end_line,
                        });
                    } else if has_body {
                        if let Some(hname) = &hook_name {
                            let segment = format!("{prop}::{hname}");
                            let qualified =
                                join_qualified(namespace.as_deref(), containers, &segment);
                            let anchor = OwnerAnchor {
                                path: path.to_path_buf(),
                                name: segment.clone(),
                                receiver_var: None,
                                receiver_type: None,
                                package_dir: std::path::PathBuf::from("."),
                                start_line,
                                end_line,
                                language: Lang::Php,
                                display_name: qualified,
                            };
                            let region = OwnerRegion::Named(anchor);
                            let region = degrade_named_on_error(
                                region,
                                errors,
                                hook.start_byte(),
                                hook.end_byte(),
                            );
                            regions.push(region);
                        }
                    }
                }
            }
            // Hook bodies are callable bodies; descend with in_function = true
            // so nested named functions inside them become barriers.
            walk_children(
                node,
                bytes,
                path,
                in_anonymous,
                true,
                namespace,
                containers,
                errors,
                regions,
            );
        }
        _ => walk_children(
            node,
            bytes,
            path,
            in_anonymous,
            in_function,
            namespace,
            containers,
            errors,
            regions,
        ),
    }
}

fn walk_children(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    in_function: bool,
    namespace: &mut Option<String>,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    for child in children {
        // Namespace declarations are handled at the sibling level because the
        // unbraced form (`namespace Acme\Api;`) is a scope directive whose
        // declarations are siblings, while the braced form is a container.
        if child.kind() == "namespace_definition" {
            let name_field = child.child_by_field_name("name");
            let has_body = child.child_by_field_name("body").is_some();
            // A missing name field is a valid global namespace (braced
            // `namespace { ... }`); a name field that is present but unreadable
            // or empty is malformed and becomes a fail-closed barrier.
            let name = name_field
                .and_then(|n| n.utf8_text(bytes).ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);
            if name_field.is_some() && name.is_none() {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                });
                walk_children(
                    child,
                    bytes,
                    path,
                    true,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
                continue;
            }
            if has_body {
                // Braced namespace: a lexical container for its body.
                let prev = namespace.take();
                *namespace = name;
                walk_children(
                    child,
                    bytes,
                    path,
                    in_anonymous,
                    in_function,
                    namespace,
                    containers,
                    errors,
                    regions,
                );
                *namespace = prev;
            } else {
                // Unbraced namespace: applies to the following siblings.
                *namespace = name;
            }
            continue;
        }
        walk(
            child,
            bytes,
            path,
            in_anonymous,
            in_function,
            namespace,
            containers,
            errors,
            regions,
        );
    }
}

/// Build the display-qualified identity: an optional namespace prefix spelled
/// with `\`, then the lexical `::`-joined class containers, then the callable
/// segment. Examples: `load`, `Acme\Api\load`, `Service::load`,
/// `Acme\Api\Service::load`, `Service::$name::get`.
fn join_qualified(namespace: Option<&str>, containers: &[String], segment: &str) -> String {
    let mut out = String::new();
    if let Some(ns) = namespace {
        out.push_str(ns);
        out.push('\\');
    }
    if !containers.is_empty() {
        out.push_str(&containers.join("::"));
        out.push_str("::");
    }
    out.push_str(segment);
    out
}

/// Attribute a PHP hit line to a named owner, honoring local errors.
pub(crate) fn php_owner_for<'a>(
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
        PathBuf::from("tests/fixtures/x.php")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        php_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = php_owner_for(regions, errors, line).named();
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            php_owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn top_level_function_and_namespace() {
        let (r, e) = parse(
            "<?php\nfunction load() {\n    return 1;\n}\nnamespace Acme\\Api;\nfunction go() {\n    return 2;\n}\n",
        );
        assert_owner(&r, &e, 2, "load");
        assert_owner(&r, &e, 6, "Acme\\Api\\go");
    }

    #[test]
    fn class_method_and_unbraced_namespace() {
        let (r, e) = parse(
            "<?php\nnamespace Acme\\Api;\nclass Service {\n    public function load() {\n        return 1;\n    }\n}\n",
        );
        assert_owner(&r, &e, 4, "Acme\\Api\\Service::load");
        assert_owner(&r, &e, 5, "Acme\\Api\\Service::load");
    }

    #[test]
    fn braced_namespace_same_identity_as_unbraced() {
        let (r, e) = parse(
            "<?php\nnamespace Acme\\Api {\n    class Service {\n        public function load() {\n            return 1;\n        }\n    }\n}\n",
        );
        assert_owner(&r, &e, 4, "Acme\\Api\\Service::load");
    }

    #[test]
    fn closure_arrow_and_anonymous_class_are_barriers() {
        let (r, e) = parse(
            "<?php\nfunction outer() {\n    $c = function() { return 1; };\n    $a = fn($x) => $x + 1;\n    $o = new class { public function m() { return 2; } };\n}\n",
        );
        assert_owner(&r, &e, 2, "outer");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
        // The anonymous class method must not attribute to outer or be fabricated.
        assert_abstain(&r, &e, 5);
    }

    #[test]
    fn nested_named_function_is_identity_barrier() {
        let (r, e) = parse(
            "<?php\nfunction outer() {\n    function inner() {\n        return 1;\n    }\n}\n",
        );
        // Neither `outer::inner` nor `outer.inner` is honest; hits inside must
        // not fall through to `outer`.
        assert_owner(&r, &e, 2, "outer");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn nested_named_function_inside_method_is_barrier() {
        let (r, e) = parse(
            "<?php\nclass C {\n    public function m() {\n        function inner() { return 1; }\n    }\n}\n",
        );
        assert_owner(&r, &e, 3, "C::m");
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn class_declared_inside_function_keeps_global_identity() {
        let (r, e) = parse(
            "<?php\nfunction outer() {\n    class Local {\n        public function m() { return 1; }\n    }\n}\n",
        );
        // PHP registers Local globally; the method must not append to `outer`.
        assert_owner(&r, &e, 4, "Local::m");
        assert_owner(&r, &e, 2, "outer");
    }

    #[test]
    fn interface_bodyless_method_abstains() {
        let (r, e) = parse(
            "<?php\ninterface I {\n    public function noBody(): void;\n    public function withBody() { return 1; }\n}\n",
        );
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 4, "I::withBody");
    }

    #[test]
    fn trait_and_enum_methods() {
        let (r, e) = parse(
            "<?php\ntrait T {\n    public function t1() { return 1; }\n}\nenum E {\n    case A;\n    public function e1() { return 2; }\n}\n",
        );
        assert_owner(&r, &e, 3, "T::t1");
        assert_owner(&r, &e, 7, "E::e1");
    }

    #[test]
    fn property_hooks_get_set() {
        let (r, e) = parse(
            "<?php\nclass C {\n    public string $name {\n        get => $this->name;\n        set { $this->name = $value; }\n    }\n}\n",
        );
        assert_owner(&r, &e, 4, "C::$name::get");
        assert_owner(&r, &e, 5, "C::$name::set");
    }

    #[test]
    fn bodyless_property_hook_abstains() {
        let (r, e) = parse(
            "<?php\nclass C {\n    public string $name {\n        get;\n        set;\n    }\n}\n",
        );
        assert_abstain(&r, &e, 4);
        assert_abstain(&r, &e, 5);
    }

    #[test]
    fn malformed_local_error_degrades_to_barrier() {
        let (r, e) = parse("<?php\nfunction broken() {\n    $x = (\n    return 1;\n}\n");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn clean_callable_elsewhere_in_partial_tree_still_eligible() {
        let (r, e) = parse(
            "<?php\nfunction good() {\n    return 1;\n}\nfunction broken() {\n    $x = (\n}\n",
        );
        assert_owner(&r, &e, 2, "good");
        assert_abstain(&r, &e, 5);
    }
    #[test]
    fn malformed_container_nested_method_abstains_clean_sibling_ok() {
        let (r, e) = parse(
            "<?php\nclass {\n    public function m() { return 1; }\n}\nfunction clean() { return 2; }\n",
        );
        // The nameless container is an ERROR region: its nested method must
        // not receive a fabricated identity, while the clean sibling function
        // stays eligible.
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 5, "clean");
    }

    #[test]
    fn malformed_method_without_name_abstains_fail_closed() {
        let (r, e) = parse(
            "<?php\nclass C {\n    public function () { return 1; }\n    public function ok() { return 2; }\n}\n",
        );
        // A method without a readable name is an ERROR region. Parser
        // error-recovery absorbs the following method into the malformed
        // declaration, so the whole overlapping region degrades to a barrier
        // and abstains rather than emitting a corrupted owner.
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    /// A single independent fixture, parsed on its own with 1-based local line
    /// numbers. `owners` lists `(hit_line, name, start_line, end_line)` that
    /// must attribute to the exact owner; `abstain` lists lines that must
    /// abstain and count toward the intentional-abstention floor; `incidental`
    /// lists lines that must abstain but do NOT count.
    struct Case {
        label: &'static str,
        source: &'static str,
        owners: &'static [(u32, &'static str, u32, u32)],
        abstain: &'static [u32],
        incidental: &'static [u32],
    }

    /// US-068 curated PHP accuracy gate.
    #[test]
    fn php_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level function",
                source: "<?php\nfunction alpha() {\n    return 1;\n}\n",
                owners: &[(2, "alpha", 2, 4), (3, "alpha", 2, 4), (4, "alpha", 2, 4)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "two top-level functions",
                source: "<?php\nfunction a() {\n    return 1;\n}\nfunction b() {\n    return 2;\n}\n",
                owners: &[
                    (2, "a", 2, 4),
                    (3, "a", 2, 4),
                    (4, "a", 2, 4),
                    (5, "b", 5, 7),
                    (6, "b", 5, 7),
                    (7, "b", 5, 7),
                ],
                abstain: &[1],
                incidental: &[8],
            },
            Case {
                label: "unbraced namespace function",
                source: "<?php\nnamespace Acme\\Api;\nfunction go() {\n    return 2;\n}\n",
                owners: &[
                    (3, "Acme\\Api\\go", 3, 5),
                    (4, "Acme\\Api\\go", 3, 5),
                    (5, "Acme\\Api\\go", 3, 5),
                ],
                abstain: &[1],
                incidental: &[2, 6],
            },
            Case {
                label: "braced namespace function",
                source: "<?php\nnamespace Acme\\Api {\n    function go() {\n        return 2;\n    }\n}\n",
                owners: &[
                    (3, "Acme\\Api\\go", 3, 5),
                    (4, "Acme\\Api\\go", 3, 5),
                    (5, "Acme\\Api\\go", 3, 5),
                ],
                abstain: &[1],
                incidental: &[2, 6, 7],
            },
            Case {
                label: "class method no namespace",
                source: "<?php\nclass Service {\n    public function load() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (3, "Service::load", 3, 5),
                    (4, "Service::load", 3, 5),
                    (5, "Service::load", 3, 5),
                ],
                abstain: &[1, 2],
                incidental: &[6],
            },
            Case {
                label: "unbraced namespace class method",
                source: "<?php\nnamespace Acme\\Api;\nclass Service {\n    public function load() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (4, "Acme\\Api\\Service::load", 4, 6),
                    (5, "Acme\\Api\\Service::load", 4, 6),
                    (6, "Acme\\Api\\Service::load", 4, 6),
                ],
                abstain: &[1],
                incidental: &[2, 3, 7],
            },
            Case {
                label: "braced namespace class method",
                source: "<?php\nnamespace Acme\\Api {\n    class Service {\n        public function load() {\n            return 1;\n        }\n    }\n}\n",
                owners: &[
                    (4, "Acme\\Api\\Service::load", 4, 6),
                    (5, "Acme\\Api\\Service::load", 4, 6),
                    (6, "Acme\\Api\\Service::load", 4, 6),
                ],
                abstain: &[1],
                incidental: &[2, 3, 7, 8],
            },
            Case {
                label: "trait method",
                source: "<?php\ntrait T {\n    public function t1() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (3, "T::t1", 3, 5),
                    (4, "T::t1", 3, 5),
                    (5, "T::t1", 3, 5),
                ],
                abstain: &[1],
                incidental: &[2, 6],
            },
            Case {
                label: "interface default method",
                source: "<?php\ninterface I {\n    public function withBody() {\n        return 1;\n    }\n}\n",
                owners: &[
                    (3, "I::withBody", 3, 5),
                    (4, "I::withBody", 3, 5),
                    (5, "I::withBody", 3, 5),
                ],
                abstain: &[1],
                incidental: &[2, 6],
            },
            Case {
                label: "enum method",
                source: "<?php\nenum E {\n    case A;\n    public function e1() {\n        return 2;\n    }\n}\n",
                owners: &[
                    (4, "E::e1", 4, 6),
                    (5, "E::e1", 4, 6),
                    (6, "E::e1", 4, 6),
                ],
                abstain: &[1],
                incidental: &[2, 3, 7],
            },
            Case {
                label: "property hook get",
                source: "<?php\nclass C {\n    public int $value {\n        get => 1;\n    }\n}\n",
                owners: &[(4, "C::$value::get", 4, 4)],
                abstain: &[1, 2, 3],
                incidental: &[5, 6],
            },
            Case {
                label: "property hook set",
                source: "<?php\nclass C {\n    public int $value {\n        set { $this->value = $v; }\n    }\n}\n",
                owners: &[(4, "C::$value::set", 4, 4)],
                abstain: &[1, 2, 3],
                incidental: &[5, 6],
            },
            Case {
                label: "multi-property hooks abstain",
                source: "<?php\nclass C {\n    public int $a, $b {\n        get => 1;\n        set { $this->a = $v; }\n    }\n}\n",
                owners: &[],
                // A shared hook list has no unique property identity; the
                // per-property readings would tie on an equal range, so it
                // abstains instead.
                abstain: &[3, 4, 5],
                incidental: &[1, 2, 6, 7],
            },
            Case {
                label: "class inside function global identity",
                source: "<?php\nfunction outer() {\n    class Local {\n        public function m() {\n            return 1;\n        }\n    }\n}\n",
                owners: &[
                    (2, "outer", 2, 8),
                    (3, "outer", 2, 8),
                    (4, "Local::m", 4, 6),
                    (5, "Local::m", 4, 6),
                    (6, "Local::m", 4, 6),
                    (7, "outer", 2, 8),
                    (8, "outer", 2, 8),
                ],
                abstain: &[],
                incidental: &[1, 9],
            },
            Case {
                label: "nested named function barrier",
                source: "<?php\nfunction outer() {\n    return 1;\n    function inner() {\n        return 2;\n    }\n}\n",
                owners: &[(2, "outer", 2, 7), (3, "outer", 2, 7), (7, "outer", 2, 7)],
                abstain: &[4, 5],
                incidental: &[1, 6],
            },
            Case {
                label: "nested function inside method barrier",
                source: "<?php\nclass C {\n    public function m() {\n        function inner() { return 1; }\n    }\n}\n",
                owners: &[(3, "C::m", 3, 5), (5, "C::m", 3, 5)],
                abstain: &[4],
                incidental: &[1, 2, 6],
            },
            Case {
                label: "closure barrier",
                source: "<?php\nfunction outer() {\n    $c = function() {\n        return 1;\n    };\n}\n",
                owners: &[(2, "outer", 2, 6), (6, "outer", 2, 6)],
                abstain: &[3, 4],
                incidental: &[1, 5, 7],
            },
            Case {
                label: "arrow function barrier",
                source: "<?php\nfunction outer() {\n    $a = fn($x) => $x + 1;\n}\n",
                owners: &[(2, "outer", 2, 4), (4, "outer", 2, 4)],
                abstain: &[3],
                incidental: &[1, 5],
            },
            Case {
                label: "anonymous class barrier",
                source: "<?php\nfunction outer() {\n    $o = new class {\n        public function m() {\n            return 2;\n        }\n    };\n}\n",
                owners: &[(2, "outer", 2, 8), (8, "outer", 2, 8)],
                abstain: &[4, 5],
                incidental: &[1, 3, 6, 7, 9],
            },
            Case {
                label: "interface bodyless abstains",
                source: "<?php\ninterface I {\n    public function noBody(): void;\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "abstract class bodyless method",
                source: "<?php\nabstract class Base {\n    abstract public function abs();\n}\n",
                owners: &[],
                abstain: &[2],
                incidental: &[1, 3],
            },
            Case {
                label: "bodyless property hook",
                source: "<?php\nclass C {\n    public string $name {\n        get;\n        set;\n    }\n}\n",
                owners: &[],
                abstain: &[3, 4],
                incidental: &[1, 2, 5, 6],
            },
            Case {
                label: "malformed error degrades",
                source: "<?php\nfunction broken() {\n    $x = (\n    return 1;\n}\n",
                owners: &[],
                abstain: &[2, 3, 4],
                incidental: &[1, 5],
            },
            Case {
                label: "clean plus malformed partial",
                source: "<?php\nfunction good() {\n    return 1;\n}\nfunction broken() {\n    $x = (\n}\n",
                owners: &[(2, "good", 2, 4), (3, "good", 2, 4), (4, "good", 2, 4)],
                abstain: &[5, 6],
                incidental: &[1, 7],
            },
            Case {
                label: "two methods same class",
                source: "<?php\nclass B {\n    public function one() {\n        return 1;\n    }\n    public function two() {\n        return 2;\n    }\n}\n",
                owners: &[
                    (3, "B::one", 3, 5),
                    (4, "B::one", 3, 5),
                    (5, "B::one", 3, 5),
                    (6, "B::two", 6, 8),
                    (7, "B::two", 6, 8),
                    (8, "B::two", 6, 8),
                ],
                abstain: &[1, 2],
                incidental: &[9],
            },
            Case {
                label: "empty class no regions",
                source: "<?php\nclass Empty {}\n",
                owners: &[],
                abstain: &[1],
                incidental: &[2],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = php_owner_for(&regions, &errors, hit_line).named();
                assert!(
                    owner.is_some(),
                    "[{}] line {hit_line}: expected {name}@{start}-{end}, got abstain",
                    case.label
                );
                let owner = owner.unwrap();
                assert_eq!(owner.qualified_name(), name, "[{}] name", case.label);
                assert_eq!(owner.start_line, start, "[{}] start", case.label);
                assert_eq!(owner.end_line, end, "[{}] end", case.label);
            }
            for &line in case.abstain {
                abstentions += 1;
                assert!(
                    php_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    php_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] incidental line {line} should abstain",
                    case.label
                );
            }
        }
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
        let v: serde_json::Value = serde_json::from_str(tree_sitter_php::PHP_NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        for k in PHP_EXECUTABLE_CALLABLES
            .iter()
            .chain(PHP_CONTAINER_KINDS)
            .chain(PHP_ANONYMOUS_OR_BARRIER_KINDS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        let mut all: Vec<&str> = Vec::new();
        all.extend(PHP_EXECUTABLE_CALLABLES);
        all.extend(PHP_CONTAINER_KINDS);
        all.extend(PHP_ANONYMOUS_OR_BARRIER_KINDS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        assert_eq!(
            fnv1a(tree_sitter_php::PHP_NODE_TYPES.as_bytes()),
            PHP_NODE_TYPES_FINGERPRINT,
            "tree-sitter-php NODE_TYPES changed; re-audit the manifest"
        );
    }
}
