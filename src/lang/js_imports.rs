//! Shared logical-import producer for JS/TS/TSX (US-055).
//!
//! The dependency consumers historically treated one physical line as one import
//! statement, so a multi-line `import { ... } from "..."` whose source specifier
//! lands on a later line was invisible to every consumer. This producer walks the
//! existing JS/TS tree-sitter grammars once and yields each static import/export /
//! `require(...)` as a logical statement carrying its source span/text and opening
//! physical line, so all consumers can share one stream.
//!
//! Contract (per `docs/stories/US-055-multiline-import-statement-scan/design.md`):
//! - static `import`/`export ... from` statements and static `require` calls that
//!   the current extractor already accepts;
//! - a logical source span over 1 MiB, an unsupported form, or a dynamic
//!   `import(...)` abstains instead of guessing;
//! - parser errors are handled conservatively: statements inside ERROR/missing
//!   spans abstain, while trusted statements outside those spans may still emit
//!   evidence; grammar forms represented as ERROR remain intentionally fail-closed;
//! - non-JS/TS input yields nothing here and keeps the physical-line stream.

use crate::lang::outline::outline_language;
use crate::types::Lang;

/// 1 MiB logical source-span bound, in bytes. A statement whose source span
/// exceeds this abstains (defensive bound for minified/artifact-ish input).
const MAX_LOGICAL_SPAN_BYTES: usize = 1024 * 1024;

/// A single logical import statement extracted from a tree-sitter span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogicalImport {
    /// The unquoted source specifier (e.g. `./foo`, `semver`).
    pub source: String,
    /// The full logical statement text to feed the existing provider
    /// (`outline::extract_import_source`). For `import`/`export` this is the
    /// whole statement; for `require` it is the `require(...)` call text.
    pub statement: String,
    /// 1-based physical opening line used for evidence attribution.
    pub opening_line: usize,
}

/// Ordered `(source, opening_line)` stream for JS/TS/TSX, parsed once per file.
/// This is the single shared stream all dependency consumers consume so they
/// cannot disagree. Empty for non-JS/TS (they keep the physical-line stream).
pub(crate) fn logical_sources(content: &str, lang: Lang) -> Vec<(String, usize)> {
    logical_imports(content, lang)
        .into_iter()
        .map(|i| (i.source, i.opening_line))
        .collect()
}

/// Yield logical import statements for JS/TS/TSX. Returns empty for other
/// languages (they keep the physical-line stream).
///
/// Parser-error remediation (R1): a tree containing an ERROR node no longer
/// discards the whole file. Clean trees keep the exact current behavior. For
/// trees with ERROR, statements whose subtree is NOT inside an ERROR node are
/// still yielded, and the historical physical-line scan is merged in as a
/// fallback only outside ERROR/missing line spans, so valid imports after (or
/// around) a syntax error keep their evidence while malformed candidates still
/// abstain. The merge is deduplicated on `(source, opening_line)` before the
/// single shared stream is returned.
pub(crate) fn logical_imports(content: &str, lang: Lang) -> Vec<LogicalImport> {
    if !is_js_like(lang) {
        return Vec::new();
    }
    let Some(ts_lang) = outline_language(lang) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };

    let root = tree.root_node();
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        if !node.is_named() {
            continue;
        }
        // Skip statements whose own subtree contains an ERROR node (R1): their
        // structure is untrustworthy. Sibling ERROR nodes do not disqualify.
        if node.has_error() {
            continue;
        }
        match node.kind() {
            "import_statement" | "export_statement" => {
                if let Some(imp) = statement_import(node, content) {
                    out.push(imp);
                }
            }
            _ => collect_require_calls(node, content, &mut out),
        }
    }

    // Parser-error fallback (R1): when the tree has any ERROR node, merge the
    // historical physical-line scan so valid single-line imports around the
    // syntax error keep their evidence (deduplicated on source+opening line).
    // Clean trees never reach this path, preserving existing behavior. The
    // union is re-sorted by opening line: fallback-only entries (e.g. an early
    // `obj.require(...)` line the AST does not yield) must not trail a later
    // AST import.
    if root.has_error() {
        let mut error_line_spans = Vec::new();
        collect_error_line_spans(root, &mut error_line_spans);
        merge_physical_line_scan(&mut out, content, lang, &error_line_spans);
        out.sort_by_key(|imp| imp.opening_line); // stable: keeps sibling order on the same line
    }
    out
}

/// Collect physical line spans occupied by parser ERROR/missing nodes. The
/// fallback must not reinterpret a malformed candidate inside one of these
/// spans (R1); valid lines outside the error subtree remain eligible.
fn collect_error_line_spans(node: tree_sitter::Node, out: &mut Vec<(usize, usize)>) {
    if node.kind() == "ERROR" || node.is_missing() {
        out.push((node.start_position().row + 1, node.end_position().row + 1));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_error_line_spans(child, out);
    }
}

