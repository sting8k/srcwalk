mod artifact;

pub use artifact::{
    extract_call_sites_for_target as extract_call_sites_for_artifact_target,
    extract_callee_names_for_target as extract_callee_names_for_artifact_target,
    resolve_same_file as resolve_callees_same_file_artifact,
};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use streaming_iterator::StreamingIterator;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::lang::outline::{get_outline_entries, outline_language};
use crate::types::{Lang, OutlineEntry};

/// A resolved callee: a function/method called from within an expanded definition.
#[derive(Debug)]
pub struct ResolvedCallee {
    pub name: String,
    pub file: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Option<String>,
}

/// A call site with contextual information: arguments, return variable, line.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub line: u32,
    /// Captured callee name, e.g. `parse_hubs` or `startAnalysis`.
    pub callee: String,
    /// Full call text, e.g. `parse_hubs(skip_hubs)`
    pub call_text: String,
    /// Text before the call's argument list, e.g. `client.fetch`.
    pub call_prefix: Option<String>,
    /// Top-level argument snippets, ordered as written at the call site.
    pub args: Vec<String>,
    /// Variable the return value is assigned to, if any.
    pub return_var: Option<String>,
    /// True if this call is the direct return expression of the function.
    pub is_return: bool,
    /// Exact byte range of the enclosing call expression, for artifact byte-window evidence.
    pub call_byte_range: Option<(usize, usize)>,
}

/// A resolved callee with its own callees (2nd hop).
#[derive(Debug)]
pub struct ResolvedCalleeNode {
    pub callee: ResolvedCallee,
    /// 2nd-hop callees resolved from within this callee's body.
    pub children: Vec<ResolvedCallee>,
}

/// Return the tree-sitter query string for extracting callee names in the given language.
/// Each language has patterns targeting `@callee` captures on call-like expressions.
pub(crate) fn callee_query_str(lang: Lang) -> Option<&'static str> {
    match lang {
        Lang::Rust => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (field_expression field: (field_identifier) @callee))\n",
            "(call_expression function: (scoped_identifier name: (identifier) @callee))\n",
            "(macro_invocation macro: (identifier) @callee)\n",
            // Type references: struct literals, generics
            "(struct_expression name: (type_identifier) @callee)\n",
            "(type_arguments (type_identifier) @callee)\n",
        )),
        Lang::Go => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (selector_expression field: (field_identifier) @callee))\n",
            // Type references: composite literals (ProgramDB{...})
            "(composite_literal type: (type_identifier) @callee)\n",
            "(composite_literal type: (qualified_type name: (type_identifier) @callee))\n",
        )),
        Lang::Python => Some(concat!(
            "(call function: (identifier) @callee)\n",
            "(call function: (attribute attribute: (identifier) @callee))\n",
            // class Foo(Base) — superclass
            "(class_definition superclasses: (argument_list (identifier) @callee))\n",
        )),
        Lang::JavaScript => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (member_expression property: (property_identifier) @callee))\n",
            // new Foo()
            "(new_expression constructor: (identifier) @callee)\n",
            // class Foo extends Bar
            "(class_heritage (identifier) @callee)\n",
        )),
        Lang::TypeScript | Lang::Tsx => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (member_expression property: (property_identifier) @callee))\n",
            // new Foo()
            "(new_expression constructor: (identifier) @callee)\n",
            // extends / implements
            "(extends_clause value: (identifier) @callee)\n",
        )),
        Lang::Java => Some(concat!(
            "(method_invocation name: (identifier) @callee)\n",
            // new ProgramDB()
            "(object_creation_expression type: (type_identifier) @callee)\n",
            // extends ProgramDB
            "(superclass (type_identifier) @callee)\n",
            // implements X, Y
            "(super_interfaces (type_list (type_identifier) @callee))\n",
        )),
        Lang::Scala => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (field_expression field: (identifier) @callee))\n",
            "(infix_expression operator: (identifier) @callee)\n",
        )),
        Lang::C | Lang::Cpp => Some(concat!(
            "(call_expression function: (identifier) @callee)\n",
            "(call_expression function: (field_expression field: (field_identifier) @callee))\n",
        )),
        Lang::Ruby => Some(
            "(call method: (identifier) @callee)\n",
        ),
        Lang::Php => Some(concat!(
            "(function_call_expression function: (name) @callee)\n",
            "(function_call_expression function: (qualified_name) @callee)\n",
            "(function_call_expression function: (relative_name) @callee)\n",
            "(member_call_expression name: (name) @callee)\n",
            "(nullsafe_member_call_expression name: (name) @callee)\n",
            "(scoped_call_expression name: (name) @callee)\n",
            // new Foo()
            "(object_creation_expression (name) @callee)\n",
            "(object_creation_expression (qualified_name) @callee)\n",
        )),
        Lang::CSharp => Some(concat!(
            "(invocation_expression function: (identifier) @callee)\n",
            "(invocation_expression function: (member_access_expression name: (identifier) @callee))\n",
            "(invocation_expression function: (conditional_access_expression (member_binding_expression name: (identifier) @callee)))\n",
            // new ProgramDB()
            "(object_creation_expression (identifier) @callee)\n",
            // : BaseService, IDisposable
            "(base_list (identifier) @callee)\n",
            // <ProgramDB>
            "(type_argument_list (identifier) @callee)\n",
        )),
        Lang::Swift => Some(concat!(
            "(call_expression (simple_identifier) @callee)\n",
            "(call_expression (navigation_expression suffix: (navigation_suffix suffix: (simple_identifier) @callee)))\n",
        )),
        Lang::Kotlin => Some(concat!(
            "(call_expression (identifier) @callee)\n",
            "(call_expression (navigation_expression (identifier) @callee .))\n",
        )),
        Lang::Elixir => Some(concat!(
            "(call target: (identifier) @callee)\n",
            "(call target: (dot right: (identifier) @callee))\n",
        )),
        _ => None,
    }
}

