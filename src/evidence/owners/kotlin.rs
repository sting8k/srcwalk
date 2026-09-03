//! Kotlin owner-region extraction (US-068 Wave 2A).
//!
//! Supported named owners: body-bearing `function_declaration` (its declared
//! name), class/interface/named-object/nested type containers appended with `.`
//! (`Outer.Inner.load`), a companion object as an explicit lexical container
//! (its declared name or the stable language name `Companion`), extension
//! functions prefixed by the structurally spelled receiver type
//! (`String.slug`, `Helpers.String.slug`), body-bearing secondary constructors
//! (the enclosing declared type name), property accessors
//! (`Container.property.get`/`.set`), and supported local named functions
//! appended to the complete lexical callable path (`outer.inner`).
//!
//! Abstentions/barriers: every `lambda_literal`, `annotated_lambda`, and
//! `anonymous_function` is an `AnonymousBarrier`; every `anonymous_initializer`
//! (`init {}`) and `object_literal` is an identity barrier; a function or
//! constructor without a body is not a named owner; a primary constructor
//! without a body-bearing callable range abstains; `.kts` wrappers are never
//! synthesized.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerAttribution,
    OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned Kotlin callable manifest (anti-drift contract), holding the
/// pinned tree-sitter-kotlin-ng 1.1.0 callable kinds.
#[cfg(test)]
const KOTLIN_EXECUTABLE_CALLABLES: &[&str] = &[
    "function_declaration",
    "secondary_constructor",
    "property_declaration",
    "getter",
    "setter",
    "lambda_literal",
    "annotated_lambda",
    "anonymous_function",
];
#[cfg(test)]
const KOTLIN_CONTAINER_KINDS: &[&str] = &[
    "class_declaration",
    "object_declaration",
    "companion_object",
    "class_body",
];
#[cfg(test)]
const KOTLIN_BODILESS_OR_BARRIER_KINDS: &[&str] = &[
    "primary_constructor",
    "anonymous_initializer",
    "object_literal",
];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-kotlin-ng` `NODE_TYPES`
/// JSON (tree-sitter-kotlin-ng 1.1.0). Pinned so any grammar metadata change
/// fails the manifest gate and forces a re-audit.
#[cfg(test)]
const KOTLIN_NODE_TYPES_FINGERPRINT: u64 = 0xf23c_3a3e_303d_b7e;

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

/// Collapse runs of ASCII whitespace to one space and trim, per the extension
/// receiver spelling rule. Preserves the source spelling otherwise.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim().to_string()
}

/// Parse a Kotlin file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn kotlin_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Kotlin)?;
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
    let mut containers: Vec<String> = Vec::new();
    walk(
        tree.root_node(),
        bytes,
        path,
        false,
        &mut containers,
        &errors,
        &mut regions,
    );
    Some((regions, errors))
}

/// Emit a full-range `AnonymousBarrier` for a node and descend `in_anonymous`.
fn barrier_and_descend(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
    });
    walk_children(node, bytes, path, true, containers, errors, regions);
}

/// Walk a Kotlin tree, emitting `Named` regions for body-bearing callables and
/// `AnonymousBarrier` regions for lambdas, anonymous initializers, and object
/// literals. `in_anonymous` is true when inside such an identity barrier.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    // Anonymous identity barriers (no structural name).
    if matches!(
        node.kind(),
        "lambda_literal" | "annotated_lambda" | "anonymous_function"
    ) {
        barrier_and_descend(node, bytes, path, containers, errors, regions);
        return;
    }
    if matches!(node.kind(), "anonymous_initializer" | "object_literal") {
        barrier_and_descend(node, bytes, path, containers, errors, regions);
        return;
    }
    // A primary constructor carries no body-bearing callable range; descend
    // without emitting an owner (never a container).
    if node.kind() == "primary_constructor" {
        walk_children(node, bytes, path, in_anonymous, containers, errors, regions);
        return;
    }

    match node.kind() {
        "function_declaration" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
            let has_body = node
                .named_children(&mut node.walk())
                .any(|c| c.kind() == "function_body");
            if in_anonymous {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                walk_children(node, bytes, path, true, containers, errors, regions);
            } else if has_body {
                if let Some(name) = node
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(bytes).ok())
                {
                    let name = name.to_string();
                    // Extension receiver: the named child immediately before the
                    // name field, spelled structurally (e.g. `String`).
                    let receiver = receiver_of(node, bytes);
                    let segment = match &receiver {
                        Some(r) => format!("{r}.{name}"),
                        None => name.clone(),
                    };
                    let qualified = join_containers(containers, &segment);
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: name.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: std::path::PathBuf::from("."),
                        start_line,
                        end_line,
                        language: Lang::Kotlin,
                        display_name: qualified,
                    };
                    let region = OwnerRegion::Named(anchor);
                    let region =
                        degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                    regions.push(region);
                    // Push the display segment so nested local functions append
                    // to the complete lexical callable path (`outer.inner`).
                    containers.push(segment);
                    walk_children(node, bytes, path, false, containers, errors, regions);
                    containers.pop();
                } else {
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line,
                        end_line,
                    });
                    walk_children(node, bytes, path, true, containers, errors, regions);
                }
            } else {
                // Body-less function (interface/abstract): not an owner.
                walk_children(node, bytes, path, in_anonymous, containers, errors, regions);
            }
        }
        "secondary_constructor" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
            let has_body = node
                .named_children(&mut node.walk())
                .any(|c| c.kind() == "block");
            if in_anonymous {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                walk_children(node, bytes, path, true, containers, errors, regions);
            } else if has_body {
                // Uses the enclosing declared type name.
                if let Some(type_name) = containers.last() {
                    let type_name = type_name.clone();
                    let qualified = join_containers(containers, &type_name);
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: type_name.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: std::path::PathBuf::from("."),
                        start_line,
                        end_line,
                        language: Lang::Kotlin,
                        display_name: qualified,
                    };
                    let region = OwnerRegion::Named(anchor);
                    let region =
                        degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                    regions.push(region);
                }
                walk_children(node, bytes, path, false, containers, errors, regions);
            } else {
                walk_children(node, bytes, path, in_anonymous, containers, errors, regions);
            }
        }
        "property_declaration" => {
            // Extract the property name from the variable_declaration child,
            // then emit getter/setter owners with their accessor kind.
            let prop_name = node
                .named_children(&mut node.walk())
                .find(|c| c.kind() == "variable_declaration")
                .and_then(|v| v.named_children(&mut v.walk()).next())
                .and_then(|id| id.utf8_text(bytes).ok())
                .map(String::from);
            let mut cur = node.walk();
            for child in node.named_children(&mut cur) {
                if matches!(child.kind(), "getter" | "setter") {
                    let start_line = child.start_position().row as u32 + 1;
                    let end_line = child.end_position().row as u32 + 1;
                    let has_body = child
                        .named_children(&mut child.walk())
                        .any(|c| c.kind() == "function_body");
                    // Grammar kind is `getter`/`setter`; display uses `get`/`set`.
                    let kind = if child.kind() == "setter" {
                        "set"
                    } else {
                        "get"
                    };
                    if in_anonymous {
                        regions.push(OwnerRegion::AnonymousBarrier {
                            start_line,
                            end_line,
                        });
                    } else if has_body {
                        if let Some(prop) = &prop_name {
                            let segment = format!("{prop}.{kind}");
                            let qualified = join_containers(containers, &segment);
                            let anchor = OwnerAnchor {
                                path: path.to_path_buf(),
                                name: segment.clone(),
                                receiver_var: None,
                                receiver_type: None,
                                package_dir: std::path::PathBuf::from("."),
                                start_line,
                                end_line,
                                language: Lang::Kotlin,
                                display_name: qualified,
                            };
                            let region = OwnerRegion::Named(anchor);
                            let region = degrade_named_on_error(
                                region,
                                errors,
                                child.start_byte(),
                                child.end_byte(),
                            );
                            regions.push(region);
                        }
                    }
                } else {
                    walk(
                        child,
                        bytes,
                        path,
                        in_anonymous,
                        containers,
                        errors,
                        regions,
                    );
                }
            }
        }
        "class_declaration" | "object_declaration" | "companion_object" => {
            if in_anonymous {
                walk_children(node, bytes, path, true, containers, errors, regions);
            } else {
                let name = node
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(bytes).ok())
                    .map(String::from)
                    // A companion object without a declared name uses the stable
                    // language name `Companion`.
                    .or_else(|| {
                        (node.kind() == "companion_object").then(|| "Companion".to_string())
                    });
                if let Some(name) = name {
                    containers.push(name);
                    walk_children(node, bytes, path, false, containers, errors, regions);
                    containers.pop();
                } else {
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                    });
                    walk_children(node, bytes, path, true, containers, errors, regions);
                }
            }
        }
        _ => walk_children(node, bytes, path, in_anonymous, containers, errors, regions),
    }
}

/// The structurally spelled receiver type of an extension function, if any: the
/// type node immediately preceding the name field. Non-type nodes (for example
/// `type_parameters`) are not receivers, so the child must be a type kind.
fn receiver_of(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let mut cur = node.walk();
    let kids: Vec<Node> = node.named_children(&mut cur).collect();
    for (i, k) in kids.iter().enumerate() {
        if k.id() == name.id() && i > 0 {
            let prev = &kids[i - 1];
            if !matches!(
                prev.kind(),
                "user_type"
                    | "nullable_type"
                    | "parenthesized_type"
                    | "function_type"
                    | "dynamic_type"
            ) {
                return None;
            }
            let text = prev.utf8_text(bytes).ok()?;
            return Some(collapse_whitespace(text));
        }
    }
    None
}

fn walk_children(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(
            child,
            bytes,
            path,
            in_anonymous,
            containers,
            errors,
            regions,
        );
    }
}

fn join_containers(containers: &[String], name: &str) -> String {
    if containers.is_empty() {
        name.to_string()
    } else {
        let mut joined = containers.join(".");
        joined.push('.');
        joined.push_str(name);
        joined
    }
}

/// Attribute a Kotlin hit line to a named owner, honoring local errors.
pub(crate) fn kotlin_owner_for<'a>(
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
        PathBuf::from("tests/fixtures/x.kt")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        kotlin_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = kotlin_owner_for(regions, errors, line).named();
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            kotlin_owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn top_level_function_and_class_method() {
        let (r, e) = parse(
            "fun topLevel() {\n    val x = 1\n}\nclass Service {\n    fun load() {\n    }\n}\n",
        );
        assert_owner(&r, &e, 1, "topLevel");
        assert_owner(&r, &e, 5, "Service.load");
    }

    #[test]
    fn nested_types_and_named_object() {
        let (r, e) = parse("class Outer {\n    class Inner {\n        fun handle() {\n        }\n    }\n    object Named {\n        fun go() {\n        }\n    }\n}\n");
        assert_owner(&r, &e, 3, "Outer.Inner.handle");
        assert_owner(&r, &e, 7, "Outer.Named.go");
    }

    #[test]
    fn companion_object_default_and_named() {
        let (r, e) = parse("class Service {\n    companion object {\n        fun factory() {\n        }\n    }\n    companion object Factory {\n        fun make() {\n        }\n    }\n}\n");
        assert_owner(&r, &e, 3, "Service.Companion.factory");
        assert_owner(&r, &e, 7, "Service.Factory.make");
    }

    #[test]
    fn extension_function_receiver_prefix() {
        let (r, e) = parse("fun String.slug() {\n    return 1\n}\n");
        assert_owner(&r, &e, 1, "String.slug");
        assert_owner(&r, &e, 2, "String.slug");
    }

    #[test]
    fn extension_function_with_lexical_container() {
        let (r, e) =
            parse("class Helpers {\n    fun String.slug() {\n        return 1\n    }\n}\n");
        assert_owner(&r, &e, 2, "Helpers.String.slug");
    }

    #[test]
    fn secondary_constructor_uses_declared_type_name() {
        let (r, e) =
            parse("class WithCtor(val x: Int) {\n    constructor(s: String) {\n    }\n}\n");
        assert_owner(&r, &e, 2, "WithCtor.WithCtor");
    }

    #[test]
    fn property_accessors_get_set() {
        let (r, e) = parse("class Registry {\n    var prop: Int = 0\n        get() =\n            1\n        set(value) {\n        }\n}\n");
        assert_owner(&r, &e, 3, "Registry.prop.get");
        assert_owner(&r, &e, 5, "Registry.prop.set");
    }

    #[test]
    fn local_function_appends_to_callable_path() {
        let (r, e) = parse("fun outer() {\n    fun inner() {\n    }\n}\n");
        assert_owner(&r, &e, 1, "outer");
        assert_owner(&r, &e, 2, "outer.inner");
    }

    #[test]
    fn lambda_and_anonymous_function_are_barriers() {
        let (r, e) = parse(
            "fun outer() {\n    val l = { x: Int -> x + 1 }\n    val ef = fun(x: Int) = x\n}\n",
        );
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn anonymous_initializer_and_object_literal_are_identity_barriers() {
        let (r, e) = parse("class A {\n    init {\n        val x = 1\n    }\n    val o = object {\n        fun anon() {\n        }\n    }\n}\n");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 6);
    }

    #[test]
    fn local_function_inside_initializer_abstains() {
        // A named local function declared inside an init block has no complete
        // identity: its lexical path crosses the unnamed initializer context.
        // Hits inside `local` must NOT attribute to `A.local`.
        let (r, e) = parse("class A {\n    init {\n        fun local() {\n            val x = 1\n        }\n    }\n}\n");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn generic_function_has_no_receiver() {
        let (r, e) = parse("fun <T> foo(x: T) {\n    return x\n}\n");
        assert_owner(&r, &e, 1, "foo");
        assert_owner(&r, &e, 2, "foo");
    }

    #[test]
    fn generic_extension_function_reader() {
        let (r, e) = parse("fun <T> List<T>.first(): T {\n    return this[0]\n}\n");
        assert_owner(&r, &e, 1, "List<T>.first");
    }

    #[test]
    fn bodyless_function_abstains() {
        let (r, e) = parse("interface Iface {\n    fun abs()\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn primary_constructor_abstains() {
        let (r, e) = parse("class C(val x: Int) {\n}\n");
        assert_abstain(&r, &e, 1);
    }

    #[test]
    fn malformed_local_error_degrades_to_barrier() {
        let (r, e) = parse("fun broken() {\n    val x = (\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn clean_callable_elsewhere_in_partial_tree_still_eligible() {
        let (r, e) = parse("fun good() {\n    val x = 1\n}\nfun broken() {\n    val x = (\n}\n");
        assert_owner(&r, &e, 1, "good");
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

    /// US-068 curated Kotlin accuracy gate.
    #[test]
    fn kotlin_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level function",
                source: "fun alpha() {\n    val x = 1\n}\n",
                owners: &[(1, "alpha", 1, 3), (2, "alpha", 1, 3), (3, "alpha", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "two top-level functions",
                source: "fun a() {\n    val x = 1\n}\nfun b() {\n    val y = 2\n}\n",
                owners: &[
                    (1, "a", 1, 3),
                    (2, "a", 1, 3),
                    (3, "a", 1, 3),
                    (4, "b", 4, 6),
                    (5, "b", 4, 6),
                    (6, "b", 4, 6),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "class method",
                source: "class Service {\n    fun load() {\n        return 1\n    }\n}\n",
                owners: &[
                    (2, "Service.load", 2, 4),
                    (3, "Service.load", 2, 4),
                    (4, "Service.load", 2, 4),
                ],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "nested declared types",
                source: "class Outer {\n    class Inner {\n        fun handle() {\n            return 1\n        }\n    }\n}\n",
                owners: &[((3, "Outer.Inner.handle", 3, 5)), ((4, "Outer.Inner.handle", 3, 5)), ((5, "Outer.Inner.handle", 3, 5))],
                abstain: &[1, 2],
                incidental: &[6, 7],
            },
            Case {
                label: "named object",
                source: "class Service {\n    object Named {\n        fun go() {\n        }\n    }\n}\n",
                owners: &[((3, "Service.Named.go", 3, 4)), ((4, "Service.Named.go", 3, 4))],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "default companion object",
                source: "class Service {\n    companion object {\n        fun factory() {\n        }\n    }\n}\n",
                owners: &[(3, "Service.Companion.factory", 3, 4), (4, "Service.Companion.factory", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "named companion object",
                source: "class Service {\n    companion object Factory {\n        fun make() {\n        }\n    }\n}\n",
                owners: &[(3, "Service.Factory.make", 3, 4), (4, "Service.Factory.make", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "extension function receiver",
                source: "fun String.slug() {\n    return 1\n}\n",
                owners: &[(1, "String.slug", 1, 3), (2, "String.slug", 1, 3), (3, "String.slug", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "extension with container",
                source: "class Helpers {\n    fun String.slug() {\n        return 1\n    }\n}\n",
                owners: &[(2, "Helpers.String.slug", 2, 4), (3, "Helpers.String.slug", 2, 4), (4, "Helpers.String.slug", 2, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "generic receiver spelling",
                source: "fun List<T>.firstOrNullSafe() {\n    return null\n}\n",
                owners: &[(1, "List<T>.firstOrNullSafe", 1, 3), (2, "List<T>.firstOrNullSafe", 1, 3), (3, "List<T>.firstOrNullSafe", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "secondary constructor",
                source: "class WithCtor(val x: Int) {\n    constructor(s: String) {\n    }\n}\n",
                owners: &[(2, "WithCtor.WithCtor", 2, 3), (3, "WithCtor.WithCtor", 2, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "property getter",
                source: "class Registry {\n    val prop: Int\n        get() =\n            1\n}\n",
                owners: &[(3, "Registry.prop.get", 3, 4), (4, "Registry.prop.get", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5],
            },
            Case {
                label: "property setter",
                source: "class Registry {\n    var prop: Int = 0\n        set(value) {\n        }\n}\n",
                owners: &[(3, "Registry.prop.set", 3, 4), (4, "Registry.prop.set", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5],
            },
            Case {
                label: "local function full path",
                source: "fun outer() {\n    fun inner() {\n        return 1\n    }\n}\n",
                owners: &[
                    (1, "outer", 1, 5),
                    (2, "outer.inner", 2, 4),
                    (3, "outer.inner", 2, 4),
                    (4, "outer.inner", 2, 4),
                    (5, "outer", 1, 5),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "deep local nesting",
                source: "fun a() {\n    fun b() {\n        fun c() {\n        }\n    }\n}\n",
                owners: &[(1, "a", 1, 6), (2, "a.b", 2, 5), (3, "a.b.c", 3, 4), (4, "a.b.c", 3, 4), (5, "a.b", 2, 5), (6, "a", 1, 6)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "lambda barrier",
                source: "fun outer() {\n    val l = { x: Int -> x + 1 }\n    return l\n}\n",
                owners: &[(1, "outer", 1, 4), (3, "outer", 1, 4), (4, "outer", 1, 4)],
                abstain: &[2],
                incidental: &[],
            },
            Case {
                label: "annotated lambda barrier",
                source: "fun outer() {\n    call {\n        val x = 1\n    }\n}\n",
                owners: &[(1, "outer", 1, 5), (5, "outer", 1, 5)],
                abstain: &[2, 3],
                incidental: &[4],
            },
            Case {
                label: "anonymous function barrier",
                source: "fun outer() {\n    val ef = fun(x: Int) = x\n}\n",
                owners: &[(1, "outer", 1, 3), (3, "outer", 1, 3)],
                abstain: &[2],
                incidental: &[],
            },
            Case {
                label: "interface default method",
                source: "interface Iface {\n    fun defaultImpl() {\n    }\n}\n",
                owners: &[(2, "Iface.defaultImpl", 2, 3), (3, "Iface.defaultImpl", 2, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "nested object with member",
                source: "class Holder {\n    object Conf {\n        fun load() {\n        }\n    }\n}\n",
                owners: &[(3, "Holder.Conf.load", 3, 4), (4, "Holder.Conf.load", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "anonymous initializer barrier",
                source: "class A {\n    init {\n        val x = 1\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3],
                incidental: &[1, 4, 5],
            },
            Case {
                label: "object literal barrier",
                source: "class A {\n    val o = object {\n        fun anon() {\n        }\n    }\n}\n",
                owners: &[],
                abstain: &[3, 4],
                incidental: &[1, 2, 5, 6, 7],
            },
            Case {
                label: "bodyless interface abstains",
                source: "interface Iface {\n    fun abs()\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "primary constructor abstains",
                source: "class C(val x: Int) {\n}\n",
                owners: &[],
                abstain: &[1],
                incidental: &[2],
            },
            Case {
                label: "malformed error degrades",
                source: "fun broken() {\n    val x = (\n    val y = 2\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[4],
            },
            Case {
                label: "clean plus malformed partial",
                source: "fun good() {\n    val x = 1\n}\nfun broken() {\n    val x = (\n}\n",
                owners: &[(1, "good", 1, 3), (2, "good", 1, 3), (3, "good", 1, 3)],
                abstain: &[4, 5],
                incidental: &[6],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = kotlin_owner_for(&regions, &errors, hit_line).named();
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
                    kotlin_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    kotlin_owner_for(&regions, &errors, line).named().is_none(),
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
        let v: serde_json::Value = serde_json::from_str(tree_sitter_kotlin_ng::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        for k in KOTLIN_EXECUTABLE_CALLABLES
            .iter()
            .chain(KOTLIN_CONTAINER_KINDS)
            .chain(KOTLIN_BODILESS_OR_BARRIER_KINDS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        let mut all: Vec<&str> = Vec::new();
        all.extend(KOTLIN_EXECUTABLE_CALLABLES);
        all.extend(KOTLIN_CONTAINER_KINDS);
        all.extend(KOTLIN_BODILESS_OR_BARRIER_KINDS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        assert_eq!(
            fnv1a(tree_sitter_kotlin_ng::NODE_TYPES.as_bytes()),
            KOTLIN_NODE_TYPES_FINGERPRINT,
            "tree-sitter-kotlin-ng NODE_TYPES changed; re-audit the manifest"
        );
    }
}