fn line_is_in_error_span(line: usize, spans: &[(usize, usize)]) -> bool {
    spans
        .iter()
        .any(|(start, end)| (*start..=*end).contains(&line))
}

/// Merge the historical physical-line JS/TS/TSX scan into `out`, keeping the
/// shared stream deduplicated on `(source, opening_line)`. Reuses the existing
/// `is_import_line` / `extract_import_source` contract (read/imports.rs,
/// outline.rs) — no new line-scan behavior is invented here. Lines inside an
/// ERROR/missing span are excluded so malformed candidates still abstain.
fn merge_physical_line_scan(
    out: &mut Vec<LogicalImport>,
    content: &str,
    lang: Lang,
    error_line_spans: &[(usize, usize)],
) {
    // Dedupe the AST portion as well as fallback additions. Keep the first AST
    // occurrence deterministically, then append only unseen physical evidence.
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert((i.source.clone(), i.opening_line)));
    for (idx, line) in content.lines().enumerate() {
        let opening_line = idx + 1;
        if line_is_in_error_span(opening_line, error_line_spans)
            || !crate::read::imports::is_import_line(line, lang)
        {
            continue;
        }
        let source = crate::lang::outline::extract_import_source(line, Some(lang));
        if source.is_empty() {
            continue;
        }
        if seen.insert((source.clone(), opening_line)) {
            out.push(LogicalImport {
                source,
                // Fallback entries are physical-line evidence (Text-level):
                // the statement is the line itself.
                statement: line.to_string(),
                opening_line,
            });
        }
    }
}

fn is_js_like(lang: Lang) -> bool {
    matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript)
}

/// Build a `LogicalImport` for an `import_statement` / `export_statement` node
/// that has a `source` field (`... from "..."`). `export { x }` without a
/// `source` field yields nothing. TS `import x = require("./m")` carries its
/// source on the `import_require_clause` child, so the search covers that too.
fn statement_import(node: tree_sitter::Node, content: &str) -> Option<LogicalImport> {
    let source_node = find_source_field(node)?;
    let source = string_literal_value(source_node, content)?;
    span_text(node, content).map(|statement| LogicalImport {
        source,
        statement,
        opening_line: node.start_position().row + 1,
    })
}

/// Find a `source` field on `node` or on one of its named clause children
/// (e.g. `import_require_clause` for TS `import = require`).
fn find_source_field(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    if let Some(source) = node.child_by_field_name("source") {
        return Some(source);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            if let Some(source) = child.child_by_field_name("source") {
                return Some(source);
            }
        }
    }
    None
}

/// Recursively find static `require("...")` call expressions and emit one
/// `LogicalImport` per call. Dynamic `import("...")` is not a `require`, so it
/// is never matched here (function field is the `import` keyword node).
fn collect_require_calls(node: tree_sitter::Node, content: &str, out: &mut Vec<LogicalImport>) {
    if node.kind() == "call_expression" {
        if let Some(imp) = require_import(node, content) {
            out.push(imp);
            return; // do not descend into the require call / its arguments
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_require_calls(child, content, out);
        }
    }
}

/// Build a `LogicalImport` for a static `require("...")` call expression.
fn require_import(node: tree_sitter::Node, content: &str) -> Option<LogicalImport> {
    let function = node.child_by_field_name("function")?;
    if function.kind() != "identifier" || node_text(function, content)? != "require" {
        return None;
    }
    let source = first_string_argument(node, content)?;
    span_text(node, content).map(|statement| LogicalImport {
        source,
        statement,
        // Anchor to the call's own line (matches the current single-line scan).
        opening_line: node.start_position().row + 1,
    })
}

/// First `string` literal argument of a `call_expression`'s `arguments` node.
fn first_string_argument(call: tree_sitter::Node, content: &str) -> Option<String> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    for arg in arguments.children(&mut cursor) {
        if arg.is_named() && arg.kind() == "string" {
            return string_literal_value(arg, content);
        }
    }
    None
}

