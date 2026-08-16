//! Ruby owner-region extraction (US-072 Wave 2).
//!
//! Supported named owners: grammar `method` and `singleton_method` definition
//! nodes with a complete parser-backed identity, covering ordinary multi-line,
//! empty (`def m; end`), endless, predicate/bang, setter/index, and operator
//! names. Display follows Ruby source-language convention: instance methods
//! render as `A::B#run`, singleton methods as `A::B.run` (or `self.run` with no
//! lexical class/module), and receiver-specific singleton methods render the
//! receiver authoritatively (`obj.m`, `@target.go`) without any lexical class
//! prefix. `OwnerAnchor.name` stays the flat terminal method name for shared
//! internals; the qualified identity lives in `display_name`.
//!
//! Containers (`class`/`module`) qualify descendants but are never owner
//! regions; entering a named container resets an inherited `class << self`
//! singleton mode. `singleton_class` is never a named owner: `class << self`
//! establishes exactly one lexical singleton level (`Path.m`), while non-`self`
//! receivers, higher-order eigenclasses (`def self.m` or another `class <<
//! self` inside an active `class << self`), and error-contaminated values are
//! full-range barriers.
//!
//! Ordinary `block`/`do_block`/`lambda` closures are transparent (unlike the
//! C++ lambda barrier policy): a hit inside a block inherits the containing
//! method. Dynamic metaprogramming constructs — `define_method`,
//! `define_singleton_method`, the eval/exec families, keyword `alias`,
//! `alias_method`, and the `attr_*` accessors — are exact grammar-backed
//! hazards: the complete call/alias expression is an `AnonymousBarrier` before
//! descent so an attached block or a parser-recovered nested `def` can never
//! leak a fabricated owner.
//!
//! Barrier-before-descend is a release invariant (Wave 1 P1 lesson): every
//! identity-fail branch — malformed method name, malformed class/module path,
//! unsupported singleton receiver, non-`self`/higher-order singleton class,
//! metaprogramming hazard, and any error-degraded candidate — emits a full-range
//! `AnonymousBarrier` and descends in anonymous state, where a nested
//! `method`/`singleton_method` can never regain a name. Local `ERROR`/missing
//! degradation flows through the shared primitives.
//!
//! This module is syntax-only. A Ruby owner means the pinned grammar places the
//! hit in one complete lexical method definition; it never proves runtime
//! receiver identity, method-table mutation, dispatch, mixin ancestry,
//! reopening equivalence, or call binding.

use std::path::Path;

use tree_sitter::Node;

use crate::evidence::owner_links::OwnerAnchor;
use crate::evidence::owners::{
    any_error_overlaps_bytes, attribute_line, collect_error_ranges, degrade_named_on_error,
    ErrorRange, OwnerAttribution, OwnerRegion,
};
use crate::lang::outline::outline_language;
use crate::types::Lang;

/// Explicit disposition of every Ruby definition/container/closure/hazard kind
/// relevant to ownership (anti-drift contract, US-072 Wave 2 §Grammar): named,
/// container, transparent, barrier, or abstain. Every entry must exist in the
/// pinned grammar's `NODE_TYPES`. `call` is transparent for ordinary calls and
/// a barrier when its audited `method` terminal is one of
/// [`RUBY_METAPROGRAMMING_HAZARD_METHODS`]; keyword `alias` is always a barrier.
#[cfg(test)]
const RUBY_DISPOSITIONS: &[(&str, &str)] = &[
    ("method", "named"),
    ("singleton_method", "named"),
    ("class", "container"),
    ("module", "container"),
    ("singleton_class", "container"),
    ("body_statement", "transparent"),
    ("block", "transparent"),
    ("do_block", "transparent"),
    ("lambda", "transparent"),
    ("block_body", "transparent"),
    ("alias", "barrier"),
    ("call", "transparent"),
];

/// Complete name/receiver component kinds the adapter reads as identity
/// evidence. `self` is the structural singleton receiver; `scope_resolution`
/// yields a complete `A::B` path; `_method_name` is the grammar supertype of
/// the `name` field. All must exist in `NODE_TYPES`.
#[cfg(test)]
const RUBY_NAME_OR_RECEIVER_KINDS: &[&str] = &[
    "identifier",
    "constant",
    "setter",
    "operator",
    "simple_symbol",
    "delimited_symbol",
    "instance_variable",
    "class_variable",
    "global_variable",
    "self",
    "scope_resolution",
    "_method_name",
    "_variable",
    "_primary",
    "_arg",
];

/// Audited dynamic-metaprogramming call hazards (12 call-style names; keyword
/// `alias` is handled by node kind). Recognition is intentionally narrow: only
/// an exact `call` whose `method` field terminal is one of these names becomes
/// a barrier — never arbitrary text or comments containing the words.
const RUBY_METAPROGRAMMING_HAZARD_METHODS: &[&str] = &[
    "define_method",
    "define_singleton_method",
    "class_eval",
    "module_eval",
    "instance_eval",
    "class_exec",
    "module_exec",
    "instance_exec",
    "alias_method",
    "attr_accessor",
    "attr_reader",
    "attr_writer",
];

/// FNV-1a-64 fingerprint of the bundled `tree-sitter-ruby` `NODE_TYPES` JSON
/// (tree-sitter-ruby 0.23.1). Pinned so any grammar metadata change fails the
/// manifest gate and forces a re-audit. Computed from the exact bundled crate.
#[cfg(test)]
const RUBY_NODE_TYPES_FINGERPRINT: u64 = 0x3bdf_a906_ceee_d4a1;

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

/// Lexical walk state. `containers` is the class/module path (joined with
/// `::`); `singleton_mode` is active exactly inside a `class << self` body;
/// `in_anonymous` is set while descending under an `AnonymousBarrier` so
/// nested definitions can never regain a fabricated name.
#[derive(Clone)]
struct WalkState {
    containers: Vec<String>,
    singleton_mode: bool,
    in_anonymous: bool,
}

/// Parse a Ruby file and produce its owner regions and local error ranges.
/// Returns `None` when parser setup/parsing fails or the root itself is
/// `ERROR` (preserve raw hits, emit no owner evidence).
pub(crate) fn ruby_regions(
    path: &Path,
    content: &str,
) -> Option<(Vec<OwnerRegion>, Vec<ErrorRange>)> {
    let language = outline_language(Lang::Ruby)?;
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
    let mut state = WalkState {
        containers: Vec::new(),
        singleton_mode: false,
        in_anonymous: false,
    };
    walk(
        tree.root_node(),
        bytes,
        path,
        &mut state,
        &errors,
        &mut regions,
    );
    Some((regions, errors))
}

/// Attribute a Ruby hit line to a named owner, honoring local errors.
pub(crate) fn ruby_owner_for<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> OwnerAttribution<'a> {
    attribute_line(regions, errors, line)
}

