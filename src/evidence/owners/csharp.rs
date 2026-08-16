//! C# owner-region extraction (US-068 Wave 2A).
//!
//! Supported named owners: body-bearing methods under their lexical type and
//! namespace containers (`Service.Load`, `Acme.Api.Service.Load`); nested
//! class/struct/interface/record/enum containers appended with `.`; property
//! accessors (`Container.Property.get`/`.set`/`.init`), event accessors
//! (`Container.Event.add`/`.remove`), and indexer accessors
//! (`Container.this.get`/`.set`); expression-bodied property getters
//! (`Container.Property.get`); expression-bodied methods, constructors,
//! destructors, operators, local functions, and explicit accessors; local
//! functions appended to their enclosing callable (`Service.Load.parse`);
//! constructors and destructors using the declared type name; operators
//! (`Service.operator +`); and conversion operators including `implicit`/
//! `explicit` and the structurally spelled target type.
//!
//! Abstentions/barriers: every `lambda_expression` and
//! `anonymous_method_expression` is an anonymous barrier; body-less callables
//! (abstract, extern, interface-only, partial-signature) are not owners; an
//! accessor abstains when the parent identity or accessor kind is not complete;
//! a local function abstains when its enclosing callable/type identity is
//! anonymous or unreadable; a local function among top-level statements abstains
//! (no supported enclosing callable identity); explicit interface members are
//! preserved completely or abstain. Partial type parts are parsed independently
//! and never merged.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerAttribution,
    OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Version-pinned C# callable manifest (anti-drift contract) for the pinned
/// tree-sitter-c-sharp grammar.
#[cfg(test)]
const CSHARP_EXECUTABLE_CALLABLES: &[&str] = &[
    "method_declaration",
    "local_function_statement",
    "constructor_declaration",
    "destructor_declaration",
    "operator_declaration",
    "conversion_operator_declaration",
    "property_declaration",
    "event_declaration",
    "indexer_declaration",
    "accessor_declaration",
    "lambda_expression",
    "anonymous_method_expression",
];
#[cfg(test)]
const CSHARP_CONTAINER_KINDS: &[&str] = &[
    "namespace_declaration",
    "file_scoped_namespace_declaration",
    "class_declaration",
    "struct_declaration",
    "interface_declaration",
    "record_declaration",
    "enum_declaration",
    "declaration_list",
];
#[cfg(test)]
const CSHARP_BODILESS_OR_BARRIER_KINDS: &[&str] = &[
    "arrow_expression_clause",
    "block",
    "explicit_interface_specifier",
    "global_statement",
];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-c-sharp` `NODE_TYPES` JSON
/// (tree-sitter-c-sharp 0.23.5). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit.
#[cfg(test)]
const CSHARP_NODE_TYPES_FINGERPRINT: u64 = 0xeb71_2764_c7f6_699f;

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

/// Parse a C# file and produce its owner regions and local error ranges.
/// Returns `None` when the file cannot be read, has no tree, or its root node
/// itself is an `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn csharp_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::CSharp)?;
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
    in_callable: bool,
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
        in_callable,
        containers,
        errors,
        regions,
    );
}

/// Emit a `Named` owner covering `node`'s body-bearing range after degrading on
/// local errors, then descend.
fn named_and_descend(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    anchor_builder: impl FnOnce() -> OwnerAnchor,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let anchor = anchor_builder();
    let start_line = anchor.start_line;
    let end_line = anchor.end_line;
    let callable_segment = anchor.name.clone();
    let region = OwnerRegion::Named(anchor);
    let region = degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
    regions.push(region);
    let _ = (start_line, end_line);
    // Push the callable segment so nested local functions append to the
    // complete lexical callable path (`Service.Load.parse`).
    containers.push(callable_segment);
    walk_children(node, bytes, path, false, true, containers, errors, regions);
    containers.pop();
}

/// The structurally spelled explicit-interface prefix, if any (e.g. `IFoo.`
/// for `IFoo.GetBar`). Returns the trimmed prefix without the trailing dot.
fn explicit_interface_prefix(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.named_children(&mut node.walk())
        .find(|c| c.kind() == "explicit_interface_specifier")
        .and_then(|c| c.utf8_text(bytes).ok())
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty())
}

/// The `implicit`/`explicit` keyword of a conversion operator, if present. The
/// grammar surfaces it as an unnamed child node kind, not a modifier. Returns
/// `None` when absent so callers never synthesize a default keyword.
fn conversion_keyword(node: Node<'_>) -> Option<String> {
    node.children(&mut node.walk())
        .find(|c| matches!(c.kind(), "implicit" | "explicit"))
        .map(|c| c.kind().to_string())
}