/// Extract the unquoted value of a `string` literal node via its
/// `string_fragment` child. Only `"..."` / `'...'` strings are handled
/// (matching the current provider); template strings are unsupported.
///
/// Conservative on escapes (R2 remediation): a string containing an
/// `escape_sequence` child (e.g. `"./tar\\u0067et"`) is NOT decoded here —
/// decoding only the first fragment would fabricate a truncated source — so
/// the candidate abstains instead of emitting a made-up specifier. Ordinary
/// strings (single fragment) and the empty string `""` are preserved.
fn string_literal_value(string_node: tree_sitter::Node, content: &str) -> Option<String> {
    let mut cursor = string_node.walk();
    let mut fragment: Option<String> = None;
    for child in string_node.children(&mut cursor) {
        // Stay conservative on escapes: never emit a truncated/un-decoded
        // prefix as the source specifier.
        if child.kind() == "escape_sequence" {
            return None;
        }
        if child.kind() == "string_fragment" && fragment.is_none() {
            fragment = node_text(child, content);
        }
    }
    // Empty string literal (`""`) has no fragment child.
    Some(fragment.unwrap_or_default())
}

/// Full text of a node as a String, provided it is within the 1 MiB bound.
fn span_text(node: tree_sitter::Node, content: &str) -> Option<String> {
    if node.end_byte() - node.start_byte() > MAX_LOGICAL_SPAN_BYTES {
        return None; // abstain on over-bound spans
    }
    node_text(node, content)
}

