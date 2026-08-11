use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::lang::outline::{get_outline_entries, outline_language};
use crate::lang::qualified::normalize_receiver_type;
use crate::search::callees::{extract_call_sites, CallSite};
use crate::types::{Lang, OutlineEntry, OutlineKind};

pub(crate) const OWNER_LINK_EDGE_CAP: usize = 10;
pub(crate) const OWNER_LINK_CAVEAT: &str = "> Caveat: structural owner and mechanically filtered direct-call evidence only; not runtime order, dynamic dispatch, or an inferred chain.";
pub(crate) const OWNER_LINK_ZERO_EDGE: &str = "No direct name-level call evidence among hit owners. Dynamic dispatch, DI, callbacks, and protocol wiring are not ruled out.";
/// Honesty caveat for non-Go owner attribution: ranges are structural lexical
/// ownership candidates, not runtime ownership/binding proof, and no call
/// analysis was run for non-Go languages.
pub(crate) const OWNER_NON_GO_CAVEAT: &str =
    "> Caveat: owner ranges are structural lexical ownership candidates, not runtime ownership or binding proof; no call analysis was run for non-Go languages.";

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct OwnerAnchor {
    pub(crate) path: PathBuf,
    /// Unqualified callable name (e.g. `handle`).
    pub(crate) name: String,
    /// Go edge analysis may use a receiver binding; other languages leave this
    /// `None`. Never overload this field to encode non-Go containers.
    pub(crate) receiver_var: Option<String>,
    /// Go receiver type used only for Go edge identity. Non-Go anchors leave
    /// this `None`; their qualified identity lives in `display_name`.
    pub(crate) receiver_type: Option<String>,
    pub(crate) package_dir: PathBuf,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    /// Source language of the owning file.
    pub(crate) language: Lang,
    /// Explicit display-qualified name for every owner (e.g. `Service.handle`,
    /// `Outer.Inner.handle`, `DB.Set`). This is the single source of truth for
    /// rendered identity; it is required for ALL anchors, including Go. Go
    /// receiver binding stays in `receiver_var`/`receiver_type` and is never
    /// derived from this field.
    pub(crate) display_name: String,
}

impl OwnerAnchor {
    pub(crate) fn qualified_name(&self) -> String {
        self.display_name.clone()
    }