/// Global cache of compiled tree-sitter queries for callee extraction.
///
/// Keyed by `(symbol_count, field_count)` — a pair that uniquely identifies
/// each grammar in practice. We avoid keying by `Language::name()` because
/// older grammars (ABI < 15) do not register a name and would return `None`,
/// silently disabling the cache and callee extraction entirely.
///
/// `Query` is `Send + Sync` in tree-sitter 0.25, so a global `Mutex`-guarded
/// map is safe and avoids recompiling the same query on every call.
static QUERY_CACHE: LazyLock<Mutex<HashMap<(usize, usize), tree_sitter::Query>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stable cache key for a tree-sitter language. Uses `(symbol_count,
/// field_count)` which is unique for every grammar shipped with srcwalk.
fn lang_cache_key(ts_lang: &tree_sitter::Language) -> (usize, usize) {
    (ts_lang.node_kind_count(), ts_lang.field_count())
}

/// Look up or compile the callee query for `ts_lang`, then invoke `f` with a
/// reference to the cached `Query`.  Returns `None` if compilation fails.
pub(super) fn with_callee_query<R>(
    ts_lang: &tree_sitter::Language,
    query_str: &str,
    f: impl FnOnce(&tree_sitter::Query) -> R,
) -> Option<R> {
    let key = lang_cache_key(ts_lang);
    let mut cache = QUERY_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
        let query = tree_sitter::Query::new(ts_lang, query_str).ok()?;
        e.insert(query);
    }
    // Safety: we just inserted if absent, so the key is always present here.
    Some(f(cache.get(&key).expect("just inserted")))
}