/// Extract a required identity component, trimming whitespace and rejecting empty
/// results. Empty required components (e.g. `identifier err ''`) must not yield
/// dangling segments like `Service.` / `.get` / `operator `; callers fall back to
/// their existing fail-closed barrier/abstain path.
fn required_identity(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(bytes).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Walk a C# tree, emitting `Named` regions for body-bearing callables and
/// `AnonymousBarrier` regions for lambdas and anonymous methods.
/// `in_anonymous` is true inside such an identity barrier; `in_callable` is true
/// when inside a body-bearing callable body (used to gate top-level local
/// functions).
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    in_callable: bool,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let kind = node.kind();
    // The file-scoped namespace is emitted as a sibling of the top-level
    // declarations, not a lexical wrapper. Hoist its name into the container
    // path for the whole compilation unit.
    if kind == "compilation_unit" {
        let mut cur = node.walk();
        let has_fs_ns = node
            .named_children(&mut cur)
            .any(|c| c.kind() == "file_scoped_namespace_declaration");
        let mut cur2 = node.walk();
        let fs_name: Option<String> = node
            .named_children(&mut cur2)
            .find(|c| c.kind() == "file_scoped_namespace_declaration")
            .and_then(|c| c.child_by_field_name("name"))
            .and_then(|n| n.utf8_text(bytes).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // A malformed file-scoped namespace (e.g. `namespace ;`) surfaces as an
        // ERROR child whose text begins with the `namespace` keyword.
        let mut cur3 = node.walk();
        let has_malformed_fs_ns = node.named_children(&mut cur3).any(|c| {
            c.kind() == "ERROR"
                && c.utf8_text(bytes)
                    .ok()
                    .is_some_and(|t| t.trim_start().starts_with("namespace"))
        });
        // A file-scoped namespace exists but its name is missing/unreadable:
        // fail closed so sibling declarations cannot leak unqualified to an
        // outer container.
        if (has_fs_ns && fs_name.is_none()) || has_malformed_fs_ns {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            regions.push(OwnerRegion::AnonymousBarrier {
                start_line: start,
                end_line: end,
            });
            for child in node.named_children(&mut node.walk()) {
                if child.kind() == "file_scoped_namespace_declaration" {
                    continue;
                }
                walk(
                    child,
                    bytes,
                    path,
                    true,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
            }
            return;
        }
        if let Some(name) = fs_name.as_ref() {
            containers.push(name.clone());
        }
        let mut c2 = node.walk();
        for child in node.named_children(&mut c2) {
            if child.kind() == "file_scoped_namespace_declaration" {
                continue;
            }
            walk(
                child,
                bytes,
                path,
                in_anonymous,
                in_callable,
                containers,
                errors,
                regions,
            );
        }
        if fs_name.is_some() {
            containers.pop();
        }
        return;
    }

    // Anonymous identity barriers.
    if matches!(kind, "lambda_expression" | "anonymous_method_expression") {
        barrier_and_descend(node, bytes, path, in_callable, containers, errors, regions);
        return;
    }

    // Axis-crossing identity barriers: a body-bearing callable inside an
    // anonymous context is itself a barrier (its hits must not cross into the
    // anonymous lambda/initializer identity).
    if in_anonymous {
        match kind {
            "method_declaration"
            | "local_function_statement"
            | "constructor_declaration"
            | "destructor_declaration"
            | "operator_declaration"
            | "conversion_operator_declaration"
            | "accessor_declaration" => {
                let start = node.start_position().row as u32 + 1;
                let end = node.end_position().row as u32 + 1;
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: start,
                    end_line: end,
                });
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            _ => {}
        }
    }

    match kind {
        "namespace_declaration" | "file_scoped_namespace_declaration" => {
            if let Some(name) = required_identity(node, "name", bytes) {
                containers.push(name.clone());
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                containers.pop();
            } else {
                // Missing/unreadable namespace identity: fail closed so nested
                // methods cannot leak to an outer container.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
            }
        }
        "class_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "record_declaration"
        | "enum_declaration" => {
            if let Some(name) = required_identity(node, "name", bytes) {
                containers.push(name.clone());
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                containers.pop();
            } else {
                // Missing/unreadable type identity: fail closed so nested
                // methods cannot leak to an outer container.
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                });
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
            }
        }
        "method_declaration" => {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if !has_body {
                // Body-less (abstract/extern/interface-only/partial-signature).
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            let Some(name) = required_identity(node, "name", bytes) else {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            };
            // Explicit interface members preserve the interface prefix from the
            // AST (`IFoo.GetBar`); if it cannot be read, abstain.
            let segment = match explicit_interface_prefix(node, bytes) {
                Some(prefix) => format!("{prefix}.{name}"),
                None => name.clone(),
            };
            let qualified = join_containers(containers, &segment);
            let anchor = OwnerAnchor {
                path: path.to_path_buf(),
                name: segment.clone(),
                receiver_var: None,
                receiver_type: None,
                package_dir: Path::new(".").to_path_buf(),
                start_line: start,
                end_line: end,
                language: Lang::CSharp,
                display_name: qualified,
            };
            named_and_descend(
                node,
                bytes,
                path,
                move || anchor,
                containers,
                errors,
                regions,
            );
        }
        "local_function_statement" => {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if !has_body {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            let Some(name) = required_identity(node, "name", bytes) else {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            };
            // A local function among top-level statements has no supported
            // enclosing callable identity: intentional abstention.
            if !in_callable {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: start,
                    end_line: end,
                });
                walk_children(
                    node,
                    bytes,
                    path,
                    true,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            let qualified = join_containers(containers, &name);
            let anchor = OwnerAnchor {
                path: path.to_path_buf(),
                name: name.clone(),
                receiver_var: None,
                receiver_type: None,
                package_dir: Path::new(".").to_path_buf(),
                start_line: start,
                end_line: end,
                language: Lang::CSharp,
                display_name: qualified,
            };
            named_and_descend(
                node,
                bytes,
                path,
                move || anchor,
                containers,
                errors,
                regions,
            );
        }
        "constructor_declaration" | "destructor_declaration" => {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if !has_body {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            // Uses the structurally declared type name.
            let Some(type_name) = required_identity(node, "name", bytes)
                .or_else(|| containers.last().cloned())
                .filter(|s| !s.is_empty())
            else {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            };
            let qualified = join_containers(containers, &type_name);
            let anchor = OwnerAnchor {
                path: path.to_path_buf(),
                name: type_name.clone(),
                receiver_var: None,
                receiver_type: None,
                package_dir: Path::new(".").to_path_buf(),
                start_line: start,
                end_line: end,
                language: Lang::CSharp,
                display_name: qualified,
            };
            named_and_descend(
                node,
                bytes,
                path,
                move || anchor,
                containers,
                errors,
                regions,
            );
        }
        "operator_declaration" => {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if !has_body {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            if let Some(op) = required_identity(node, "operator", bytes) {
                let segment = format!("operator {op}");
                let qualified = join_containers(containers, &segment);
                let anchor = OwnerAnchor {
                    path: path.to_path_buf(),
                    name: segment.clone(),
                    receiver_var: None,
                    receiver_type: None,
                    package_dir: Path::new(".").to_path_buf(),
                    start_line: start,
                    end_line: end,
                    language: Lang::CSharp,
                    display_name: qualified,
                };
                named_and_descend(
                    node,
                    bytes,
                    path,
                    move || anchor,
                    containers,
                    errors,
                    regions,
                );
            } else {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
            }
        }
        "conversion_operator_declaration" => {
            let start = node.start_position().row as u32 + 1;
            let end = node.end_position().row as u32 + 1;
            let has_body = node.child_by_field_name("body").is_some();
            if !has_body {
                walk_children(
                    node,
                    bytes,
                    path,
                    in_anonymous,
                    in_callable,
                    containers,
                    errors,
                    regions,
                );
                return;
            }
            let target = required_identity(node, "type", bytes);
            if let Some(target) = target {
                let kv = conversion_keyword(node);
                if let Some(kv) = kv {
                    let segment = format!("operator {kv} {target}");
                    let qualified = join_containers(containers, &segment);
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: segment.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: Path::new(".").to_path_buf(),
                        start_line: start,
                        end_line: end,
                        language: Lang::CSharp,
                        display_name: qualified,
                    };
                    named_and_descend(
                        node,
                        bytes,
                        path,
                        move || anchor,
                        containers,
                        errors,
                        regions,
                    );
                } else {
                    // Missing `implicit`/`explicit` keyword: fail closed. Descend
                    // anonymous so nested named callables cannot leak.
                    barrier_and_descend(
                        node,
                        bytes,
                        path,
                        in_callable,
                        containers,
                        errors,
                        regions,
                    );
                }
            } else {
                // Missing/unreadable conversion target type: fail closed.
                barrier_and_descend(node, bytes, path, in_callable, containers, errors, regions);
            }
        }
        "property_declaration" | "event_declaration" | "indexer_declaration" => {
            handle_accessor_container(
                node,
                bytes,
                path,
                in_anonymous,
                in_callable,
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
            in_callable,
            containers,
            errors,
            regions,
        ),
    }
}

/// Handle property/event/indexer declarations: expression-bodied properties
/// become getter owners, and accessor lists become per-accessor owners.
fn handle_accessor_container(
    declaration: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    in_callable: bool,
    containers: &mut Vec<String>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let kind = declaration.kind();
    // Determine the base identity segment.
    //   property -> `Property`, event -> `Event`, indexer -> `this`.
    let base = if kind == "indexer_declaration" {
        Some("this".to_string())
    } else {
        required_identity(declaration, "name", bytes)
    };

    // Expression-bodied property: the `value` field is the arrow clause.
    if kind == "property_declaration" && declaration.child_by_field_name("accessors").is_none() {
        if let Some(value) = declaration.child_by_field_name("value") {
            if let Some(base) = &base {
                let start = value.start_position().row as u32 + 1;
                let end = value.end_position().row as u32 + 1;
                let segment = format!("{base}.get");
                let qualified = join_containers(containers, &segment);
                if in_anonymous {
                    regions.push(OwnerRegion::AnonymousBarrier {
                        start_line: start,
                        end_line: end,
                    });
                } else {
                    let anchor = OwnerAnchor {
                        path: path.to_path_buf(),
                        name: segment.clone(),
                        receiver_var: None,
                        receiver_type: None,
                        package_dir: Path::new(".").to_path_buf(),
                        start_line: start,
                        end_line: end,
                        language: Lang::CSharp,
                        display_name: qualified,
                    };
                    let region = OwnerRegion::Named(anchor);
                    let region = degrade_named_on_error(
                        region,
                        errors,
                        value.start_byte(),
                        value.end_byte(),
                    );
                    regions.push(region);
                }
            }
        }
        walk_children(
            declaration,
            bytes,
            path,
            in_anonymous,
            in_callable,
            containers,
            errors,
            regions,
        );
        return;
    }

    // Accessor list: accessor_declaration nodes live under an `accessor_list`
    // child. Iterate them.
    let mut cur = declaration.walk();
    let accessors: Vec<Node> = declaration
        .named_children(&mut cur)
        .filter(|c| c.kind() == "accessor_list")
        .flat_map(|al| {
            let mut alwalk = al.walk();
            al.named_children(&mut alwalk).collect::<Vec<_>>()
        })
        .collect();
    for child in accessors {
        let start = child.start_position().row as u32 + 1;
        let end = child.end_position().row as u32 + 1;
        let has_body = child.child_by_field_name("body").is_some();
        let accessor_kind = required_identity(child, "name", bytes);
        let segment = match (&base, accessor_kind.as_deref()) {
            (Some(base), Some(accessor_kind)) => Some(format!("{base}.{accessor_kind}")),
            _ => None,
        };
        if let Some(segment) = segment.as_ref() {
            let qualified = join_containers(containers, segment);
            if in_anonymous {
                regions.push(OwnerRegion::AnonymousBarrier {
                    start_line: start,
                    end_line: end,
                });
            } else if has_body {
                let anchor = OwnerAnchor {
                    path: path.to_path_buf(),
                    name: segment.clone(),
                    receiver_var: None,
                    receiver_type: None,
                    package_dir: Path::new(".").to_path_buf(),
                    start_line: start,
                    end_line: end,
                    language: Lang::CSharp,
                    display_name: qualified,
                };
                let region = OwnerRegion::Named(anchor);
                let region =
                    degrade_named_on_error(region, errors, child.start_byte(), child.end_byte());
                regions.push(region);
            }
        }
        // Required parent/accessor identity unreadable: fail closed so nested
        // local fns cannot leak to an outer container. Only a complete identity
        // (segment Some) + body + non-anonymous lets nested fns regain under the
        // full pushed accessor segment.
        match (segment.as_ref(), has_body, in_anonymous) {
            (Some(segment), true, false) => {
                // Owner already emitted above; push the segment so nested local
                // fns regain their full enclosing callable path.
                containers.push(segment.clone());
                walk_children(child, bytes, path, false, true, containers, errors, regions);
                containers.pop();
            }
            (None, true, _) => {
                // Identity unreadable: full-range barrier + anonymous descend.
                barrier_and_descend(child, bytes, path, true, containers, errors, regions);
            }
            _ => walk_children(
                child,
                bytes,
                path,
                in_anonymous,
                in_callable,
                containers,
                errors,
                regions,
            ),
        }
    }
}

fn walk_children(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    in_anonymous: bool,
    in_callable: bool,
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
            in_callable,
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

/// Attribute a C# hit line to a named owner, honoring local errors.
pub(crate) fn csharp_owner_for<'a>(
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
        PathBuf::from("tests/fixtures/x.cs")
    }

    fn parse(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        csharp_regions(&path(), src).expect("should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = csharp_owner_for(regions, errors, line).named();
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            csharp_owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    #[test]
    fn malformed_named_container_abstains_nested_method() {
        // Anonymous/missing-name class must fail closed: nested `Leaf` abstains,
        // and sibling `A` still resolves to the outer `Service` container.
        let (r, e) = parse(
            "class Service {\n    public static class {\n        void Leaf() {}\n    }\n    void A() {}\n}\n",
        );
        assert_abstain(&r, &e, 3); // Leaf inside malformed anonymous class
        assert_owner(&r, &e, 5, "Service.A"); // sibling unaffected
    }

    #[test]
    fn conversion_keyword_requires_implicit_explicit() {
        use tree_sitter::Parser;
        let mut p = Parser::new();
        let lang = tree_sitter_c_sharp::LANGUAGE;
        p.set_language(&lang.into()).unwrap();
        let valid = "class Service { public static explicit operator int(Service s) {} }\n";
        let t = p.parse(valid, None).unwrap();
        let conv = find_kind(t.root_node(), "conversion_operator_declaration");
        assert_eq!(conversion_keyword(conv), Some("explicit".to_string()),);
        let missing = "class Service { public static operator int(Service s) {} }\n";
        let t2 = p.parse(missing, None).unwrap();
        let m = find_kind(t2.root_node(), "method_declaration");
        assert_eq!(conversion_keyword(m), None);
    }

    fn find_kind<'t>(n: tree_sitter::Node<'t>, kind: &str) -> tree_sitter::Node<'t> {
        if n.kind() == kind {
            return n;
        }
        for c in n.named_children(&mut n.walk()) {
            let r = find_kind(c, kind);
            if r.kind() == kind {
                return r;
            }
        }
        n
    }

    #[test]
    fn required_identity_trims_and_rejects_empty() {
        // Every required identity component must be trimmed and must never yield
        // a dangling segment (`Service.` / `.get` / `operator `).
        use tree_sitter::Parser;
        let mut p = Parser::new();
        let lang = tree_sitter_c_sharp::LANGUAGE;
        p.set_language(&lang.into()).unwrap();
        let s = "class Service {\n    void Load() {}\n}\n";
        let tree = p.parse(s, None).unwrap();
        let root = tree.root_node();
        let cls = find_kind(root, "class_declaration");
        let method = find_kind(root, "method_declaration");
        assert_eq!(
            required_identity(cls, "name", s.as_bytes()),
            Some("Service".to_string()),
        );
        assert_eq!(
            required_identity(method, "name", s.as_bytes()),
            Some("Load".to_string()),
        );
        // Absent fields yield None (no dangling `operator ` / `.get`).
        assert_eq!(required_identity(cls, "operator", s.as_bytes()), None);
        assert_eq!(required_identity(method, "type", s.as_bytes()), None);
    }

    #[test]
    fn top_level_method_and_class_method() {
        let (r, e) = parse("class Service {\n    void Load() {\n        int x = 1;\n    }\n}\n");
        assert_owner(&r, &e, 2, "Service.Load");
        assert_owner(&r, &e, 3, "Service.Load");
    }

    #[test]
    fn namespace_container_dotted() {
        let (r, e) = parse("namespace Acme.Api {\n    class Service {\n        void Load() {\n        }\n    }\n}\n");
        assert_owner(&r, &e, 3, "Acme.Api.Service.Load");
    }

    #[test]
    fn malformed_file_scoped_namespace_abstains_sibling() {
        // A malformed file-scoped namespace (`namespace ;`) must not let the
        // sibling method leak unqualified as `Service.Load`.
        let (r, e) = parse("namespace ;\nclass Service {\n    void Load() {}\n}\n");
        assert_abstain(&r, &e, 3);
    }
    #[test]
    fn file_scoped_namespace_container() {
        let (r, e) = parse("namespace Acme;\nclass Service {\n    void Load() {\n    }\n}\n");
        assert_owner(&r, &e, 3, "Acme.Service.Load");
    }

    #[test]
    fn malformed_accessor_identity_abstains_nested_local() {
        // An accessor whose `get`/`set` identity is unreadable (empty name) must
        // fail closed: the nested local function cannot leak to the parent type.
        let (r, e) = parse("class Service {\n    int P {\n        { int Parse(){} }\n    }\n}\n");
        assert_abstain(&r, &e, 3); // Parse inside malformed accessor
    }

    #[test]
    fn nested_type_containers() {
        let (r, e) = parse(
            "class Outer {\n    class Inner {\n        void Handle() {\n        }\n    }\n}\n",
        );
        assert_owner(&r, &e, 3, "Outer.Inner.Handle");
    }

    #[test]
    fn expression_bodied_method_is_body_bearing() {
        let (r, e) = parse("class Service {\n    int Load() => 42;\n}\n");
        assert_owner(&r, &e, 2, "Service.Load");
    }

    #[test]
    fn property_accessor_get_set() {
        let (r, e) = parse(
            "class Service {\n    int P {\n        get { return 1; }\n        set { }\n    }\n}\n",
        );
        assert_owner(&r, &e, 3, "Service.P.get");
        assert_owner(&r, &e, 4, "Service.P.set");
    }

    #[test]
    fn local_fn_inside_getter_follows_full_accessor_path() {
        let (r, e) = parse(
            "class Service {\n    int P {\n        get {\n            int Parse() { return 1; }\n        }\n    }\n}\n",
        );
        assert_owner(&r, &e, 4, "Service.P.get.Parse");
    }

    #[test]
    fn expression_bodied_property_is_getter() {
        let (r, e) = parse("class Service {\n    int P => 42;\n}\n");
        assert_owner(&r, &e, 2, "Service.P.get");
    }

    #[test]
    fn auto_property_accessors_abstain() {
        let (r, e) = parse("class Service {\n    int P { get; set; }\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn event_accessors_add_remove() {
        let (r, e) = parse("class Service {\n    event System.Action E {\n        add { }\n        remove { }\n    }\n}\n");
        assert_owner(&r, &e, 3, "Service.E.add");
        assert_owner(&r, &e, 4, "Service.E.remove");
    }

    #[test]
    fn indexer_accessor_this() {
        let (r, e) = parse("class Service {\n    int this[int i] {\n        get { return i; }\n        set { }\n    }\n}\n");
        assert_owner(&r, &e, 3, "Service.this.get");
        assert_owner(&r, &e, 4, "Service.this.set");
    }

    #[test]
    fn explicit_interface_method_preserved() {
        let (r, e) = parse("interface IFoo { int GetBar(); }\nclass Service : IFoo {\n    int IFoo.GetBar() => 7;\n}\n");
        assert_owner(&r, &e, 3, "Service.IFoo.GetBar");
    }

    #[test]
    fn operator_and_conversion_operator() {
        let (r, e) = parse("class Service {\n    public static Service operator +(Service a, Service b) => a;\n    public static implicit operator int(Service s) => 1;\n    public static explicit operator int(Service s) => 1;\n}\n");
        assert_owner(&r, &e, 2, "Service.operator +");
        assert_owner(&r, &e, 3, "Service.operator implicit int");
        assert_owner(&r, &e, 4, "Service.operator explicit int");
    }

    #[test]
    fn constructor_and_destructor_use_type_name() {
        let (r, e) = parse("class Service {\n    Service() { }\n    ~Service() { }\n}\n");
        assert_owner(&r, &e, 2, "Service.Service");
        assert_owner(&r, &e, 3, "Service.Service");
    }

    #[test]
    fn local_function_appends_to_callable() {
        let (r, e) = parse(
            "class Service {\n    void Load() {\n        int Parse() { return 1; }\n    }\n}\n",
        );
        assert_owner(&r, &e, 3, "Service.Load.Parse");
    }

    #[test]
    fn top_level_local_function_abstains() {
        let (r, e) = parse("int TopLocal() => 5;\n");
        assert_abstain(&r, &e, 1);
    }

    #[test]
    fn lambda_and_anonymous_method_barriers() {
        let (r, e) = parse("class Service {\n    void Load() {\n        System.Func<int> f = () => 42;\n        System.Action a = delegate { };\n    }\n}\n");
        assert_owner(&r, &e, 2, "Service.Load");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn bodyless_method_abstains() {
        let (r, e) = parse("interface IFace {\n    void Abs();\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn partial_method_signature_abstains_impl_eligible() {
        let (r, e) = parse("partial class P {\n    partial void M();\n}\n");
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn malformed_local_error_degrades() {
        let (r, e) = parse("class Service {\n    void Broken() {\n        int x = (\n    }\n}\n");
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn clean_method_elsewhere_in_partial_tree_eligible() {
        let (r, e) = parse("class Service {\n    void Good() {\n        int x = 1;\n    }\n    void Broken() {\n        int x = (\n    }\n}\n");
        assert_owner(&r, &e, 2, "Service.Good");
        assert_owner(&r, &e, 3, "Service.Good");
        assert_abstain(&r, &e, 6);
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

    /// US-068 curated C# accuracy gate.
    #[test]
    fn csharp_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "multi-line method body",
                source: "class Service {\n    void Load() {\n        int a = 1;\n        int b = 2;\n        System.Console.WriteLine(a + b);\n    }\n}\n",
                owners: &[(2, "Service.Load", 2, 6), (3, "Service.Load", 2, 6), (4, "Service.Load", 2, 6), (5, "Service.Load", 2, 6), (6, "Service.Load", 2, 6)],
                abstain: &[1],
                incidental: &[7],
            },
            Case {
                label: "class method",
                source: "class Service {\n    void Load() {\n        int x = 1;\n    }\n}\n",
                owners: &[(2, "Service.Load", 2, 4), (3, "Service.Load", 2, 4), (4, "Service.Load", 2, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "two methods",
                source: "class Service {\n    void A() {\n        int x = 1;\n    }\n    void B() {\n        int y = 2;\n    }\n}\n",
                owners: &[
                    (2, "Service.A", 2, 4),
                    (3, "Service.A", 2, 4),
                    (4, "Service.A", 2, 4),
                    (5, "Service.B", 5, 7),
                    (6, "Service.B", 5, 7),
                    (7, "Service.B", 5, 7),
                ],
                abstain: &[1],
                incidental: &[8],
            },
            Case {
                label: "multi-line method two statements",
                source: "class Service {\n    int Add(int a, int b) {\n        int sum = a + b;\n        return sum;\n    }\n}\n",
                owners: &[(2, "Service.Add", 2, 5), (3, "Service.Add", 2, 5), (4, "Service.Add", 2, 5), (5, "Service.Add", 2, 5)],
                abstain: &[1],
                incidental: &[6],
            },
            Case {
                label: "multi-line setter body",
                source: "class Service {\n    int P {\n        set {\n            int y = value + 1;\n            Apply(y);\n        }\n    }\n}\n",
                owners: &[(3, "Service.P.set", 3, 6), (4, "Service.P.set", 3, 6), (5, "Service.P.set", 3, 6), (6, "Service.P.set", 3, 6)],
                abstain: &[1, 2],
                incidental: &[7, 8],
            },
            Case {
                label: "three-method region",
                source: "class Service {\n    void A() {}\n    void B() {}\n    void C() {}\n}\n",
                owners: &[(2, "Service.A", 2, 2), (3, "Service.B", 3, 3), (4, "Service.C", 4, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "multi-line getter body",
                source: "class Service {\n    int P {\n        get {\n            return 1;\n        }\n    }\n}\n",
                owners: &[(3, "Service.P.get", 3, 5), (4, "Service.P.get", 3, 5), (5, "Service.P.get", 3, 5)],
                abstain: &[1, 2],
                incidental: &[6, 7],
            },
            Case {
                label: "namespace + nested type method",
                source: "namespace Acme {\n    namespace Api {\n        class Service {\n            void Load() {}\n        }\n    }\n}\n",
                owners: &[(4, "Acme.Api.Service.Load", 4, 4)],
                abstain: &[1, 2, 3],
                incidental: &[5, 6, 7],
            },
            Case {
                label: "block namespace container",
                source: "namespace Acme.Api {\n    class Service {\n        void Load() {\n        }\n    }\n}\n",
                owners: &[(3, "Acme.Api.Service.Load", 3, 4), (4, "Acme.Api.Service.Load", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "file-scoped namespace container",
                source: "namespace Acme;\nclass Service {\n    void Load() {\n    }\n}\n",
                owners: &[(3, "Acme.Service.Load", 3, 4), (4, "Acme.Service.Load", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5],
            },
            Case {
                label: "nested declared types",
                source: "class Outer {\n    class Inner {\n        void Handle() {\n        }\n    }\n}\n",
                owners: &[(3, "Outer.Inner.Handle", 3, 4), (4, "Outer.Inner.Handle", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "expression-bodied method",
                source: "class Service {\n    int Load() => 42;\n}\n",
                owners: &[(2, "Service.Load", 2, 2)],
                abstain: &[1],
                incidental: &[3],
            },
            Case {
                label: "expression-bodied property getter",
                source: "class Service {\n    int P => 42;\n}\n",
                owners: &[(2, "Service.P.get", 2, 2)],
                abstain: &[1],
                incidental: &[3],
            },
            Case {
                label: "property accessor get/set",
                source: "class Service {\n    int P {\n        get { return 1; }\n        set { }\n    }\n}\n",
                owners: &[(3, "Service.P.get", 3, 3), (4, "Service.P.set", 4, 4), (3, "Service.P.get", 3, 3)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "auto-property abstains",
                source: "class Service {\n    int P { get; set; }\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "event accessors",
                source: "class Service {\n    event System.Action E {\n        add { }\n        remove { }\n    }\n}\n",
                owners: &[(3, "Service.E.add", 3, 3), (4, "Service.E.remove", 4, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "indexer accessors",
                source: "class Service {\n    int this[int i] {\n        get { return i; }\n        set { }\n    }\n}\n",
                owners: &[(3, "Service.this.get", 3, 3), (4, "Service.this.set", 4, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "explicit interface method",
                source: "interface IFoo { int GetBar(); }\nclass Service : IFoo {\n    int IFoo.GetBar() => 7;\n}\n",
                owners: &[(3, "Service.IFoo.GetBar", 3, 3)],
                abstain: &[1, 2],
                incidental: &[4],
            },
            Case {
                label: "operator",
                source: "class Service {\n    public static Service operator +(Service a, Service b) => a;\n}\n",
                owners: &[(2, "Service.operator +", 2, 2)],
                abstain: &[1],
                incidental: &[3],
            },
            Case {
                label: "conversion operators",
                source: "class Service {\n    public static implicit operator int(Service s) => 1;\n    public static explicit operator string(Service s) => \"x\";\n}\n",
                owners: &[
                    (2, "Service.operator implicit int", 2, 2),
                    (3, "Service.operator explicit string", 3, 3),
                ],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "constructor and destructor",
                source: "class Service {\n    Service() { }\n    ~Service() { }\n}\n",
                owners: &[(2, "Service.Service", 2, 2), (3, "Service.Service", 3, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "local function full path",
                source: "class Service {\n    void Load() {\n        int Parse() { return 1; }\n    }\n}\n",
                owners: &[(2, "Service.Load", 2, 4), (3, "Service.Load.Parse", 3, 3), (4, "Service.Load", 2, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "top-level local function abstains",
                source: "int TopLocal() => 5;\n",
                owners: &[],
                abstain: &[1],
                incidental: &[],
            },
            Case {
                label: "lambda barrier",
                source: "class Service {\n    void Load() {\n        System.Func<int> f = () => 42;\n    }\n}\n",
                owners: &[(2, "Service.Load", 2, 4), (4, "Service.Load", 2, 4)],
                abstain: &[3],
                incidental: &[5],
            },
            Case {
                label: "anonymous method barrier",
                source: "class Service {\n    void Load() {\n        System.Action a = delegate { };\n    }\n}\n",
                owners: &[(2, "Service.Load", 2, 4), (4, "Service.Load", 2, 4)],
                abstain: &[3],
                incidental: &[5],
            },
            Case {
                label: "bodyless interface method abstains",
                source: "interface IFace {\n    void Abs();\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "partial method signature abstains",
                source: "partial class P {\n    partial void M();\n}\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "malformed degrades",
                source: "class Service {\n    void Broken() {\n        int x = (\n    }\n}\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[4, 5],
            },
            Case {
                label: "clean plus malformed partial",
                source: "class Service {\n    void Good() {\n        int x = 1;\n    }\n    void Broken() {\n        int x = (\n    }\n}\n",
                owners: &[(2, "Service.Good", 2, 4), (3, "Service.Good", 2, 4), (4, "Service.Good", 2, 4)],
                abstain: &[6],
                incidental: &[5, 7, 8],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = csharp_owner_for(&regions, &errors, hit_line).named();
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
                    csharp_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    csharp_owner_for(&regions, &errors, line).named().is_none(),
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
        let v: serde_json::Value = serde_json::from_str(tree_sitter_c_sharp::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(!kinds.is_empty(), "NODE_TYPES must declare node kinds");
        for k in CSHARP_EXECUTABLE_CALLABLES
            .iter()
            .chain(CSHARP_CONTAINER_KINDS)
            .chain(CSHARP_BODILESS_OR_BARRIER_KINDS)
        {
            assert!(
                kinds.contains(*k),
                "manifest kind {k} missing from NODE_TYPES"
            );
        }
        let mut all: Vec<&str> = Vec::new();
        all.extend(CSHARP_EXECUTABLE_CALLABLES);
        all.extend(CSHARP_CONTAINER_KINDS);
        all.extend(CSHARP_BODILESS_OR_BARRIER_KINDS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "manifest inventories must be disjoint"
        );
        assert_eq!(
            fnv1a(tree_sitter_c_sharp::NODE_TYPES.as_bytes()),
            CSHARP_NODE_TYPES_FINGERPRINT,
            "tree-sitter-c-sharp NODE_TYPES changed; re-audit the manifest"
        );
    }
}