fn node_text(node: tree_sitter::Node, content: &str) -> Option<String> {
    node.utf8_text(content.as_bytes()).ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(content: &str, lang: Lang) -> Vec<(String, usize)> {
        logical_imports(content, lang)
            .into_iter()
            .map(|i| (i.source, i.opening_line))
            .collect()
    }

    #[test]
    fn one_line_named_default_side_effect_imports() {
        let code = r#"
import { a, b } from "./foo";
import def from "./bar";
import "./side";
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![
                ("./foo".to_string(), 2),
                ("./bar".to_string(), 3),
                ("./side".to_string(), 4),
            ]
        );
    }

    #[test]
    fn multi_line_imports_attributed_to_opening_line() {
        let code = r#"
import {
  x,
  y as z
} from "./multi";
"#;
        // Opening line is the line with `import` (1-based), not the source line.
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./multi".to_string(), 2)]
        );
    }

    #[test]
    fn export_from_is_captured() {
        let code = r#"
export { thing } from "./exported";
export * from "./star";
export { default } from "./defexp";
export const local = 1;
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![
                ("./exported".to_string(), 2),
                ("./star".to_string(), 3),
                ("./defexp".to_string(), 4),
            ]
        );
    }

    #[test]
    fn static_require_is_captured() {
        let code = r#"
const k = require("./req");
const both = require("./a") + require("./b");
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![
                ("./req".to_string(), 2),
                ("./a".to_string(), 3),
                ("./b".to_string(), 3),
            ]
        );
    }

    #[test]
    fn comments_and_strings_are_not_imports() {
        let code = r#"
// import "from-comment";
/* import "block";
   import "still-block"; */
const s = "import \"./not-real\";";
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn adjacent_statements_do_not_concatenate() {
        // A statement ending in a string adjacent to an import must not merge.
        let code = r#"
import "./a";
const msg = "hello";
import "./b";
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./a".to_string(), 2), ("./b".to_string(), 4)]
        );
    }

    #[test]
    fn dynamic_import_is_not_captured() {
        let code = r#"
const d = import("./dyn");
const e = import("./dyn-two");
"#;
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn malformed_input_candidate_abstains() {
        // R1: a malformed candidate inside the parser ERROR span remains
        // absent; the fallback must not reinterpret its physical line.
        let code = "import { from \"./broken\"\nconst x = 1;\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn malformed_then_valid_import_keeps_valid_evidence() {
        // R1: a valid import AFTER a syntax error is kept — the statement's own
        // subtree has no ERROR node, and the physical-line fallback also
        // recovers it. The malformed statement itself stays absent.
        let code = "const broken = ;\nimport { a } from \"./after-error\";\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./after-error".to_string(), 2)]
        );
    }

    #[test]
    fn unsupported_template_string_import_abstains() {
        // Template literals are not handled by the current provider.
        let code = "import x from `./tpl`;\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn non_js_language_yields_nothing() {
        assert_eq!(logical_imports("use foo::bar;\n", Lang::Rust), Vec::new());
        assert_eq!(
            logical_imports("from x import y\n", Lang::Python),
            Vec::new()
        );
    }

    #[test]
    fn over_one_mib_span_abstains() {
        // A multi-line import whose total span exceeds 1 MiB must abstain
        // (no panic, no entry).
        let mut code = String::from("import {\n");
        for _ in 0..(1_200_000 / 8) {
            code.push_str("  aaaaaaaa,\n");
        }
        code.push_str("} from \"./huge\";\n");
        assert_eq!(
            sources(&code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn opening_line_attribution_for_nested_require() {
        // require inside a deeper statement still anchors to the require line.
        let code = "const obj = {\n  dep: require(\"./nested\")\n};\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./nested".to_string(), 2)]
        );
    }

    #[test]
    fn tsx_and_typescript_work() {
        let code = "import { Component } from \"./component\";\n";
        assert_eq!(
            sources(code, Lang::TypeScript),
            vec![("./component".to_string(), 1)]
        );
        assert_eq!(
            sources(code, Lang::Tsx),
            vec![("./component".to_string(), 1)]
        );
    }

    #[test]
    fn ts_import_equals_require_is_captured() {
        // `import x = require("./m")` has no `source` field but carries a static
        // require; parity with the current single-line extractor.
        let code = "import def = require(\"./def\");\n";
        assert_eq!(
            sources(code, Lang::TypeScript),
            vec![("./def".to_string(), 1)]
        );
    }

    #[test]
    fn ts_import_type_and_export_type_from_are_captured() {
        let code = "import type { A } from \"./types\";\nexport type { B } from \"./bt\";\n";
        assert_eq!(
            sources(code, Lang::TypeScript),
            vec![("./types".to_string(), 1), ("./bt".to_string(), 2)]
        );
    }

    #[test]
    fn statement_text_is_whole_logical_statement() {
        let code = "import {\n  x,\n  y\n} from \"./multi\";\n";
        let imports = logical_imports(code, Lang::JavaScript);
        assert_eq!(imports.len(), 1);
        assert!(imports[0].statement.contains("import {"));
        assert!(imports[0].statement.contains("from \"./multi\""));
        // The statement carries the full logical span, not just one line.
        assert!(imports[0].statement.contains('}'));
    }

    #[test]
    fn physical_line_scan_recovered_outside_error_spans() {
        // R1 invariant for valid candidates outside ERROR spans: every
        // (source, line) the historical physical-line scan catches must be
        // present in the shared stream (dedupe keeps both). Malformed lines are
        // intentionally excluded by the fail-closed contract.
        let code = "import { a } from \"./good\";\nconst broken = ;\nrequire(\"./req\");\n";
        let stream = sources(code, Lang::JavaScript);
        // Physical-line scan of the same file.
        for (idx, line) in code.lines().enumerate() {
            if crate::read::imports::is_import_line(line, Lang::JavaScript) {
                let src = crate::lang::outline::extract_import_source(line, Some(Lang::JavaScript));
                assert!(
                    !src.is_empty() && stream.contains(&(src.clone(), idx + 1)),
                    "physical-line source {src:?} at line {} missing from stream: {stream:?}",
                    idx + 1
                );
            }
        }
        // And the stream only contains real import evidence.
        assert_eq!(
            stream,
            vec![("./good".to_string(), 1), ("./req".to_string(), 3),]
        );
    }

    #[test]
    fn error_at_end_of_file_keeps_leading_imports() {
        // Fixture: parser error at the END of the file, valid imports at the
        // top — the imports must survive (R1).
        let code = "import { x } from \"./a\";\nexport * from \"./b\";\nconst broken = ;\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./a".to_string(), 1), ("./b".to_string(), 2),]
        );
    }

    #[test]
    fn escaped_import_string_abstains_no_truncation() {
        // R2: `"./tar\u0067et"` must NOT emit `./tar`. Escape sequences are
        // not decoded, so the candidate abstains (no fabricated source).
        let code = "import a from \"./tar\\u0067et\";\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn escaped_require_string_abstains_no_truncation() {
        // R2: same for require: `"./tar\u0067et"` must not become `./tar`.
        let code = "const r = require(\"./tar\\u0067et\");\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            Vec::<(String, usize)>::new()
        );
    }

    #[test]
    fn escape_sequence_in_middle_does_not_affect_other_imports() {
        // R2: an escaped candidate abstains; a clean sibling still yields.
        let code = "import a from \"./tar\\u0067et\";\nimport b from \"./ok\";\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./ok".to_string(), 2)]
        );
    }

    #[test]
    fn empty_string_and_ordinary_string_preserved() {
        // R2: ordinary strings keep working; the empty string stays empty
        // (matches provider contract: no fragment -> empty source).
        let code = "import a from \"./ok\";\nimport b from \"\";\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./ok".to_string(), 1), (String::new(), 2)]
        );
    }

    #[test]
    fn error_path_stream_ordered_by_opening_line() {
        // R1 ordering: a fallback-only single-line require on an EARLY line must
        // appear before a later valid AST import (no append-trailing artifact).
        // `obj.require("./early")` is not an AST logical import (receiver-scoped,
        // like the current provider), but the physical-line scan catches it.
        let code = "obj.require(\"./early\");\nconst broken = ;\nimport { a } from \"./late\";\n";
        assert_eq!(
            sources(code, Lang::JavaScript),
            vec![("./early".to_string(), 1), ("./late".to_string(), 3),]
        );
    }
}
