use tree_sitter::Node;

use crate::lang::{decision_flow, outline::outline_language, treesitter};
use crate::types::Lang;

pub(crate) const DEFAULT_SCOPED_OCCURRENCE_CAP: usize = 12;
const MAX_SCOPED_SNIPPET_CHARS: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopedOccurrence {
    pub(crate) line: u32,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopedOccurrences {
    pub(crate) name: String,
    pub(crate) scope_start: u32,
    pub(crate) scope_end: u32,
    pub(crate) occurrences: Vec<ScopedOccurrence>,
    pub(crate) omitted: usize,
}

struct OccurrenceCollector {
    cap: usize,
    occurrences: Vec<ScopedOccurrence>,
    omitted: usize,
}

impl OccurrenceCollector {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            occurrences: Vec::with_capacity(cap),
            omitted: 0,
        }
    }

    fn push(&mut self, line: u32, lines: &[&str]) {
        if self
            .occurrences
            .iter()
            .any(|occurrence| occurrence.line == line)
        {
            return;
        }
        if self.occurrences.len() >= self.cap {
            self.omitted += 1;
            return;
        }

        let text = lines
            .get(line.saturating_sub(1) as usize)
            .map_or_else(String::new, |line| compact_snippet(line));
        self.occurrences.push(ScopedOccurrence { line, text });
    }
}

pub(crate) fn extract_scoped_occurrences(
    source: &str,
    lang: Lang,
    selector: &decision_flow::TargetSelector,
    cap: usize,
) -> Option<ScopedOccurrences> {
    let language = outline_language(lang)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let target = decision_flow::find_unique_flow_target_definition(root, source, lang, selector)?;
    let lines = source.lines().collect::<Vec<_>>();
    let name = treesitter::extract_definition_name(target, &lines)?;
    let declaration_name = target.child_by_field_name("name")?;
    let scope = enclosing_scope(target, root, lang);

    if scope_has_conflicting_binding(scope, target, &name, source, &lines, lang) {
        return None;
    }

    let mut collector = OccurrenceCollector::new(cap);
    collect_occurrences(
        scope,
        scope,
        target,
        declaration_name,
        &name,
        source,
        &lines,
        lang,
        &mut collector,
    );

    let physical_line_count = lines.len().max(1) as u32;
    Some(ScopedOccurrences {
        name,
        scope_start: line_start(scope),
        scope_end: line_end(scope).min(physical_line_count),
        occurrences: collector.occurrences,
        omitted: collector.omitted,
    })
}

fn enclosing_scope<'tree>(target: Node<'tree>, root: Node<'tree>, lang: Lang) -> Node<'tree> {
    let mut ancestor = target.parent();
    while let Some(node) = ancestor {
        if is_structural_scope(node, lang) {
            return node;
        }
        ancestor = node.parent();
    }
    root
}

#[allow(clippy::too_many_arguments)]
fn collect_occurrences(
    node: Node<'_>,
    scope: Node<'_>,
    target: Node<'_>,
    declaration_name: Node<'_>,
    name: &str,
    source: &str,
    lines: &[&str],
    lang: Lang,
    out: &mut OccurrenceCollector,
) {
    let is_root_body = scope
        .child_by_field_name("body")
        .is_some_and(|body| body.id() == node.id());
    if node.id() != scope.id()
        && !is_root_body
        && is_scope_boundary(node, lang)
        && boundary_shadows_name(node, target, name, source, lines, lang)
    {
        return;
    }

    if node.id() != target.id() && node_declares_name(node, name, source, lines, lang) {
        return;
    }

    if is_identifier_like(node.kind())
        && node.id() != declaration_name.id()
        && node.utf8_text(source.as_bytes()).ok() == Some(name)
    {
        out.push(line_start(node), lines);
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_occurrences(
            child,
            scope,
            target,
            declaration_name,
            name,
            source,
            lines,
            lang,
            out,
        );
    }
}

fn scope_has_conflicting_binding(
    scope: Node<'_>,
    target: Node<'_>,
    name: &str,
    source: &str,
    lines: &[&str],
    lang: Lang,
) -> bool {
    if decision_flow::is_function_like_node(scope, lang)
        && decision_flow::function_has_parameter_named(scope, source, name)
    {
        return true;
    }
    has_direct_binding(scope, scope, target, name, source, lines, lang)
}

#[allow(clippy::too_many_arguments)]
fn has_direct_binding(
    node: Node<'_>,
    owner: Node<'_>,
    target: Node<'_>,
    name: &str,
    source: &str,
    lines: &[&str],
    lang: Lang,
) -> bool {
    let owner_body = owner.child_by_field_name("body");
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let is_owner_body = owner_body.is_some_and(|body| body.id() == child.id());
        let declares_name = node_declares_name(child, name, source, lines, lang);
        let wraps_target =
            child.start_byte() <= target.start_byte() && child.end_byte() >= target.end_byte();

        if declares_name {
            if wraps_target {
                continue;
            }
            return true;
        }
        if is_scope_boundary(child, lang) && !is_owner_body {
            continue;
        }
        if has_direct_binding(child, owner, target, name, source, lines, lang) {
            return true;
        }
    }
    false
}

fn boundary_shadows_name(
    node: Node<'_>,
    target: Node<'_>,
    name: &str,
    source: &str,
    lines: &[&str],
    lang: Lang,
) -> bool {
    let own_definition_shadows = node.id() != target.id()
        && treesitter::extract_definition_name(node, lines).as_deref() == Some(name);
    own_definition_shadows
        || (decision_flow::is_function_like_node(node, lang)
            && decision_flow::function_has_parameter_named(node, source, name))
        || has_direct_binding(node, node, target, name, source, lines, lang)
}

