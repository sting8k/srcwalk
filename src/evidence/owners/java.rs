//! Java owner-region extraction (US-068 Wave 2A).
//!
//! Supported named owners: body-bearing `method_declaration` (its identifier),
//! body-bearing `constructor_declaration` / `compact_constructor_declaration`
//! (the enclosing declared type name), and declared class/interface/enum/
//! record/annotation-type containers forming a `.` lexical hierarchy
//! (`Service.handle`, `Outer.Inner.handle`, `RecordName.RecordName`). A named
//! local class declared inside a supported method contributes its complete
//! declared-type path (`Service.handle.Local.run`). The owner range is the
//! callable AST node itself; annotations/Javadoc inside that node are owned,
//! those outside it are not.
//!
//! Abstentions/barriers: every `lambda_expression` is an `AnonymousBarrier`;
//! static and instance initializer blocks are identity barriers; an anonymous
//! class body is an identity barrier; a method without a body (interface/
//! abstract/native signature) is not a named owner; method references,
//! invocations, and object-creation expressions are never promoted to owners.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned Java callable manifest (anti-drift contract), holding the
/// pinned tree-sitter-java 0.23.5 callable kinds.
#[cfg(test)]
const JAVA_EXECUTABLE_CALLABLES: &[&str] = &[
    "method_declaration",
    "constructor_declaration",
    "compact_constructor_declaration",
    "lambda_expression",
];
#[cfg(test)]
const JAVA_CONTAINER_KINDS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "annotation_type_declaration",
];
#[cfg(test)]
const JAVA_BODILESS_OR_BARRIER_KINDS: &[&str] = &[
    "static_initializer",
    "class_body",
    "object_creation_expression",
    "method_reference",
    "method_invocation",
];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-java` `NODE_TYPES` JSON
/// (tree-sitter-java 0.23.5). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit.
#[cfg(test)]
const JAVA_NODE_TYPES_FINGERPRINT: u64 = 0x4f88_944f_73e9_4562;

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

/// Parse a Java file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn java_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Java)?;
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

/// Whether a node is an anonymous identity barrier regardless of its own kind:
/// an anonymous class body (`class_body` under `object_creation_expression` or
/// `enum_constant`) or an instance initializer (bare `block` directly under a
/// `class_body`). These carry no structural name, so everything crossing them
/// abstains.
fn is_anonymous_identity_barrier(node: Node<'_>) -> bool {
    if node.kind() == "class_body" {
        return matches!(
            node.parent().map(|p| p.kind()),
            Some("object_creation_expression" | "enum_constant")
        );
    }
    if node.kind() == "block" {
        return matches!(node.parent().map(|p| p.kind()), Some("class_body"));
    }
    false
}

/// Walk a Java tree, emitting `Named` regions for body-bearing callables and
/// `AnonymousBarrier` regions for lambdas, initializers, and anonymous class
/// bodies. `in_anonymous` is true when inside such an identity barrier: nested
/// callables then emit barriers (never `Named`) so they cannot contribute an
/// incomplete identity.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    // Anonymous identity barriers (no structural name): lambda, initializer,
    // anonymous class body. Emit a full-range barrier and descend in_anonymous.
    if node.kind() == "lambda_expression" {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
        walk_children(node, bytes, path, true, containers, errors, regions);
        return;
    }
    if node.kind() == "static_initializer" || is_anonymous_identity_barrier(node) {
        regions.push(OwnerRegion::AnonymousBarrier {
            start_line: node.start_position().row as u32 + 1,
            end_line: node.end_position().row as u32 + 1,
        });
        walk_children(node, bytes, path, true, containers, errors, regions);
        return;
    }

    match node.kind() {
        "method_declaration" | "constructor_declaration" | "compact_constructor_declaration" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
            // A body is required: interface/abstract/native signatures are not
            // named owners.
            let has_body = node.child_by_field_name("body").is_some();
            if in_anonymous {
                // Inside an anonymous identity barrier: this callable has no
                // complete stable identity; make it a barrier too.
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
                    let qualified = join_containers(containers, &name);
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: name.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: std::path::PathBuf::from("."),
                        start_line,
                        end_line,
                        language: Lang::Java,
                        display_name: qualified,
                    };
                    let region = OwnerRegion::Named(anchor);
                    let region =
                        degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                    regions.push(region);
                    // Push the callable name so a local class inside it receives
                    // the complete method path (`Service.handle.Local.run`).
                    containers.push(name);
                    walk_children(node, bytes, path, false, containers, errors, regions);
                    containers.pop();
                } else {
                    // Missing/unreadable identifier: abstain as a barrier.
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line,
                        end_line,
                    });
                    walk_children(node, bytes, path, true, containers, errors, regions);
                }
            } else {
                // Body-less declaration: not an owner, no containers to add.
                walk_children(node, bytes, path, in_anonymous, containers, errors, regions);
            }
        }
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration"
        | "annotation_type_declaration" => {
            if in_anonymous {
                // A declared type inside an anonymous barrier has no complete
                // identity; descend as a barrier so its members abstain.
                walk_children(node, bytes, path, true, containers, errors, regions);
            } else if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                containers.push(name.to_string());
                walk_children(node, bytes, path, false, containers, errors, regions);
                containers.pop();
            } else {
                // Unnamed/malformed container: pin a conservative barrier so
                // nested malformed regions cannot leak to an outer callable.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                walk_children(node, bytes, path, true, containers, errors, regions);
            }
        }
        _ => walk_children(node, bytes, path, in_anonymous, containers, errors, regions),
    }
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

/// Attribute a Java hit line to a named owner, honoring local errors.
pub(crate) fn java_owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> Option<&'a OwnerAnchor> {
    attribute_line(regions, errors, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.java")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        java_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = java_owner_for(regions, errors, line);
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            java_owner_for(regions, errors, line).is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn top_level_method_and_nested_class_hierarchy() {
        let (r, e) = parse(
            "class Service {\n    void handle() { int x = 1; }\n    class Inner { void go() { int y = 1; } }\n}\n",
        );
        assert_owner(&r, &e, 2, "Service.handle");
        assert_owner(&r, &e, 3, "Service.Inner.go");
    }

    #[test]
    fn constructor_uses_declared_type_name() {
        let (r, e) = parse(
            "class Service {\n    Service() { int x = 1; }\n    void m() { int y = 1; }\n}\n",
        );
        assert_owner(&r, &e, 2, "Service.Service");
        assert_owner(&r, &e, 3, "Service.m");
    }

    #[test]
    fn record_and_compact_constructor() {
        let (r, e) = parse("record Point(int x, int y) {\n    Point { int c = 1; }\n    void m() { int d = 1; }\n}\n");
        assert_owner(&r, &e, 2, "Point.Point");
        assert_owner(&r, &e, 3, "Point.m");
    }

    #[test]
    fn local_class_uses_complete_method_path() {
        let (r, e) = parse("class Service {\n    void outer() {\n        class Local { void run() { int a = 1; } }\n    }\n}\n");
        // The local class's method must NOT collapse to Service.run or
        // Service.outer.run; it uses the full declared-type path.
        assert_owner(&r, &e, 3, "Service.outer.Local.run");
        assert_owner(&r, &e, 2, "Service.outer");
    }

    #[test]
    fn lambda_is_anonymous_barrier() {
        let (r, e) = parse("class Service {\n    void outer() {\n        Runnable r = () -> { int a = 1; };\n    }\n}\n");
        assert_owner(&r, &e, 2, "Service.outer");
        // The lambda line falls inside the lambda barrier -> abstain.
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn static_and_instance_initializer_are_identity_barriers() {
        let (r, e) = parse("class Service {\n    static { int init = 1; }\n    { int instance = 1; }\n    void m() { int y = 1; }\n}\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 4, "Service.m");
    }

    #[test]
    fn bodyless_method_is_not_an_owner() {
        let (r, e) = parse("interface Iface {\n    void noBody();\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn anonymous_class_body_is_identity_barrier() {
        let (r, e) = parse("class Service {\n    void outer() {\n        Object o = new Object() {\n            void anon() { int b = 1; }\n        };\n    }\n}\n");
        // The anonymous class method must not be attributed to outer or
        // fabricated a class name.
        assert_owner(&r, &e, 2, "Service.outer");
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn enum_constant_class_body_is_identity_barrier() {
        let (r, e) = parse("enum Color {\n    RED { void special() { int s = 1; } }\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn malformed_local_error_degrades_to_barrier() {
        let (r, e) = parse("class Service {\n    void broken() {\n        int x = (\n    }\n}\n");
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn local_class_inside_static_initializer_abstains() {
        // A named local class declared inside an initializer has no complete
        // identity: its lexical path crosses the unnamed initializer context.
        // `run` must NOT attribute to `Service.Local.run` (or `Service.run`).
        let (r, e) = parse(
            "class Service {\n    static {\n        class Local { void run() { int a = 1; } }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn local_class_inside_instance_initializer_abstains() {
        // Same strict crossing-context invariant for a bare instance initializer.
        let (r, e) = parse(
            "class Service {\n    {\n        class Local { void run() { int a = 1; } }\n    }\n}\n",
        );
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn clean_callable_elsewhere_in_partial_tree_still_eligible() {
        let (r, e) = parse("class Service {\n    void good() { int x = 1; }\n    void broken() {\n        int x = (\n    }\n}\n");
        assert_owner(&r, &e, 2, "Service.good");
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

    /// US-068 curated Java accuracy gate: a table of independent, varied
    /// fixtures. Counts assertions programmatically and asserts the floor
    /// (`>=60` positive hits, `>=20` intentional abstentions); blank/separator
    /// lines never count toward the abstention floor. Every positive row
    /// asserts the exact owner (`qualified_name`, `start_line`, `end_line`).
    #[test]
    fn java_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level method",
                source: "class A {\n    void m() {\n        int x = 1;\n    }\n}\n",
                owners: &[(2, "A.m", 2, 4), (3, "A.m", 2, 4), (4, "A.m", 2, 4)],
                abstain: &[1],
                incidental: &[0],
            },
            Case {
                label: "two methods",
                source: "class B {\n    void a() {\n        int x = 1;\n    }\n    void b(int y) {\n        return;\n    }\n}\n",
                owners: &[
                    (2, "B.a", 2, 4),
                    (3, "B.a", 2, 4),
                    (4, "B.a", 2, 4),
                    (5, "B.b", 5, 7),
                    (6, "B.b", 5, 7),
                    (7, "B.b", 5, 7),
                ],
                abstain: &[1, 8],
                incidental: &[],
            },
            Case {
                label: "nested declared types",
                source: "class Outer {\n    class Inner {\n        void handle() {\n            int x = 1;\n        }\n    }\n\n}\n",
                owners: &[(3, "Outer.Inner.handle", 3, 5), (4, "Outer.Inner.handle", 3, 5)],
                abstain: &[1, 2],
                incidental: &[6, 7],
            },
            Case {
                label: "local class full path",
                source: "class Service {\n    void outer() {\n        class Local {\n            void run() {\n                int a = 1;\n            }\n        }\n    }\n}\n",
                owners: &[
                    (2, "Service.outer", 2, 8),
                    (3, "Service.outer", 2, 8),
                    (4, "Service.outer.Local.run", 4, 6),
                    (5, "Service.outer.Local.run", 4, 6),
                    (7, "Service.outer", 2, 8),
                    (8, "Service.outer", 2, 8),
                ],
                abstain: &[],
                incidental: &[1, 9],
            },
            Case {
                label: "constructor + method",
                source: "class Service {\n    Service() {\n        int x = 1;\n    }\n    void handle() {\n        int y = 1;\n    }\n}\n",
                owners: &[
                    (2, "Service.Service", 2, 4),
                    (3, "Service.Service", 2, 4),
                    (4, "Service.Service", 2, 4),
                    (5, "Service.handle", 5, 7),
                    (6, "Service.handle", 5, 7),
                    (7, "Service.handle", 5, 7),
                ],
                abstain: &[1],
                incidental: &[8],
            },
            Case {
                label: "record compact constructor",
                source: "record Point(int x) {\n    Point {\n        int c = 1;\n    }\n}\n",
                owners: &[
                    (3, "Point.Point", 2, 4),
                    (2, "Point.Point", 2, 4),
                    (4, "Point.Point", 2, 4),
                ],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "interface bodyless abstains",
                source: "interface Iface {\n    void noBody();\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "abstract class bodyless method",
                source: "abstract class Base {\n    abstract void abs();\n}\n",
                owners: &[],
                abstain: &[2],
                incidental: &[1, 3],
            },
            Case {
                label: "enum methods",
                source: "enum Color {\n    RED, GREEN;\n    void colorMethod() {\n        int d = 1;\n    }\n}\n",
                owners: &[
                    (3, "Color.colorMethod", 3, 5),
                    (4, "Color.colorMethod", 3, 5),
                    (5, "Color.colorMethod", 3, 5),
                ],
                abstain: &[1, 2],
                incidental: &[6],
            },
            Case {
                label: "enum constant class body barrier",
                source: "enum Color {\n    RED {\n        void special() {\n            int s = 1;\n        }\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3, 4],
                incidental: &[1, 5, 6, 7],
            },
            Case {
                label: "lambda barrier",
                source: "class Service {\n    void outer() {\n        Runnable r = () -> {\n            int a = 1;\n        };\n    }\n}\n",
                owners: &[(2, "Service.outer", 2, 6), (6, "Service.outer", 2, 6)],
                abstain: &[3, 4],
                incidental: &[1, 5, 7],
            },
            Case {
                label: "static initializer barrier",
                source: "class Service {\n    static {\n        int init = 1;\n    }\n    void m() {\n        int y = 1;\n    }\n}\n",
                owners: &[(5, "Service.m", 5, 7), (6, "Service.m", 5, 7)],
                abstain: &[2, 3],
                incidental: &[1, 4, 8],
            },
            Case {
                label: "instance initializer barrier",
                source: "class Service {\n    {\n        int inst = 1;\n    }\n    void m() {\n        int y = 1;\n    }\n}\n",
                owners: &[(5, "Service.m", 5, 7), (6, "Service.m", 5, 7)],
                abstain: &[2, 3],
                incidental: &[1, 4, 8],
            },
            Case {
                label: "anonymous class body barrier",
                source: "class Service {\n    void outer() {\n        Object o = new Object() {\n            void anon() {\n                int b = 1;\n            }\n        };\n    }\n}\n",
                owners: &[(2, "Service.outer", 2, 8), (8, "Service.outer", 2, 8)],
                abstain: &[4, 5],
                incidental: &[1, 3, 6, 7, 9],
            },
            Case {
                label: "annotation type element bodyless",
                source: "@interface Ann {\n    String value();\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "malformed error degrades",
                source: "class Service {\n    void broken() {\n        int x = (\n        int y = 2;\n    }\n}\n",
                owners: &[],
                abstain: &[2, 3, 4],
                incidental: &[1, 5, 6],
            },
            Case {
                label: "clean plus malformed partial",
                source: "class Service {\n    void good() {\n        int x = 1;\n    }\n    void broken() {\n        int x = (\n    }\n}\n",
                owners: &[(2, "Service.good", 2, 4), (3, "Service.good", 2, 4)],
                abstain: &[5, 6],
                incidental: &[1, 7, 8, 9],
            },
            Case {
                label: "method reference not an owner",
                source: "class Service {\n    void outer() {\n        Runnable r = this::m;\n    }\n}\n",
                owners: &[
                    (2, "Service.outer", 2, 4),
                    (3, "Service.outer", 2, 4),
                    (4, "Service.outer", 2, 4),
                ],
                abstain: &[],
                // The method reference itself is never promoted to a named owner;
                // hits on it fall through to the containing method.
                incidental: &[1, 5],
            },
            Case {
                label: "invocation not promoted",
                source: "class Service {\n    void a() {\n        int x = 1;\n    }\n    void b() {\n        a();\n    }\n}\n",
                owners: &[
                    (2, "Service.a", 2, 4),
                    (3, "Service.a", 2, 4),
                    (4, "Service.a", 2, 4),
                    (5, "Service.b", 5, 7),
                    (6, "Service.b", 5, 7),
                    (7, "Service.b", 5, 7),
                ],
                abstain: &[1],
                incidental: &[8],
            },
            Case {
                label: "nested local class inside method full path",
                source: "class Service {\n    void outer() {\n        class Local {\n            void run() {\n                class Deep {\n                    void go() {\n                        int a = 1;\n                    }\n                }\n            }\n        }\n    }\n}\n",
                owners: &[
                    (2, "Service.outer", 2, 12),
                    (3, "Service.outer", 2, 12),
                    (4, "Service.outer.Local.run", 4, 10),
                    (5, "Service.outer.Local.run", 4, 10),
                    (6, "Service.outer.Local.run.Deep.go", 6, 8),
                    (7, "Service.outer.Local.run.Deep.go", 6, 8),
                    (8, "Service.outer.Local.run.Deep.go", 6, 8),
                    (9, "Service.outer.Local.run", 4, 10),
                    (10, "Service.outer.Local.run", 4, 10),
                    (11, "Service.outer", 2, 12),
                    (12, "Service.outer", 2, 12),
                ],
                abstain: &[],
                incidental: &[1, 13],
            },
            Case {
                label: "qualified generic method",
                source: "class Util {\n    <T> T identity(T v) {\n        return v;\n    }\n}\n",
                owners: &[
                    (2, "Util.identity", 2, 4),
                    (3, "Util.identity", 2, 4),
                    (4, "Util.identity", 2, 4),
                ],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "empty class no regions",
                source: "class Empty {\n}\n",
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
                let owner = java_owner_for(&regions, &errors, hit_line);
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
                assert!(
                    java_owner_for(&regions, &errors, line).is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    java_owner_for(&regions, &errors, line).is_none(),
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
        let v: serde_json::Value = serde_json::from_str(tree_sitter_java::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        for k in JAVA_EXECUTABLE_CALLABLES
            .iter()
            .chain(JAVA_CONTAINER_KINDS)
            .chain(JAVA_BODILESS_OR_BARRIER_KINDS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        let mut all: Vec<&str> = Vec::new();
        all.extend(JAVA_EXECUTABLE_CALLABLES);
        all.extend(JAVA_CONTAINER_KINDS);
        all.extend(JAVA_BODILESS_OR_BARRIER_KINDS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        assert_eq!(
            fnv1a(tree_sitter_java::NODE_TYPES.as_bytes()),
            JAVA_NODE_TYPES_FINGERPRINT,
            "tree-sitter-java NODE_TYPES changed; re-audit the manifest"
        );
    }
}