/// Extract names of functions/methods called within a given line range.
/// Uses tree-sitter query patterns to find call expressions.
///
/// If `def_range` is `Some((start, end))`, only callees whose match position
/// falls within lines `start..=end` (1-indexed) are returned.
/// Returns a deduplicated, sorted list of callee names.
pub fn extract_callee_names(
    content: &str,
    lang: Lang,
    def_range: Option<(u32, u32)>,
) -> Vec<String> {
    let Some(ts_lang) = outline_language(lang) else {
        return Vec::new();
    };

    let Some(query_str) = callee_query_str(lang) else {
        return Vec::new();
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return Vec::new();
    }

    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };

    let content_bytes = content.as_bytes();

    let Some(names) = with_callee_query(&ts_lang, query_str, |query| {
        let Some(callee_idx) = query.capture_index_for_name("callee") else {
            return Vec::new();
        };

        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content_bytes);
        let mut names: Vec<String> = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != callee_idx {
                    continue;
                }

                // 1-indexed line number of the capture
                let line = cap.node.start_position().row as u32 + 1;

                // Filter by def_range if provided
                if let Some((start, end)) = def_range {
                    if line < start || line > end {
                        continue;
                    }
                }

                if let Ok(text) = cap.node.utf8_text(content_bytes) {
                    names.push(text.to_string());
                }
            }
        }

        names
    }) else {
        return Vec::new();
    };

    let mut names = names;
    names.sort();
    names.dedup();

    // Elixir: the callee query `(call target: (identifier) @callee)` also captures
    // definition keywords (def, defmodule, etc.) and import keywords (use, import,
    // alias, require) since those are all `call` nodes. Filter them out.
    if lang == Lang::Elixir {
        names.retain(|n| !is_elixir_keyword(n));
    }

    names
}

/// Extract detailed call sites from a function body, ordered by line.
/// Walks up from each `@callee` capture to find the enclosing call expression,
/// assignment context, and return-expression status.
pub fn extract_call_sites(
    content: &str,
    lang: Lang,
    def_range: Option<(u32, u32)>,
) -> Vec<CallSite> {
    extract_call_sites_scoped(content, lang, def_range, None)
}

pub fn extract_call_sites_in_byte_range(
    content: &str,
    lang: Lang,
    start_byte: usize,
    end_byte: usize,
) -> Vec<CallSite> {
    extract_call_sites_scoped(content, lang, None, Some((start_byte, end_byte)))
}

fn extract_call_sites_scoped(
    content: &str,
    lang: Lang,
    def_range: Option<(u32, u32)>,
    byte_range: Option<(usize, usize)>,
) -> Vec<CallSite> {
    let Some(ts_lang) = outline_language(lang) else {
        return Vec::new();
    };
    let Some(query_str) = callee_query_str(lang) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let content_bytes = content.as_bytes();

    let sites = with_callee_query(&ts_lang, query_str, |query| {
        let Some(callee_idx) = query.capture_index_for_name("callee") else {
            return (Vec::new(), Vec::new());
        };
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), content_bytes);
        let mut sites: Vec<CallSite> = Vec::new();
        let mut call_ranges: Vec<(usize, usize)> = Vec::new();

        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != callee_idx {
                    continue;
                }
                let line = cap.node.start_position().row as u32 + 1;
                if let Some((start_byte, end_byte)) = byte_range {
                    if cap.node.start_byte() < start_byte || cap.node.start_byte() >= end_byte {
                        continue;
                    }
                } else if let Some((start, end)) = def_range {
                    if line < start || line > end {
                        continue;
                    }
                }
                let name = match cap.node.utf8_text(content_bytes) {
                    Ok(t) => t.to_string(),
                    Err(_) => continue,
                };
                if lang == Lang::Elixir && is_elixir_keyword(&name) {
                    continue;
                }

                let call_node = find_call_ancestor(cap.node);
                // Skip if we didn't find a real call expression — e.g. type params.
                let ck = call_node.kind();
                if !ck.contains("call")
                    && !ck.contains("invocation")
                    && !ck.contains("creation")
                    && !ck.contains("macro")
                {
                    continue;
                }
                let range = (call_node.start_byte(), call_node.end_byte());
                call_ranges.push(range);

                let call_text = call_node
                    .utf8_text(content_bytes)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();

                let (call_prefix, args) = extract_argument_info(call_node, content_bytes);
                let (return_var, is_return) = find_assignment_context(call_node, content_bytes);

                sites.push(CallSite {
                    line,
                    callee: name,
                    call_text,
                    call_prefix,
                    args,
                    return_var,
                    is_return,
                    call_byte_range: Some(range),
                });
            }
        }
        (sites, call_ranges)
    })
    .unwrap_or_default();

    let (sites, call_ranges) = sites;
    let mut sites = sites;
    if sites.len() > 1 {
        let keep: Vec<bool> = (0..sites.len())
            .map(|i| {
                let (start_i, end_i) = call_ranges[i];
                !call_ranges
                    .iter()
                    .enumerate()
                    .any(|(j, &(start_j, end_j))| {
                        j != i
                            && start_j <= start_i
                            && end_j >= end_i
                            && (start_j < start_i || end_j > end_i)
                    })
            })
            .collect();
        let mut idx = 0;
        sites.retain(|_| {
            let k = keep[idx];
            idx += 1;
            k
        });
    }
    sites.sort_by_key(|s| s.line);
    // Dedup same line + same call_text (method chains can produce duplicates).
    sites.dedup_by(|a, b| a.line == b.line && a.call_text == b.call_text);
    sites
}