    fn receiver_identity(&self) -> Option<(&Path, &str)> {
        Some((self.package_dir.as_path(), self.receiver_type.as_deref()?))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedTextHit {
    pub(crate) path: PathBuf,
    pub(crate) line: u32,
    pub(crate) owner: OwnerAnchor,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum OwnerCallMechanism {
    SingleAssignmentLocalConstructor,
    CrossFileSameQualifiedReceiver,
    SameFileSameQualifiedReceiver,
    SamePackageBareInvocation,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct OwnerCallEvidence {
    pub(crate) caller: OwnerAnchor,
    pub(crate) call_line: u32,
    pub(crate) callee_name: String,
    pub(crate) candidate: OwnerAnchor,
    pub(crate) mechanism: OwnerCallMechanism,
}

#[derive(Debug, Default)]
pub(crate) struct OwnerLinkEvidence {
    pub(crate) hits: Vec<OwnedTextHit>,
    pub(crate) edges: Vec<OwnerCallEvidence>,
    /// True when Go call/edge analysis was actually attempted for this query
    /// (at least one Go file was parsed for edge evidence). This is the only
    /// gate for the Go mechanical-call appendix; it is never inferred from
    /// `edges.is_empty()`.
    pub(crate) go_call_analysis_attempted: bool,
}

impl OwnerLinkEvidence {
    pub(crate) fn owner_for(&self, path: &Path, line: u32) -> Option<&OwnerAnchor> {
        self.hits
            .iter()
            .find(|hit| hit.path == path && hit.line == line)
            .map(|hit| &hit.owner)
    }

    pub(crate) fn has_owners(&self) -> bool {
        !self.hits.is_empty()
    }

    /// Whether any attributed owner is a non-Go language. Derived from the
    /// anchors' required `language` field so it cannot drift from the evidence.
    pub(crate) fn has_non_go_owners(&self) -> bool {
        self.hits.iter().any(|hit| hit.owner.language != Lang::Go)
    }

    /// Whether any attributed owner is Go. Gating the Go mechanical appendix on
    /// this (not merely on `go_call_analysis_attempted`) prevents a non-Go-only
    /// or mixed result from rendering a Go zero-edge/caveat it cannot support.
    pub(crate) fn has_go_owners(&self) -> bool {
        self.hits.iter().any(|hit| hit.owner.language == Lang::Go)
    }

    /// Count of distinct attributed Go owners. The Go zero-edge sentence is
    /// gated on this (>= 2) so two Python owners can never satisfy it.
    pub(crate) fn attributed_go_owner_count(&self) -> usize {
        self.hits
            .iter()
            .filter(|hit| hit.owner.language == Lang::Go)
            .map(|hit| hit.owner.clone())
            .collect::<BTreeSet<_>>()
            .len()
    }
}

pub(crate) struct OwnerLinkHitInput<'a> {
    pub(crate) path: &'a Path,
    pub(crate) line: u32,
}

#[derive(Debug)]
struct FileOwner {
    anchor: OwnerAnchor,
}

#[derive(Debug)]
struct FileAnalysis {
    owners: Vec<FileOwner>,
    calls: Vec<CallSite>,
    /// Per-owner lexical bindings. `None` when the file could not be analyzed
    /// for bindings (e.g. a parse error); a file with no binding evidence must
    /// omit all edge evidence rather than fall back to weak guesses.
    bindings: Option<BTreeMap<u32, Vec<Binding>>>,
    /// Function start line -> single unqualified identifier return type, built
    /// structurally from the AST (see `go_function_results`).
    functions: BTreeMap<u32, String>,
}

/// A lexical binding of a simple identifier inside one named owner.
/// `start_byte` is where the binding is introduced; `scope_end` is the byte
/// just past the end of the lexical scope in which the binding is visible.
#[derive(Debug, Clone)]
struct Binding {
    name: String,
    start_byte: usize,
    scope_end: usize,
    is_param: bool,
    is_receiver: bool,
    initializer: Option<Initializer>,
    write_count: usize,
}

#[derive(Debug, Clone)]
enum Initializer {
    LocalType(String),
    Constructor(String),
}

pub(crate) fn build_owner_link_evidence(inputs: &[OwnerLinkHitInput<'_>]) -> OwnerLinkEvidence {
    // Group inputs by path, tracking which go through the Go (owners + edges)
    // pipeline versus a non-Go owner-only pipeline. Non-Go files are routed by
    // their detected language; anything unsupported is skipped.
    let mut by_path: BTreeMap<PathBuf, Vec<&OwnerLinkHitInput<'_>>> = BTreeMap::new();
    for input in inputs {
        let detected = crate::lang::detect_file_type(input.path);
        let is_go = detected == crate::types::FileType::Code(Lang::Go);
        let is_python = detected == crate::types::FileType::Code(Lang::Python);
        let is_rust = detected == crate::types::FileType::Code(Lang::Rust);
        let is_javascript = detected == crate::types::FileType::Code(Lang::JavaScript);
        if is_go || is_python || is_rust || is_javascript {
            by_path
                .entry(input.path.to_path_buf())
                .or_default()
                .push(input);
        }
    }

    let mut files = BTreeMap::new();
    let mut hits = Vec::new();
    for (path, path_inputs) in &by_path {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let detected = crate::lang::detect_file_type(path);
        if detected == crate::types::FileType::Code(Lang::Python) {
            // Python is owner-only: no call edges are inferred in phase 1.
            let Some((regions, errors)) =
                crate::evidence::owners::python::python_regions(path, &content)
            else {
                continue;
            };
            for input in path_inputs {
                if let Some(owner) =
                    crate::evidence::owners::python::python_owner_for(&regions, &errors, input.line)
                {
                    hits.push(OwnedTextHit {
                        path: path.clone(),
                        line: input.line,
                        owner: owner.clone(),
                    });
                }
            }
            continue;
        }
        if detected == crate::types::FileType::Code(Lang::Rust) {
            // Rust is owner-only: no call edges are inferred in phase 2.
            let Some((regions, errors)) =
                crate::evidence::owners::rust::rust_regions(path, &content)
            else {
                continue;
            };
            for input in path_inputs {
                if let Some(owner) =
                    crate::evidence::owners::rust::rust_owner_for(&regions, &errors, input.line)
                {
                    hits.push(OwnedTextHit {
                        path: path.clone(),
                        line: input.line,
                        owner: owner.clone(),
                    });
                }
            }
            continue;
        }
        if detected == crate::types::FileType::Code(Lang::JavaScript) {
            // JavaScript is owner-only: no call edges are inferred in phase 3B.
            let Some((regions, errors)) =
                crate::evidence::owners::js_ts::js_regions(path, &content)
            else {
                continue;
            };
            for input in path_inputs {
                if let Some(owner) =
                    crate::evidence::owners::js_ts::js_owner_for(&regions, &errors, input.line)
                {
                    hits.push(OwnedTextHit {
                        path: path.clone(),
                        line: input.line,
                        owner: owner.clone(),
                    });
                }
            }
            continue;
        }
        let package_dir = canonical_package_dir(path);
        let Some((owners, functions)) = go_file_owners(path, &package_dir, &content) else {
            // Malformed/unparsed Go: preserve raw hits, omit owner/edge evidence.
            continue;
        };
        for input in path_inputs {
            if let Some(owner) = narrowest_owner(&owners, input.line) {
                hits.push(OwnedTextHit {
                    path: path.clone(),
                    line: input.line,
                    owner: owner.anchor.clone(),
                });
            }
        }
        files.insert(
            path.clone(),
            FileAnalysis {
                calls: extract_call_sites(&content, Lang::Go, None),
                bindings: collect_go_bindings(&content),
                owners,
                functions,
            },
        );
    }

    // Edge analysis is Go-only: the candidate set must contain ONLY Go owners.
    // `hits` also carries Python/Rust owners, but they must never populate
    // `OwnerCallEvidence` — a same-name non-Go owner in the same package/dir
    // would otherwise create a fake Go->non-Go bare edge (cross-language
    // contamination). Callers and candidates accordingly remain Go-only.
    let candidate_owners = hits
        .iter()
        .map(|hit| hit.owner.clone())
        .filter(|owner| owner.language == Lang::Go)
        .collect::<BTreeSet<_>>();
    let constructors = collect_constructors(&files);
    let mut edges = BTreeSet::new();

    for (path, file) in &files {
        // A file whose bindings could not be analyzed has no binding evidence;
        // omit all of its edge evidence rather than guess from an empty scope.
        if file.bindings.is_none() {
            continue;
        }
        for call in &file.calls {
            // The calling owner is the unique narrowest owner containing the
            // call line; abstain on ties (column-less line cannot disambiguate).
            let Some(caller) = unique_narrowest_owner(
                candidate_owners.iter().filter(|owner| owner.path == *path),
                call.line,
            ) else {
                continue;
            };
            let (bare, receiver) = call_shape(call);
            for candidate in candidate_owners
                .iter()
                .filter(|owner| owner.name == call.callee && *owner != caller)
            {
                let mechanism =
                    resolve_mechanism(caller, candidate, call, bare, receiver, file, &constructors);
                if let Some(mechanism) = mechanism {
                    edges.insert(OwnerCallEvidence {
                        caller: caller.clone(),
                        call_line: call.line,
                        callee_name: call.callee.clone(),
                        candidate: candidate.clone(),
                        mechanism,
                    });
                }
            }
        }
    }

    let mut edges = edges.into_iter().collect::<Vec<_>>();
    edges.sort_by(|a, b| {
        a.mechanism
            .cmp(&b.mechanism)
            .then(a.caller.path.cmp(&b.caller.path))
            .then(a.call_line.cmp(&b.call_line))
            .then(a.caller.name.cmp(&b.caller.name))
            .then(a.candidate.path.cmp(&b.candidate.path))
            .then(a.candidate.start_line.cmp(&b.candidate.start_line))
    });

    // `files` holds exactly the Go files that parsed successfully and entered
    // edge-analysis setup. A malformed/unparseable Go input is skipped before
    // insertion, so this is the only honest signal that Go call/edge analysis
    // was actually attempted for the query.
    let go_call_analysis_attempted = !files.is_empty();

    OwnerLinkEvidence {
        hits,
        edges,
        go_call_analysis_attempted,
    }
}

fn resolve_mechanism(
    caller: &OwnerAnchor,
    candidate: &OwnerAnchor,
    call: &CallSite,
    bare: bool,
    receiver: Option<&str>,
    file: &FileAnalysis,
    constructors: &BTreeMap<(PathBuf, String), String>,
) -> Option<OwnerCallMechanism> {
    let call_byte = call.call_byte_range.map(|(start, _)| start);

    // P1: same qualified receiver — only when the receiver variable's lexical
    // binding at the call is the method receiver itself (not shadowed by a
    // nested func-literal parameter or local declaration).
    if let (Some(receiver), Some(caller_var)) = (receiver, caller.receiver_var.as_deref()) {
        if receiver == caller_var
            && caller.receiver_identity().is_some()
            && caller.receiver_identity() == candidate.receiver_identity()
            && binding_at(file, caller, receiver, call_byte)
                .is_some_and(|binding| binding.is_receiver)
        {
            return Some(if caller.path == candidate.path {
                OwnerCallMechanism::SameFileSameQualifiedReceiver
            } else {
                OwnerCallMechanism::CrossFileSameQualifiedReceiver
            });
        }
    }

    if bare && candidate.receiver_type.is_none() && caller.package_dir == candidate.package_dir {
        // Reject a bare invocation when the callee name is shadowed by a local
        // or parameter binding visible at the call, so we never link to a
        // package-level function that is actually a local variable here.
        if binding_at(file, caller, &call.callee, call_byte).is_none() {
            return Some(OwnerCallMechanism::SamePackageBareInvocation);
        }
        return None;
    }

    let receiver = receiver.filter(|receiver| is_simple_go_identifier(receiver))?;
    let binding = binding_at(file, caller, receiver, call_byte)?;
    if binding.is_param || binding.write_count != 1 {
        return None;
    }
    let receiver_type = match binding.initializer.as_ref()? {
        Initializer::LocalType(receiver_type) => receiver_type.clone(),
        Initializer::Constructor(name) => {
            // Resolve the constructor against the package-level map only when
            // the constructor name is not locally bound at the initializer
            // byte (i.e. a local/parameter shadows the package constructor).
            if binding_at(file, caller, name, Some(binding.start_byte)).is_some() {
                return None;
            }
            constructors
                .get(&(caller.package_dir.clone(), name.clone()))?
                .clone()
        }
    };
    if candidate.receiver_identity() == Some((caller.package_dir.as_path(), receiver_type.as_str()))
    {
        Some(OwnerCallMechanism::SingleAssignmentLocalConstructor)
    } else {
        None
    }
}

/// Resolve the innermost lexical binding of `receiver` visible at the call's
/// byte offset. Returns `None` if the binding is out of scope or the receiver
/// is not a simple local identifier.
fn binding_at<'a>(
    file: &'a FileAnalysis,
    caller: &OwnerAnchor,
    receiver: &str,
    call_byte: Option<usize>,
) -> Option<&'a Binding> {
    let call_byte = call_byte?;
    file.bindings
        .as_ref()?
        .get(&caller.start_line)?
        .iter()
        .filter(|binding| binding.name == receiver)
        .filter(|binding| binding.start_byte <= call_byte && call_byte < binding.scope_end)
        .max_by_key(|binding| binding.start_byte)
}

fn call_shape(call: &CallSite) -> (bool, Option<&str>) {
    let Some(prefix) = call.call_prefix.as_deref() else {
        return (false, None);
    };
    if prefix == call.callee {
        return (true, None);
    }
    let receiver = prefix
        .strip_suffix(&call.callee)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .map(str::trim)
        .filter(|receiver| !receiver.is_empty());
    (false, receiver)
}

/// Parse a Go file once. Returns `None` when the tree has parse errors
/// (malformed Go), in which case the caller preserves raw hits but omits all
/// owner/edge evidence for the file. Receivers are derived structurally from
/// each `method_declaration` AST node keyed by start line — never from the
/// rendered outline signature.
fn go_file_owners(
    path: &Path,
    package_dir: &Path,
    content: &str,
) -> Option<(Vec<FileOwner>, BTreeMap<u32, String>)> {
    let language = outline_language(Lang::Go)?;
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return None;
    }
    let tree = parser.parse(content, None)?;
    if tree.root_node().has_error() {
        return None;
    }
    let receivers = go_method_receivers(&tree, content.as_bytes());
    let mut owners = Vec::new();
    collect_file_owners(
        path,
        package_dir,
        &get_outline_entries(content, Lang::Go),
        &receivers,
        &mut owners,
    );
    Some((owners, go_function_results(&tree, content.as_bytes())))
}

/// Build a map from top-level `method_declaration` start line to
/// `(receiver variable, normalized receiver type)` derived from the AST's
/// `receiver` parameter list.
fn go_method_receivers(
    tree: &tree_sitter::Tree,
    content: &[u8],
) -> BTreeMap<u32, Option<(Option<String>, String)>> {
    let mut receivers = BTreeMap::new();
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().children(&mut cursor) {
        if node.kind() != "method_declaration" {
            continue;
        }
        let start_line = node.start_position().row as u32 + 1;
        receivers.insert(start_line, go_node_receiver(node, content));
    }
    receivers
}

/// Extract the receiver variable and normalized type from a top-level
/// `method_declaration` node's `receiver` parameter list.
fn go_node_receiver(
    node: tree_sitter::Node<'_>,
    content: &[u8],
) -> Option<(Option<String>, String)> {
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let pd = receiver
        .named_children(&mut cursor)
        .find(|child| child.kind() == "parameter_declaration")?;
    let variable = pd
        .child_by_field_name("name")
        .and_then(|name| name.utf8_text(content).ok())
        .map(str::to_string);
    let type_node = pd.child_by_field_name("type")?;
    let type_text = type_node.utf8_text(content).ok()?;
    Some((variable, normalize_receiver_type(type_text)))
}

/// Build a map from top-level `function_declaration` start line to its single
/// return type, derived structurally from the AST `result` node. Only a return
/// that is exactly one unqualified identifier (after stripping pointer/array
/// wrappers) is recorded — this is the conservative constructor contract.
/// Construction is structural, never signature-string parsed, so generics,
/// qualified types, multi-return, interfaces, and parse errors all abstain.
fn go_function_results(tree: &tree_sitter::Tree, content: &[u8]) -> BTreeMap<u32, String> {
    let mut results = BTreeMap::new();
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().children(&mut cursor) {
        if node.kind() != "function_declaration" {
            continue;
        }
        let start_line = node.start_position().row as u32 + 1;
        if let Some(return_type) = go_single_unqualified_result(node, content) {
            results.insert(start_line, return_type);
        }
    }
    results
}

/// Extract a single unqualified identifier return type from a function's
/// `result` node, using only AST node kinds (never raw text heuristics).
///
/// Accepted shapes (exactly):
///   * a direct `type_identifier` (e.g. `*`-free `Batch`), or
///   * exactly one `pointer_type` whose single named child is a
///     `type_identifier` (e.g. `*Batch`).
///
/// Rejected (return `None`): multi-return (`(a, b int)`, `(T, error)`),
/// generics, qualified types, interfaces, arrays, `**T`, unnamed results,
/// parenthesized types, and any other AST kind.
fn go_single_unqualified_result(node: tree_sitter::Node<'_>, content: &[u8]) -> Option<String> {
    let result = node.child_by_field_name("result")?;
    let type_node = if result.kind() == "parameter_list" {
        // Count the named/slot results across every parameter declaration so
        // `(a, b int)` (two values) and `(int, error)` (two types) abstain.
        let mut cursor = result.walk();
        let pds = result
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "parameter_declaration")
            .collect::<Vec<_>>();
        if pds.len() != 1 {
            return None;
        }
        let pd = pds[0];
        let mut name_cursor = pd.walk();
        let name_count = pd.children_by_field_name("name", &mut name_cursor).count();
        if name_count != 1 {
            return None;
        }
        pd.child_by_field_name("type")?
    } else {
        result
    };
    go_identifier_through_at_most_one_pointer(type_node, content)
}

