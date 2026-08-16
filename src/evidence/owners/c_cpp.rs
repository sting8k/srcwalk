//! C/C++ owner-region extraction (US-072).
//!
//! Supported named owners: body-bearing `function_definition` nodes with a
//! complete parser-backed identity. C owners render as the plain callable name
//! (`add`). C++ owners qualify through lexical namespaces and types
//! (`ns::run`, `ns::Class::method`, `outer::Inner::method`) and through
//! complete qualified out-of-line declarators (`Foo::bar`, `ns::Foo::bar`,
//! `Box<T>::set`). Templates are transparent for identity (template parameter
//! text is never part of the owner name); constructor/destructor/operator
//! terminals are preserved only when the grammar exposes one complete identity
//! (`Foo::Foo`, `Foo::~Foo`, `Foo::operator=`).
//!
//! Abstentions/barriers: every C++ `lambda_expression` is a full-range
//! `AnonymousBarrier`; anonymous namespaces/types and malformed containers
//! establish identity barriers so nested functions cannot leak an incorrectly
//! shortened name; `friend_declaration` is a barrier (a friend function is not
//! a lexical member of the enclosing class and the grammar provides no
//! namespace evidence for it); body-less callables (`= default`, `= delete`,
//! prototypes) are not owners; K&R C definitions parse without a complete
//! `function_definition` and abstain naturally; macro definitions and macro
//! invocations never fabricate owners; a C++ type-less `function_definition`
//! (e.g. googletest `TEST(Foo, Bar) { ... }`) is treated as a macro hazard and
//! abstains unless its terminal is a destructor or a constructor whose name
//! matches the innermost lexical type or the last declarator scope component;
//! local `ERROR`/missing-node degradation flows through the shared primitives.
//!
//! This module is syntax-only. It never claims which preprocessor branch is
//! active, what a macro expands into, binding/linkage, or any call edge.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    attribute_line, collect_error_ranges, degrade_named_on_error, ErrorRange, OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Disposition of every grammar callable/preprocessor hazard relevant to the C
/// adapter (anti-drift contract, US-072 §10.2): named, transparent, barrier, or
/// abstain. Every entry must exist in the pinned grammar's `NODE_TYPES`, and
/// every `preproc_*` / callable / macro kind in `NODE_TYPES` must appear here.
#[cfg(test)]
const C_CALLABLE_OR_HAZARD_DISPOSITIONS: &[(&str, &str)] = &[
    ("function_definition", "named"),
    ("preproc_if", "transparent"),
    ("preproc_ifdef", "transparent"),
    ("preproc_elif", "transparent"),
    ("preproc_elifdef", "transparent"),
    ("preproc_else", "transparent"),
    ("preproc_include", "transparent"),
    ("preproc_defined", "transparent"),
    ("preproc_arg", "transparent"),
    ("preproc_params", "transparent"),
    ("preproc_directive", "transparent"),
    ("preproc_function_def", "abstain"),
    ("preproc_def", "abstain"),
    ("preproc_call", "abstain"),
    ("macro_type_specifier", "abstain"),
];

/// C++ disposition table: the C table plus C++-only callable/container hazards.
#[cfg(test)]
const CPP_CALLABLE_OR_HAZARD_DISPOSITIONS: &[(&str, &str)] = &[
    ("function_definition", "named"),
    ("lambda_expression", "barrier"),
    ("template_declaration", "transparent"),
    ("friend_declaration", "barrier"),
    ("preproc_if", "transparent"),
    ("preproc_ifdef", "transparent"),
    ("preproc_elif", "transparent"),
    ("preproc_elifdef", "transparent"),
    ("preproc_else", "transparent"),
    ("preproc_include", "transparent"),
    ("preproc_defined", "transparent"),
    ("preproc_arg", "transparent"),
    ("preproc_params", "transparent"),
    ("preproc_directive", "transparent"),
    ("preproc_function_def", "abstain"),
    ("preproc_def", "abstain"),
    ("preproc_call", "abstain"),
];

/// Container node kinds that qualify C++ identities. C struct/union are
/// traversed without qualification (C functions cannot be lexical members).
#[cfg(test)]
const C_CONTAINER_KINDS: &[&str] = &["struct_specifier", "union_specifier"];

#[cfg(test)]
const CPP_CONTAINER_KINDS: &[&str] = &[
    "namespace_definition",
    "nested_namespace_specifier",
    "class_specifier",
    "struct_specifier",
    "union_specifier",
];

/// Declarator wrapper kinds unwrapped (via their `declarator` field or a single
/// declarator-shaped named child) before reaching the identity terminal.
/// `reference_declarator` exists only in the C++ grammar.
#[cfg(test)]
const C_WRAPPER_KINDS: &[&str] = &[
    "function_declarator",
    "pointer_declarator",
    "parenthesized_declarator",
    "array_declarator",
    "attributed_declarator",
];

#[cfg(test)]
const CPP_WRAPPER_KINDS: &[&str] = &[
    "function_declarator",
    "pointer_declarator",
    "reference_declarator",
    "parenthesized_declarator",
    "array_declarator",
    "attributed_declarator",
];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-c` `NODE_TYPES` JSON
/// (tree-sitter-c 0.24.2). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit. Computed from the exact bundled crate.
#[cfg(test)]
const C_NODE_TYPES_FINGERPRINT: u64 = 0x954a_3d94_fd42_a08e;

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-cpp` `NODE_TYPES` JSON
/// (tree-sitter-cpp 0.23.4). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit.
#[cfg(test)]
const CPP_NODE_TYPES_FINGERPRINT: u64 = 0x709a_08d1_1ae7_0d51;

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

/// Parse a C or C++ file and produce its owner regions and local error ranges.
/// Accepts only `Lang::C | Lang::Cpp`. Returns `None` when the language is
/// unsupported, parser setup/parsing fails, or the root itself is `ERROR`
/// (preserve raw hits, emit no owner evidence).
pub(crate) fn regions_for(
    lang: Lang,
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    if !matches!(lang, Lang::C | Lang::Cpp) {
        return None;
    }
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
    let mut containers: Vec<(String, bool)> = Vec::new();
    walk(
        tree.root_node(),
        bytes,
        path,
        lang,
        false,
        &mut containers,
        &errors,
        &mut regions,
    );
    Some((regions, errors))
}

/// Attribute a C/C++ hit line to a named owner, honoring local errors.
pub(crate) fn owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> Option<&'a OwnerAnchor> {
    attribute_line(regions, errors, line)
}

/// Emit a full-range `AnonymousBarrier` for `node` and descend `in_anonymous`.
fn barrier_and_descend(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
    });
    walk_children(node, bytes, path, lang, true, containers, errors, regions);
}

fn walk_children(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    in_anonymous: bool,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(
            child,
            bytes,
            path,
            lang,
            in_anonymous,
            containers,
            errors,
            regions,
        );
    }
}