pub fn filter_call_sites(
    sites: Vec<CallSite>,
    filter: Option<&str>,
) -> Result<Vec<CallSite>, SrcwalkError> {
    let Some(filter) = filter else {
        return Ok(sites);
    };

    let mut callee_filters = Vec::new();
    for part in filter.split_whitespace() {
        let Some((field, value)) = part.split_once(':') else {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filters must use field:value qualifiers".to_string(),
            });
        };
        if field.is_empty() || value.is_empty() {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filter field and value cannot be empty".to_string(),
            });
        }
        match field {
            "callee" => callee_filters.push(value.to_string()),
            _ => {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: format!("unsupported callee filter field `{field}`; use callee"),
                });
            }
        }
    }

    Ok(sites
        .into_iter()
        .filter(|site| callee_filters.iter().all(|wanted| site.callee == *wanted))
        .collect())
}

fn extract_argument_info(
    call_node: tree_sitter::Node,
    content_bytes: &[u8],
) -> (Option<String>, Vec<String>) {
    let Some(args_node) = call_node
        .child_by_field_name("arguments")
        .or_else(|| direct_argument_list_child(call_node))
    else {
        return (None, Vec::new());
    };

    let call_prefix = content_bytes
        .get(call_node.start_byte()..args_node.start_byte())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(compact_call_prefix)
        .filter(|prefix| !prefix.is_empty());

    let mut cursor = args_node.walk();
    let args = args_node
        .named_children(&mut cursor)
        .filter_map(|arg| arg.utf8_text(content_bytes).ok())
        .map(compact_whitespace)
        .filter(|arg| !arg.is_empty())
        .collect();

    (call_prefix, args)
}

fn compact_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_call_prefix(prefix: &str) -> String {
    compact_whitespace(prefix)
        .replace(" .", ".")
        .replace(". ", ".")
        .replace(" ::", "::")
        .replace(":: ", "::")
        .replace(" ->", "->")
        .replace("-> ", "->")
}

fn direct_argument_list_child(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = node.walk();
    let child = node
        .children(&mut cursor)
        .find(|child| child.kind().contains("argument"));
    child
}

/// Walk up from `@callee` name node to the enclosing call expression.
fn find_call_ancestor(node: tree_sitter::Node) -> tree_sitter::Node {
    let mut cur = node;
    for _ in 0..5 {
        if let Some(p) = cur.parent() {
            let k = p.kind();
            if k.contains("call")
                || k.contains("invocation")
                || k.contains("creation")
                || k.contains("macro_invocation")
            {
                return p;
            }
            cur = p;
        } else {
            break;
        }
    }
    node.parent().unwrap_or(node)
}