/// Resolve a type node to a single unqualified identifier by walking through
/// at most one `pointer_type` wrapper. `None` for any other shape or for a
/// pointer wrapping a non-identifier (e.g. `**T`, `*pkg.T`, `*[]T`).
fn go_identifier_through_at_most_one_pointer(
    type_node: tree_sitter::Node<'_>,
    content: &[u8],
) -> Option<String> {
    let inner = match type_node.kind() {
        "type_identifier" => type_node,
        "pointer_type" => {
            let child = type_node.named_child(0)?;
            // A pointer wrapping anything other than a bare identifier is
            // rejected (this also rejects `**T` because the inner node is
            // itself a `pointer_type`, not a `type_identifier`).
            if child.kind() != "type_identifier" {
                return None;
            }
            child
        }
        _ => return None,
    };
    inner.utf8_text(content).ok().map(str::to_string)
}

fn collect_file_owners(
    path: &Path,
    package_dir: &Path,
    entries: &[OutlineEntry],
    receivers: &BTreeMap<u32, Option<(Option<String>, String)>>,
    owners: &mut Vec<FileOwner>,
) {
    for entry in entries {
        if entry.kind == OutlineKind::Function {
            // `receivers.get(&start_line)` is:
            //   None              -> not a method (a plain function) -> bare owner
            //   Some(None)        -> a method whose receiver failed to parse -> omit
            //   Some(Some((v,t))) -> a method with a structural receiver -> method owner
            match receivers.get(&entry.start_line) {
                Some(None) => {
                    // AST confirmed this is a method but its receiver could not
                    // be extracted. Omit the owner rather than misclassify it
                    // as a bare function and risk a false link.
                    continue;
                }
                Some(Some((receiver_var, receiver_type))) => {
                    let receiver_type =
                        (!receiver_type.is_empty()).then_some(receiver_type.clone());
                    let display_name = receiver_type
                        .as_ref()
                        .map_or_else(|| entry.name.clone(), |r| format!("{r}.{}", entry.name));
                    owners.push(FileOwner {
                        anchor: OwnerAnchor {
                            path: path.to_path_buf(),
                            name: entry.name.clone(),
                            receiver_var: receiver_var.clone(),
                            receiver_type,
                            package_dir: package_dir.to_path_buf(),
                            start_line: entry.start_line,
                            end_line: entry.end_line,
                            language: Lang::Go,
                            display_name,
                        },
                    });
                }
                None => {
                    owners.push(FileOwner {
                        anchor: OwnerAnchor {
                            path: path.to_path_buf(),
                            name: entry.name.clone(),
                            receiver_var: None,
                            receiver_type: None,
                            package_dir: package_dir.to_path_buf(),
                            start_line: entry.start_line,
                            end_line: entry.end_line,
                            language: Lang::Go,
                            display_name: entry.name.clone(),
                        },
                    });
                }
            }
        }
        collect_file_owners(path, package_dir, &entry.children, receivers, owners);
    }
}