/// Walk a C/C++ tree, emitting `Named` regions for body-bearing identifiable
/// functions and `AnonymousBarrier` regions for lambdas, friend declarations,
/// anonymous/malformed containers, function-likes inside anonymous contexts,
/// and every function whose identity abstains. `containers` holds the C++
/// lexical path as `(name, is_type)` pairs so the type-less constructor gate
/// can distinguish a lexical class from a namespace.
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    in_anonymous: bool,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let kind = node.kind();
    if lang == Lang::Cpp {
        // Every lambda is a full-range identity barrier; hits inside abstain
        // rather than falling through to the enclosing named function.
        if kind == "lambda_expression" {
            barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
            return;
        }
        // A friend function is not a lexical member of the enclosing class and
        // the grammar exposes no namespace evidence for it: fail closed so its
        // hits never attribute to a fabricated `Class::friend` identity.
        if kind == "friend_declaration" {
            barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
            return;
        }
    }
    // Any function-like construct encountered inside an anonymous context must
    // remain a barrier and never regain a fabricated named identity.
    if in_anonymous && kind == "function_definition" {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    }
    match kind {
        "namespace_definition" if lang == Lang::Cpp => {
            handle_namespace(
                node,
                bytes,
                path,
                lang,
                in_anonymous,
                containers,
                errors,
                regions,
            );
        }
        "class_specifier" | "struct_specifier" | "union_specifier" if lang == Lang::Cpp => {
            handle_type(
                node,
                bytes,
                path,
                lang,
                in_anonymous,
                containers,
                errors,
                regions,
            );
        }
        // C struct/union: traverse but never qualify free function owners (C
        // functions cannot be lexical members). Anonymous C structs are
        // transparent for the same reason.
        "struct_specifier" | "union_specifier" if lang == Lang::C => {
            walk_children(
                node,
                bytes,
                path,
                lang,
                in_anonymous,
                containers,
                errors,
                regions,
            );
        }
        // Templates are transparent for identity: descend and attribute the
        // contained body-bearing function_definition normally.
        "template_declaration" if lang == Lang::Cpp => {
            walk_children(
                node,
                bytes,
                path,
                lang,
                in_anonymous,
                containers,
                errors,
                regions,
            );
        }
        "function_definition" => {
            handle_function(
                node,
                bytes,
                path,
                lang,
                in_anonymous,
                containers,
                errors,
                regions,
            );
        }
        // Preprocessor nodes, enum_specifier, and everything else descend
        // transparently; only complete function_definition nodes become owners.
        _ => walk_children(
            node,
            bytes,
            path,
            lang,
            in_anonymous,
            containers,
            errors,
            regions,
        ),
    }
}

/// Handle a C++ `namespace_definition`: push its complete name (flattening
/// nested `a::b` syntax into `a`, `b`), or establish an identity barrier for an
/// anonymous namespace (no fabricated text, descendants must not leak).
fn handle_namespace(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    in_anonymous: bool,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let Some(components) = namespace_components(node, bytes) else {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    };
    for component in &components {
        containers.push((component.clone(), false));
    }
    walk_children(
        node,
        bytes,
        path,
        lang,
        in_anonymous,
        containers,
        errors,
        regions,
    );
    for _ in &components {
        containers.pop();
    }
}

/// Handle a C++ class/struct/union: push a complete named type identity, or
/// fail closed with an identity barrier when the name is missing/malformed so
/// nested functions cannot leak an incorrectly shortened owner.
fn handle_type(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    in_anonymous: bool,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let Some(components) = type_identity(node, bytes) else {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    };
    for component in &components {
        containers.push((component.clone(), true));
    }
    walk_children(
        node,
        bytes,
        path,
        lang,
        in_anonymous,
        containers,
        errors,
        regions,
    );
    for _ in &components {
        containers.pop();
    }
}

/// Handle a `function_definition`: emit a `Named` owner only when the body
/// exists and the declarator unwraps to one complete identity, then descend.
fn handle_function(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    lang: Lang,
    in_anonymous: bool,
    containers: &mut Vec<(String, bool)>,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    // Body-bearing only: C++ `= default` / `= delete` and prototypes have no
    // `body` field and are never owners.
    if node.child_by_field_name("body").is_none() {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    }
    let Some(declarator) = node.child_by_field_name("declarator") else {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    };
    let Some((terminal_kind, terminal, scope_components)) = extract_identity(declarator, bytes)
    else {
        barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
        return;
    };

    // C++ type-less definitions are constructors/destructors/conversion
    // operators OR macro hazards. A type-less body-bearing function whose
    // terminal is not a destructor and does not match the innermost lexical
    // type or the last declarator scope component (e.g. googletest
    // `TEST(Foo, Bar) { ... }`) is macro-generated syntax and must abstain
    // (US-072 §7.2). C grammar requires a `type` on every function_definition.
    if lang == Lang::Cpp && node.child_by_field_name("type").is_none() {
        let is_destructor = terminal_kind == "destructor_name";
        // Only a lexical TYPE (class/struct/union) can be a constructor name;
        // a namespace component must never qualify a type-less function
        // (`namespace Faux { Faux() { ... } }` is not a constructor).
        let matches_lexical_class = containers
            .iter()
            .rev()
            .find(|(_, is_type)| *is_type)
            .is_some_and(|(name, _)| name == &terminal);
        let matches_declarator_scope = scope_components.last().is_some_and(|c| c == &terminal);
        if !is_destructor && !matches_lexical_class && !matches_declarator_scope {
            barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
            return;
        }
    }

    // Build the qualified display path. An unqualified declarator prepends the
    // lexical containers. A qualified declarator is authoritative; lexical
    // scope is prepended only when structurally compatible: if any lexical
    // component already appears in the declarator path, combining would
    // duplicate or contradict the qualification and the whole function
    // abstains (US-072 §5.2). Duplicates within the declarator path itself are
    // legitimate (`Foo::Foo` constructor, `A::A::f` nested member).
    let display: Vec<String> = if scope_components.is_empty() {
        let mut display_path: Vec<String> =
            containers.iter().map(|(name, _)| name.clone()).collect();
        display_path.push(terminal.clone());
        display_path
    } else {
        let declarator_path: Vec<&str> = scope_components
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(terminal.as_str()))
            .collect();
        if containers
            .iter()
            .any(|(name, _)| declarator_path.contains(&name.as_str()))
        {
            barrier_and_descend(node, bytes, path, lang, containers, errors, regions);
            return;
        }
        let mut display_path: Vec<String> =
            containers.iter().map(|(name, _)| name.clone()).collect();
        display_path.extend(scope_components.iter().cloned());
        display_path.push(terminal.clone());
        display_path
    };

    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    let anchor = OwnerAnchor {
        path: path.to_path_buf(),
        name: terminal.clone(),
        receiver_var: None,
        receiver_type: None,
        package_dir: Path::new(".").to_path_buf(),
        start_line,
        end_line,
        language: lang,
        display_name: display.join("::"),
    };
    let region = OwnerRegion::Named(anchor);
    let region = degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
    regions.push(region);
    walk_children(
        node,
        bytes,
        path,
        lang,
        in_anonymous,
        containers,
        errors,
        regions,
    );
}

/// Flatten a namespace name into ordered components. `namespace ns` -> `[ns]`;
/// `namespace a::b::c` -> `[a, b, c]`. Returns `None` for a missing/empty name
/// (anonymous namespace) or any unreadable component.
fn namespace_components(node: Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    let name = node.child_by_field_name("name")?;
    let mut components = Vec::new();
    let mut stack: Vec<Node> = vec![name];
    while let Some(current) = stack.pop() {
        if current.is_error() || current.is_missing() {
            return None;
        }
        match current.kind() {
            "namespace_identifier" => {
                components.push(identifier_text(current, bytes)?);
            }
            "nested_namespace_specifier" => {
                let mut cursor = current.walk();
                let mut children: Vec<Node> = current.named_children(&mut cursor).collect();
                children.reverse();
                stack.extend(children);
            }
            _ => return None,
        }
    }
    if components.is_empty() {
        return None;
    }
    Some(components)
}