/// From a call expression node, check immediate parent/grandparent for
/// assignment or return context. Max 2 levels — avoids per-lang heuristic mess.
fn find_assignment_context(call_node: tree_sitter::Node, content: &[u8]) -> (Option<String>, bool) {
    for ancestor in [
        call_node.parent(),
        call_node.parent().and_then(|p| p.parent()),
    ] {
        let Some(p) = ancestor else { continue };
        let k = p.kind();

        // Return expression.
        if k == "return_statement" || k == "return_expression" {
            return (None, true);
        }

        // Assignment / variable declaration — extract LHS.
        // Skip function/class/type definitions that happen to contain "declaration".
        if (k.contains("assignment")
            || k == "variable_declarator"
            || k == "let_declaration"
            || k == "short_var_declaration"
            || k == "lexical_declaration"
            || k == "local_variable_declaration"
            || k == "declaration")
            && !k.contains("function")
            && !k.contains("method")
            && !k.contains("class")
            && !k.contains("struct")
            && !k.contains("enum")
            && !k.contains("protocol")
            && !k.contains("interface")
            && !k.contains("trait")
            && !k.contains("impl")
        {
            let lhs = p
                .child_by_field_name("name")
                .or_else(|| p.child_by_field_name("pattern"))
                .or_else(|| p.child_by_field_name("left"))
                .or_else(|| p.named_child(0));
            if let Some(lhs_node) = lhs {
                if lhs_node.id() != call_node.id() {
                    if let Ok(text) = lhs_node.utf8_text(content) {
                        let text = text.trim();
                        if !text.is_empty() && text.len() < 60 {
                            return (Some(text.to_string()), false);
                        }
                    }
                }
            }
        }
    }

    // Implicit return: last expression in block (Rust/Ruby/Elixir).
    if let Some(p) = call_node.parent() {
        if p.kind() == "block" || p.kind() == "do_block" || p.kind() == "body_statement" {
            if let Some(last) = p.named_child(p.named_child_count().saturating_sub(1)) {
                if last.id() == call_node.id() {
                    return (None, true);
                }
            }
        }
    }

    (None, false)
}

/// Keywords that should not appear as callee names in Elixir.
/// These are definition and import forms that are syntactically `call` nodes.
/// Superset of `ELIXIR_DEFINITION_TARGETS` (treesitter.rs) plus import keywords
/// (`use`, `import`, `alias`, `require`) and `defoverridable`.
fn is_elixir_keyword(name: &str) -> bool {
    matches!(
        name,
        "def"
            | "defp"
            | "defmodule"
            | "defmacro"
            | "defmacrop"
            | "defguard"
            | "defguardp"
            | "defdelegate"
            | "defstruct"
            | "defexception"
            | "defprotocol"
            | "defimpl"
            | "defoverridable"
            | "use"
            | "import"
            | "alias"
            | "require"
    )
}

fn outline_remaining_match(
    entry: &OutlineEntry,
    remaining: &mut std::collections::HashSet<&str>,
) -> Option<String> {
    if remaining.remove(entry.name.as_str()) {
        return Some(entry.name.clone());
    }
    if let Some(signature) = entry.signature.as_deref() {
        if remaining.remove(signature) {
            return Some(signature.to_string());
        }
    }
    None
}

/// Match callee names against outline entries, moving resolved names out of `remaining`.
fn resolve_from_entries(
    entries: &[OutlineEntry],
    file_path: &Path,
    remaining: &mut std::collections::HashSet<&str>,
    resolved: &mut Vec<ResolvedCallee>,
) {
    for entry in entries {
        // Check top-level entry name
        if let Some(matched) = outline_remaining_match(entry, remaining) {
            resolved.push(ResolvedCallee {
                name: matched,
                file: file_path.to_path_buf(),
                start_line: entry.start_line,
                end_line: entry.end_line,
                signature: entry.signature.clone(),
            });
        }

        // Check children (methods in classes/impl blocks)
        for child in &entry.children {
            if let Some(matched) = outline_remaining_match(child, remaining) {
                resolved.push(ResolvedCallee {
                    name: matched,
                    file: file_path.to_path_buf(),
                    start_line: child.start_line,
                    end_line: child.end_line,
                    signature: child.signature.clone(),
                });
            }
        }

        if remaining.is_empty() {
            return;
        }
    }
}