/// Pick the unique owner whose `[start_line, end_line]` contains `line` and has
/// the smallest span. Returns `None` when nothing contains `line` or when two
/// or more distinct owners tie for the narrowest span: without a column there
/// is no deterministic way to choose, so we abstain rather than risk a false
/// owner. The minimum is computed first, then uniqueness is required, so a
/// later narrower owner is never lost to an earlier tie.
fn narrowest_owner(owners: &[FileOwner], line: u32) -> Option<&FileOwner> {
    let containing = owners
        .iter()
        .filter(|owner| owner.anchor.start_line <= line && line <= owner.anchor.end_line)
        .collect::<Vec<_>>();
    let min_span = containing
        .iter()
        .map(|owner| {
            owner
                .anchor
                .end_line
                .saturating_sub(owner.anchor.start_line)
        })
        .min()?;
    let mut narrowest = containing.iter().filter(|owner| {
        owner
            .anchor
            .end_line
            .saturating_sub(owner.anchor.start_line)
            == min_span
    });
    let first = narrowest.next()?;
    if narrowest.next().is_some() {
        None
    } else {
        Some(&**first)
    }
}

/// Pick the unique `OwnerAnchor` whose `[start_line, end_line]` contains
/// `line` and has the smallest span, abstaining on any tie (column-less lines
/// cannot be disambiguated).
pub(crate) fn unique_narrowest_owner<'a>(
    owners: impl IntoIterator<Item = &'a OwnerAnchor>,
    line: u32,
) -> Option<&'a OwnerAnchor> {
    let containing = owners
        .into_iter()
        .filter(|owner| owner.start_line <= line && line <= owner.end_line)
        .collect::<Vec<_>>();
    let min_span = containing
        .iter()
        .map(|owner| owner.end_line.saturating_sub(owner.start_line))
        .min()?;
    let mut narrowest = containing
        .iter()
        .filter(|owner| owner.end_line.saturating_sub(owner.start_line) == min_span);
    let first = narrowest.next()?;
    if narrowest.next().is_some() {
        None
    } else {
        Some(first)
    }
}

fn collect_constructors(
    files: &BTreeMap<PathBuf, FileAnalysis>,
) -> BTreeMap<(PathBuf, String), String> {
    let mut constructors = BTreeMap::new();
    for file in files.values() {
        for owner in &file.owners {
            if owner.anchor.receiver_type.is_some() {
                continue;
            }
            // Only a function whose AST `result` is a single unqualified
            // identifier is a candidate constructor. Structural extraction is
            // abstain-only on generics, qualified, multi-return, interface, and
            // parse errors.
            if let Some(return_type) = file.functions.get(&owner.anchor.start_line) {
                constructors.insert(
                    (owner.anchor.package_dir.clone(), owner.anchor.name.clone()),
                    return_type.clone(),
                );
            }
        }
    }
    constructors
}

/// Canonicalize a file's parent directory for normalized package identity.
/// Falls back to the raw parent when canonicalization fails (e.g. missing
/// dir). Normalizing makes symlink and Windows path aliases compare equal.
fn canonical_package_dir(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf())
}

/// Collect lexical bindings for every named owner in a Go file, keyed by the
/// owner's start line. A binding carries its introduction byte, the byte just
/// past its lexical scope, whether it is a parameter/receiver, and its
/// single-assignment initializer plus write count. Shadowing is resolved by
/// choosing the innermost binding whose `[start_byte, scope_end)` contains the
/// call site.
///
/// Returns `None` when the file cannot be analyzed structurally (missing
/// language, parser failure, or a parse error). A caller must omit edge
/// evidence for such a file rather than treat an absent scope as "no shadow".
fn collect_go_bindings(content: &str) -> Option<BTreeMap<u32, Vec<Binding>>> {
    let language = outline_language(Lang::Go)?;
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return None;
    }
    let tree = parser.parse(content, None)?;
    if tree.root_node().has_error() {
        return None;
    }
    let bytes = content.as_bytes();
    let mut bindings = BTreeMap::new();
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().children(&mut cursor) {
        if matches!(node.kind(), "function_declaration" | "method_declaration") {
            let owner_start = node.start_position().row as u32 + 1;
            let owner_end = node.end_byte();
            let mut scope = Vec::new();
            collect_owner_bindings(node, bytes, owner_end, &mut scope);
            bindings.insert(owner_start, scope);
        }
    }
    Some(bindings)
}

/// Walk one owner's parameter list and body, collecting lexical bindings.
fn collect_owner_bindings(
    node: tree_sitter::Node<'_>,
    content: &[u8],
    owner_end: usize,
    scope: &mut Vec<Binding>,
) {
    // Receiver and named parameters are parameters bound to the owner scope.
    if let Some(receiver) = node.child_by_field_name("receiver") {
        collect_parameter_bindings(receiver, owner_end, true, content, scope);
    }
    if let Some(parameters) = node.child_by_field_name("parameters") {
        collect_parameter_bindings(parameters, owner_end, false, content, scope);
    }
    // Named results are in scope across the body and shadow like parameters.
    if let Some(results) = node.child_by_field_name("result") {
        collect_parameter_bindings(results, owner_end, false, content, scope);
    }
    if let Some(body) = node.child_by_field_name("body") {
        collect_body_bindings(body, owner_end, content, scope);
    }
    // Every explicit write to a simple identifier records a write on the
    // innermost in-scope binding, so reassignment and shadowing disqualify the
    // single-assignment rule.
    count_binding_writes(node, content, scope);
}

/// Record the owner's receiver and named parameters as in-scope bindings.
fn collect_parameter_bindings(
    parameter_list: tree_sitter::Node<'_>,
    owner_end: usize,
    is_receiver: bool,
    content: &[u8],
    scope: &mut Vec<Binding>,
) {
    let mut cursor = parameter_list.walk();
    for child in parameter_list.named_children(&mut cursor) {
        if child.kind() != "parameter_declaration" {
            continue;
        }
        let start = child.start_byte();
        if let Some(name_node) = child.child_by_field_name("name") {
            for name in list_items(name_node) {
                if name.kind() != "identifier" {
                    continue;
                }
                if let Ok(name) = name.utf8_text(content) {
                    scope.push(Binding {
                        name: name.to_string(),
                        start_byte: start,
                        scope_end: owner_end,
                        is_param: true,
                        is_receiver,
                        initializer: None,
                        write_count: 0,
                    });
                }
            }
        }
    }
}

/// Recurse a body, pushing a nested scope boundary for each block and func
/// literal, and registering declarations as bindings scoped to that block end.
fn collect_body_bindings(
    node: tree_sitter::Node<'_>,
    scope_end: usize,
    content: &[u8],
    scope: &mut Vec<Binding>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_node_bindings(child, scope_end, content, scope);
    }
}

fn collect_node_bindings(
    node: tree_sitter::Node<'_>,
    scope_end: usize,
    content: &[u8],
    scope: &mut Vec<Binding>,
) {
    match node.kind() {
        // `block` and the control statements (`if`/`for`/`switch`) each open a
        // lexical scope that ends at the node's own boundary, so declarations
        // inside them (including init-clause/range locals) expire there instead
        // of leaking into the enclosing scope.
        "block"
        | "if_statement"
        | "for_statement"
        | "expression_switch_statement"
        | "type_switch_statement" => {
            let block_end = node.end_byte();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_node_bindings(child, block_end, content, scope);
            }
        }
        "func_literal" => {
            let literal_end = node.end_byte();
            if let Some(parameters) = node.child_by_field_name("parameters") {
                collect_parameter_bindings(parameters, literal_end, false, content, scope);
            }
            // Named results in a func literal shadow the outer receiver too.
            if let Some(results) = node.child_by_field_name("result") {
                collect_parameter_bindings(results, literal_end, false, content, scope);
            }
            if let Some(body) = node.child_by_field_name("body") {
                collect_body_bindings(body, literal_end, content, scope);
            }
        }
        "short_var_declaration" | "var_spec" => {
            collect_declaration_bindings(node, scope_end, content, scope);
        }
        "range_clause" => {
            collect_unknown_bindings(node, scope_end, content, scope);
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_node_bindings(child, scope_end, content, scope);
            }
        }
    }
}