/// Extract a complete C++ class/struct/union type identity. Supports
/// `type_identifier` (`Foo`), `template_type` (`Foo<int>`), and
/// `qualified_identifier` (`ns::Foo`). Returns `None` for anonymous or
/// malformed names (caller fails closed).
fn type_identity(node: Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    let name = node.child_by_field_name("name")?;
    if name.is_error() || name.is_missing() {
        return None;
    }
    match name.kind() {
        "type_identifier" => Some(vec![identifier_text(name, bytes)?]),
        "template_type" => Some(vec![template_type_text(name, bytes)?]),
        "qualified_identifier" => extract_qualified_identifier(name, bytes)
            .map(|path| path.into_iter().map(|(_, text)| text).collect()),
        _ => None,
    }
}

/// Unwrap declarator wrappers down to the identity terminal, returning
/// `(kind, terminal_text, scope_components)` for a complete identity.
///
/// Wrappers (`function_declarator`, `pointer_declarator`,
/// `reference_declarator`, `parenthesized_declarator`, `array_declarator`,
/// `attributed_declarator`) are unwrapped through their `declarator` field or,
/// when the grammar exposes none, through a single declarator-shaped named
/// child. The terminal must be an identifier-like node, a C++
/// `qualified_identifier` (whose scope yields the left-to-right components), or
/// a `destructor_name`/`operator_name`. Anything else (preprocessor fragments,
/// `ERROR`, `operator_cast`, `template_function`, `dependent_name`, `decltype`,
/// multiple plausible terminals) abstains.
fn extract_identity(node: Node<'_>, bytes: &[u8]) -> Option<(String, String, Vec<String>)> {
    let mut node = node;
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 64 {
            return None;
        }
        if node.is_error() || node.is_missing() {
            return None;
        }
        if !is_wrapper_kind(node.kind()) {
            break;
        }
        if let Some(inner) = node.child_by_field_name("declarator") {
            node = inner;
            continue;
        }
        // Wrappers without a `declarator` field: exactly one declarator-shaped
        // named child (e.g. `parenthesized_declarator`, `attributed_declarator`,
        // `reference_declarator`).
        let mut cursor = node.walk();
        let candidates: Vec<Node> = node
            .named_children(&mut cursor)
            .filter(|c| is_wrapper_kind(c.kind()) || is_terminal_kind(c.kind()))
            .collect();
        if candidates.len() != 1 {
            return None;
        }
        node = candidates[0];
    }
    if node.is_error() || node.is_missing() {
        return None;
    }
    let kind = node.kind().to_string();
    match node.kind() {
        "qualified_identifier" => {
            let path = extract_qualified_identifier(node, bytes)?;
            if path.is_empty() {
                return None;
            }
            let (terminal_kind, terminal) = path.last()?.clone();
            let scope = path[..path.len() - 1]
                .iter()
                .map(|(_, text)| text.clone())
                .collect();
            Some((terminal_kind, terminal, scope))
        }
        "identifier" | "field_identifier" | "type_identifier" | "statement_identifier" => {
            Some((kind, identifier_text(node, bytes)?, Vec::new()))
        }
        "destructor_name" | "operator_name" => {
            Some((kind, terminal_text(node, bytes)?, Vec::new()))
        }
        _ => None,
    }
}

/// Recursively extract a C++ `qualified_identifier` path in left-to-right
/// order as `(kind, text)` pairs (`ns::Foo::bar` -> `[(ns, ns), (ns, Foo),
/// (identifier, bar)]`). The `scope` field contributes its single component;
/// the `name` field must contain exactly one supported named child (an inner
/// `qualified_identifier` or a terminal). An `ERROR` inside the path (e.g.
/// `Foo::operator int`), a missing component, or an unsupported terminal
/// abstains. The terminal's own kind is preserved so callers can distinguish a
/// destructor (`Foo::~Foo`) from a plain qualified name.
fn extract_qualified_identifier(node: Node<'_>, bytes: &[u8]) -> Option<Vec<(String, String)>> {
    if node.is_error() || node.is_missing() {
        return None;
    }
    // Reject ERROR/missing children anywhere in the identity path (e.g.
    // `Foo::operator int` surfaces an ERROR in the name position that is not
    // field-assigned); a damaged qualification must abstain (§5.1).
    let mut direct = node.walk();
    if node
        .named_children(&mut direct)
        .any(|c| c.is_error() || c.is_missing())
    {
        return None;
    }
    let mut path = Vec::new();
    if let Some(scope) = node.child_by_field_name("scope") {
        let kind = scope.kind().to_string();
        path.push((kind, scope_component_text(scope, bytes)?));
    }
    let name_children = named_children_in_field(node, "name");
    if name_children.len() != 1 {
        return None;
    }
    let name = name_children[0];
    match name.kind() {
        "qualified_identifier" => {
            path.extend(extract_qualified_identifier(name, bytes)?);
        }
        "identifier" | "field_identifier" | "type_identifier" => {
            let kind = name.kind().to_string();
            path.push((kind, identifier_text(name, bytes)?));
        }
        "destructor_name" | "operator_name" => {
            let kind = name.kind().to_string();
            path.push((kind, terminal_text(name, bytes)?));
        }
        _ => return None,
    }
    Some(path)
}

/// Named children of `node` assigned to the given field (field names are read
/// through the parent, covering `multiple` fields like `qualified_identifier`
/// `name`).
fn named_children_in_field<'a>(node: Node<'a>, field: &str) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    children
        .into_iter()
        .enumerate()
        .filter(|(index, _)| node.field_name_for_named_child(*index as u32) == Some(field))
        .map(|(_, child)| child)
        .collect()
}

/// One deterministic scope component: `namespace_identifier` text or a
/// `template_type` spelling (`Box` + `<T>`). `decltype`/`dependent_name` scopes
/// and anything unreadable abstain.
fn scope_component_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.is_error() || node.is_missing() {
        return None;
    }
    match node.kind() {
        "namespace_identifier" => identifier_text(node, bytes),
        "template_type" => template_type_text(node, bytes),
        _ => None,
    }
}

/// Deterministic `template_type` spelling: the `name` field text joined with
/// the `arguments` field text (`Box` + `<T>` -> `Box<T>`). Template arguments
/// attached to scope components may be retained in display only when extraction
/// is complete and deterministic (US-072 §6); missing/unreadable parts abstain.
fn template_type_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let arguments = node.child_by_field_name("arguments")?;
    if name.is_error() || name.is_missing() || arguments.is_error() || arguments.is_missing() {
        return None;
    }
    let name = identifier_text(name, bytes)?;
    let arguments = raw_text(arguments, bytes)?;
    Some(format!("{name}{arguments}"))
}

/// A simple identifier-like terminal: trimmed, non-empty, no whitespace,
/// newline, or structural punctuation.
fn identifier_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?.trim().to_string();
    if text.is_empty()
        || text.chars().any(|c| {
            c.is_whitespace() || matches!(c, '(' | ')' | '{' | '}' | ';' | ',' | ':' | '<' | '>')
        })
    {
        return None;
    }
    Some(text)
}