/// Emit a full-range `AnonymousBarrier` for `node` and descend in anonymous
/// state: every identity-fail branch must barrier before descent (US-072 Wave 2
/// §ERROR, release invariant).
fn barrier_and_descend(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    regions.push(OwnerRegion::AnonymousBarrier {
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
    });
    let saved_anonymous = state.in_anonymous;
    state.in_anonymous = true;
    walk_children(node, bytes, path, state, errors, regions);
    state.in_anonymous = saved_anonymous;
}

fn walk_children(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, bytes, path, state, errors, regions);
    }
}

/// Walk a Ruby tree, emitting `Named` regions for complete identifiable
/// `method`/`singleton_method` definitions, `AnonymousBarrier` regions for
/// every abstaining definition, malformed container, metaprogramming hazard,
/// and higher-order eigenclass, and descending transparently through ordinary
/// closures (`block`, `do_block`, `lambda`, `body_statement`).
fn walk(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    match node.kind() {
        "class" | "module" => {
            handle_class_module(node, bytes, path, state, errors, regions);
        }
        "singleton_class" => {
            handle_singleton_class(node, bytes, path, state, errors, regions);
        }
        "method" => {
            handle_method(node, bytes, path, state, errors, regions);
        }
        "singleton_method" => {
            handle_singleton_method(node, bytes, path, state, errors, regions);
        }
        // Keyword `alias new old` is a dynamic metaprogramming hazard: the
        // complete expression is a barrier before descent.
        "alias" => {
            barrier_and_descend(node, bytes, path, state, errors, regions);
        }
        // `call` is transparent for ordinary calls and a barrier when its
        // audited method terminal is one of the dynamic-definition hazards.
        "call" if is_hazard_call(node, bytes) => {
            barrier_and_descend(node, bytes, path, state, errors, regions);
        }
        // Blocks, procs, lambdas, body wrappers, and everything else descend
        // transparently; only complete definition nodes become owners.
        _ => {
            walk_children(node, bytes, path, state, errors, regions);
        }
    }
}

/// Handle a `class`/`module` container. A plain constant name is relative to
/// the lexical stack (`module A; class B` -> `A::B`). A complete
/// `scope_resolution` name is authoritative (Wave 1 §5.2 precedent): it is
/// merged with the lexical stack only when the qualified path's prefix matches
/// the stack's suffix without duplication (`module A` + `class A::B` ->
/// `A::B`); an unprovable combination (`module C` + `class A::B`) establishes a
/// full-range barrier before descent — never a guessed `C::A::B`. A leading
/// `::` root-qualified name (`class ::Rooted`) is explicitly absolute and
/// replaces the lexical stack (`Rooted`, never `M::Rooted`). Entering a named
/// container resets any inherited `class << self` singleton mode.
fn handle_class_module(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let Some(identity) = container_identity(node, bytes) else {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    };
    let saved_containers = state.containers.clone();
    let saved_singleton = state.singleton_mode;
    match identity {
        ContainerIdentity::Relative(components) => state.containers.extend(components),
        ContainerIdentity::RootQualified(components) => state.containers = components,
        ContainerIdentity::Qualified(components) => {
            let Some(merged) = merge_qualified(&saved_containers, &components) else {
                state.containers = saved_containers;
                state.singleton_mode = saved_singleton;
                barrier_and_descend(node, bytes, path, state, errors, regions);
                return;
            };
            state.containers = merged;
        }
    }
    state.singleton_mode = false;
    walk_children(node, bytes, path, state, errors, regions);
    state.containers = saved_containers;
    state.singleton_mode = saved_singleton;
}