/// Register a declaration (`:=`, `=`, `var`) as a binding with its initializer.
fn collect_declaration_bindings(
    node: tree_sitter::Node<'_>,
    scope_end: usize,
    content: &[u8],
    scope: &mut Vec<Binding>,
) {
    let Some(left) = node
        .child_by_field_name("left")
        .or_else(|| node.child_by_field_name("name"))
    else {
        return;
    };
    let right = node
        .child_by_field_name("right")
        .or_else(|| node.child_by_field_name("value"));
    let left_nodes = list_items(left);
    let right_nodes = right.map(list_items).unwrap_or_default();
    for (index, lhs) in left_nodes.into_iter().enumerate() {
        if lhs.kind() != "identifier" {
            continue;
        }
        let Ok(name) = lhs.utf8_text(content) else {
            continue;
        };
        let initializer = right_nodes
            .get(index)
            .and_then(|rhs| classify_initializer(*rhs, content));
        scope.push(Binding {
            name: name.to_string(),
            start_byte: node.start_byte(),
            scope_end,
            is_param: false,
            is_receiver: false,
            initializer,
            write_count: 0,
        });
    }
}

/// Record range variables and `x++` as writes that disqualify single-assignment
/// (no constructor initializer can be proven).
fn collect_unknown_bindings(
    node: tree_sitter::Node<'_>,
    scope_end: usize,
    content: &[u8],
    scope: &mut Vec<Binding>,
) {
    let left = node
        .child_by_field_name("left")
        .or_else(|| node.child_by_field_name("operand"))
        .or_else(|| node.named_child(0));
    let Some(left) = left else { return };
    for item in list_items(left) {
        if item.kind() != "identifier" {
            continue;
        }
        if let Ok(name) = item.utf8_text(content) {
            scope.push(Binding {
                name: name.to_string(),
                start_byte: node.start_byte(),
                scope_end,
                is_param: false,
                is_receiver: false,
                initializer: None,
                write_count: 0,
            });
        }
    }
}

/// After collecting bindings for an owner, walk every write site and bump the
/// `write_count` of the innermost in-scope binding for that name. A single
/// assignment keeps `write_count == 1`; reassignment, tuple writes, branch
/// writes, and shadowed pre-existing bindings push it above one.
fn count_binding_writes(node: tree_sitter::Node<'_>, content: &[u8], scope: &mut [Binding]) {
    if matches!(
        node.kind(),
        "short_var_declaration"
            | "assignment_statement"
            | "var_spec"
            | "range_clause"
            | "inc_statement"
    ) {
        let left = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| node.child_by_field_name("operand"))
            .or_else(|| node.named_child(0));
        if let Some(left) = left {
            let write_byte = node.start_byte();
            for item in list_items(left) {
                if item.kind() != "identifier" {
                    continue;
                }
                if let Ok(name) = item.utf8_text(content) {
                    if let Some(binding) = innermost_binding_mut(scope, name, write_byte) {
                        binding.write_count += 1;
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "function_declaration" | "method_declaration") {
            continue;
        }
        count_binding_writes(child, content, scope);
    }
}

/// Find the innermost binding of `name` whose `[start_byte, scope_end)`
/// contains `at_byte`.
fn innermost_binding_mut<'a>(
    scope: &'a mut [Binding],
    name: &str,
    at_byte: usize,
) -> Option<&'a mut Binding> {
    let candidates: Vec<usize> = scope
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.name == name)
        .filter(|(_, binding)| binding.start_byte <= at_byte && at_byte < binding.scope_end)
        .map(|(idx, _)| idx)
        .collect();
    let idx = candidates
        .into_iter()
        .max_by_key(|&idx| scope[idx].start_byte)?;
    Some(&mut scope[idx])
}

fn list_items(node: tree_sitter::Node<'_>) -> Vec<tree_sitter::Node<'_>> {
    if matches!(node.kind(), "expression_list" | "identifier_list") {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    } else {
        vec![node]
    }
}

fn classify_initializer(node: tree_sitter::Node<'_>, content: &[u8]) -> Option<Initializer> {
    if node.kind() == "call_expression" {
        let function = node.child_by_field_name("function")?;
        if function.kind() == "identifier" {
            return function
                .utf8_text(content)
                .ok()
                .map(|name| Initializer::Constructor(name.to_string()));
        }
        return None;
    }
    let composite = if node.kind() == "composite_literal" {
        Some(node)
    } else if node.kind() == "unary_expression" {
        let mut cursor = node.walk();
        let composite = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "composite_literal");
        composite
    } else {
        None
    }?;
    let type_node = composite.child_by_field_name("type")?;
    let type_text = type_node.utf8_text(content).ok()?;
    let receiver_type = normalize_receiver_type(type_text);
    is_simple_go_identifier(&receiver_type).then_some(Initializer::LocalType(receiver_type))
}