pub fn resolve_callees_same_file(
    callee_names: &[String],
    source_path: &Path,
    source_content: &str,
    lang: Lang,
) -> Vec<ResolvedCallee> {
    if callee_names.is_empty() {
        return Vec::new();
    }
    let mut remaining: std::collections::HashSet<&str> =
        callee_names.iter().map(String::as_str).collect();
    let mut resolved = Vec::new();
    let entries = get_outline_entries(source_content, lang);
    resolve_from_entries(&entries, source_path, &mut remaining, &mut resolved);
    resolved
}

/// Resolve callee names to their definition locations.
///
/// Strategy: check the source file's own outline first (cheapest), then scan
/// imported files resolved from the source's import statements.
pub fn resolve_callees(
    callee_names: &[String],
    source_path: &Path,
    source_content: &str,
    _cache: &OutlineCache,
    bloom: &crate::index::bloom::BloomFilterCache,
) -> Vec<ResolvedCallee> {
    if callee_names.is_empty() {
        return Vec::new();
    }

    let file_type = crate::lang::detect_file_type(source_path);
    let crate::types::FileType::Code(lang) = file_type else {
        return Vec::new();
    };

    let mut remaining: std::collections::HashSet<&str> =
        callee_names.iter().map(String::as_str).collect();
    let mut resolved = Vec::new();

    // 1. Check source file's own outline entries
    let entries = get_outline_entries(source_content, lang);
    resolve_from_entries(&entries, source_path, &mut remaining, &mut resolved);

    if remaining.is_empty() {
        return resolved;
    }

    // 2. Check imported files
    let imported =
        crate::read::imports::resolve_related_files_with_content(source_path, source_content);

    for import_path in imported {
        if remaining.is_empty() {
            break;
        }

        // Read file content once for both bloom check and parsing
        let Ok(import_content) = std::fs::read_to_string(&import_path) else {
            continue;
        };

        // Get mtime for bloom cache
        let mtime = std::fs::metadata(&import_path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        // Bloom pre-filter: check if ANY of the remaining symbols might be in this file
        let mut might_have_any = false;
        for name in &remaining {
            if bloom.contains(&import_path, mtime, &import_content, name) {
                might_have_any = true;
                break;
            }
        }

        if !might_have_any {
            // Bloom filter says none of the symbols are in this file
            continue;
        }

        let import_type = crate::lang::detect_file_type(&import_path);
        let crate::types::FileType::Code(import_lang) = import_type else {
            continue;
        };

        let import_entries = get_outline_entries(&import_content, import_lang);
        resolve_from_entries(&import_entries, &import_path, &mut remaining, &mut resolved);
    }

    if remaining.is_empty() {
        return resolved;
    }

    // 3. For Go: scan same-directory files (same package, no explicit imports)
    if lang == Lang::Go {
        resolve_same_package(&mut remaining, &mut resolved, source_path);
    }

    resolved
}

/// Go same-package resolution: scan .go files in the same directory.
///
/// Go packages are directory-scoped — all .go files in a directory share the
/// same namespace without explicit imports. This resolves callees like
/// `safeInt8` in `context.go` that are defined in `utils.go`.
fn resolve_same_package(
    remaining: &mut std::collections::HashSet<&str>,
    resolved: &mut Vec<ResolvedCallee>,
    source_path: &Path,
) {
    const MAX_FILES: usize = 20;
    const MAX_FILE_SIZE: u64 = 100_000; // 100KB

    let Some(dir) = source_path.parent() else {
        return;
    };

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    // Collect eligible .go files, sorted for deterministic order
    let mut go_files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let path = e.path();
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            path != source_path
                && name_str.ends_with(".go")
                && !name_str.ends_with("_test.go")
                && e.metadata().is_ok_and(|m| m.len() <= MAX_FILE_SIZE)
        })
        .map(|e| e.path())
        .collect();

    go_files.sort();
    go_files.truncate(MAX_FILES);

    for go_path in go_files {
        if remaining.is_empty() {
            break;
        }

        let Ok(content) = std::fs::read_to_string(&go_path) else {
            continue;
        };

        let outline = get_outline_entries(&content, Lang::Go);
        resolve_from_entries(&outline, &go_path, remaining, resolved);
    }
}

