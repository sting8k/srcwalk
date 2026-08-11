//! Python owner-region extraction (US-067 phase 1).
//!
//! Supported named owners: synchronous and asynchronous `function_definition`
//! nodes, class methods, and nested classes/functions using their complete
//! lexical hierarchy with `.` (`Outer.Inner.handle`, `outer.inner`). The anchor
//! range starts at the `def`/`async def` node and ends at its body; a
//! `decorated_definition` wrapper is transparent, so decorator lines are not
//! inside the owner range and a hit in a decorator is not attributed to the
//! decorated function.
//!
//! Abstractions/barriers: every `lambda` is an `AnonymousBarrier`; a callable
//! with no structural name and an unnamed class container each emit a
//! full-callable `AnonymousBarrier` so a hit inside never falls through to an
//! enclosing named owner.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned Python callable manifest (anti-drift contract).
///
/// Inventories are kept distinct so a grammar change to one category cannot
/// silently reclassify a node into another:
///
/// * (a) executable callable kinds that MUST be classified `Named`/`Barrier`;
/// * (b) binding/container/wrapper kinds used only for naming/context;
/// * (c) body-less declaration kinds that abstain/barrier (no owner region).
///
/// The `manifest_gate_*` tests verify every kind exists in the bundled
/// `NODE_TYPES` and pin an FNV-1a fingerprint so any grammar metadata change
/// fails the gate and forces a re-audit.
/// ACTIVE for normal builds only when referenced; the manifest inventories,
/// fingerprint, and FNV-1a helper are exercised by the test-time manifest gate.
#[cfg(test)]
const PYTHON_EXECUTABLE_CALLABLES: &[&str] = &["function_definition", "lambda"];
#[cfg(test)]
const PYTHON_CONTAINER_KINDS: &[&str] = &["class_definition", "decorated_definition"];
#[cfg(test)]
const PYTHON_BODILESS_DECLARATIONS: &[&str] = &[];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-python` `NODE_TYPES` JSON
/// (tree-sitter-python 0.23.6). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit of the inventories above.
#[cfg(test)]
const PYTHON_NODE_TYPES_FINGERPRINT: u64 = 0x1dd4c1a2e4a91aeb;

/// Stable FNV-1a 64-bit hash, dependency-free and reproducible across builds.
#[cfg(test)]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Parse a Python file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn python_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Python)?;
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
        &mut containers,
        &errors,
        &mut regions,
    );
    Some((regions, errors))
}

/// Walk a Python tree, emitting `Named` regions for body-bearing
/// `function_definition` nodes and `AnonymousBarrier` regions for lambdas.
/// Class and function names push onto the lexical container stack so nested
/// callables receive their full `.`-qualified name.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    match node.kind() {
        "function_definition" => {
            let start_line = node.start_position().row as u32 + 1;
            let end_line = node.end_position().row as u32 + 1;
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
                    language: Lang::Python,
                    display_name: qualified,
                };
                let region = OwnerRegion::Named(anchor);
                let region =
                    degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
                regions.push(region);
                containers.push(name);
                recurse(node, bytes, path, containers, errors, regions);
                containers.pop();
            } else {
                // No structural name: emit a full-callable AnonymousBarrier and
                // recurse WITHOUT a name container, so a hit inside this callable
                // cannot fall through to an enclosing named owner.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line,
                    end_line,
                });
                recurse(node, bytes, path, containers, errors, regions);
            }
        }
        "class_definition" => {
            // Class is a lexical container for naming, not itself a callable.
            match node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                Some(name) => {
                    containers.push(name.to_string());
                    recurse(node, bytes, path, containers, errors, regions);
                    containers.pop();
                }
                None => {
                    // Unnamed class container: pin a conservative barrier over the
                    // whole class so nested malformed regions cannot leak to an
                    // outer function. Recurse (without a name container).
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                    });
                    recurse(node, bytes, path, containers, errors, regions);
                }
            }
        }
        "lambda" => {
            // Anonymous barrier: a hit inside a lambda must not fall through to
            // an enclosing named owner. Recurse so a named def nested inside a
            // lambda remains eligible within its own narrower range.
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
            recurse(node, bytes, path, containers, errors, regions);
        }
        // `decorated_definition` is a transparent wrapper: recurse into its
        // inner callable rather than treating the decoration as a callable. A
        // malformed decoration collapses to an ERROR node, which the error
        // ranges use to degrade any overlapping named callable.
        _ => recurse(node, bytes, path, containers, errors, regions),
    }
}

fn recurse(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, bytes, path, containers, errors, regions);
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

/// Attribute a Python hit line to a named owner, honoring local errors.
pub(crate) fn python_owner_for<'a>(
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

    /// Stable synthetic path; `python_regions` never reads the file, so no
    /// temp dir is needed and nothing must be cleaned up.
    fn path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.py")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        python_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = python_owner_for(regions, errors, line);
        assert_eq!(
            owner.map(|o| o.qualified_name()),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            python_owner_for(regions, errors, line).is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn top_level_function_and_class_method() {
        let (r, e) =
            parse("def load():\n    pass\nclass Service:\n    def handle(self):\n        pass\n");
        assert_owner(&r, &e, 1, "load");
        assert_owner(&r, &e, 4, "Service.handle");
        // Outside any callable abstains.
        assert_abstain(&r, &e, 3); // class header line
    }

    #[test]
    fn nested_class_and_def_hierarchy() {
        let (r, e) = parse("class Outer:\n    class Inner:\n        def handle(self):\n            pass\n    def method(self):\n        def inner(self):\n            pass\n");
        assert_owner(&r, &e, 3, "Outer.Inner.handle");
        assert_owner(&r, &e, 5, "Outer.method");
        assert_owner(&r, &e, 6, "Outer.method.inner");
    }

    #[test]
    fn async_function_and_decorator_range_excludes_decorator() {
        let (r, e) =
            parse("@decorator\ndef sync():\n    pass\n@decorator\nasync def go():\n    pass\n");
        // Decorator line is not inside the owner range and is not attributed.
        assert_abstain(&r, &e, 1);
        assert_owner(&r, &e, 2, "sync");
        assert_abstain(&r, &e, 4);
        assert_owner(&r, &e, 5, "go");
    }

    #[test]
    fn lambda_is_barrier_preventing_outer_fallthrough() {
        let (r, e) = parse("def outer():\n    f = lambda: 1\n    return f\n");
        // lambda line abstains (barrier), not attributed to outer.
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn nested_lambda_abstains_without_named_def() {
        let (r, e) = parse("def outer():\n    return lambda: (lambda: None)\n");
        // The inner line is inside the lambda; abstains (no named def).
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn local_error_degrades_overlapping_callable_to_barrier() {
        let (r, e) = parse("def broken():\n    x = (\n    return x\n");
        // The function spans the malformed line; it must abstain, not attribute.
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn clean_callable_elsewhere_in_partial_tree_still_eligible() {
        let (r, e) = parse("def good():\n    pass\ndef broken():\n    x = (\n    return x\n");
        assert_owner(&r, &e, 1, "good");
        // The `broken` callable overlaps the error range, so it degrades to an
        // AnonymousBarrier covering the full callable (def line included).
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn distinct_top_level_functions_each_unique_narrowest() {
        let (r, e) = parse("def a():\n    pass\ndef b():\n    pass\n");
        assert_owner(&r, &e, 1, "a");
        assert_owner(&r, &e, 3, "b");
    }

    #[test]
    fn empty_content_yields_no_regions() {
        let (r, e) = parse("x = 1\n");
        assert!(r.is_empty());
        assert!(e.is_empty());
    }

    #[test]
    fn missing_node_fixture_is_recorded_point_and_abstains() {
        // `def f(:` has a zero-width MISSING `)` node (unnamed token) at byte 6.
        // Walk-all-children must record it as a point that abstains that line.
        let (_r, e) = parse("def f(:");
        // The missing `)` point is recorded (zero-width, is_point) on line 1.
        assert!(
            e.iter()
                .any(|er| er.is_point && er.start_byte == er.end_byte),
            "{:?}",
            e
        );
        assert!(e.iter().any(|er| er.contains_line(1)), "{:?}", e);
        // A synthetic named region on line 1 must abstain due to the point.
        let anchor = OwnerAnchor {
            path: path(),
            name: "f".into(),
            receiver_var: None,
            receiver_type: None,
            package_dir: PathBuf::from("."),
            start_line: 1,
            end_line: 1,
            language: Lang::Python,
            display_name: "f".into(),
        };
        assert!(attribute_line(&[OwnerRegion::Named(anchor)], &e, 1).is_none());
    }

    #[test]
    fn malformed_unnamed_class_does_not_leak_to_outer_function() {
        // `def outer()` containing a malformed `class :` — the class collapses
        // to an ERROR spanning `outer`'s body, so `outer` degrades to a barrier
        // and a hit inside must abstain (never attribute to outer).
        let (r, e) = parse("def outer():\n    class :\n        pass\n");
        assert_abstain(&r, &e, 1); // outer def line
        assert_abstain(&r, &e, 2); // class line
        assert_abstain(&r, &e, 3);
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

    /// Slice-1 binding quality gate: a table of independent, varied fixtures.
    /// Each case is parsed separately with local line numbers (no shared buffer,
    /// no offset math). Counts assertions programmatically and asserts the floor
    /// (>=60 positive hits, >=20 intentional abstentions); mechanically repeated
    /// blank/separator lines never count toward the abstention floor. Every
    /// positive row asserts the exact owner (qualified_name, start_line,
    /// end_line); every abstention row asserts no attribution (0 mismatches).
    #[test]
    fn python_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level sync multiline",
                source: "def alpha():\n    return 1\n",
                owners: &[(1, "alpha", 1, 2), (2, "alpha", 1, 2)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level params + body",
                source: "def beta(a, b):\n    c = a + b\n    return c\n",
                owners: &[(1, "beta", 1, 3), (2, "beta", 1, 3), (3, "beta", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level defaults + branch",
                source: "def gamma(x=1, y=None):\n    if x:\n        return y\n    return x\n",
                owners: &[
                    (1, "gamma", 1, 4),
                    (2, "gamma", 1, 4),
                    (3, "gamma", 1, 4),
                    (4, "gamma", 1, 4),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level annotated",
                source: "def parse(text: str) -> int:\n    return len(text)\n",
                owners: &[(1, "parse", 1, 2), (2, "parse", 1, 2)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level loop body",
                source: "def transform(items):\n    out = []\n    for it in items:\n        out.append(it)\n    return out\n",
                owners: &[
                    (1, "transform", 1, 5),
                    (2, "transform", 1, 5),
                    (3, "transform", 1, 5),
                    (4, "transform", 1, 5),
                    (5, "transform", 1, 5),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level async",
                source: "async def fetch():\n    data = load()\n    return data\n",
                owners: &[(1, "fetch", 1, 3), (2, "fetch", 1, 3), (3, "fetch", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "top-level async loop",
                source: "async def poll():\n    while True:\n        item = await next_item()\n        if item:\n            return item\n",
                owners: &[
                    (1, "poll", 1, 5),
                    (2, "poll", 1, 5),
                    (3, "poll", 1, 5),
                    (4, "poll", 1, 5),
                    (5, "poll", 1, 5),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "decorated sync",
                source: "@app.route(\"/\")\ndef home():\n    return render()\n",
                owners: &[(2, "home", 2, 3), (3, "home", 2, 3)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "decorated async",
                source: "@decorator\nasync def go():\n    await step1()\n    await step2()\n",
                owners: &[(2, "go", 2, 4), (3, "go", 2, 4), (4, "go", 2, 4)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "class methods",
                source: "class Service:\n    def start(self):\n        self.running = True\n    def stop(self):\n        self.running = False\n\n",
                owners: &[
                    (2, "Service.start", 2, 3),
                    (3, "Service.start", 2, 3),
                    (4, "Service.stop", 4, 5),
                    (5, "Service.stop", 4, 5),
                ],
                abstain: &[1],
                incidental: &[6],
            },
            Case {
                label: "async class methods",
                source: "class AsyncService:\n    async def run(self):\n        await self._prep()\n        await self._exec()\n    async def cancel(self):\n        await self._stop()\n",
                owners: &[
                    (2, "AsyncService.run", 2, 4),
                    (3, "AsyncService.run", 2, 4),
                    (4, "AsyncService.run", 2, 4),
                    (5, "AsyncService.cancel", 5, 6),
                    (6, "AsyncService.cancel", 5, 6),
                ],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "repository methods",
                source: "class Repository:\n    def find(self, pk):\n        return self._rows[pk]\n    def insert(self, row):\n        self._rows.append(row)\n        return len(self._rows)\n    def count(self):\n        return len(self._rows)\n",
                owners: &[
                    (2, "Repository.find", 2, 3),
                    (3, "Repository.find", 2, 3),
                    (4, "Repository.insert", 4, 6),
                    (5, "Repository.insert", 4, 6),
                    (6, "Repository.insert", 4, 6),
                    (7, "Repository.count", 7, 8),
                    (8, "Repository.count", 7, 8),
                ],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "nested class + method hierarchy",
                source: "class Outer:\n    class Inner:\n        def handle(self):\n            return self\n    def method(self):\n        def inner(self):\n            return self\n        return inner\n",
                owners: &[
                    (3, "Outer.Inner.handle", 3, 4),
                    (4, "Outer.Inner.handle", 3, 4),
                    (5, "Outer.method", 5, 8),
                    (6, "Outer.method.inner", 6, 7),
                    (7, "Outer.method.inner", 6, 7),
                    (8, "Outer.method", 5, 8),
                ],
                abstain: &[1, 2],
                incidental: &[],
            },
            Case {
                label: "nested def inside def",
                source: "def factory():\n    def make():\n        return object()\n    return make\n",
                owners: &[
                    (1, "factory", 1, 4),
                    (2, "factory.make", 2, 3),
                    (3, "factory.make", 2, 3),
                    (4, "factory", 1, 4),
                ],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "deep triple nesting",
                source: "class A:\n    class B:\n        class C:\n            def deep(self):\n                return self\n\n",
                owners: &[(4, "A.B.C.deep", 4, 5), (5, "A.B.C.deep", 4, 5)],
                abstain: &[1, 2, 3],
                incidental: &[6],
            },
            Case {
                label: "lambda placement",
                source: "def outer():\n    f = lambda x: x + 1\n    return f\n",
                owners: &[(1, "outer", 1, 3), (3, "outer", 1, 3)],
                abstain: &[2],
                incidental: &[],
            },
            Case {
                label: "nested lambda",
                source: "def outer2():\n    g = lambda: (lambda: None)\n    return g\n",
                owners: &[(1, "outer2", 1, 3), (3, "outer2", 1, 3)],
                abstain: &[2],
                incidental: &[],
            },
            Case {
                label: "class-level statement outside method",
                source: "class Logger:\n    level = \"info\"\n    def log(self, msg):\n        print(msg)\n",
                owners: &[(3, "Logger.log", 3, 4), (4, "Logger.log", 3, 4)],
                abstain: &[1, 2],
                incidental: &[],
            },
            Case {
                label: "module-level statement outside callables",
                source: "def one():\n    pass\nx = 1\n\ndef two():\n    pass\n",
                owners: &[(1, "one", 1, 2), (2, "one", 1, 2), (5, "two", 5, 6), (6, "two", 5, 6)],
                abstain: &[3],
                incidental: &[4],
            },
            Case {
                label: "comment outside callable",
                source: "# comment\ndef three():\n    pass\n",
                owners: &[(2, "three", 2, 3), (3, "three", 2, 3)],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "malformed callable degrades to barrier",
                source: "def broken():\n    x = (\n    return x\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "malformed unnamed class hierarchy",
                source: "def outer():\n    class :\n        pass\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[],
            },
            Case {
                label: "missing token point",
                source: "def f(:",
                owners: &[],
                abstain: &[1],
                incidental: &[],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = python_owner_for(&regions, &errors, hit_line);
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
        // Parse the bundled NODE_TYPES and collect every declared node kind.
        let v: serde_json::Value = serde_json::from_str(tree_sitter_python::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        // Every executable/config/bodyless kind must exist in the grammar.
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        for k in PYTHON_EXECUTABLE_CALLABLES
            .iter()
            .chain(PYTHON_CONTAINER_KINDS)
            .chain(PYTHON_BODILESS_DECLARATIONS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        // Inventories must be disjoint (a node cannot be two categories).
        let mut all: Vec<&str> = Vec::new();
        all.extend(PYTHON_EXECUTABLE_CALLABLES);
        all.extend(PYTHON_CONTAINER_KINDS);
        all.extend(PYTHON_BODILESS_DECLARATIONS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        // Fingerprint is pinned so grammar metadata changes fail here.
        assert_eq!(
            fnv1a(tree_sitter_python::NODE_TYPES.as_bytes()),
            PYTHON_NODE_TYPES_FINGERPRINT,
            "tree-sitter-python NODE_TYPES changed; re-audit the manifest"
        );
    }
}