fn is_simple_go_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("srcwalk-owner-links-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn inputs<'a>(_term: &'a str, path: &'a Path, lines: &[u32]) -> Vec<OwnerLinkHitInput<'a>> {
        lines
            .iter()
            .map(|line| OwnerLinkHitInput { path, line: *line })
            .collect()
    }

    #[test]
    fn attributes_method_hits_and_abstains_outside_functions() {
        let dir = temp_dir("owners");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\n// Set docs\ntype T struct{}\nfunc (t *T) Set() {\n // Set inside\n}\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("Set", &path, &[2, 4, 5]));

        assert!(evidence.owner_for(&path, 2).is_none());
        assert_eq!(
            evidence
                .owner_for(&path, 4)
                .map(OwnerAnchor::qualified_name),
            Some("T.Set".to_string())
        );
        assert_eq!(evidence.hits.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolves_same_receiver_bare_and_single_assignment_edges() {
        let dir = temp_dir("positive");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype DB struct{}\ntype Batch struct{}\nfunc helper() {}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (d *DB) Apply() {}\nfunc (b *Batch) Close() {}\nfunc (d *DB) Set() {\n helper()\n d.Apply()\n b := newBatch()\n b.Close()\n}\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7, 8]));
        let mechanisms = evidence
            .edges
            .iter()
            .map(|edge| edge.mechanism)
            .collect::<BTreeSet<_>>();

        assert!(mechanisms.contains(&OwnerCallMechanism::SamePackageBareInvocation));
        assert!(mechanisms.contains(&OwnerCallMechanism::SameFileSameQualifiedReceiver));
        assert!(mechanisms.contains(&OwnerCallMechanism::SingleAssignmentLocalConstructor));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_composite_literal_reassignment_field_and_cross_package_type_name() {
        let root = temp_dir("negative");
        let p1 = root.join("p1");
        let p2 = root.join("p2");
        fs::create_dir_all(&p1).unwrap();
        fs::create_dir_all(&p2).unwrap();
        let path1 = p1.join("one.go");
        let path2 = p2.join("two.go");
        fs::write(
            &path1,
            "package p1\ntype Metrics struct{}\ntype T struct{ other *T }\nfunc MetricsFn() {}\nfunc newT() *T { return &T{} }\nfunc (t *T) Close() {}\nfunc (t *T) Run() {\n _ = &Metrics{}\n x := newT()\n x = t.other\n x.Close()\n t.other.Close()\n}\n",
        )
        .unwrap();
        fs::write(
            &path2,
            "package p2\ntype T struct{}\nfunc (t *T) Close() {}\n",
        )
        .unwrap();
        let mut all = inputs("hit", &path1, &[3, 4, 5, 6, 7]);
        all.extend(inputs("hit", &path2, &[2, 3]));
        let evidence = build_owner_link_evidence(&all);

        assert_eq!(evidence.edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(evidence.edges[0].callee_name, "newT");
        assert_eq!(
            evidence.edges[0].mechanism,
            OwnerCallMechanism::SamePackageBareInvocation
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_assignment_accepts_closure_capture_and_rejects_ambiguous_writes() {
        let dir = temp_dir("single-assignment");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc Safe() { b := newBatch(); defer func() { b.Close() }() }\nfunc Reassigned() { b := newBatch(); b = &Batch{}; b.Close() }\nfunc WrittenAfter() { b := newBatch(); b.Close(); b = &Batch{} }\nfunc Tuple() { b := newBatch(); b, _ = &Batch{}, 1; b.Close() }\nfunc Branch(ok bool) { var b *Batch; if ok { b = newBatch() }; b.Close() }\nfunc Ranged() { for b := range []*Batch{} { b.Close() } }\nfunc ParamBind(x *Other) { x = newBatch(); x.Close() }\nfunc ClosureShadow() { b := newBatch(); func(b *Other) { b.Close() }() }\n",
        )
        .unwrap();
        let evidence =
            build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7, 8, 9, 10, 11, 12, 13]));
        let close_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "Close")
            .collect::<Vec<_>>();

        assert_eq!(close_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(close_edges[0].caller.name, "Safe");
        assert_eq!(
            close_edges[0].mechanism,
            OwnerCallMechanism::SingleAssignmentLocalConstructor
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_receiver_identity_crosses_files_but_not_package_directories() {
        let root = temp_dir("package-identity");
        let package = root.join("same");
        let other = root.join("other");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(&other).unwrap();
        let apply = package.join("apply.go");
        let set = package.join("set.go");
        let other_apply = other.join("apply.go");
        fs::write(
            &apply,
            "package same\ntype DB struct{}\nfunc (d *DB) Apply() {}\n",
        )
        .unwrap();
        fs::write(&set, "package same\nfunc (d *DB) Set() { d.Apply() }\n").unwrap();
        fs::write(
            &other_apply,
            "package other\ntype DB struct{}\nfunc (d *DB) Apply() {}\n",
        )
        .unwrap();
        let mut all = inputs("hit", &apply, &[3]);
        all.extend(inputs("hit", &set, &[2]));
        all.extend(inputs("hit", &other_apply, &[3]));
        let evidence = build_owner_link_evidence(&all);
        let apply_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "Apply")
            .collect::<Vec<_>>();

        assert_eq!(apply_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(
            apply_edges[0].mechanism,
            OwnerCallMechanism::CrossFileSameQualifiedReceiver
        );
        assert_eq!(apply_edges[0].candidate.path, apply);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_method_names_attribute_by_receiver_and_range() {
        let dir = temp_dir("duplicate-methods");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype A struct{}\ntype B struct{}\nfunc (a *A) Set() { /* alpha */ }\nfunc (b *B) Set() { /* beta */ }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5]));

        assert_eq!(
            evidence
                .owner_for(&path, 4)
                .map(OwnerAnchor::qualified_name),
            Some("A.Set".to_string())
        );
        assert_eq!(
            evidence
                .owner_for(&path, 5)
                .map(OwnerAnchor::qualified_name),
            Some("B.Set".to_string())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn call_shape_distinguishes_bare_and_selector_calls() {
        let bare = CallSite {
            line: 1,
            callee: "f".into(),
            call_text: "f()".into(),
            call_prefix: Some("f".into()),
            args: vec![],
            return_var: None,
            is_return: false,
            call_byte_range: None,
        };
        let selector = CallSite {
            call_prefix: Some("x.f".into()),
            call_text: "x.f()".into(),
            ..bare.clone()
        };
        assert_eq!(call_shape(&bare), (true, None));
        assert_eq!(call_shape(&selector), (false, Some("x")));
    }

    #[test]
    fn p1_receiver_shadowed_by_closure_param_abstains() {
        let dir = temp_dir("p1-shadow");
        let path = dir.join("sample.go");
        // Inner `d` is a func-literal parameter, so `d.Apply()` inside must NOT
        // resolve to the outer `DB.Apply` receiver method.
        fs::write(
            &path,
            "package p\ntype DB struct{}\ntype Other struct{}\nfunc (d *DB) Apply() {}\nfunc (d *DB) Run() { func(d *Other) { d.Apply() }() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5]));
        assert_eq!(evidence.edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn p1_receiver_unshadowed_closure_still_links() {
        let dir = temp_dir("p1-unshadowed");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype DB struct{}\nfunc (d *DB) Apply() {}\nfunc (d *DB) Run() { func() { d.Apply() }() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[3, 4]));
        assert_eq!(evidence.edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(
            evidence.edges[0].mechanism,
            OwnerCallMechanism::SameFileSameQualifiedReceiver
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn p2_closure_param_shadow_and_owner_param_binding_abstain() {
        let dir = temp_dir("p2-shadow");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc (o *Other) Close() {}\nfunc OwnerParam(x *Batch) { x = newBatch(); x.Close() }\nfunc ClosureShadow() { b := newBatch(); func(b *Other) { b.Close() }() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7, 8]));
        let close_edges = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Close"
                    && edge.candidate.receiver_type.as_deref() == Some("Batch")
            })
            .collect::<Vec<_>>();
        // OwnerParam rebinds the `*Batch` parameter, so its `x.Close()` must
        // abstain (param binding); ClosureShadow's inner `b *Other` shadows and
        // its `b.Close()` resolves to Other.Close, not Batch.Close.
        assert_eq!(close_edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mixed_go_rust_python_owners_keep_edges_go_only() {
        // US-067 cross-language guard: Go edge resolution must only ever link
        // Go-owned callers to Go-owned candidates. Python/Rust owners sharing
        // the same bare callable name are owner-only (no call analysis) and must
        // never populate `OwnerCallEvidence`; a mixed query whose hits include a
        // non-Go `helper` must still emit only Go->Go edges, and the valid Go
        // bare edge must remain unchanged.
        let dir = temp_dir("cross-lang");
        let go = dir.join("handler.go");
        let rs = dir.join("other.rs");
        let py = dir.join("other.py");
        fs::write(
            &go,
            "package p\nfunc helper() {}\nfunc Run() {\n    helper()\n}\n",
        )
        .unwrap();
        fs::write(&rs, "fn helper() {\n    let x = 1;\n}\n").unwrap();
        fs::write(&py, "def helper():\n    pass\n").unwrap();
        let mut all = inputs("hit", &go, &[2, 3]);
        all.extend(inputs("hit", &rs, &[1]));
        all.extend(inputs("hit", &py, &[1]));
        let evidence = build_owner_link_evidence(&all);

        // Every attributed edge is Go->Go; no non-Go path appears as a caller
        // or candidate anywhere (the cross-language candidate never renders).
        for edge in &evidence.edges {
            assert_eq!(edge.caller.language, Lang::Go, "{:#?}", evidence.edges);
            assert_eq!(edge.candidate.language, Lang::Go, "{:#?}", evidence.edges);
            assert_eq!(edge.caller.path, go, "{:#?}", evidence.edges);
            assert_eq!(edge.candidate.path, go, "{:#?}", evidence.edges);
        }
        // The valid Go->Go bare edge is preserved unchanged.
        let helper_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "helper")
            .collect::<Vec<_>>();
        assert_eq!(helper_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(helper_edges[0].caller.name, "Run");
        assert_eq!(
            helper_edges[0].mechanism,
            OwnerCallMechanism::SamePackageBareInvocation
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_go_abstains_completely() {
        let dir = temp_dir("malformed");
        let path = dir.join("broken.go");
        fs::write(&path, "package p\nfunc (d *DB { d.Apply() \n").unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[2]));
        assert!(evidence.hits.is_empty(), "{:#?}", evidence.hits);
        assert!(evidence.edges.is_empty(), "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hit_inside_nested_closure_attributes_to_named_owner() {
        let dir = temp_dir("nested-closure");
        let path = dir.join("sample.go");
        // A hit inside a func literal body is attributed to the enclosing named
        // owner (the closure is not a top-level outline owner).
        fs::write(
            &path,
            "package p\ntype DB struct{}\nfunc (d *DB) Run() {\n    f := func() { /* alpha */ }\n    f()\n}\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4]));
        assert_eq!(evidence.hits.len(), 1, "{:#?}", evidence.hits);
        assert_eq!(
            evidence.hits[0].owner.name.as_str(),
            "Run",
            "{:#?}",
            evidence.hits
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn self_recursive_call_pair_is_omitted() {
        let dir = temp_dir("self-recursion");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype DB struct{}\nfunc (d *DB) Run() { d.Run() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[3]));
        assert_eq!(evidence.edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn constructor_ast_helper_accepts_only_identifier_or_single_pointer_to_identifier() {
        // Direct unit assertions on the structural constructor helper, parsing
        // each candidate return type and checking the node-kind proof.
        fn single_result(src: &str) -> Option<String> {
            let lang = outline_language(Lang::Go).unwrap();
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&lang).unwrap();
            let tree = parser.parse(src, None).unwrap();
            let mut cursor = tree.root_node().walk();
            for node in tree.root_node().children(&mut cursor) {
                if node.kind() == "function_declaration" {
                    return go_single_unqualified_result(node, src.as_bytes());
                }
            }
            None
        }
        // Accepted: exact identifier and single pointer to identifier.
        assert_eq!(
            single_result("package p\nfunc newT() Batch { return Batch{} }\n").as_deref(),
            Some("Batch")
        );
        assert_eq!(
            single_result("package p\nfunc newT() *Batch { return &Batch{} }\n").as_deref(),
            Some("Batch")
        );
        // Rejected: generics, qualified, multi-return, interface, **T, arrays.
        assert_eq!(
            single_result("package p\nfunc f[T any]() *Box[T] { return nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() *pkg.Batch { return nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() (Batch, error) { return Batch{}, nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() interface{} { return nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() **Batch { return nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() *[3]Batch { return nil }\n"),
            None
        );
        assert_eq!(
            single_result("package p\nfunc f() []Batch { return nil }\n"),
            None
        );
        assert_eq!(single_result("package p\nfunc f() {}\n"), None);
        assert_eq!(
            single_result("package p\nfunc f() (b Batch, e error) { return Batch{}, nil }\n"),
            None
        );
    }

    #[test]
    fn structural_constructor_abstains_on_generic_qualified_multi_interface() {
        let dir = temp_dir("ctor-abstain");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype BatchT[T any] struct{}\nfunc (b *Batch) Close() {}\nfunc newBatch() *Batch { return &Batch{} }\nfunc gen[T any]() *BatchT[T] { return &BatchT[T]{} }\nfunc qual() *pkg.Batch { return nil }\nfunc multi() (*Batch, error) { return nil, nil }\nfunc iface() interface{} { return nil }\nfunc hmm() *[3]Batch { return nil }\n",
        )
        .unwrap();
        // Only the exact single unqualified ident return `*Batch` qualifies as
        // a constructor. Generics, qualified, multi-return, interface, and
        // array-of-pointer all abstain.
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7, 8, 9, 10]));
        let close_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "Close")
            .collect::<Vec<_>>();
        assert_eq!(close_edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bare_callee_shadowed_by_local_abstains_but_unshadowed_links() {
        let dir = temp_dir("bare-shadow");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\nfunc helper() {}\nfunc Shadowed() { helper := func() {}; helper() }\nfunc Unshadowed() { helper() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[2, 3, 4]));
        let helper_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "helper")
            .collect::<Vec<_>>();
        // Only the unshadowed call links; the local `helper` shadows the
        // package-level function in Shadowed.
        assert_eq!(helper_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(helper_edges[0].caller.name, "Unshadowed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn constructor_callee_shadowed_by_local_abstains() {
        let dir = temp_dir("ctor-shadow");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc Shadowed(x *Batch) { newBatch := x; b := newBatch(); b.Close() }\nfunc Unshadowed() { b := newBatch(); b.Close() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[3, 4, 5, 6]));
        let close_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "Close")
            .collect::<Vec<_>>();
        // Shadowed rebinds `newBatch` to a local, so its `b := newBatch()` must
        // not resolve to the package constructor; Unshadowed still links.
        assert_eq!(close_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(close_edges[0].caller.name, "Unshadowed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn p1_local_shadow_before_call_abstains_but_shadow_after_still_links() {
        let dir = temp_dir("p1-local-shadow");
        let path = dir.join("sample.go");
        // A nested block shadows the receiver `d`; the inner `d.Apply()` must
        // abstain. A call before the nested block still links to DB.Apply.
        fs::write(
            &path,
            "package p\ntype DB struct{}\ntype Other struct{}\nfunc (d *DB) Apply() {}\nfunc (d *DB) ShadowedScope() { { d := &Other{}; d.Apply() } }\nfunc (d *DB) ShadowAfter() { d.Apply(); { d := &Other{}; _ = d } }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6]));
        let apply_edges = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Apply" && edge.candidate.receiver_type.as_deref() == Some("DB")
            })
            .collect::<Vec<_>>();
        // ShadowAfter calls before the inner block shadows `d`, so it still
        // links to DB.Apply; ShadowedScope's nested block shadows the receiver.
        assert_eq!(apply_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(apply_edges[0].caller.name, "ShadowAfter");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn p1_multi_level_nested_closure_shadow_abstains() {
        let dir = temp_dir("p1-multi-shadow");
        let path = dir.join("sample.go");
        // Two levels of nested closures; the innermost declares `d *Other`,
        // shadowing the receiver `d`, so `d.Apply()` must abstain from DB.Apply.
        fs::write(
            &path,
            "package p\ntype DB struct{}\ntype Other struct{}\nfunc (d *DB) Apply() {}\nfunc (o *Other) Apply() {}\nfunc (d *DB) Run() { func() { func(d *Other) { d.Apply() }() }() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6]));
        let apply_edges = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Apply" && edge.candidate.receiver_type.as_deref() == Some("DB")
            })
            .collect::<Vec<_>>();
        assert_eq!(apply_edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn p1_named_result_in_func_literal_shadows_receiver_abstains() {
        let dir = temp_dir("p1-named-result");
        let path = dir.join("sample.go");
        // The func literal declares a named result `d *DB` that shadows the
        // receiver `d` across the literal body; `d.Apply()` must abstain.
        fs::write(
            &path,
            "package p\ntype DB struct{}\ntype Other struct{}\nfunc (d *DB) Apply() {}\nfunc (d *DB) Run() { func() (d *DB) { d.Apply(); return d }() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5]));
        let apply_edges = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Apply" && edge.candidate.receiver_type.as_deref() == Some("DB")
            })
            .collect::<Vec<_>>();
        assert_eq!(apply_edges.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn control_stmt_init_clause_ctor_local_does_not_leak_past_statement() {
        let dir = temp_dir("control-scope");
        let path = dir.join("sample.go");
        // A ctor local declared in the init clause of `if`/`for`/`switch` is
        // scoped to that statement; a `.Close()` after the statement must NOT
        // resolve to it via the leaked binding.
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc (o *Other) Close() {}\nfunc Run(b *Other) { if b := newBatch(); true { _ = b }; b.Close() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7]));
        let batch_close = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Close"
                    && edge.candidate.receiver_type.as_deref() == Some("Batch")
            })
            .collect::<Vec<_>>();
        // The `b := newBatch()` inside the if-init is scoped to the `if`; the
        // trailing `b.Close()` uses the outer `*Other` param `b`, so it links
        // to Other.Close (or abstains), never Batch.Close.
        assert_eq!(batch_close.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn control_stmt_init_clause_ctor_local_still_links_inside_body() {
        let dir = temp_dir("control-scope-in");
        let path = dir.join("sample.go");
        // Inside the statement body the ctor local is in scope and links.
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc Run() { if b := newBatch(); true { b.Close() } }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6]));
        let batch_close = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Close"
                    && edge.candidate.receiver_type.as_deref() == Some("Batch")
            })
            .collect::<Vec<_>>();
        assert_eq!(batch_close.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(batch_close[0].caller.name, "Run");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn range_and_for_clause_ctor_locals_do_not_leak_past_statement() {
        let dir = temp_dir("range-scope");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc (o *Other) Close() {}\nfunc R(b *Other) { for b := newBatch(); ; { break }; b.Close() }\nfunc S(b *Other) { for ; ; { _ = b }; b.Close() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7, 8]));
        let batch_close = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Close"
                    && edge.candidate.receiver_type.as_deref() == Some("Batch")
            })
            .collect::<Vec<_>>();
        assert_eq!(batch_close.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn range_clause_ctor_local_does_not_leak_past_loop() {
        let dir = temp_dir("range-leak");
        let path = dir.join("sample.go");
        // `range` binds a ctor local with per-iteration scope; after the loop
        // the outer `*Other` param is used, so no Batch.Close link.
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc newBatch() *Batch { return &Batch{} }\nfunc (b *Batch) Close() {}\nfunc (o *Other) Close() {}\nfunc R(xs *Other, ys []Other) { for b := range ys { _ = b }; xs.Close() }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6, 7]));
        let batch_close = evidence
            .edges
            .iter()
            .filter(|edge| {
                edge.callee_name == "Close"
                    && edge.candidate.receiver_type.as_deref() == Some("Batch")
            })
            .collect::<Vec<_>>();
        assert_eq!(batch_close.len(), 0, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn direct_composite_literal_positive_and_same_line_write_after_call_reject() {
        let dir = temp_dir("direct-composite");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype Batch struct{}\ntype Other struct{}\nfunc (b *Batch) Close() {}\nfunc Direct() { x := &Batch{}; x.Close() }\nfunc WriteAfter() { b := &Batch{}; b.Close(); b = &Other{} }\n",
        )
        .unwrap();
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &[4, 5, 6]));
        let close_edges = evidence
            .edges
            .iter()
            .filter(|edge| edge.callee_name == "Close")
            .collect::<Vec<_>>();
        // Direct composite literal is a positive single-assignment link;
        // the same-line write-after-call (`b.Close(); b = &Other{}`) rejects.
        assert_eq!(close_edges.len(), 1, "{:#?}", evidence.edges);
        assert_eq!(close_edges[0].caller.name, "Direct");
        assert_eq!(
            close_edges[0].mechanism,
            OwnerCallMechanism::SingleAssignmentLocalConstructor
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn equal_narrowest_owner_span_abstains() {
        let dir = temp_dir("tie-abstain");
        let path = dir.join("sample.go");
        // Two methods on the same single line; a hit on that line has no column,
        // so attribution must abstain rather than pick one arbitrarily.
        fs::write(&path, "package p\ntype A struct{}\ntype B struct{}\nfunc (a *A) Set() {}; func (b *B) Set() {}\n").unwrap();
        let owners = vec![
            FileOwner {
                anchor: OwnerAnchor {
                    path: path.clone(),
                    name: "Set".into(),
                    receiver_var: Some("a".into()),
                    receiver_type: Some("A".into()),
                    package_dir: dir.clone(),
                    start_line: 4,
                    end_line: 4,
                    language: Lang::Go,
                    display_name: "A.Set".into(),
                },
            },
            FileOwner {
                anchor: OwnerAnchor {
                    path: path.clone(),
                    name: "Set".into(),
                    receiver_var: Some("b".into()),
                    receiver_type: Some("B".into()),
                    package_dir: dir.clone(),
                    start_line: 4,
                    end_line: 4,
                    language: Lang::Go,
                    display_name: "B.Set".into(),
                },
            },
        ];
        assert!(narrowest_owner(&owners, 4).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn duplicate_input_lines_produce_no_duplicate_edges() {
        let dir = temp_dir("dup-input");
        let path = dir.join("sample.go");
        fs::write(
            &path,
            "package p\ntype DB struct{}\nfunc (d *DB) Apply() {}\nfunc (d *DB) Set() { d.Apply() }\n",
        )
        .unwrap();
        let mut all = inputs("hit", &path, &[3, 4]);
        all.extend(inputs("hit", &path, &[3, 4])); // duplicate inputs
        let evidence = build_owner_link_evidence(&all);
        assert_eq!(evidence.edges.len(), 1, "{:#?}", evidence.edges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deterministic_dedup_and_cap_10_edges_and_renderer_caps() {
        let dir = temp_dir("cap10");
        let path = dir.join("sample.go");
        // 12 candidate methods on the same receiver; Run calls each once so we
        // exceed the 10-edge cap with deterministic ordering.
        let mut src = String::from("package p\ntype DB struct{}");
        let mut call_lines = vec![];
        for i in 0..12 {
            let _ = write!(src, "\nfunc (d *DB) M{i}() {{ /* body */ }}");
            call_lines.push(i + 3);
        }
        // Run calls each candidate once; each M method body is hit so every M
        // is a candidate owner in the shown set.
        let _ = writeln!(src, "\nfunc (d *DB) Run() {{");
        for i in 0..12 {
            let _ = writeln!(src, "    d.M{i}()");
        }
        src.push('}');
        fs::write(&path, src).unwrap();
        let mut lines = call_lines.clone();
        for i in 0..12 {
            lines.push(15 + i);
        }
        let evidence = build_owner_link_evidence(&inputs("hit", &path, &lines));
        assert!(evidence.edges.len() > 10, "{:#?}", evidence.edges.len());
        // The builder keeps all 12 edges (deterministic); the renderer caps at
        // 10 via `.take(OWNER_LINK_EDGE_CAP)`.
        assert_eq!(evidence.edges.len(), 12, "{:#?}", evidence.edges);
        // Deterministic ordering by mechanism then path/line.
        let mut sorted = evidence.edges.clone();
        sorted.sort_by(|a, b| {
            a.mechanism
                .cmp(&b.mechanism)
                .then(a.caller.path.cmp(&b.caller.path))
                .then(a.call_line.cmp(&b.call_line))
                .then(a.caller.name.cmp(&b.caller.name))
                .then(a.candidate.path.cmp(&b.candidate.path))
                .then(a.candidate.start_line.cmp(&b.candidate.start_line))
        });
        assert_eq!(evidence.edges, sorted, "edges must already be sorted");
        // The renderer applies `.take(OWNER_LINK_EDGE_CAP)`; assert exactly 10
        // would be emitted by replicating the renderer's capping seam.
        let rendered_count = evidence.edges.iter().take(OWNER_LINK_EDGE_CAP).count();
        assert_eq!(rendered_count, OWNER_LINK_EDGE_CAP);
        assert_eq!(rendered_count, 10);
        fs::remove_dir_all(dir).unwrap();
    }
}