/// A grammar-terminal spelling (`~Foo`, `operator+`, `operator new`,
/// `operator()`): trimmed, non-empty, no newline or structural punctuation.
/// Spaces are allowed because `operator new`/`operator delete` spellings
/// contain them. Parentheses are allowed because `operator()` is a valid
/// terminal spelling; this helper only ever reads `destructor_name` /
/// `operator_name` nodes, so plain identifiers stay protected by
/// `identifier_text`.
fn terminal_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?.trim().to_string();
    if text.is_empty()
        || text
            .chars()
            .any(|c| c == '\n' || matches!(c, '{' | '}' | ';' | ',' | ':'))
    {
        return None;
    }
    Some(text)
}

/// Raw structural text (`template_argument_list`): trimmed, non-empty, no
/// newline or braces/semicolon.
fn raw_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?.trim().to_string();
    if text.is_empty()
        || text
            .chars()
            .any(|c| c == '\n' || matches!(c, '{' | '}' | ';'))
    {
        return None;
    }
    Some(text)
}

fn is_wrapper_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_declarator"
            | "pointer_declarator"
            | "reference_declarator"
            | "parenthesized_declarator"
            | "array_declarator"
            | "attributed_declarator"
    )
}

/// Kinds that can terminate an identity path after wrapper unwrapping. Unwrap
/// keeps them so they can reach the terminal match; unsupported terminals
/// (`operator_cast`, `template_function`, `dependent_name`, ...) abstain there.
fn is_terminal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "field_identifier"
            | "type_identifier"
            | "statement_identifier"
            | "qualified_identifier"
            | "destructor_name"
            | "operator_name"
            | "operator_cast"
            | "template_type"
            | "template_function"
            | "template_method"
            | "dependent_name"
            | "decltype"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn path(lang: Lang) -> PathBuf {
        match lang {
            Lang::C => PathBuf::from("tests/fixtures/x.c"),
            Lang::Cpp => PathBuf::from("tests/fixtures/x.cpp"),
            other => panic!("unexpected lang {other:?}"),
        }
    }

    fn parse_c(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        regions_for(Lang::C, &path(Lang::C), src).expect("C fixture should parse")
    }

    fn parse_cpp(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        regions_for(Lang::Cpp, &path(Lang::Cpp), src).expect("C++ fixture should parse")
    }

    fn assert_owner(
        regions: &[OwnerRegion],
        errors: &[ErrorRange],
        line: u32,
        name: &str,
        lang: Lang,
    ) {
        let owner = owner_for(regions, errors, line);
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
        if let Some(owner) = owner {
            assert_eq!(owner.language, lang, "line {line} language");
        }
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            owner_for(regions, errors, line).is_none(),
            "line {line} should abstain"
        );
    }

    // ---- C focused cases ----

    #[test]
    fn c_top_level_ansi_function_exact_full_range() {
        let (r, e) = parse_c("int add(int a, int b) {\n    return a + b;\n}\n");
        assert_owner(&r, &e, 1, "add", Lang::C);
        assert_owner(&r, &e, 2, "add", Lang::C);
        assert_owner(&r, &e, 3, "add", Lang::C);
        let owner = owner_for(&r, &e, 1).unwrap();
        assert_eq!((owner.start_line, owner.end_line), (1, 3));
        assert_eq!(owner.name, "add");
    }

    #[test]
    fn c_pointer_and_parenthesized_declarators() {
        let (r, e) =
            parse_c("int *get_ptr(void) {\n    return 0;\n}\nint (f)(int x) {\n    return x;\n}\n");
        assert_owner(&r, &e, 1, "get_ptr", Lang::C);
        assert_owner(&r, &e, 2, "get_ptr", Lang::C);
        assert_owner(&r, &e, 4, "f", Lang::C);
    }

    #[test]
    fn c_prototype_without_body_abstains() {
        let (r, e) = parse_c("int add(int a, int b);\nint real(int x) {\n    return x;\n}\n");
        assert_abstain(&r, &e, 1);
        assert_owner(&r, &e, 2, "real", Lang::C);
    }

    #[test]
    fn c_nested_blocks_do_not_change_owner() {
        let (r, e) = parse_c("void outer(void) {\n    if (1) {\n        while (1) {\n            break;\n        }\n    }\n}\n");
        for line in 1..=7 {
            assert_owner(&r, &e, line, "outer", Lang::C);
        }
    }

    #[test]
    fn c_struct_union_do_not_qualify_free_functions() {
        let (r, e) = parse_c("struct S {\n    int x;\n    int (*handler)(int);\n};\nunion U {\n    int y;\n};\nvoid f(void) {\n    return;\n}\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 5);
        assert_owner(&r, &e, 8, "f", Lang::C);
        assert_owner(&r, &e, 9, "f", Lang::C);
    }

    #[test]
    fn c_kr_definition_abstains() {
        let (r, e) = parse_c("sum(a, b)\nint a;\nint b;\n{\n    return a + b;\n}\n");
        for line in 1..=6 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn c_conditional_children_are_structural_owners() {
        let (r, e) = parse_c("#ifdef FOO\nint a(void) {\n    return 1;\n}\n#else\nint b(void) {\n    return 2;\n}\n#endif\n");
        assert_owner(&r, &e, 2, "a", Lang::C);
        assert_owner(&r, &e, 3, "a", Lang::C);
        assert_owner(&r, &e, 6, "b", Lang::C);
        assert_abstain(&r, &e, 1); // directive lines have no owner
        assert_abstain(&r, &e, 5);
    }

    #[test]
    fn c_macro_never_fabricates_an_owner() {
        // A macro definition and a semicolon-terminated macro invocation never
        // become owners; the following real function stays clean and named.
        let (r, e) = parse_c("#define MAKE_FN(name) int name(void) { return 1; }\nMAKE_FN(foo);\nint real(void) {\n    return 1;\n}\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 3, "real", Lang::C);
        assert_owner(&r, &e, 4, "real", Lang::C);
    }

    #[test]
    fn c_macro_fragmented_definition_abstains() {
        // An unterminated macro invocation directly before a definition merges
        // into one damaged function_definition with an ERROR: no owner.
        let (r, e) = parse_c("#define MAKE_FN(name) int name(void) { return 1; }\nMAKE_FN(foo)\nint real(void) { return 1; }\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn c_macro_tokens_inside_body_still_attribute_to_enclosing_function() {
        let (r, e) = parse_c("void f(void) {\n    LOG(\"x\");\n    int y = TRACE_LEVEL;\n}\n");
        for line in 1..=4 {
            assert_owner(&r, &e, line, "f", Lang::C);
        }
    }

    #[test]
    fn c_local_error_degrades_only_affected_function() {
        // The C grammar keeps `broken` as a function_definition with a local
        // ERROR inside it, so the whole function degrades to a barrier while
        // the clean `also_good` sibling keeps its owner.
        let (r, e) = parse_c("int good(void) {\n    return 1;\n}\nint broken(void) {\n    int x = (\n}\nint also_good(void) {\n    return 2;\n}\n");
        assert_owner(&r, &e, 1, "good", Lang::C);
        assert_owner(&r, &e, 2, "good", Lang::C);
        assert_abstain(&r, &e, 4); // broken opening (degraded barrier)
        assert_abstain(&r, &e, 5); // error line
        assert_abstain(&r, &e, 6); // broken closing (degraded barrier)
        assert_owner(&r, &e, 7, "also_good", Lang::C);
        assert_owner(&r, &e, 8, "also_good", Lang::C);
    }

    // ---- C++ focused cases ----

    #[test]
    fn cpp_namespace_free_function() {
        let (r, e) = parse_cpp("namespace ns {\nvoid run(void) {\n    return;\n}\n}\n");
        assert_owner(&r, &e, 2, "ns::run", Lang::Cpp);
        assert_owner(&r, &e, 3, "ns::run", Lang::Cpp);
    }

    #[test]
    fn cpp_nested_namespace_syntax_and_nodes() {
        let (r, e) = parse_cpp("namespace a::b {\nvoid run(void) {\n}\n}\nnamespace x {\nnamespace y {\nvoid go(void) {\n}\n}\n}\n");
        assert_owner(&r, &e, 2, "a::b::run", Lang::Cpp);
        assert_owner(&r, &e, 7, "x::y::go", Lang::Cpp);
    }

    #[test]
    fn cpp_inline_class_struct_union_methods() {
        let (r, e) = parse_cpp("class A {\npublic:\n    void m(void) {\n    }\n};\nstruct S {\n    void n(void) {\n    }\n};\nunion U {\n    void o(void) {\n    }\n};\n");
        assert_owner(&r, &e, 3, "A::m", Lang::Cpp);
        assert_owner(&r, &e, 4, "A::m", Lang::Cpp);
        assert_owner(&r, &e, 8, "S::n", Lang::Cpp);
        assert_owner(&r, &e, 12, "U::o", Lang::Cpp);
    }

    #[test]
    fn cpp_nested_types() {
        let (r, e) = parse_cpp("class Outer {\n    class Inner {\n        void handle(void) {\n        }\n    };\n};\n");
        assert_owner(&r, &e, 3, "Outer::Inner::handle", Lang::Cpp);
        assert_owner(&r, &e, 4, "Outer::Inner::handle", Lang::Cpp);
    }

    #[test]
    fn cpp_templates_transparent_for_identity() {
        let (r, e) = parse_cpp("template<class T> T max_value(T a, T b) {\n    return a;\n}\nnamespace n {\ntemplate<class T> T make() {\n    return T();\n}\n}\n");
        assert_owner(&r, &e, 1, "max_value", Lang::Cpp);
        assert_owner(&r, &e, 2, "max_value", Lang::Cpp);
        assert_owner(&r, &e, 5, "n::make", Lang::Cpp);
    }

    #[test]
    fn cpp_out_of_line_definitions() {
        let (r, e) = parse_cpp("void Foo::bar() {\n}\nvoid ns::Foo::bar() {\n}\nnamespace ns2 {\nvoid Foo::bar() {\n}\n}\n");
        assert_owner(&r, &e, 1, "Foo::bar", Lang::Cpp);
        assert_owner(&r, &e, 3, "ns::Foo::bar", Lang::Cpp);
        assert_owner(&r, &e, 6, "ns2::Foo::bar", Lang::Cpp);
    }

    #[test]
    fn cpp_out_of_line_template_member() {
        let (r, e) = parse_cpp("template<class T> void Box<T>::set(T v) {\n    this->v = v;\n}\n");
        assert_owner(&r, &e, 1, "Box<T>::set", Lang::Cpp);
        assert_owner(&r, &e, 2, "Box<T>::set", Lang::Cpp);
    }

    #[test]
    fn cpp_duplicate_qualification_abstains() {
        // `namespace A { namespace B { void A::C::f() {} } }` would duplicate A
        // when combined; the whole function abstains rather than guessing.
        let (r, e) = parse_cpp("namespace A {\nnamespace B {\nvoid A::C::f() {\n}\n}\n}\n");
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    #[test]
    fn cpp_constructor_destructor_operator() {
        let (r, e) = parse_cpp("struct Foo {\n    Foo() {\n    }\n    ~Foo() {\n    }\n    Foo& operator=(const Foo& o) {\n        return *this;\n    }\n};\nFoo::Foo() {\n}\nFoo::~Foo() {\n}\n");
        assert_owner(&r, &e, 2, "Foo::Foo", Lang::Cpp);
        assert_owner(&r, &e, 4, "Foo::~Foo", Lang::Cpp);
        assert_owner(&r, &e, 6, "Foo::operator=", Lang::Cpp);
        assert_owner(&r, &e, 7, "Foo::operator=", Lang::Cpp);
        assert_owner(&r, &e, 10, "Foo::Foo", Lang::Cpp);
        assert_owner(&r, &e, 12, "Foo::~Foo", Lang::Cpp);
    }

    #[test]
    fn cpp_call_operator_inline_member() {
        let (r, e) =
            parse_cpp("struct F {\n    int operator()(int x) {\n        return x;\n    }\n};\n");
        assert_owner(&r, &e, 2, "F::operator()", Lang::Cpp);
        assert_owner(&r, &e, 3, "F::operator()", Lang::Cpp);
        let owner = owner_for(&r, &e, 2).unwrap();
        assert_eq!((owner.start_line, owner.end_line), (2, 4));
        assert_eq!(owner.name, "operator()");
    }

    #[test]
    fn cpp_call_operator_const_overload() {
        let (r, e) = parse_cpp(
            "struct F {\n    int operator()(int x) const {\n        return x;\n    }\n};\n",
        );
        assert_owner(&r, &e, 2, "F::operator()", Lang::Cpp);
        assert_owner(&r, &e, 3, "F::operator()", Lang::Cpp);
    }

    #[test]
    fn cpp_call_operator_out_of_line() {
        let (r, e) = parse_cpp("int Foo::operator()(int x) {\n    return x;\n}\n");
        assert_owner(&r, &e, 1, "Foo::operator()", Lang::Cpp);
        assert_owner(&r, &e, 2, "Foo::operator()", Lang::Cpp);
        let owner = owner_for(&r, &e, 1).unwrap();
        assert_eq!((owner.start_line, owner.end_line), (1, 3));
    }

    #[test]
    fn cpp_call_operator_out_of_line_namespace_qualified() {
        let (r, e) = parse_cpp("int ns::Foo::operator()() {\n    return 0;\n}\n");
        assert_owner(&r, &e, 1, "ns::Foo::operator()", Lang::Cpp);
        assert_owner(&r, &e, 2, "ns::Foo::operator()", Lang::Cpp);
    }

    #[test]
    fn cpp_overloads_keep_structural_display() {
        let (r, e) =
            parse_cpp("class A {\n    void f(int) {\n    }\n    void f(double) {\n    }\n};\n");
        assert_owner(&r, &e, 2, "A::f", Lang::Cpp);
        assert_owner(&r, &e, 4, "A::f", Lang::Cpp);
        // Distinct ranges: overload 1 is 2-3, overload 2 is 4-5. Four owned
        // hit lines across the two distinct anchors.
        let owners: Vec<&OwnerAnchor> =
            (1..=5).filter_map(|line| owner_for(&r, &e, line)).collect();
        assert_eq!(owners.len(), 4);
        let mut ranges: std::collections::HashSet<(u32, u32)> =
            owners.iter().map(|o| (o.start_line, o.end_line)).collect();
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn cpp_lambda_barriers_nested_and_in_method() {
        let (r, e) = parse_cpp("void outer() {\n    auto l = [](int x) {\n        return x;\n    };\n    auto m = []() {\n        return []() {\n            return 1;\n        };\n    };\n}\n");
        assert_owner(&r, &e, 1, "outer", Lang::Cpp);
        assert_abstain(&r, &e, 3); // inside first lambda
        assert_abstain(&r, &e, 7); // inside nested lambda
        assert_owner(&r, &e, 10, "outer", Lang::Cpp);
    }

    #[test]
    fn cpp_anonymous_namespace_and_type_prevent_leakage() {
        let (r, e) = parse_cpp("namespace outer {\nnamespace {\nvoid f() {\n}\n}\nclass {\npublic:\n    void g() {\n    }\n} anon;\nvoid h() {\n}\n}\n");
        assert_abstain(&r, &e, 3); // f in anonymous namespace
        assert_abstain(&r, &e, 4);
        assert_abstain(&r, &e, 8); // g in anonymous class
        assert_owner(&r, &e, 11, "outer::h", Lang::Cpp); // sibling unaffected
    }

    #[test]
    fn cpp_malformed_container_abstains_nested_function() {
        let (r, e) = parse_cpp("class A {\n    class {\n        void leaf() {\n        }\n    };\n    void sibling() {\n    }\n};\n");
        assert_abstain(&r, &e, 3); // leaf inside anonymous class
        assert_owner(&r, &e, 6, "A::sibling", Lang::Cpp); // clean sibling survives
    }

    #[test]
    fn cpp_friend_definition_is_a_barrier() {
        let (r, e) = parse_cpp("class A {\n    friend void f() {\n    }\n};\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn cpp_test_macro_generated_function_abstains() {
        // googletest-style macro-generated body: parses as a type-less
        // function_definition but must never become owner `TEST` (§7.2).
        let (r, e) =
            parse_cpp("TEST(Foo, Bar) {\n    EXPECT_EQ(1, 1);\n}\nvoid real() {\n    return;\n}\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 4, "real", Lang::Cpp);
    }

    #[test]
    fn cpp_defaulted_and_deleted_abstain() {
        let (r, e) = parse_cpp(
            "struct S {\n    S() = default;\n    ~S() = delete;\n    void real() {\n    }\n};\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 4, "S::real", Lang::Cpp);
    }

    #[test]
    fn cpp_conditional_and_macro_hazards_mirror_c() {
        let (r, e) = parse_cpp("#ifdef FOO\nvoid a() {\n}\n#else\nvoid b() {\n}\n#endif\n#define FN(x) void x() {}\nFN(c)\n");
        assert_owner(&r, &e, 2, "a", Lang::Cpp);
        assert_owner(&r, &e, 5, "b", Lang::Cpp);
        assert_abstain(&r, &e, 7); // #define line
        assert_abstain(&r, &e, 8); // FN(c) macro invocation
    }

    #[test]
    fn cpp_clean_sibling_remains_eligible_beside_malformed_function() {
        let (r, e) = parse_cpp("void good() {\n    return;\n}\nvoid broken( {\n    int x = 1;\n}\nvoid also_good() {\n    return;\n}\n");
        assert_owner(&r, &e, 1, "good", Lang::Cpp);
        assert_owner(&r, &e, 2, "good", Lang::Cpp);
        assert_abstain(&r, &e, 5); // malformed region
        assert_abstain(&r, &e, 6);
    }

    #[test]
    fn cpp_bodyless_prototype_abstains() {
        let (r, e) =
            parse_cpp("class A {\n    void declared();\n    void defined() {\n    }\n};\n");
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 3, "A::defined", Lang::Cpp);
    }

    // ---- US-072 P1 regression (Biscuit review) ----

    #[test]
    fn cpp_type_less_namespace_name_is_not_a_constructor() {
        // P1-1: `Faux()` inside `namespace Faux` is NOT a constructor. The
        // type-less gate must match only a lexical class/struct/union
        // container or the final declarator scope component, never a
        // namespace, so the body abstains.
        let (r, e) = parse_cpp("namespace Faux {\nFaux() {\n    namespace_hit();\n}\n}\n");
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
    }

    #[test]
    fn cpp_type_less_constructor_inside_class_control() {
        // Control for P1-1: a real constructor (terminal matches the innermost
        // lexical CLASS) still owns with the full lexical path.
        let (r, e) = parse_cpp("namespace N {\nclass Faux {\n    Faux() {\n    }\n};\n}\n");
        assert_owner(&r, &e, 3, "N::Faux::Faux", Lang::Cpp);
        assert_owner(&r, &e, 4, "N::Faux::Faux", Lang::Cpp);
    }

    #[test]
    fn cpp_abstained_macro_function_does_not_leak_nested_owner() {
        // P1-2: a macro-generated type-less function (TEST) becomes a
        // full-range AnonymousBarrier; a nested function inside its body must
        // not regain a fabricated named identity.
        let (r, e) =
            parse_cpp("TEST(Foo, Bar) {\n    void leaked() {\n        return;\n    }\n}\n");
        for line in 1..=4 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn cpp_malformed_identity_does_not_leak_nested_owner() {
        // P1-2: an unidentifiable declarator (ERROR inside `operator int`)
        // abstains as a barrier; a nested function must not leak.
        let (r, e) = parse_cpp(
            "void Foo::operator int() {\n    void leaked2() {\n        return;\n    }\n}\n",
        );
        for line in 1..=3 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn cpp_duplicate_qualification_does_not_leak_nested_owner() {
        // P1-2: the duplicate-qualification abstention is also a barrier; a
        // nested function inside the abstained definition must not leak.
        let (r, e) = parse_cpp(
            "namespace A {\nnamespace B {\nvoid A::C::f() {\n    void leaked3() {\n        return;\n    }\n}\n}\n}\n",
        );
        for line in 1..=7 {
            assert_abstain(&r, &e, line);
        }
    }

    /// A single independent fixture, parsed on its own with 1-based local line
    /// numbers. `owners` lists `(hit_line, name, start_line, end_line)` that
    /// must attribute to the exact owner; `abstain` lists lines that must
    /// abstain and count toward the intentional-abstention floor; `incidental`
    /// lists lines that must abstain but do NOT count.
    struct Case {
        label: &'static str,
        source: &'static str,
        lang: Lang,
        owners: &'static [(u32, &'static str, u32, u32)],
        abstain: &'static [u32],
        incidental: &'static [u32],
    }

    /// US-072 curated C/C++ accuracy gate.
    #[test]
    fn c_cpp_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "C multi-line function",
                source: "int add(int a, int b) {\n    return a + b;\n}\n",
                lang: Lang::C,
                owners: &[(1, "add", 1, 3), (2, "add", 1, 3), (3, "add", 1, 3)],
                abstain: &[4],
                incidental: &[],
            },
            Case {
                label: "C pointer return",
                source: "int *get_ptr(void) {\n    return 0;\n}\n",
                lang: Lang::C,
                owners: &[(1, "get_ptr", 1, 3), (2, "get_ptr", 1, 3)],
                abstain: &[],
                incidental: &[4],
            },
            Case {
                label: "C parenthesized declarator",
                source: "int (f)(int x) {\n    return x;\n}\n",
                lang: Lang::C,
                owners: &[(1, "f", 1, 3), (2, "f", 1, 3)],
                abstain: &[],
                incidental: &[4],
            },
            Case {
                label: "C two functions exact ranges",
                source: "int a(void) {\n    return 1;\n}\nint b(void) {\n    return 2;\n}\n",
                lang: Lang::C,
                owners: &[
                    (1, "a", 1, 3),
                    (2, "a", 1, 3),
                    (3, "a", 1, 3),
                    (4, "b", 4, 6),
                    (5, "b", 4, 6),
                    (6, "b", 4, 6),
                ],
                abstain: &[],
                incidental: &[7],
            },
            Case {
                label: "C conditional branches structural",
                source: "#ifdef FOO\nint a(void) {\n    return 1;\n}\n#else\nint b(void) {\n    return 2;\n}\n#endif\n",
                lang: Lang::C,
                owners: &[(2, "a", 2, 4), (3, "a", 2, 4), (6, "b", 6, 8), (7, "b", 6, 8)],
                abstain: &[1, 5],
                incidental: &[9],
            },
            Case {
                label: "C macro never owner",
                source: "#define MAKE_FN(name) int name(void) { return 1; }\nMAKE_FN(foo);\nint real(void) {\n    return 1;\n}\n",
                lang: Lang::C,
                owners: &[(3, "real", 3, 5), (4, "real", 3, 5)],
                abstain: &[1, 2],
                incidental: &[6],
            },
            Case {
                label: "C K&R abstains",
                source: "sum(a, b)\nint a;\nint b;\n{\n    return a + b;\n}\n",
                lang: Lang::C,
                owners: &[],
                abstain: &[1, 2, 3, 4, 5],
                incidental: &[6],
            },
            Case {
                label: "C struct does not qualify",
                source: "struct S {\n    int x;\n};\nvoid f(void) {\n    return;\n}\n",
                lang: Lang::C,
                owners: &[(4, "f", 4, 6)],
                abstain: &[1, 2, 3],
                incidental: &[7],
            },
            Case {
                label: "C++ namespace free function",
                source: "namespace ns {\nvoid run(void) {\n    return;\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[(2, "ns::run", 2, 4), (3, "ns::run", 2, 4), (4, "ns::run", 2, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "C++ nested namespace syntax",
                source: "namespace a::b {\nvoid run(void) {\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[(2, "a::b::run", 2, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "C++ nested namespace nodes",
                source: "namespace x {\nnamespace y {\nvoid go(void) {\n}\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[(3, "x::y::go", 3, 4), (4, "x::y::go", 3, 4)],
                abstain: &[1, 2],
                incidental: &[5, 6],
            },
            Case {
                label: "C++ inline methods",
                source: "class A {\n    void m(void) {\n    }\n};\nstruct S {\n    void n(void) {\n    }\n};\nunion U {\n    void o(void) {\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[
                    (2, "A::m", 2, 3),
                    (3, "A::m", 2, 3),
                    (6, "S::n", 6, 7),
                    (10, "U::o", 10, 11),
                ],
                abstain: &[1, 5, 9],
                incidental: &[4, 8, 12, 13],
            },
            Case {
                label: "C++ nested types",
                source: "class Outer {\n    class Inner {\n        void handle(void) {\n        }\n    };\n};\n",
                lang: Lang::Cpp,
                owners: &[(3, "Outer::Inner::handle", 3, 4), (4, "Outer::Inner::handle", 3, 4)],
                abstain: &[1, 2],
                incidental: &[6, 7],
            },
            Case {
                label: "C++ template free function",
                source: "template<class T> T max_value(T a, T b) {\n    return a;\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "max_value", 1, 3), (2, "max_value", 1, 3)],
                abstain: &[],
                incidental: &[],
            },
            Case {
                label: "C++ template member in namespace",
                source: "namespace n {\ntemplate<class T> T make() {\n    return T();\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[(2, "n::make", 2, 4), (3, "n::make", 2, 4)],
                abstain: &[1],
                incidental: &[5],
            },
            Case {
                label: "C++ out-of-line Foo::bar",
                source: "void Foo::bar() {\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "Foo::bar", 1, 2)],
                abstain: &[],
                incidental: &[3],
            },
            Case {
                label: "C++ out-of-line ns::Foo::bar",
                source: "void ns::Foo::bar() {\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "ns::Foo::bar", 1, 2)],
                abstain: &[],
                incidental: &[3],
            },
            Case {
                label: "C++ lexical namespace + qualified declarator",
                source: "namespace ns2 {\nvoid Foo::bar() {\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[(2, "ns2::Foo::bar", 2, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "C++ out-of-line template member",
                source: "template<class T> void Box<T>::set(T v) {\n    this->v = v;\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "Box<T>::set", 1, 3), (2, "Box<T>::set", 1, 3)],
                abstain: &[],
                incidental: &[4],
            },
            Case {
                label: "C++ ctor/dtor/operator",
                source: "struct Foo {\n    Foo() {\n    }\n    ~Foo() {\n    }\n    Foo& operator=(const Foo& o) {\n        return *this;\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[
                    (2, "Foo::Foo", 2, 3),
                    (3, "Foo::Foo", 2, 3),
                    (4, "Foo::~Foo", 4, 5),
                    (6, "Foo::operator=", 6, 8),
                    (7, "Foo::operator=", 6, 8),
                ],
                abstain: &[1],
                incidental: &[9],
            },
            Case {
                label: "C++ overloads",
                source: "class A {\n    void f(int) {\n    }\n    void f(double) {\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[(2, "A::f", 2, 3), (4, "A::f", 4, 5)],
                abstain: &[1],
                incidental: &[6],
            },
            Case {
                label: "C++ lambda barrier",
                source: "void outer() {\n    auto l = [](int x) {\n        return x;\n    };\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "outer", 1, 5), (5, "outer", 1, 5)],
                abstain: &[2, 3],
                incidental: &[6],
            },
            Case {
                label: "C++ nested lambda barrier",
                source: "void outer() {\n    auto m = []() {\n        return []() {\n            return 1;\n        };\n    };\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "outer", 1, 7), (7, "outer", 1, 7)],
                abstain: &[3, 4],
                incidental: &[2, 5, 6],
            },
            Case {
                label: "C++ anonymous namespace",
                source: "namespace {\nvoid f() {\n}\n}\nvoid g() {\n}\n",
                lang: Lang::Cpp,
                owners: &[(5, "g", 5, 6)],
                abstain: &[1, 2, 3],
                incidental: &[4, 7],
            },
            Case {
                label: "C++ anonymous class",
                source: "class A {\n    class {\n        void leaf() {\n        }\n    };\n    void sibling() {\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[(6, "A::sibling", 6, 7), (7, "A::sibling", 6, 7)],
                abstain: &[1, 2, 3, 4],
                incidental: &[5, 8, 9],
            },
            Case {
                label: "C++ TEST macro abstains",
                source: "TEST(Foo, Bar) {\n    EXPECT_EQ(1, 1);\n}\nvoid real() {\n    return;\n}\n",
                lang: Lang::Cpp,
                owners: &[(4, "real", 4, 6)],
                abstain: &[1, 2],
                incidental: &[3, 7],
            },
            Case {
                label: "C++ friend barrier",
                source: "class A {\n    friend void f() {\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[4],
            },
            Case {
                label: "C++ malformed function degrades clean sibling survives",
                source: "void good() {\n    return;\n}\nvoid broken( {\n    int x = 1;\n}\nvoid also_good() {\n    return;\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "good", 1, 3), (2, "good", 1, 3)],
                abstain: &[5, 6],
                incidental: &[4, 7, 8, 9],
            },
            Case {
                label: "C++ duplicate qualification abstains",
                source: "namespace A {\nnamespace B {\nvoid A::C::f() {\n}\n}\n}\n",
                lang: Lang::Cpp,
                owners: &[],
                abstain: &[1, 2, 3, 4],
                incidental: &[5, 6],
            },
            Case {
                label: "C++ method multi-line body",
                source: "class Service {\n    void Load() {\n        int x = 1;\n        apply(x);\n    }\n}\n",
                lang: Lang::Cpp,
                owners: &[
                    (2, "Service::Load", 2, 5),
                    (3, "Service::Load", 2, 5),
                    (4, "Service::Load", 2, 5),
                    (5, "Service::Load", 2, 5),
                ],
                abstain: &[1],
                incidental: &[6],
            },
            Case {
                label: "C++ out-of-line ctor/dtor",
                source: "Foo::Foo() {\n    init();\n}\nFoo::~Foo() {\n    done();\n}\n",
                lang: Lang::Cpp,
                owners: &[
                    (1, "Foo::Foo", 1, 3),
                    (2, "Foo::Foo", 1, 3),
                    (4, "Foo::~Foo", 4, 6),
                    (5, "Foo::~Foo", 4, 6),
                ],
                abstain: &[],
                incidental: &[7],
            },
            Case {
                label: "C multi-statement body",
                source: "int compute(int a) {\n    int b = a + 1;\n    b *= 2;\n    return b;\n}\n",
                lang: Lang::C,
                owners: &[
                    (1, "compute", 1, 5),
                    (2, "compute", 1, 5),
                    (3, "compute", 1, 5),
                    (4, "compute", 1, 5),
                    (5, "compute", 1, 5),
                ],
                abstain: &[],
                incidental: &[6],
            },
            Case {
                label: "C++ local class member in method",
                source: "class A {\n    void f() {\n        struct L {\n            void g() {\n            }\n        };\n    }\n};\n",
                lang: Lang::Cpp,
                owners: &[
                    (2, "A::f", 2, 7),
                    (3, "A::f", 2, 7),
                    (4, "A::L::g", 4, 5),
                    (5, "A::L::g", 4, 5),
                    (6, "A::f", 2, 7),
                    (7, "A::f", 2, 7),
                ],
                abstain: &[1],
                incidental: &[8],
            },
            Case {
                label: "C++ out-of-line operator=",
                source: "Foo& Foo::operator=(const Foo& o) {\n    return *this;\n}\n",
                lang: Lang::Cpp,
                owners: &[(1, "Foo::operator=", 1, 3), (2, "Foo::operator=", 1, 3)],
                abstain: &[],
                incidental: &[4],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = match case.lang {
                Lang::C => parse_c(case.source),
                Lang::Cpp => parse_cpp(case.source),
                other => panic!("unexpected lang {other:?}"),
            };
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = owner_for(&regions, &errors, hit_line);
                assert!(
                    owner.is_some(),
                    "[{}] line {hit_line}: expected {name}@{start}-{end}, got abstain",
                    case.label
                );
                let owner = owner.unwrap();
                assert_eq!(owner.qualified_name(), name, "[{}] name", case.label);
                assert_eq!(owner.start_line, start, "[{}] start", case.label);
                assert_eq!(owner.end_line, end, "[{}] end", case.label);
                assert_eq!(owner.language, case.lang, "[{}] language", case.label);
            }
            for &line in case.abstain {
                abstentions += 1;
                assert!(
                    owner_for(&regions, &errors, line).is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    owner_for(&regions, &errors, line).is_none(),
                    "[{}] incidental line {line} should abstain",
                    case.label
                );
            }
        }
        assert!(
            positives >= 70,
            "positive floor: only {positives} assertions"
        );
        assert!(
            abstentions >= 30,
            "intentional-abstention floor: only {abstentions} assertions"
        );
    }

    /// US-072 §10.2: pinned fingerprints + explicit disposition manifests. Any
    /// grammar metadata change must fail here with an actionable re-audit
    /// message rather than silently widening claims.
    #[test]
    fn manifest_gate_verifies_kinds_exist_and_fingerprints_are_pinned() {
        for (name, node_types, dispositions, fingerprint) in [
            (
                "tree-sitter-c",
                tree_sitter_c::NODE_TYPES,
                C_CALLABLE_OR_HAZARD_DISPOSITIONS,
                C_NODE_TYPES_FINGERPRINT,
            ),
            (
                "tree-sitter-cpp",
                tree_sitter_cpp::NODE_TYPES,
                CPP_CALLABLE_OR_HAZARD_DISPOSITIONS,
                CPP_NODE_TYPES_FINGERPRINT,
            ),
        ] {
            let v: serde_json::Value = serde_json::from_str(node_types).unwrap();
            let kinds: HashSet<String> = v
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
                .collect();
            assert!(
                !kinds.is_empty(),
                "{name} NODE_TYPES must declare node kinds"
            );

            let mut all: Vec<&str> = Vec::new();
            let container_kinds: &[&str] = if name == "tree-sitter-c" {
                C_CONTAINER_KINDS
            } else {
                CPP_CONTAINER_KINDS
            };
            all.extend(container_kinds);
            let wrapper_kinds: &[&str] = if name == "tree-sitter-c" {
                C_WRAPPER_KINDS
            } else {
                CPP_WRAPPER_KINDS
            };
            all.extend(wrapper_kinds);
            for (kind, _) in dispositions {
                all.push(kind);
            }
            let uniq: HashSet<&str> = all.iter().copied().collect();
            assert_eq!(
                uniq.len(),
                all.len(),
                "{name} manifest inventories must be disjoint"
            );
            for kind in &uniq {
                assert!(
                    kinds.contains(*kind),
                    "{name}: manifest kind {kind} missing from NODE_TYPES"
                );
            }

            // Every grammar callable/hazard relevant to this adapter has an
            // explicit disposition: named, transparent, barrier, or abstain.
            let disposition_of: std::collections::HashMap<&str, &str> =
                dispositions.iter().copied().collect();
            for kind in &kinds {
                let is_relevant = kind.starts_with("preproc_")
                    || matches!(
                        kind.as_str(),
                        "function_definition"
                            | "lambda_expression"
                            | "template_declaration"
                            | "friend_declaration"
                            | "macro_type_specifier"
                    );
                if !is_relevant {
                    continue;
                }
                assert!(
                    disposition_of.contains_key(kind.as_str()),
                    "{name}: grammar callable/hazard {kind} has no explicit disposition; add one (named/transparent/barrier/abstain)"
                );
            }
            for (kind, disposition) in dispositions {
                assert!(
                    matches!(
                        *disposition,
                        "named" | "transparent" | "barrier" | "abstain"
                    ),
                    "{name}: invalid disposition {disposition} for {kind}"
                );
            }

            assert_eq!(
                fnv1a(node_types.as_bytes()),
                fingerprint,
                "{name} NODE_TYPES changed; re-audit the manifest and recompute the fingerprint"
            );
        }
    }
}