/// Resolve callees transitively up to `depth_limit` hops with budget cap.
///
/// First hop uses `resolve_callees()` on the source content. For each resolved
/// callee at depth < `depth_limit`, reads the callee's file, extracts nested
/// callee names from the definition range, and resolves them as children.
///
/// `budget` caps the total number of 2nd-hop (child) callees across all parents.
/// Cycle detection prevents infinite loops via `(file, start_line)` tracking.
pub fn resolve_callees_transitive(
    initial_names: &[String],
    source_path: &Path,
    source_content: &str,
    cache: &OutlineCache,
    bloom: &crate::index::bloom::BloomFilterCache,
    depth_limit: u32,
    budget: usize,
) -> Vec<ResolvedCalleeNode> {
    // 1st hop: resolve direct callees (existing logic)
    let first_hop = resolve_callees(initial_names, source_path, source_content, cache, bloom);

    if depth_limit < 2 || first_hop.is_empty() {
        return first_hop
            .into_iter()
            .map(|c| ResolvedCalleeNode {
                callee: c,
                children: Vec::new(),
            })
            .collect();
    }

    // Cycle detection: track visited (file, start_line) pairs
    let mut visited: HashSet<(PathBuf, u32)> = HashSet::new();

    // Mark all 1st-hop callees as visited
    for c in &first_hop {
        visited.insert((c.file.clone(), c.start_line));
    }

    let mut budget_remaining = budget;
    let mut result = Vec::with_capacity(first_hop.len());

    for parent in first_hop {
        let children = if budget_remaining > 0 {
            resolve_second_hop(&parent, cache, bloom, &mut visited, &mut budget_remaining)
        } else {
            Vec::new()
        };
        result.push(ResolvedCalleeNode {
            callee: parent,
            children,
        });
    }

    result
}

/// Resolve 2nd-hop callees for a single parent callee.
fn resolve_second_hop(
    parent: &ResolvedCallee,
    cache: &OutlineCache,
    bloom: &crate::index::bloom::BloomFilterCache,
    visited: &mut HashSet<(PathBuf, u32)>,
    budget: &mut usize,
) -> Vec<ResolvedCallee> {
    let file_type = crate::lang::detect_file_type(&parent.file);
    let crate::types::FileType::Code(lang) = file_type else {
        return Vec::new();
    };

    let Ok(content) = std::fs::read_to_string(&parent.file) else {
        return Vec::new();
    };

    let def_range = Some((parent.start_line, parent.end_line));
    let nested_names = extract_callee_names(&content, lang, def_range);

    if nested_names.is_empty() {
        return Vec::new();
    }

    let mut resolved = resolve_callees(&nested_names, &parent.file, &content, cache, bloom);

    // Filter: skip self-recursive calls and already-visited callees
    resolved.retain(|c| {
        let key = (c.file.clone(), c.start_line);
        // Skip if same definition as parent
        if c.file == parent.file && c.start_line == parent.start_line {
            return false;
        }
        // Skip if already visited (cycle detection)
        if visited.contains(&key) {
            return false;
        }
        true
    });

    // Apply budget cap
    if resolved.len() > *budget {
        resolved.truncate(*budget);
    }

    // Mark children as visited and decrement budget
    for c in &resolved {
        visited.insert((c.file.clone(), c.start_line));
    }
    *budget = budget.saturating_sub(resolved.len());

    resolved
}

#[cfg(test)]
mod tests;