fn node_declares_name(
    node: Node<'_>,
    name: &str,
    source: &str,
    lines: &[&str],
    lang: Lang,
) -> bool {
    if is_structural_scope(node, lang) {
        return treesitter::extract_definition_name(node, lines).as_deref() == Some(name);
    }

    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = node.walk();
            let declares = node
                .named_children(&mut cursor)
                .any(|child| node_declares_name(child, name, source, lines, lang));
            declares
        }
        "variable_declarator" => binding_field_matches(node, &["name", "pattern"], name, source),
        "let_declaration" => binding_field_matches(node, &["pattern"], name, source),
        "assignment" if lang == Lang::Python => node
            .child_by_field_name("left")
            .is_some_and(|binding| python_assignment_binding_contains_name(binding, name, source)),
        _ => false,
    }
}

fn python_assignment_binding_contains_name(node: Node<'_>, name: &str, source: &str) -> bool {
    if node.kind() == "identifier" {
        return node.utf8_text(source.as_bytes()).ok() == Some(name);
    }

    if !matches!(
        node.kind(),
        "pattern_list" | "tuple_pattern" | "list_pattern" | "list_splat_pattern"
    ) {
        return false;
    }

    let mut cursor = node.walk();
    let contains = node
        .named_children(&mut cursor)
        .any(|child| python_assignment_binding_contains_name(child, name, source));
    contains
}

fn binding_field_matches(node: Node<'_>, fields: &[&str], name: &str, source: &str) -> bool {
    fields.iter().any(|field| {
        node.child_by_field_name(field)
            .is_some_and(|binding| binding_contains_name(binding, name, source))
    })
}

fn binding_contains_name(node: Node<'_>, name: &str, source: &str) -> bool {
    if is_binding_identifier_like(node.kind())
        && node.utf8_text(source.as_bytes()).ok() == Some(name)
    {
        return true;
    }

    let mut cursor = node.walk();
    let contains = node
        .named_children(&mut cursor)
        .any(|child| binding_contains_name(child, name, source));
    contains
}

fn is_scope_boundary(node: Node<'_>, lang: Lang) -> bool {
    is_structural_scope(node, lang)
        || matches!(
            node.kind(),
            "block" | "statement_block" | "compound_statement" | "suite"
        )
}

fn is_structural_scope(node: Node<'_>, lang: Lang) -> bool {
    decision_flow::is_function_like_node(node, lang)
        || matches!(
            node.kind(),
            "class_declaration"
                | "class_definition"
                | "class_specifier"
                | "impl_item"
                | "trait_item"
                | "module"
                | "mod_item"
                | "namespace_definition"
        )
}

fn is_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "property_identifier"
            | "field_identifier"
            | "constant"
            | "simple_identifier"
    )
}

fn is_binding_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "shorthand_property_identifier_pattern" | "simple_identifier" | "constant"
    )
}

fn compact_snippet(line: &str) -> String {
    let mut chars = line.trim().chars();
    let mut compact = String::with_capacity(MAX_SCOPED_SNIPPET_CHARS);
    for _ in 0..MAX_SCOPED_SNIPPET_CHARS {
        let Some(ch) = chars.next() else {
            return compact;
        };
        compact.push(ch);
    }
    if chars.next().is_some() {
        compact.pop();
        compact.push('…');
    }
    compact
}

fn line_start(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn line_end(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"function outer(value) {
  function helper(input) { return input + 1; }
  const first = helper(value);
  return helper(first);
}
"#;

    #[test]
    fn extracts_parent_scope_with_a_deterministic_cap() {
        let scoped = extract_scoped_occurrences(
            SOURCE,
            Lang::JavaScript,
            &decision_flow::TargetSelector::FocusedLineRange { start: 2, end: 2 },
            1,
        )
        .unwrap();

        assert_eq!(scoped.name, "helper");
        assert_eq!((scoped.scope_start, scoped.scope_end), (1, 5));
        assert_eq!(scoped.occurrences.len(), 1);
        assert_eq!(scoped.occurrences[0].line, 3);
        assert_eq!(scoped.omitted, 1);
    }

    #[test]
    fn abstains_when_the_focused_line_is_not_the_declaration() {
        assert!(extract_scoped_occurrences(
            SOURCE,
            Lang::JavaScript,
            &decision_flow::TargetSelector::FocusedLineRange { start: 3, end: 3 },
            DEFAULT_SCOPED_OCCURRENCE_CAP,
        )
        .is_none());
    }

    #[test]
    fn coalesces_multiple_occurrences_on_one_line() {
        let source = "function helper() {} helper(); helper();\n";
        let scoped = extract_scoped_occurrences(
            source,
            Lang::JavaScript,
            &decision_flow::TargetSelector::Symbol("helper".to_string()),
            DEFAULT_SCOPED_OCCURRENCE_CAP,
        )
        .unwrap();

        assert_eq!(scoped.occurrences.len(), 1);
        assert_eq!(scoped.occurrences[0].line, 1);
        assert_eq!(scoped.omitted, 0);
    }

    #[test]
    fn bounds_minified_snippets_and_normalizes_root_end() {
        let tail = "x".repeat(10_000);
        let source = format!("function helper() {{}} helper(); {tail}\n");
        let scoped = extract_scoped_occurrences(
            &source,
            Lang::JavaScript,
            &decision_flow::TargetSelector::Symbol("helper".to_string()),
            DEFAULT_SCOPED_OCCURRENCE_CAP,
        )
        .unwrap();

        assert_eq!(scoped.scope_end, 1);
        assert_eq!(scoped.occurrences.len(), 1);
        assert!(scoped.occurrences[0].text.chars().count() <= MAX_SCOPED_SNIPPET_CHARS);
        assert!(scoped.occurrences[0].text.ends_with('…'));
    }
}
