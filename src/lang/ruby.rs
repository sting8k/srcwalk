//! Parser-backed extraction of static Ruby `require` / `require_relative`
//! references.
//!
//! Uses the tree-sitter Ruby grammar (0.23) to accept only receiverless
//! `require` / `require_relative` calls with exactly one simple literal string
//! argument, in command or parenthesized form, including modifier-condition
//! forms (`require 'x' if cond`). Dynamic, interpolated, percent, heredoc, and
//! receiver-scoped forms are conservatively rejected because their target is
//! not provably exact from the AST alone.

use tree_sitter::Node;

/// Which require form produced a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RubyRequireKind {
    Require,
    RequireRelative,
}

/// A static `require` / `require_relative` reference in Ruby source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RubyRequireRef {
    pub kind: RubyRequireKind,
    /// Exact literal content (no quotes). Escapes and interpolation are rejected
    /// by the parser, so this is never decoded from a longer token.
    pub source: String,
    /// 1-based source line of the call.
    pub line: u32,
}

/// Extract static require references from Ruby source, in source order.
pub(crate) fn require_refs(content: &str) -> Vec<RubyRequireRef> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    collect_calls(tree.root_node(), content, &mut refs);
    refs
}

fn collect_calls(node: Node, content: &str, out: &mut Vec<RubyRequireRef>) {
    if node.kind() == "call" {
        if let Some(reference) = parse_require_call(node, content) {
            out.push(reference);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls(child, content, out);
    }
}

/// Parse a single `call` node as a require reference, if it matches the strict
/// receiverless single-literal-string form.
fn parse_require_call(node: Node, content: &str) -> Option<RubyRequireRef> {
    // Receiverless only: `obj.require 'x'` is not a load edge.
    if node.child_by_field_name("receiver").is_some() {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    if method.kind() != "identifier" {
        return None;
    }
    let name = method.utf8_text(content.as_bytes()).ok()?;
    let kind = match name {
        "require" => RubyRequireKind::Require,
        "require_relative" => RubyRequireKind::RequireRelative,
        _ => return None, // load, autoload, other receivers, method defs named require
    };

    let args = node.child_by_field_name("arguments")?;
    if args.kind() != "argument_list" {
        return None;
    }
    let source = single_simple_string(args, content)?;
    let line = node.start_position().row as u32 + 1;
    Some(RubyRequireRef { kind, source, line })
}

/// Extract the exact literal content of a single `string` argument.
///
/// Accepts only an argument list containing exactly one argument node that is a
/// plain quoted `string` with a `string_content` child, no interpolation, and no
/// escape sequences. Percent/heredoc forms and multiple/dynamic arguments are
/// rejected.
fn single_simple_string(args: Node, content: &str) -> Option<String> {
    let mut cursor = args.walk();
    let mut strings = Vec::new();
    let mut other_args = 0usize;
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => {}
            "string" => strings.push(child),
            _ => other_args += 1,
        }
    }
    if strings.len() != 1 || other_args != 0 {
        return None;
    }
    let string = strings[0];

    // Reject percent literals (%q/%Q/...) and heredocs that parse as `string`.
    if content.as_bytes().get(string.start_byte()) == Some(&b'%') {
        return None;
    }

    let mut string_cursor = string.walk();
    let mut interpolation = false;
    let mut content_nodes = Vec::new();
    for child in string.children(&mut string_cursor) {
        match child.kind() {
            "interpolation" => interpolation = true,
            "string_content" => content_nodes.push(child),
            _ => {}
        }
    }
    if interpolation {
        return None;
    }
    // A single content node; empty string has none.
    if content_nodes.len() != 1 {
        return None;
    }
    let text = content_nodes[0].utf8_text(content.as_bytes()).ok()?;
    if text.is_empty() || text.contains('\\') {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(src: &str) -> Vec<RubyRequireRef> {
        require_refs(src)
    }

    #[test]
    fn accepts_command_and_parenthesized_single_quoted() {
        let out = refs("require 'json'\nrequire_relative './foo'\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, RubyRequireKind::Require);
        assert_eq!(out[0].source, "json");
        assert_eq!(out[0].line, 1);
        assert_eq!(out[1].kind, RubyRequireKind::RequireRelative);
        assert_eq!(out[1].source, "./foo");
        assert_eq!(out[1].line, 2);
    }

    #[test]
    fn accepts_double_quoted_and_parenthesized() {
        let out = refs("require(\"json\")\nrequire_relative('./foo')\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].source, "json");
        assert_eq!(out[1].source, "./foo");
    }

    #[test]
    fn accepts_modifier_condition_form() {
        let out = refs("require 'json' if ENV['X']\nrequire_relative 'x' unless false\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, RubyRequireKind::Require);
        assert_eq!(out[1].kind, RubyRequireKind::RequireRelative);
    }

    #[test]
    fn reports_one_based_line() {
        let out = refs("\n\n  require 'foo'\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, 3);
    }

    #[test]
    fn rejects_receiver_scoped_require() {
        assert!(refs("obj.require 'x'\n").is_empty());
    }

    #[test]
    fn rejects_dynamic_and_expression_arguments() {
        assert!(refs("require foo\n").is_empty());
        assert!(refs("require File.expand_path('x')\n").is_empty());
        assert!(refs("require_relative File.expand_path('x')\n").is_empty());
    }

    #[test]
    fn rejects_multiple_arguments() {
        assert!(refs("require 'a', 'b'\n").is_empty());
    }

    #[test]
    fn rejects_interpolation_and_escapes_and_percent_and_heredoc() {
        assert!(refs("require \"#{x}\"\n").is_empty());
        assert!(refs("require 'a\\nb'\n").is_empty());
        assert!(refs("require %q(foo)\n").is_empty());
        assert!(refs("require %w[foo bar]\n").is_empty());
        assert!(refs("require <<~EOF\nfoo\nEOF\n").is_empty());
    }

    #[test]
    fn rejects_empty_string() {
        assert!(refs("require ''\n").is_empty());
    }

    #[test]
    fn rejects_other_methods_and_definitions_and_mentions() {
        assert!(refs("load 'x'\n").is_empty());
        assert!(refs("autoload :Foo, 'foo'\n").is_empty());
        assert!(refs("def require(x); end\n").is_empty());
        assert!(refs("# require 'x'\n").is_empty());
        assert!(refs("s = \"require 'x'\"\n").is_empty());
    }

    #[test]
    fn keeps_source_order_across_forms() {
        let out = refs("require 'b'\nrequire 'a'\n");
        assert_eq!(out[0].source, "b");
        assert_eq!(out[1].source, "a");
    }
}