/// Handle a `singleton_class`. `class << self` establishes one lexical
/// singleton level; everything else — non-`self` value, unreadable/errored
/// value, a nested `class << self` while singleton mode is already active
/// (higher-order eigenclass), or any occurrence inside an anonymous barrier —
/// is a full-range barrier before descent. Never emits a named owner.
fn handle_singleton_class(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    if state.in_anonymous {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    let Some(value) = node.child_by_field_name("value") else {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    };
    if value.is_error()
        || value.is_missing()
        || value.kind() != "self"
        || any_error_overlaps_bytes(errors, value.start_byte(), value.end_byte())
    {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    if state.singleton_mode {
        // A second `class << self` targets a higher-order eigenclass that this
        // lexical display cannot represent; fail closed rather than flattening
        // its methods to the wrong `Path.m`.
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    state.singleton_mode = true;
    walk_children(node, bytes, path, state, errors, regions);
    state.singleton_mode = false;
}

/// Handle an ordinary `method` definition. Emits `Named` only when the name
/// resolves to exactly one readable `_method_name` spelling; a body is not
/// required (an empty `def m; end` is a complete body-bearing definition node).
/// Inside an active `class << self` the display becomes `Path.m`; otherwise
/// `Path#m`, or the bare name at top level.
fn handle_method(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    if state.in_anonymous {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    let Some(name) = method_name(node, bytes) else {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    };
    let display = if state.singleton_mode {
        let prefix = singleton_prefix(state);
        format!("{prefix}.{name}")
    } else if state.containers.is_empty() {
        name.clone()
    } else {
        format!("{}#{name}", state.containers.join("::"))
    };
    emit_anchor(node, bytes, path, state, &name, &display, errors, regions);
}

/// Handle a `singleton_method` definition. `def self.m` renders `Path.m` (or
/// `self.m` outside a container). `def obj.m` with a supported complete
/// receiver (`identifier`, `constant`, instance/class/global variable, or a
/// complete `scope_resolution`) renders the receiver authoritatively with no
/// lexical class prefix. Any unsupported/errored receiver abstains as a
/// barrier. Inside an active `class << self` every `singleton_method` targets a
/// higher-order eigenclass and is a barrier before descent.
fn handle_singleton_method(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    if state.in_anonymous {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    if state.singleton_mode {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    let Some(name) = method_name(node, bytes) else {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    };
    let Some(object) = node.child_by_field_name("object") else {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    };
    if object.is_error() || object.is_missing() {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    // Parenthesized receivers (`def (x).m`) are explicitly unsupported: the
    // grammar unwraps the parens into a bare object, but the parens prove the
    // receiver was an expression, so the whole definition must abstain.
    if has_direct_paren_tokens(node) {
        barrier_and_descend(node, bytes, path, state, errors, regions);
        return;
    }
    let display = if object.kind() == "self" {
        let prefix = singleton_prefix(state);
        format!("{prefix}.{name}")
    } else {
        let Some(receiver) = receiver_text(object, bytes) else {
            barrier_and_descend(node, bytes, path, state, errors, regions);
            return;
        };
        format!("{receiver}.{name}")
    };
    emit_anchor(node, bytes, path, state, &name, &display, errors, regions);
}

/// `self` when no lexical class/module exists, else the `::`-joined path.
fn singleton_prefix(state: &WalkState) -> String {
    if state.containers.is_empty() {
        "self".to_string()
    } else {
        state.containers.join("::")
    }
}

/// Build the `Named` anchor for a complete definition, pass it through the
/// shared error degradation, and descend. When degradation converts the
/// candidate to a barrier, descend in anonymous state (do not continue with
/// the prior transparent state), per the Wave 2 release invariant.
fn emit_anchor(
    node: Node<'_>,
    bytes: &[u8],
    path: &Path,
    state: &mut WalkState,
    name: &str,
    display: &str,
    errors: &[ErrorRange],
    regions: &mut Vec<OwnerRegion>,
) {
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    let anchor = OwnerAnchor {
        path: path.to_path_buf(),
        name: name.to_string(),
        receiver_var: None,
        receiver_type: None,
        package_dir: Path::new(".").to_path_buf(),
        start_line,
        end_line,
        language: Lang::Ruby,
        display_name: display.to_string(),
    };
    let region = OwnerRegion::Named(anchor);
    let region = degrade_named_on_error(region, errors, node.start_byte(), node.end_byte());
    let degraded = matches!(region, OwnerRegion::AnonymousBarrier { .. });
    regions.push(region);
    let saved_anonymous = state.in_anonymous;
    if degraded {
        state.in_anonymous = true;
    }
    walk_children(node, bytes, path, state, errors, regions);
    state.in_anonymous = saved_anonymous;
}

/// Whether a `call` node's audited `method` terminal is a dynamic
/// metaprogramming hazard. Receiver-agnostic by design: `Foo.class_eval { }`
/// and bare `class_eval { }` are both barriers because the expression's
/// method-table effect is dynamic.
fn is_hazard_call(node: Node<'_>, bytes: &[u8]) -> bool {
    let Some(method) = node.child_by_field_name("method") else {
        return false;
    };
    if method.is_error() || method.is_missing() {
        return false;
    }
    let Ok(text) = method.utf8_text(bytes) else {
        return false;
    };
    RUBY_METAPROGRAMMING_HAZARD_METHODS.contains(&text.trim())
}

/// How a complete container name relates to the lexical container stack.
/// `Relative` (plain constant) appends to the stack; `Qualified`
/// (`scope_resolution` without a leading root marker) is authoritative and
/// merges only when structurally compatible; `RootQualified` (leading `::`) is
/// explicitly absolute and replaces the stack.
enum ContainerIdentity {
    Relative(Vec<String>),
    Qualified(Vec<String>),
    RootQualified(Vec<String>),
}

/// Extract a complete `class`/`module` container identity: one `constant` or
/// one complete recursive `scope_resolution`, normalized with Ruby `::` (a
/// leading `::` root marker is tracked, not part of the display). Superclass
/// text is never part of the identity. Returns `None` for a
/// missing/unsupported/error-contaminated name (caller fails closed).
fn container_identity(node: Node<'_>, bytes: &[u8]) -> Option<ContainerIdentity> {
    let name = node.child_by_field_name("name")?;
    if name.is_error() || name.is_missing() {
        return None;
    }
    match name.kind() {
        "constant" => Some(ContainerIdentity::Relative(vec![readable_text(
            name, bytes,
        )?])),
        "scope_resolution" => {
            let (components, rooted) = scope_resolution_components(name, bytes)?;
            Some(if rooted {
                ContainerIdentity::RootQualified(components)
            } else {
                ContainerIdentity::Qualified(components)
            })
        }
        _ => None,
    }
}

/// Merge an authoritative qualified container path with the lexical stack
/// without duplication (Wave 1 §5.2): when the qualified path's prefix matches
/// the stack's suffix, the qualified path replaces that suffix
/// (`module A` + `class A::B` -> `[A, B]`). An empty stack is absolute. Any
/// other combination is unprovable and returns `None` so the caller barriers
/// the container instead of guessing (e.g. never `C::A::B`).
fn merge_qualified(lexical: &[String], qualified: &[String]) -> Option<Vec<String>> {
    if lexical.is_empty() {
        return Some(qualified.to_vec());
    }
    let max_k = qualified.len().min(lexical.len());
    for k in (1..=max_k).rev() {
        if qualified[..k] == lexical[lexical.len() - k..] {
            let mut merged = lexical[..lexical.len() - k].to_vec();
            merged.extend_from_slice(qualified);
            return Some(merged);
        }
    }
    None
}

/// Recursively extract a `scope_resolution` path in left-to-right order
/// (`A::B::C` -> `[A, B, C]`), plus whether the leftmost component is
/// root-anchored (`::A::B` -> `([A, B], true)`; the grammar makes the `scope`
/// field optional for a leading `::`). Rejects any `ERROR`/missing node
/// anywhere in the path (Wave 1 P1 lesson: an unfield-assigned ERROR must not
/// slip past the field-name checks) and any unsupported scope component.
fn scope_resolution_components(node: Node<'_>, bytes: &[u8]) -> Option<(Vec<String>, bool)> {
    if node.is_error() || node.is_missing() {
        return None;
    }
    let mut direct = node.walk();
    if node
        .named_children(&mut direct)
        .any(|c| c.is_error() || c.is_missing())
    {
        return None;
    }
    let mut components = Vec::new();
    let mut rooted = false;
    if let Some(scope) = node.child_by_field_name("scope") {
        match scope.kind() {
            "constant" => components.push(readable_text(scope, bytes)?),
            "scope_resolution" => {
                let (inner, inner_rooted) = scope_resolution_components(scope, bytes)?;
                components.extend(inner);
                rooted = inner_rooted;
            }
            _ => return None,
        }
    } else {
        // No scope field: a leading `::` root marker (`::Rooted`).
        rooted = true;
    }
    let name = node.child_by_field_name("name")?;
    match name.kind() {
        "constant" => components.push(readable_text(name, bytes)?),
        _ => return None,
    }
    if components.is_empty() {
        return None;
    }
    Some((components, rooted))
}

/// The method `name` field, resolved to exactly one readable `_method_name`
/// spelling (`identifier`, `constant`, `setter`, `operator`, `simple_symbol`,
/// or `delimited_symbol`). Never scans arbitrary source tokens or takes the
/// first identifier.
fn method_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    if name.is_error() || name.is_missing() {
        return None;
    }
    let mut cursor = name.walk();
    if name
        .named_children(&mut cursor)
        .any(|c| c.is_error() || c.is_missing())
    {
        return None;
    }
    match name.kind() {
        "identifier" | "constant" | "setter" | "operator" | "simple_symbol"
        | "delimited_symbol" => readable_text(name, bytes),
        _ => None,
    }
}

/// Whether a definition node has a `(` or `)` token among its direct
/// children. The grammar unwraps parenthesized receivers into a bare object
/// (`def (x).m` -> object `x`), so this detects the parenthesization that a
/// receiver-kind check alone would miss. Parameter parens live inside the
/// `method_parameters` field node, so ordinary `def obj.m(v)` is unaffected.
fn has_direct_paren_tokens(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let kids: Vec<Node<'_>> = node.children(&mut cursor).collect();
    kids.iter().any(|c| matches!(c.kind(), "(" | ")"))
}

/// A complete supported singleton receiver: `identifier`, `constant`,
/// instance/class/global variable, or a complete `scope_resolution` path.
/// `self` is handled by the caller. Anything else (call, index expression,
/// arithmetic/parenthesized expression, interpolation, chained invocation,
/// missing node, `ERROR`) returns `None` so the whole `singleton_method`
/// becomes a barrier — no fallback to a bare name and no fabricated lexical
/// `Class.m`.
fn receiver_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.is_error() || node.is_missing() {
        return None;
    }
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|c| c.is_error() || c.is_missing())
    {
        return None;
    }
    match node.kind() {
        "identifier" | "constant" | "instance_variable" | "class_variable" | "global_variable" => {
            readable_text(node, bytes)
        }
        "scope_resolution" => {
            scope_resolution_components(node, bytes).map(|(components, _)| components.join("::"))
        }
        _ => None,
    }
}

/// A readable identity spelling: trimmed, non-empty, with no whitespace,
/// newline, or structural/punctuation characters that could make the spelling
/// ambiguous or non-round-trippable.
fn readable_text(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?.trim().to_string();
    if text.is_empty()
        || text.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | ')' | '{' | '}' | ';' | ',' | ':' | '"' | '\'' | '#' | '.'
                )
        })
    {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn ruby_path() -> PathBuf {
        PathBuf::from("tests/fixtures/x.rb")
    }

    fn parse_ruby(src: &str) -> (Vec<OwnerRegion>, Vec<ErrorRange>) {
        ruby_regions(&ruby_path(), src).expect("Ruby fixture should parse")
    }

    fn assert_owner(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32, name: &str) {
        let owner = ruby_owner_for(regions, errors, line).named();
        assert_eq!(
            owner.map(OwnerAnchor::qualified_name),
            Some(name.to_string()),
            "line {line} should attribute to {name}"
        );
        if let Some(owner) = owner {
            assert_eq!(owner.language, Lang::Ruby, "line {line} language");
        }
    }

    fn assert_abstain(regions: &[OwnerRegion], errors: &[ErrorRange], line: u32) {
        assert!(
            ruby_owner_for(regions, errors, line).named().is_none(),
            "line {line} should abstain"
        );
    }

    // ---- Method forms and exact ranges ----

    #[test]
    fn ruby_top_level_method_forms_exact_full_ranges() {
        let (r, e) = parse_ruby(
            "def run\n  work\nend\ndef empty; end\ndef endless = 1\ndef pred?; end\ndef bang!; end\ndef m=(v); end\ndef [](i); end\n",
        );
        assert_owner(&r, &e, 1, "run");
        assert_owner(&r, &e, 2, "run");
        assert_owner(&r, &e, 3, "run");
        let owner = ruby_owner_for(&r, &e, 1).named().unwrap();
        assert_eq!((owner.start_line, owner.end_line), (1, 3));
        assert_eq!(owner.name, "run");
        // Empty, endless, predicate, bang, setter, index forms.
        assert_owner(&r, &e, 4, "empty");
        assert_owner(&r, &e, 5, "endless");
        assert_owner(&r, &e, 6, "pred?");
        assert_owner(&r, &e, 7, "bang!");
        assert_owner(&r, &e, 8, "m=");
        assert_owner(&r, &e, 9, "[]");
    }

    #[test]
    fn ruby_operator_methods() {
        let (r, e) = parse_ruby("def +(o)\n  o\nend\ndef [](k, v)\nend\ndef <=>(o); end\n");
        assert_owner(&r, &e, 1, "+");
        assert_owner(&r, &e, 2, "+");
        assert_owner(&r, &e, 4, "[]");
        assert_owner(&r, &e, 6, "<=>");
    }

    #[test]
    fn ruby_multiline_empty_endless_methods_keep_owner_inside() {
        // A hit on any body line attributes to the full method range.
        let (r, e) = parse_ruby("def outer\n  if x\n    hit_here\n  end\nend\n");
        for line in 1..=5 {
            assert_owner(&r, &e, line, "outer");
        }
        let owner = ruby_owner_for(&r, &e, 1).named().unwrap();
        assert_eq!((owner.start_line, owner.end_line), (1, 5));
    }

    // ---- Containers and qualification ----

    #[test]
    fn ruby_nested_module_class_qualification() {
        let (r, e) = parse_ruby(
            "module A\n  class B\n    def run\n    end\n  end\nend\nmodule X\n  module Y\n    def go; end\n  end\nend\n",
        );
        assert_owner(&r, &e, 3, "A::B#run");
        assert_owner(&r, &e, 4, "A::B#run");
        assert_owner(&r, &e, 9, "X::Y#go");
    }

    #[test]
    fn ruby_complete_scope_resolution_container_paths() {
        let (r, e) =
            parse_ruby("class A::B\n  def run; end\nend\nmodule X::Y\n  def go; end\nend\n");
        assert_owner(&r, &e, 2, "A::B#run");
        assert_owner(&r, &e, 5, "X::Y#go");
    }

    #[test]
    fn ruby_qualified_container_merges_matching_lexical_suffix_without_duplication() {
        // Wave 2 P1: `module A; class A::B` defines A::B, not A::A::B. The
        // qualified path is authoritative; the matching lexical suffix is
        // consumed, never duplicated.
        let (r, e) = parse_ruby(
            "module A\n  class A::B\n    def hit_method\n      duplicate_container_hit\n    end\n  end\nend\n",
        );
        assert_owner(&r, &e, 3, "A::B#hit_method");
        assert_owner(&r, &e, 4, "A::B#hit_method");
        assert_owner(&r, &e, 5, "A::B#hit_method");
        let owner = ruby_owner_for(&r, &e, 4).named().unwrap();
        assert_eq!((owner.start_line, owner.end_line), (3, 5));
        // Deeper stack: the qualified prefix matches the stack suffix, so the
        // outer lexical component is kept exactly once.
        let (r, e) = parse_ruby(
            "module X\n  module A\n    class A::B\n      def m; end\n    end\n  end\nend\n",
        );
        assert_owner(&r, &e, 4, "X::A::B#m");
        // Longest match wins: the whole stack is consumed, not just one level.
        let (r, e) = parse_ruby(
            "module A\n  class B\n    class A::B::C\n      def m; end\n    end\n  end\nend\n",
        );
        assert_owner(&r, &e, 4, "A::B::C#m");
    }

    #[test]
    fn ruby_unprovable_qualified_container_barriers_before_descent() {
        // `module C; class A::B` — the grammar cannot prove where `A` resolves
        // (Module.nesting could find C::A or ::A, decided in another file), so
        // the container is a full-range barrier. Never a guessed C::A::B.
        let (r, e) =
            parse_ruby("module C\n  class A::B\n    def m\n      hit\n    end\n  end\nend\n");
        for line in 2..=6 {
            assert_abstain(&r, &e, line);
        }
        // The barrier is scoped: a sibling container after it still attributes.
        let (r, e) = parse_ruby(
            "module C\n  class A::B\n    def hidden; end\n  end\n  class Ok\n    def shown; end\n  end\nend\n",
        );
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 6, "C::Ok#shown");
        // Deeper stack with no witness anywhere in it: still a barrier, and a
        // nested def inside the barriered container cannot leak a shortened
        // owner either.
        let (r, e) = parse_ruby(
            "module X\n  module Y\n    class A::B\n      def m\n        def inner; end\n      end\n    end\n  end\nend\n",
        );
        for line in 3..=7 {
            assert_abstain(&r, &e, line);
        }
        // Sibling scoping holds at depth too.
        let (r, e) = parse_ruby(
            "module X\n  module Y\n    class A::B\n      def hidden; end\n    end\n    class Ok\n      def shown; end\n    end\n  end\nend\n",
        );
        assert_abstain(&r, &e, 4);
        assert_owner(&r, &e, 7, "X::Y::Ok#shown");
    }

    #[test]
    fn ruby_root_qualified_container_is_absolute() {
        // Wave 2 P2: the grammar makes `scope` optional for a leading `::`.
        // A root-qualified name is complete and absolute.
        let (r, e) =
            parse_ruby("class ::Rooted\n  def rooted_method\n    rooted_hit\n  end\nend\n");
        assert_owner(&r, &e, 2, "Rooted#rooted_method");
        assert_owner(&r, &e, 3, "Rooted#rooted_method");
        // Absolute ignores the lexical stack: never M::Rooted.
        let (r, e) =
            parse_ruby("module M\n  class ::Rooted\n    def nested_rooted; end\n  end\nend\n");
        assert_owner(&r, &e, 3, "Rooted#nested_rooted");
        // Multi-component root path, inside and outside a module.
        let (r, e) = parse_ruby("class ::A::B\n  def deep; end\nend\nmodule M\n  module ::X::Y\n    def go; end\n  end\nend\n");
        assert_owner(&r, &e, 2, "A::B#deep");
        assert_owner(&r, &e, 6, "X::Y#go");
        // The lexical stack is restored after an absolute container closes.
        let (r, e) = parse_ruby(
            "module M\n  class ::Rooted\n    def rooted; end\n  end\n  class Sibling\n    def sib; end\n  end\nend\n",
        );
        assert_owner(&r, &e, 3, "Rooted#rooted");
        assert_owner(&r, &e, 6, "M::Sibling#sib");
    }

    #[test]
    fn ruby_merge_qualified_combination_table() {
        fn owned(parts: &[&str]) -> Vec<String> {
            parts.iter().copied().map(String::from).collect()
        }
        /// (lexical stack, qualified path, merged result or `None` = barrier).
        type MergeCase = (
            &'static [&'static str],
            &'static [&'static str],
            Option<&'static [&'static str]>,
        );
        let cases: &[MergeCase] = &[
            // Empty stack: absolute.
            (&[], &["A", "B"], Some(&["A", "B"])),
            // Suffix match consumes the duplicate exactly once.
            (&["A"], &["A", "B"], Some(&["A", "B"])),
            (&["X", "A"], &["A", "B"], Some(&["X", "A", "B"])),
            (&["A", "B"], &["A", "B", "C"], Some(&["A", "B", "C"])),
            // Longest match wins over a shorter accidental one.
            (&["A", "A"], &["A", "A", "B"], Some(&["A", "A", "B"])),
            // Mid-stack witness: the qualified prefix matches the stack's
            // suffix, so the merge is syntactically supported. Verified against
            // the Ruby runtime: `module A; class B; class B::C` -> A::B::C.
            (&["A", "B"], &["B", "C"], Some(&["A", "B", "C"])),
            // No witness anywhere: caller barriers instead of guessing.
            (&["C"], &["A", "B"], None),
            (&["A"], &["B", "A"], None),
            (&["X", "Y"], &["A", "B"], None),
        ];
        for (lexical, qualified, expected) in cases {
            let got = merge_qualified(&owned(lexical), &owned(qualified));
            assert_eq!(
                got,
                expected.map(owned),
                "lexical {lexical:?} + qualified {qualified:?}"
            );
        }
    }

    #[test]
    fn ruby_instance_vs_singleton_display() {
        let (r, e) = parse_ruby(
            "module A\n  class B\n    def run; end\n    def self.find; end\n  end\nend\ndef self.top; end\n",
        );
        assert_owner(&r, &e, 3, "A::B#run");
        assert_owner(&r, &e, 4, "A::B.find");
        assert_owner(&r, &e, 7, "self.top");
    }

    // ---- Receiver-specific singleton methods ----

    #[test]
    fn ruby_supported_receiver_singleton_methods() {
        let (r, e) = parse_ruby(
            "def obj.m; end\ndef @target.go; end\ndef @@shared.hit; end\ndef $global.kick; end\ndef CONST.poke; end\n",
        );
        assert_owner(&r, &e, 1, "obj.m");
        assert_owner(&r, &e, 2, "@target.go");
        assert_owner(&r, &e, 3, "@@shared.hit");
        assert_owner(&r, &e, 4, "$global.kick");
        assert_owner(&r, &e, 5, "CONST.poke");
    }

    #[test]
    fn ruby_receiver_is_authoritative_no_lexical_prefix() {
        // The receiver object defines the singleton method, not the lexical
        // class: `obj.m` inside `class A` stays `obj.m`, never `A.obj.m`.
        let (r, e) = parse_ruby("class A\n  def obj.m; end\nend\n");
        assert_owner(&r, &e, 2, "obj.m");
    }

    #[test]
    fn ruby_complex_or_errored_receiver_abstains() {
        // Chained invocation, arithmetic, parenthesized/expression receivers,
        // and the grammar-unsupported `def A::B.m` (ERROR inside the node) all
        // abstain as barriers — never a bare `m` and never a fabricated
        // `A.m`/`A::B.m`.
        let (r, e) =
            parse_ruby("def a.b.c; end\ndef (x).m; end\ndef 1+2.m; end\ndef A::B.static; end\n");
        for line in 1..=4 {
            assert_abstain(&r, &e, line);
        }
    }

    // ---- class << self ----

    #[test]
    fn ruby_class_self_singleton_mode() {
        let (r, e) = parse_ruby(
            "class A\n  class << self\n    def build\n    end\n    def find; end\n  end\nend\nclass << self\n  def top_self; end\nend\n",
        );
        assert_owner(&r, &e, 3, "A.build");
        assert_owner(&r, &e, 4, "A.build");
        assert_owner(&r, &e, 5, "A.find");
        assert_owner(&r, &e, 9, "self.top_self");
    }

    #[test]
    fn ruby_class_self_nested_body_wrappers_transparent() {
        // Body wrappers inside `class << self` stay transparent; methods
        // defined under an ordinary block still render `Path.m`.
        let (r, e) = parse_ruby(
            "class A\n  class << self\n    if cond\n      def guarded; end\n    end\n  end\nend\n",
        );
        assert_owner(&r, &e, 4, "A.guarded");
    }

    #[test]
    fn ruby_named_container_resets_singleton_mode() {
        // A named class inside `class << self` resets inherited singleton
        // mode: its direct method is an instance method of the new container.
        let (r, e) = parse_ruby(
            "class << self\n  class Inner\n    def m; end\n  end\n  def outer_m; end\nend\n",
        );
        assert_owner(&r, &e, 3, "Inner#m");
        assert_owner(&r, &e, 5, "self.outer_m");
    }

    #[test]
    fn ruby_higher_order_eigenclass_barriers() {
        // `def self.m` and a nested `class << self` inside an active
        // `class << self` target a higher-order eigenclass: full barriers,
        // and nested definitions never leak.
        let (r, e) = parse_ruby(
            "class A\n  class << self\n    def self.ho; end\n    class << self\n      def deep; end\n    end\n  end\nend\n",
        );
        for line in 1..=7 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_non_self_singleton_class_barrier() {
        let (r, e) = parse_ruby("class A\n  class << obj\n    def om; end\n  end\nend\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
    }

    // ---- Transparent closures ----

    #[test]
    fn ruby_ordinary_closures_inherit_enclosing_method() {
        let (r, e) = parse_ruby(
            "def outer\n  [1, 2].each do |i|\n    puts i\n  end\n  x = ->(a) { a }\n  proc { hit }\n  items.map { |it| it }\nend\n",
        );
        for line in 1..=8 {
            assert_owner(&r, &e, line, "outer");
        }
    }

    #[test]
    fn ruby_top_level_closures_are_ownerless() {
        let (r, e) = parse_ruby("[1].each { |i| hit(i) }\ndo_it do\n  x\nend\n->(a) { a }\n");
        for line in 1..=5 {
            assert_abstain(&r, &e, line);
        }
    }

    // ---- Nested definitions ----

    #[test]
    fn ruby_nested_def_is_independent_owner() {
        // `def inner` inside `def outer` is an independent definition site
        // qualified only by the class/module context, never `outer#inner`.
        let (r, e) =
            parse_ruby("class A\n  def outer\n    def inner\n    end\n    hit_body\n  end\nend\n");
        assert_owner(&r, &e, 2, "A#outer");
        assert_owner(&r, &e, 3, "A#inner");
        assert_owner(&r, &e, 4, "A#inner");
        assert_owner(&r, &e, 5, "A#outer");
        assert_owner(&r, &e, 6, "A#outer");
        assert_abstain(&r, &e, 7);
    }

    #[test]
    fn ruby_malformed_nested_def_is_barrier() {
        // A nested def that cannot establish identity (parenthesized receiver
        // is an unsupported singleton form) becomes its own barrier; the
        // surrounding outer method body still attributes normally.
        let (r, e) = parse_ruby("def outer\n  def (x).inner; end\n  tail\nend\n");
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 3, "outer");
        assert_owner(&r, &e, 4, "outer");
    }

    // ---- Dynamic metaprogramming barriers ----

    #[test]
    fn ruby_define_method_family_produces_no_owner_and_blocks_do_not_leak() {
        let (r, e) = parse_ruby(
            "define_method(:run) { |x| x }\ndefine_singleton_method(:go) do\n  def leaked\n  end\nend\n",
        );
        for line in 1..=5 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_eval_and_exec_families_are_barriers_with_no_block_leak() {
        let (r, e) = parse_ruby(
            "class_eval { def leaker1; end }\nmodule_eval do\n  def leaker2; end\nend\ninstance_eval { }\nclass_exec { def leaker3; end }\nmodule_exec { }\ninstance_exec { }\n",
        );
        for line in 1..=7 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_alias_and_alias_method_produce_no_owner() {
        let (r, e) = parse_ruby("alias new_name old_name\nalias_method :new_name, :old_name\n");
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
    }

    #[test]
    fn ruby_attr_accessors_produce_no_owner() {
        let (r, e) = parse_ruby(
            "class A\n  attr_accessor :name\n  attr_reader :read_only\n  attr_writer :write_only\n  def real; end\nend\n",
        );
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_abstain(&r, &e, 4);
        assert_owner(&r, &e, 5, "A#real");
    }

    #[test]
    fn ruby_metaprogramming_hazard_barrier_blocks_enclosing_fallthrough() {
        // A hazard line inside a method must not attribute to the method; the
        // surrounding body still does.
        let (r, e) = parse_ruby("def outer\n  define_method(:x) { }\n  hit_tail\nend\n");
        assert_owner(&r, &e, 1, "outer");
        assert_abstain(&r, &e, 2);
        assert_owner(&r, &e, 3, "outer");
    }

    #[test]
    fn ruby_ordinary_calls_remain_transparent() {
        // `proc`, `Proc.new`, `send`, and plain calls are NOT hazards; their
        // bodies inherit the enclosing method.
        let (r, e) = parse_ruby(
            "def outer\n  proc { hit }\n  Proc.new { hit2 }\n  send(:define_method, :x) { hit3 }\n  helper\nend\n",
        );
        for line in 1..=6 {
            assert_owner(&r, &e, line, "outer");
        }
    }

    // ---- Reopened sites and determinism ----

    #[test]
    fn ruby_reopened_classes_keep_independent_ranges() {
        let (r, e) = parse_ruby("class A\n  def m; end\nend\nclass A\n  def m; end\nend\n");
        let owners: Vec<&OwnerAnchor> = (1..=6)
            .filter_map(|line| ruby_owner_for(&r, &e, line).named())
            .collect();
        let mut ranges: HashSet<(u32, u32)> =
            owners.iter().map(|o| (o.start_line, o.end_line)).collect();
        assert_eq!(ranges.len(), 2, "two reopened sites must stay distinct");
        assert!(ranges.remove(&(2, 2)));
        assert!(ranges.remove(&(5, 5)));
        assert_owner(&r, &e, 2, "A#m");
        assert_owner(&r, &e, 5, "A#m");
    }

    // ---- Error degradation and anti-leak controls ----

    #[test]
    fn ruby_local_error_degrades_method_while_clean_sibling_survives() {
        // An unclosed parameter list leaves a local missing token inside
        // `broken` only: the whole method degrades to a barrier while the
        // clean `also_good` sibling keeps its owner.
        let (r, e) =
            parse_ruby("def good\n  hit\nend\ndef broken(\n  x\nend\ndef also_good\n  hit2\nend\n");
        assert_owner(&r, &e, 1, "good");
        assert_owner(&r, &e, 2, "good");
        assert_abstain(&r, &e, 4);
        assert_abstain(&r, &e, 5);
        assert_abstain(&r, &e, 6);
        assert_owner(&r, &e, 8, "also_good");
        assert_owner(&r, &e, 9, "also_good");
    }

    #[test]
    fn ruby_error_degraded_method_does_not_leak_recovered_nested_method() {
        // An error-overlapping outer method degrades to a barrier AND descends
        // in anonymous state: a parser-recovered nested method must not regain
        // a name (Wave 2 §ERROR point 5/7).
        let (r, e) = parse_ruby("def broken\n  x = (\n  def recovered\n    hit\n  end\nend\n");
        for line in 1..=5 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_metaprogramming_block_nested_def_never_leaks() {
        // The mandatory anti-leak repro: a `define_method` block containing a
        // nested `def leaked` emits zero owner evidence.
        let (r, e) = parse_ruby("define_method(:sym) do\n  def leaked\n    hit\n  end\nend\n");
        for line in 1..=5 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_non_self_singleton_class_block_nested_def_never_leaks() {
        let (r, e) = parse_ruby("class A\n  class << obj\n    def om\n      def leaked\n      end\n    end\n  end\nend\n");
        for line in 1..=7 {
            assert_abstain(&r, &e, line);
        }
    }

    #[test]
    fn ruby_malformed_class_path_never_leaks() {
        let (r, e) =
            parse_ruby("class A::\n  def leaked; end\nend\nclass Good\n  def fine; end\nend\n");
        // `class A::` parses with a missing constant -> the container is a
        // barrier; the leaked method never regains a name.
        assert_abstain(&r, &e, 1);
        assert_abstain(&r, &e, 2);
        assert_abstain(&r, &e, 3);
        assert_owner(&r, &e, 5, "Good#fine");
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

    /// US-072 Wave 2 curated Ruby accuracy gate.
    #[test]
    fn ruby_owner_matrix_meets_quality_gates() {
        let cases: &[Case] = &[
            Case {
                label: "top-level method",
                source: "def run\n  work\nend\n",
                owners: &[(1, "run", 1, 3), (2, "run", 1, 3), (3, "run", 1, 3)],
                abstain: &[4],
                incidental: &[],
            },
            Case {
                label: "empty and endless methods",
                source: "def empty; end\ndef endless = 1\n",
                owners: &[(1, "empty", 1, 1), (2, "endless", 2, 2)],
                abstain: &[3],
                incidental: &[],
            },
            Case {
                label: "predicate bang setter index operator",
                source: "def p?; end\ndef b!; end\ndef s=(v); end\ndef [](i); end\ndef +(o); end\n",
                owners: &[
                    (1, "p?", 1, 1),
                    (2, "b!", 2, 2),
                    (3, "s=", 3, 3),
                    (4, "[]", 4, 4),
                    (5, "+", 5, 5),
                ],
                abstain: &[6],
                incidental: &[],
            },
            Case {
                label: "nested module class",
                source: "module A\n  class B\n    def run; end\n  end\nend\n",
                owners: &[(3, "A::B#run", 3, 3)],
                abstain: &[1, 2, 4],
                incidental: &[5],
            },
            Case {
                label: "scope resolution container",
                source: "class A::B\n  def run; end\nend\n",
                owners: &[(2, "A::B#run", 2, 2)],
                abstain: &[1],
                incidental: &[3],
            },
            Case {
                label: "qualified container suffix-witness merge",
                source: "module A\n  class A::B\n    def m; end\n  end\nend\n",
                owners: &[(3, "A::B#m", 3, 3)],
                abstain: &[1, 2, 4],
                incidental: &[5],
            },
            Case {
                label: "unprovable qualified container barrier",
                source: "module C\n  class A::B\n    def m; end\n  end\nend\n",
                owners: &[],
                abstain: &[1, 2, 3, 4],
                incidental: &[5],
            },
            Case {
                label: "root-qualified container is absolute",
                source: "module M\n  class ::Rooted\n    def m; end\n  end\nend\n",
                owners: &[(3, "Rooted#m", 3, 3)],
                abstain: &[1, 2, 4],
                incidental: &[5],
            },
            Case {
                label: "instance vs singleton",
                source: "class A\n  def run; end\n  def self.find; end\nend\n",
                owners: &[(2, "A#run", 2, 2), (3, "A.find", 3, 3)],
                abstain: &[1],
                incidental: &[4],
            },
            Case {
                label: "top-level def self",
                source: "def self.top; end\n",
                owners: &[(1, "self.top", 1, 1)],
                abstain: &[],
                incidental: &[2],
            },
            Case {
                label: "supported receivers",
                source: "def obj.m; end\ndef @t.go; end\ndef $g.kick; end\n",
                owners: &[(1, "obj.m", 1, 1), (2, "@t.go", 2, 2), (3, "$g.kick", 3, 3)],
                abstain: &[4],
                incidental: &[],
            },
            Case {
                label: "complex receiver abstains",
                source: "def a.b.c; end\ndef (x).m; end\ndef A::B.static; end\n",
                owners: &[],
                abstain: &[1, 2, 3],
                incidental: &[4],
            },
            Case {
                label: "class << self",
                source: "class A\n  class << self\n    def build; end\n  end\nend\n",
                owners: &[(3, "A.build", 3, 3)],
                abstain: &[1, 2, 4],
                incidental: &[5],
            },
            Case {
                label: "class << self top-level",
                source: "class << self\n  def top_self; end\nend\n",
                owners: &[(2, "self.top_self", 2, 2)],
                abstain: &[1],
                incidental: &[3],
            },
            Case {
                label: "singleton mode reset",
                source: "class << self\n  class Inner\n    def m; end\n  end\nend\n",
                owners: &[(3, "Inner#m", 3, 3)],
                abstain: &[1, 2, 4],
                incidental: &[5],
            },
            Case {
                label: "higher-order eigenclass barriers",
                source: "class A\n  class << self\n    def self.ho; end\n  end\nend\n",
                owners: &[],
                abstain: &[1, 2, 3, 4],
                incidental: &[5],
            },
            Case {
                label: "non-self singleton class barrier",
                source: "class A\n  class << obj\n    def om; end\n  end\nend\n",
                owners: &[],
                abstain: &[1, 2, 3, 4],
                incidental: &[5],
            },
            Case {
                label: "transparent block inside method",
                source: "def outer\n  [1].each do |i|\n    puts i\n  end\nend\n",
                owners: &[
                    (1, "outer", 1, 5),
                    (2, "outer", 1, 5),
                    (3, "outer", 1, 5),
                    (4, "outer", 1, 5),
                    (5, "outer", 1, 5),
                ],
                abstain: &[],
                incidental: &[6],
            },
            Case {
                label: "top-level closure ownerless",
                source: "[1].each { |i| hit }\n->(a) { a }\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "nested def independent",
                source: "def outer\n  def inner; end\n  tail\nend\n",
                owners: &[
                    (1, "outer", 1, 4),
                    (2, "inner", 2, 2),
                    (3, "outer", 1, 4),
                    (4, "outer", 1, 4),
                ],
                abstain: &[5],
                incidental: &[],
            },
            Case {
                label: "define_method barrier",
                source: "define_method(:run) { |x| x }\n",
                owners: &[],
                abstain: &[1],
                incidental: &[2],
            },
            Case {
                label: "eval family barrier",
                source: "class_eval { def leaker; end }\n",
                owners: &[],
                abstain: &[1],
                incidental: &[2],
            },
            Case {
                label: "alias and alias_method barrier",
                source: "alias new_name old_name\nalias_method :a, :b\n",
                owners: &[],
                abstain: &[1, 2],
                incidental: &[3],
            },
            Case {
                label: "attr_* barrier with clean sibling",
                source: "class A\n  attr_accessor :name\n  def real; end\nend\n",
                owners: &[(3, "A#real", 3, 3)],
                abstain: &[1, 2],
                incidental: &[4],
            },
            Case {
                label: "hazard line abstains body keeps owner",
                source: "def outer\n  define_method(:x) { }\n  tail\nend\n",
                owners: &[(1, "outer", 1, 4), (3, "outer", 1, 4), (4, "outer", 1, 4)],
                abstain: &[2],
                incidental: &[5],
            },
            Case {
                label: "reopened class sites",
                source: "class A\n  def m; end\nend\nclass A\n  def m; end\nend\n",
                owners: &[(2, "A#m", 2, 2), (5, "A#m", 5, 5)],
                abstain: &[1, 3, 4, 6],
                incidental: &[],
            },
            Case {
                label: "error degrades method sibling survives",
                source: "def good\n  hit\nend\ndef broken(\n  x\nend\ndef also\n  hit2\nend\n",
                owners: &[
                    (1, "good", 1, 3),
                    (2, "good", 1, 3),
                    (3, "good", 1, 3),
                    (7, "also", 7, 9),
                    (8, "also", 7, 9),
                    (9, "also", 7, 9),
                ],
                abstain: &[4, 5, 6],
                incidental: &[],
            },
            Case {
                label: "malformed class path barrier",
                source: "class A::\n  def leaked; end\nend\nclass Good\n  def fine; end\nend\n",
                owners: &[(5, "Good#fine", 5, 5)],
                abstain: &[1, 2, 3],
                incidental: &[4, 6],
            },
        ];

        let mut positives = 0u32;
        let mut abstentions = 0u32;
        for case in cases {
            let (regions, errors) = parse_ruby(case.source);
            for &(hit_line, name, start, end) in case.owners {
                positives += 1;
                let owner = ruby_owner_for(&regions, &errors, hit_line).named();
                assert!(
                    owner.is_some(),
                    "[{}] line {hit_line}: expected {name}@{start}-{end}, got abstain",
                    case.label
                );
                let owner = owner.unwrap();
                assert_eq!(owner.qualified_name(), name, "[{}] name", case.label);
                assert_eq!(owner.start_line, start, "[{}] start", case.label);
                assert_eq!(owner.end_line, end, "[{}] end", case.label);
                assert_eq!(owner.language, Lang::Ruby, "[{}] language", case.label);
            }
            for &line in case.abstain {
                abstentions += 1;
                assert!(
                    ruby_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] line {line} should abstain",
                    case.label
                );
            }
            for &line in case.incidental {
                assert!(
                    ruby_owner_for(&regions, &errors, line).named().is_none(),
                    "[{}] incidental line {line} should abstain",
                    case.label
                );
            }
        }
        assert!(
            positives >= 40,
            "positive floor: only {positives} assertions"
        );
        assert!(
            abstentions >= 40,
            "intentional-abstention floor: only {abstentions} assertions"
        );
    }

    /// US-072 Wave 2 §Grammar: pinned fingerprint + explicit disposition
    /// manifests. Any grammar metadata change must fail here with an
    /// actionable re-audit message rather than silently widening claims.
    #[test]
    fn manifest_gate_verifies_kinds_exist_and_fingerprint_is_pinned() {
        let v: serde_json::Value = serde_json::from_str(tree_sitter_ruby::NODE_TYPES).unwrap();
        let kinds: HashSet<String> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(String::from))
            .collect();
        assert!(
            !kinds.is_empty(),
            "tree-sitter-ruby NODE_TYPES must declare node kinds"
        );

        let mut all: Vec<&str> = Vec::new();
        all.extend(RUBY_DISPOSITIONS.iter().map(|(kind, _)| *kind));
        all.extend(RUBY_NAME_OR_RECEIVER_KINDS);
        let uniq: HashSet<&str> = all.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            all.len(),
            "tree-sitter-ruby manifest inventories must be disjoint"
        );
        for kind in &uniq {
            assert!(
                kinds.contains(*kind),
                "tree-sitter-ruby: manifest kind {kind} missing from NODE_TYPES"
            );
        }

        // Every Ruby definition/container/closure/hazard kind relevant to
        // ownership has an explicit disposition.
        let disposition_of: std::collections::HashMap<&str, &str> =
            RUBY_DISPOSITIONS.iter().copied().collect();
        for kind in &kinds {
            let is_relevant = matches!(
                kind.as_str(),
                "method"
                    | "singleton_method"
                    | "class"
                    | "module"
                    | "singleton_class"
                    | "body_statement"
                    | "block"
                    | "do_block"
                    | "lambda"
                    | "block_body"
                    | "alias"
                    | "call"
            );
            if !is_relevant {
                continue;
            }
            assert!(
                disposition_of.contains_key(kind.as_str()),
                "tree-sitter-ruby: grammar kind {kind} has no explicit disposition; add one (named/container/transparent/barrier/abstain)"
            );
        }
        for (kind, disposition) in RUBY_DISPOSITIONS {
            assert!(
                matches!(
                    *disposition,
                    "named" | "container" | "transparent" | "barrier" | "abstain"
                ),
                "tree-sitter-ruby: invalid disposition {disposition} for {kind}"
            );
        }

        // The audited hazard-name manifest is a set of distinct, non-empty
        // call method terminals (keyword `alias` is covered by node kind).
        let hazard_uniq: HashSet<&str> = RUBY_METAPROGRAMMING_HAZARD_METHODS
            .iter()
            .copied()
            .collect();
        assert_eq!(
            hazard_uniq.len(),
            RUBY_METAPROGRAMMING_HAZARD_METHODS.len(),
            "hazard manifest must not duplicate names"
        );
        assert!(
            hazard_uniq.iter().all(|h| !h.is_empty()),
            "hazard manifest must not contain empty names"
        );

        assert_eq!(
            fnv1a(tree_sitter_ruby::NODE_TYPES.as_bytes()),
            RUBY_NODE_TYPES_FINGERPRINT,
            "tree-sitter-ruby NODE_TYPES changed; re-audit the manifest and recompute the fingerprint"
        );
    }
}
