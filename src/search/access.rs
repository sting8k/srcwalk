use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{
    confidence_label_for, render_next_actions, Anchor, EvidenceAtom, EvidenceKind, EvidenceRole,
    EvidenceSource, NextAction,
};
use crate::format::{display_path, rel_nonempty};
use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::path_match_contains;
use crate::search::truncate::compact_match_line;
use crate::types::{estimate_tokens, FileType};

use super::io::{file_metadata, is_minified_filename, walker};

const DEFAULT_ACCESS_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum AccessKind {
    Write,
    Reset,
    Read,
    Unknown,
}

impl AccessKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Reset => "reset",
            Self::Read => "read",
            Self::Unknown => "unknown",
        }
    }

    fn from_filter(value: &str) -> Option<Self> {
        match value {
            "write" | "writes" => Some(Self::Write),
            "reset" | "resets" => Some(Self::Reset),
            "read" | "reads" => Some(Self::Read),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct AccessHit {
    path: PathBuf,
    line: u32,
    byte: usize,
    text: String,
    kind: AccessKind,
    role: Option<EvidenceRole>,
    structural: bool,
    function: Option<String>,
    mtime: SystemTime,
}
impl AccessKind {
    const fn evidence_kind(self) -> EvidenceKind {
        match self {
            Self::Write => EvidenceKind::Write,
            Self::Reset => EvidenceKind::Reset,
            Self::Read => EvidenceKind::Read,
            Self::Unknown => EvidenceKind::UnknownAccess,
        }
    }
}

impl AccessHit {
    fn to_evidence_atom(&self) -> EvidenceAtom {
        let source = if self.structural {
            EvidenceSource::Ast
        } else {
            EvidenceSource::Text
        };
        EvidenceAtom::new(
            self.kind.evidence_kind(),
            self.role,
            Anchor::line(&self.path, self.line),
            self.text.clone(),
            source,
        )
    }
}

#[derive(Default)]
struct AccessFilter {
    access: Vec<AccessKind>,
    path: Vec<String>,
    file: Vec<String>,
    text: Vec<String>,
    line: Vec<(u32, u32)>,
}

#[derive(Default)]
struct Counts {
    write: usize,
    reset: usize,
    read: usize,
    unknown: usize,
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(content: &str) -> Self {
        let mut starts = vec![0];
        for (idx, byte) in content.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(idx + 1);
            }
        }
        Self { starts }
    }

    fn line_for_byte(&self, byte: usize) -> u32 {
        let line_idx = self
            .starts
            .partition_point(|start| *start <= byte)
            .saturating_sub(1);
        (line_idx + 1) as u32
    }

    fn text_for_line<'a>(&self, content: &'a str, line: u32) -> &'a str {
        let idx = line.saturating_sub(1) as usize;
        let Some(start) = self.starts.get(idx).copied() else {
            return "";
        };
        let end = self
            .starts
            .get(idx + 1)
            .map_or(content.len(), |next| next.saturating_sub(1));
        content[start..end].trim_end_matches('\r')
    }
}

pub fn search_access(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    limit: Option<usize>,
    offset: usize,
    glob: Option<&str>,
    filter: Option<&str>,
) -> Result<String, SrcwalkError> {
    if query.trim().is_empty() {
        return Err(SrcwalkError::InvalidQuery {
            query: query.to_string(),
            reason: "access query cannot be empty".to_string(),
        });
    }

    let filter = AccessFilter::parse(filter)?;
    let mut hits = collect_access_hits(query, scope, glob)?;
    hits.retain(|hit| filter.matches(hit, scope));
    hits.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.cmp(&b.line))
            .then(a.byte.cmp(&b.byte))
    });

    let total_found = hits.len();
    let counts = Counts::from_hits(&hits);
    let file_count = hits
        .iter()
        .map(|hit| hit.path.as_path())
        .collect::<HashSet<_>>()
        .len();
    let function_count = hits
        .iter()
        .filter_map(|hit| hit.function.as_ref().map(|name| (hit.path.as_path(), name)))
        .collect::<HashSet<_>>()
        .len();

    let effective_limit = limit.unwrap_or(DEFAULT_ACCESS_LIMIT);
    let shown_hits: Vec<AccessHit> = hits
        .iter()
        .skip(offset)
        .take(effective_limit)
        .cloned()
        .collect();

    let summary_hits = if limit.is_some() {
        hits.as_slice()
    } else {
        shown_hits.as_slice()
    };

    Ok(format_access_result(
        query,
        scope,
        cache,
        &hits,
        summary_hits,
        &shown_hits,
        total_found,
        &counts,
        file_count,
        function_count,
        Some(effective_limit),
        offset,
    ))
}

fn collect_access_hits(
    query: &str,
    scope: &Path,
    glob: Option<&str>,
) -> Result<Vec<AccessHit>, SrcwalkError> {
    let hits = Mutex::new(Vec::new());
    let walker = walker(scope, glob)?;

    walker.run(|| {
        let hits = &hits;
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            if is_minified_filename(path) {
                return ignore::WalkState::Continue;
            }
            let FileType::Code(lang) = detect_file_type(path) else {
                return ignore::WalkState::Continue;
            };
            let Ok(meta) = std::fs::metadata(path) else {
                return ignore::WalkState::Continue;
            };
            if meta.len() > 500_000 {
                return ignore::WalkState::Continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return ignore::WalkState::Continue;
            };
            if !content.contains(query) {
                return ignore::WalkState::Continue;
            }

            let (_, mtime) = file_metadata(path);
            let file_hits = collect_file_hits(path, &content, lang, query, mtime);
            if !file_hits.is_empty() {
                let mut all = hits
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                all.extend(file_hits);
            }
            ignore::WalkState::Continue
        })
    });

    Ok(hits
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn collect_file_hits(
    path: &Path,
    content: &str,
    lang: crate::types::Lang,
    query: &str,
    mtime: SystemTime,
) -> Vec<AccessHit> {
    let line_index = LineIndex::new(content);
    let mut hits = Vec::new();
    let mut ast_lines = HashSet::new();

    if let Some(ts_lang) = outline_language(lang) {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&ts_lang).is_ok() {
            if let Some(tree) = parser.parse(content, None) {
                collect_ast_hits(
                    tree.root_node(),
                    path,
                    content,
                    query,
                    &line_index,
                    mtime,
                    &mut hits,
                    &mut ast_lines,
                );
            }
        }
    }

    collect_unknown_text_hits(
        path,
        content,
        query,
        &line_index,
        mtime,
        &ast_lines,
        &mut hits,
    );
    hits
}

#[allow(clippy::too_many_arguments)]
fn collect_ast_hits(
    node: tree_sitter::Node,
    path: &Path,
    content: &str,
    query: &str,
    line_index: &LineIndex,
    mtime: SystemTime,
    hits: &mut Vec<AccessHit>,
    ast_lines: &mut HashSet<u32>,
) {
    if is_access_name_node(node, query, content) {
        let line = line_index.line_for_byte(node.start_byte());
        ast_lines.insert(line);
        let access_node = access_expression_ancestor(node);
        let kind = access_node.map_or(AccessKind::Unknown, |access| {
            classify_access(access, content)
        });
        let role = access_node.map(|access| classify_access_role(access, kind));
        hits.push(AccessHit {
            path: path.to_path_buf(),
            line,
            byte: node.start_byte(),
            text: compact_match_line(line_index.text_for_line(content, line).trim(), query, false),
            kind,
            role,
            structural: access_node.is_some(),
            function: enclosing_function_name(node, content),
            mtime,
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_ast_hits(
            child, path, content, query, line_index, mtime, hits, ast_lines,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_unknown_text_hits(
    path: &Path,
    content: &str,
    query: &str,
    line_index: &LineIndex,
    mtime: SystemTime,
    ast_lines: &HashSet<u32>,
    hits: &mut Vec<AccessHit>,
) {
    for (line_idx, line) in content.lines().enumerate() {
        let line_no = (line_idx + 1) as u32;
        if ast_lines.contains(&line_no) || is_commentish_line(line) || !contains_word(line, query) {
            continue;
        }
        hits.push(AccessHit {
            path: path.to_path_buf(),
            line: line_no,
            byte: line_index.starts.get(line_idx).copied().unwrap_or_default(),
            text: compact_match_line(line.trim(), query, false),
            kind: AccessKind::Unknown,
            role: None,
            structural: false,
            function: None,
            mtime,
        });
    }
}

fn is_access_name_node(node: tree_sitter::Node, query: &str, content: &str) -> bool {
    let Ok(text) = node.utf8_text(content.as_bytes()) else {
        return false;
    };
    if !access_name_matches_query(text, query) {
        return false;
    }
    if is_direct_access_name_kind(node.kind()) {
        return true;
    }
    if !is_generic_identifier_kind(node.kind()) {
        return false;
    }
    access_expression_ancestor(node)
        .is_some_and(|access| access_text_names_query(access, query, content))
}

fn is_direct_access_name_kind(kind: &str) -> bool {
    is_field_name_node(kind) || matches!(kind, "instance_variable")
}

fn is_generic_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "name" | "simple_identifier")
}

fn is_field_name_node(kind: &str) -> bool {
    matches!(kind, "field_identifier" | "property_identifier")
}

fn access_name_matches_query(text: &str, query: &str) -> bool {
    text == query
        || text
            .strip_prefix('@')
            .or_else(|| text.strip_prefix('$'))
            .is_some_and(|name| name == query)
}

fn access_text_names_query(access: tree_sitter::Node, query: &str, content: &str) -> bool {
    let Ok(text) = access.utf8_text(content.as_bytes()) else {
        return false;
    };
    let trimmed = text.trim();
    trimmed.ends_with(&format!(".{query}"))
        || trimmed.ends_with(&format!("->{query}"))
        || trimmed.ends_with(&format!("::{query}"))
        || trimmed == format!("@{query}")
        || trimmed == format!("${query}")
}

fn is_access_expression(kind: &str) -> bool {
    matches!(
        kind,
        "field_expression"
            | "selector_expression"
            | "member_expression"
            | "member_access_expression"
            | "field_access"
            | "attribute"
            | "navigation_expression"
            | "navigation_suffix"
            | "dot"
            | "instance_variable"
    )
}

fn access_expression_ancestor(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut current = node;
    if is_access_expression(current.kind()) {
        return Some(current);
    }
    while let Some(parent) = current.parent() {
        if is_access_expression(parent.kind()) {
            return Some(parent);
        }
        if is_function_like(parent.kind()) || parent.kind().contains("declaration") {
            return None;
        }
        current = parent;
    }
    None
}

fn classify_access(access: tree_sitter::Node, content: &str) -> AccessKind {
    if is_address_taken(access, content) {
        return AccessKind::Unknown;
    }

    let mut current = access;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "assignment" | "assignment_expression" | "assignment_statement"
                if is_assignment_left(parent, access) =>
            {
                return assignment_right(parent)
                    .filter(|right| is_zero_like(*right, content))
                    .map_or(AccessKind::Write, |_| AccessKind::Reset);
            }
            "augmented_assignment_expression" | "augmented_assignment_statement"
                if is_assignment_left(parent, access) =>
            {
                return AccessKind::Write;
            }
            "update_expression" | "inc_statement" | "dec_statement" => return AccessKind::Write,
            kind if is_function_like(kind) || kind.contains("declaration") => break,
            _ => {}
        }
        current = parent;
    }

    AccessKind::Read
}

fn classify_access_role(access: tree_sitter::Node, kind: AccessKind) -> EvidenceRole {
    if matches!(kind, AccessKind::Write | AccessKind::Reset) {
        return EvidenceRole::AssignmentLhs;
    }

    let mut current = access;
    while let Some(parent) = current.parent() {
        let parent_kind = parent.kind();
        if is_function_like(parent_kind) {
            break;
        }
        if is_condition_context(parent, access) {
            return EvidenceRole::Condition;
        }
        if is_call_argument_context(parent, access) {
            return EvidenceRole::CallArg;
        }
        if is_assignment_right(parent, access) {
            return EvidenceRole::AssignmentRhs;
        }
        if is_return_context(parent_kind) {
            return EvidenceRole::Return;
        }
        if is_index_or_key_context(parent_kind) {
            return EvidenceRole::IndexOrKey;
        }
        if is_receiver_context(parent, current) {
            return EvidenceRole::Receiver;
        }
        if is_initializer_context(parent, access) {
            return EvidenceRole::Initializer;
        }
        if parent_kind.contains("declaration") {
            break;
        }
        current = parent;
    }

    // Keep the role conservative: if no trusted AST relationship is recognized,
    // it remains structural expression evidence rather than a semantic claim.
    EvidenceRole::Expression
}

fn is_assignment_right(parent: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    matches!(
        parent.kind(),
        "assignment" | "assignment_expression" | "assignment_statement"
    ) && !is_assignment_left(parent, access)
        && assignment_right(parent).is_some_and(|right| contains_node(right, access))
}

fn is_condition_context(parent: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    match parent.kind() {
        "if_statement" | "while_statement" | "for_statement" | "elif_clause" | "else_if_clause"
        | "guard_statement" | "when_entry" | "case_statement" => parent
            .child_by_field_name("condition")
            .or_else(|| first_named_child(parent))
            .is_some_and(|condition| contains_node(condition, access)),
        "conditional_expression" | "ternary_expression" => {
            first_named_child(parent).is_some_and(|condition| contains_node(condition, access))
        }
        _ => false,
    }
}

fn is_call_argument_context(parent: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    match parent.kind() {
        "argument_list" | "arguments" | "value_arguments" | "call_arguments" => true,
        "call_expression" | "invocation_expression" | "call" => parent
            .child_by_field_name("arguments")
            .is_some_and(|args| contains_node(args, access)),
        _ => false,
    }
}

fn is_return_context(kind: &str) -> bool {
    matches!(kind, "return_statement" | "return_expression")
}

fn is_index_or_key_context(kind: &str) -> bool {
    matches!(
        kind,
        "subscript_expression" | "index_expression" | "element_access_expression" | "subscript"
    )
}

fn is_receiver_context(parent: tree_sitter::Node, current: tree_sitter::Node) -> bool {
    is_access_expression(parent.kind()) && is_first_named_child(parent, current)
}

fn is_initializer_context(parent: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    matches!(
        parent.kind(),
        "variable_declarator"
            | "init_declarator"
            | "field_declaration"
            | "lexical_declaration"
            | "property_declaration"
    ) && parent
        .child_by_field_name("value")
        .or_else(|| parent.child_by_field_name("initializer"))
        .is_some_and(|value| contains_node(value, access))
}

fn is_assignment_left(parent: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    parent
        .child_by_field_name("left")
        .or_else(|| first_named_child(parent))
        .is_some_and(|left| is_write_target_path(left, access))
}

fn is_write_target_path(left: tree_sitter::Node, access: tree_sitter::Node) -> bool {
    if !contains_node(left, access) {
        return false;
    }

    let mut current = access;
    loop {
        if same_node(current, left) {
            return true;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if same_node(parent, left) {
            return true;
        }
        if !contains_node(left, parent) || !is_write_target_wrapper(parent, current) {
            return false;
        }
        current = parent;
    }
}

fn is_write_target_wrapper(parent: tree_sitter::Node, child: tree_sitter::Node) -> bool {
    match parent.kind() {
        kind if is_access_expression(kind) => true,
        "subscript_expression" | "index_expression" | "element_access_expression" => {
            is_first_named_child(parent, child)
        }
        "parenthesized_expression"
        | "directly_assignable_expression"
        | "unary_expression"
        | "pointer_expression" => true,
        _ => false,
    }
}

fn is_first_named_child(parent: tree_sitter::Node, child: tree_sitter::Node) -> bool {
    first_named_child(parent).is_some_and(|first| same_node(first, child))
}

fn assignment_right(parent: tree_sitter::Node) -> Option<tree_sitter::Node> {
    parent.child_by_field_name("right").or_else(|| {
        let count = parent.named_child_count();
        (count >= 2)
            .then(|| parent.named_child(count - 1))
            .flatten()
    })
}

fn first_named_child(parent: tree_sitter::Node) -> Option<tree_sitter::Node> {
    (parent.named_child_count() > 0)
        .then(|| parent.named_child(0))
        .flatten()
}

fn same_node(a: tree_sitter::Node, b: tree_sitter::Node) -> bool {
    a.kind() == b.kind() && a.start_byte() == b.start_byte() && a.end_byte() == b.end_byte()
}

fn contains_node(parent: tree_sitter::Node, child: tree_sitter::Node) -> bool {
    parent.start_byte() <= child.start_byte() && child.end_byte() <= parent.end_byte()
}

fn is_address_taken(access: tree_sitter::Node, content: &str) -> bool {
    access.parent().is_some_and(|parent| {
        parent.kind() == "unary_expression"
            && parent
                .utf8_text(content.as_bytes())
                .is_ok_and(|text| text.trim_start().starts_with('&'))
    })
}

fn is_zero_like(node: tree_sitter::Node, content: &str) -> bool {
    let Ok(text) = node.utf8_text(content.as_bytes()) else {
        return false;
    };
    let value = strip_wrapping_parens(text.trim().trim_end_matches(';')).to_ascii_lowercase();
    matches!(
        value.as_str(),
        "0" | "false" | "null" | "nullptr" | "nil" | "none"
    )
}

fn strip_wrapping_parens(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('(') && trimmed.ends_with(')') {
            value = &trimmed[1..trimmed.len() - 1];
        } else {
            return trimmed;
        }
    }
}

fn is_function_like(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_item"
            | "function_declarator"
            | "function_body_declaration"
            | "method"
            | "method_declaration"
            | "method_definition"
            | "function"
    )
}

fn enclosing_function_name(node: tree_sitter::Node, content: &str) -> Option<String> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if is_function_like(parent.kind()) {
            return function_name(parent, content);
        }
        current = parent;
    }
    None
}

fn function_name(node: tree_sitter::Node, content: &str) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        if let Ok(text) = name.utf8_text(content.as_bytes()) {
            return Some(text.to_string());
        }
    }
    if let Some(declarator) = node.child_by_field_name("declarator") {
        return first_identifier(declarator, content);
    }
    first_identifier(node, content)
}

fn first_identifier(node: tree_sitter::Node, content: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "identifier" | "field_identifier" | "property_identifier"
    ) {
        return node.utf8_text(content.as_bytes()).ok().map(str::to_string);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name) = first_identifier(child, content) {
            return Some(name);
        }
    }
    None
}

fn contains_word(line: &str, query: &str) -> bool {
    let bytes = line.as_bytes();
    let query_bytes = query.as_bytes();
    if query_bytes.is_empty() || query_bytes.len() > bytes.len() {
        return false;
    }
    bytes
        .windows(query_bytes.len())
        .enumerate()
        .any(|(idx, window)| {
            window == query_bytes
                && idx
                    .checked_sub(1)
                    .is_none_or(|prev| !is_word_byte(bytes[prev]))
                && bytes
                    .get(idx + query_bytes.len())
                    .is_none_or(|next| !is_word_byte(*next))
        })
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_commentish_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
}

fn format_access_result(
    query: &str,
    scope: &Path,
    cache: &OutlineCache,
    all_hits: &[AccessHit],
    summary_hits: &[AccessHit],
    shown_hits: &[AccessHit],
    total_found: usize,
    counts: &Counts,
    file_count: usize,
    function_count: usize,
    limit: Option<usize>,
    offset: usize,
) -> String {
    let mut out = format!(
        "# Access: \"{}\" in {} — {} hits",
        query,
        display_path(scope),
        total_found
    );
    let confidence = confidence_label_for(EvidenceSource::Ast);
    let _ = write!(
        out,
        "\nconfidence: {confidence}\nhits: total={} shown={} write={} reset={} read={} unknown={} files={} functions={}",
        total_found,
        shown_hits.len(),
        counts.write,
        counts.reset,
        counts.read,
        counts.unknown,
        file_count,
        function_count
    );
    out.push_str("\nflags: --filter access:<write|reset|read|unknown> --limit N --offset N");
    append_function_lifecycle(&mut out, summary_hits, scope);
    append_lifecycle_breadcrumbs(&mut out, all_hits, shown_hits, scope);

    let mut remaining = shown_hits;
    while let Some((first, rest)) = remaining.split_first() {
        let same_file_count = rest.iter().take_while(|hit| hit.path == first.path).count();
        let (file_hits, next) = remaining.split_at(same_file_count + 1);
        remaining = next;

        let path = rel_nonempty(&first.path, scope);
        let _ = write!(out, "\n\n## {path}");
        for kind in [
            AccessKind::Write,
            AccessKind::Reset,
            AccessKind::Read,
            AccessKind::Unknown,
        ] {
            let group: Vec<&AccessHit> = file_hits.iter().filter(|hit| hit.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            let _ = write!(out, "\n[{}] {}", kind.as_str(), group.len());
            for hit in group {
                format_access_hit(hit, cache, &mut out);
            }
        }
    }

    append_access_footer(
        &mut out,
        shown_hits,
        scope,
        cache,
        total_found,
        limit,
        offset,
    );
    out
}

fn format_access_hit(hit: &AccessHit, cache: &OutlineCache, out: &mut String) {
    let atom = hit.to_evidence_atom();
    let function = hit
        .function
        .clone()
        .or_else(|| enclosing_outline_name(hit, cache))
        .unwrap_or_else(|| "?".to_string());
    let _ = write!(
        out,
        "\n- :{} {} | {}",
        atom.anchor().start_line(),
        function,
        atom.snippet().trim()
    );
    if let Some(role) = atom.role() {
        let _ = write!(out, " [{}]", role.as_str());
    }
}

fn append_function_lifecycle(out: &mut String, hits: &[AccessHit], scope: &Path) {
    let mut summaries: Vec<(String, String, Counts)> = Vec::new();
    for hit in hits
        .iter()
        .filter(|hit| hit.to_evidence_atom().source() == EvidenceSource::Ast)
    {
        let Some(function) = &hit.function else {
            continue;
        };
        let path = rel_nonempty(&hit.path, scope);
        if let Some((_, _, counts)) =
            summaries
                .iter_mut()
                .find(|(existing_path, existing_function, _)| {
                    existing_path == &path && existing_function == function
                })
        {
            counts.add(hit.kind);
        } else {
            let mut counts = Counts::default();
            counts.add(hit.kind);
            summaries.push((path, function.clone(), counts));
        }
    }
    if summaries.is_empty() {
        return;
    }

    let mut file_groups: Vec<(String, Vec<(String, Counts)>)> = Vec::new();
    for (path, function, counts) in summaries {
        if let Some((_, functions)) = file_groups
            .iter_mut()
            .find(|(existing_path, _)| existing_path == &path)
        {
            functions.push((function, counts));
        } else {
            file_groups.push((path, vec![(function, counts)]));
        }
    }

    out.push_str("\n\n## functions (structural source-order summary)");
    for (path, functions) in file_groups {
        let _ = write!(out, "\n{path}");
        for (function, counts) in functions {
            let _ = write!(
                out,
                "\n  {function} write={} reset={} read={} unknown={}",
                counts.write, counts.reset, counts.read, counts.unknown
            );
        }
    }
}

fn append_lifecycle_breadcrumbs(
    out: &mut String,
    all_hits: &[AccessHit],
    shown_hits: &[AccessHit],
    scope: &Path,
) {
    let total_structural = all_hits
        .iter()
        .filter(|hit| hit.to_evidence_atom().source() == EvidenceSource::Ast)
        .count();
    let shown_structural_hits: Vec<&AccessHit> = shown_hits
        .iter()
        .filter(|hit| hit.to_evidence_atom().source() == EvidenceSource::Ast)
        .collect();
    if shown_structural_hits.is_empty() {
        return;
    }

    let _ = write!(
        out,
        "\n\n## breadcrumbs (structural lexical order; not runtime order)\nevents: shown={} total={} page=current",
        shown_structural_hits.len(),
        total_structural
    );

    let mut groups: Vec<(String, String, Vec<&AccessHit>)> = Vec::new();
    for hit in shown_structural_hits {
        let path = rel_nonempty(&hit.path, scope);
        let function = hit.function.as_deref().unwrap_or("?").to_string();
        if let Some((_, _, group_hits)) =
            groups
                .iter_mut()
                .find(|(existing_path, existing_function, _)| {
                    existing_path == &path && existing_function == &function
                })
        {
            group_hits.push(hit);
        } else {
            groups.push((path, function, vec![hit]));
        }
    }

    for (path, function, group_hits) in groups {
        let _ = write!(out, "\n{path}:{function}");
        for hit in group_hits {
            let atom = hit.to_evidence_atom();
            let role = atom.role().map_or("expression", EvidenceRole::as_str);
            let _ = write!(
                out,
                "\n- :{} {} {} | {}",
                atom.anchor().start_line(),
                atom.kind().as_str(),
                role,
                atom.snippet().trim()
            );
        }
    }
}

fn enclosing_outline_target(hit: &AccessHit, cache: &OutlineCache) -> Option<(u32, u32)> {
    let crate::types::FileType::Code(lang) = detect_file_type(&hit.path) else {
        return None;
    };
    if !crate::lang::decision_flow::is_supported_flow_target_lang(lang) {
        return None;
    }

    let outline = cache.get_or_compute(&hit.path, hit.mtime, || {
        let content = std::fs::read_to_string(&hit.path).unwrap_or_default();
        let file_type = crate::types::FileType::Code(lang);
        crate::read::outline::generate(&hit.path, file_type, &content, content.as_bytes(), false)
    });
    let mut best: Option<(u32, u32)> = None;
    for line in outline.lines() {
        if !is_function_outline_line(line) {
            continue;
        }
        if let Some((start, end)) = extract_outline_range(line) {
            if hit.line >= start
                && hit.line <= end
                && best.is_none_or(|(best_start, best_end)| end - start < best_end - best_start)
            {
                best = Some((start, end));
            }
        }
    }
    best
}

fn is_function_outline_line(line: &str) -> bool {
    let Some((_, after_range)) = line.split_once(']') else {
        return false;
    };
    let label = after_range.trim_start();
    label.starts_with("fn ") || label.starts_with("def ") || label.starts_with("fun ")
}

fn enclosing_outline_name(hit: &AccessHit, cache: &OutlineCache) -> Option<String> {
    let outline = cache.get_or_compute(&hit.path, hit.mtime, || {
        let content = std::fs::read_to_string(&hit.path).unwrap_or_default();
        let file_type = detect_file_type(&hit.path);
        crate::read::outline::generate(&hit.path, file_type, &content, content.as_bytes(), false)
    });
    let mut best: Option<(&str, u32, u32)> = None;
    for line in outline.lines() {
        if let Some((start, end)) = extract_outline_range(line) {
            if hit.line >= start
                && hit.line <= end
                && best.is_none_or(|(_, best_start, best_end)| end - start < best_end - best_start)
            {
                best = Some((line, start, end));
            }
        }
    }
    best.and_then(|(line, _, _)| line.split_whitespace().last().map(str::to_string))
}

fn extract_outline_range(line: &str) -> Option<(u32, u32)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let range = &trimmed[1..end];
    if let Some((start, end)) = range.split_once('-') {
        let start = start.trim().parse().ok()?;
        let end = end.trim().parse().unwrap_or(start);
        Some((start, end))
    } else {
        let line = range.trim().parse().ok()?;
        Some((line, line))
    }
}

fn append_access_footer(
    out: &mut String,
    hits: &[AccessHit],
    scope: &Path,
    cache: &OutlineCache,
    total_found: usize,
    limit: Option<usize>,
    offset: usize,
) {
    let tokens = estimate_tokens(out.len() as u64);
    let token_str = if tokens >= 1000 {
        format!("~{}.{}k", tokens / 1000, (tokens % 1000) / 100)
    } else {
        format!("~{tokens}")
    };
    let _ = write!(out, "\n\n({token_str} tokens)");

    out.push_str("\n\n> Caveat: syntax-level access grouping; lexical breadcrumbs are not runtime order, type proof, alias proof, or security proof.");
    let mut actions = Vec::new();
    if let Some(first) = hits.iter().find(|hit| hit.function.is_some()) {
        if let Some((start, end)) = enclosing_outline_target(first, cache) {
            let anchor = Anchor::lines(&first.path, start, end);
            actions.push(NextAction::from_evidence(
                format!("srcwalk context {}", anchor.display_relative_to(scope)),
                "access hit has enclosing structural context target",
                10,
                EvidenceSource::Ast,
                anchor,
            ));
        } else if let Some(function) = &first.function {
            let path = rel_nonempty(&first.path, scope);
            actions.push(NextAction::from_evidence(
                format!("srcwalk {path} --section {function}"),
                "access hit has an enclosing source section",
                20,
                EvidenceSource::Ast,
                Anchor::file(&first.path),
            ));
        }
    } else if total_found > 0 {
        actions.push(NextAction::guidance(
            "drill into any hit with `srcwalk <path>:<line>`.",
            "read exact hit evidence",
            50,
        ));
    }
    if let Some(limit) = limit {
        let next_offset = offset.saturating_add(hits.len());
        if next_offset < total_found {
            let omitted = total_found - next_offset;
            actions.push(NextAction::metadata(
                format!("{omitted} more hits: add --offset {next_offset} --limit {limit}."),
                "access pagination",
                90,
            ));
        }
    }
    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        out.push('\n');
        out.push_str(&rendered);
    }
}

impl AccessFilter {
    fn parse(filter: Option<&str>) -> Result<Self, SrcwalkError> {
        let Some(filter) = filter else {
            return Ok(Self::default());
        };
        let mut parsed = Self::default();
        for part in filter.split_whitespace() {
            let Some((field, value)) = part.split_once(':') else {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: "filters must use field:value qualifiers".to_string(),
                });
            };
            let field = field.trim().to_ascii_lowercase();
            let value = value.trim();
            if field.is_empty() || value.is_empty() {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: "filter field and value cannot be empty".to_string(),
                });
            }
            match field.as_str() {
                "access" => {
                    let Some(kind) = AccessKind::from_filter(&value.to_ascii_lowercase()) else {
                        return Err(SrcwalkError::InvalidQuery {
                            query: filter.to_string(),
                            reason: "access filter must be write, reset, read, or unknown"
                                .to_string(),
                        });
                    };
                    parsed.access.push(kind);
                }
                "path" => parsed.path.push(value.to_string()),
                "file" => parsed.file.push(value.to_string()),
                "text" => parsed.text.push(value.to_string()),
                "line" => parsed
                    .line
                    .push(super::filter::parse_line_range_filter(value, filter)?),
                "kind" => {
                    return Err(SrcwalkError::InvalidQuery {
                        query: filter.to_string(),
                        reason: "kind filters do not apply with discover --as access; use access:<write|reset|read|unknown>".to_string(),
                    });
                }
                _ => {
                    return Err(SrcwalkError::InvalidQuery {
                        query: filter.to_string(),
                        reason: format!(
                            "unsupported filter field `{field}`; use access, path, file, text, or line"
                        ),
                    });
                }
            }
        }
        Ok(parsed)
    }

    fn matches(&self, hit: &AccessHit, scope: &Path) -> bool {
        (self.access.is_empty() || self.access.contains(&hit.kind))
            && self
                .path
                .iter()
                .all(|value| path_match_contains(&rel_nonempty(&hit.path, scope), value))
            && self.file.iter().all(|value| {
                hit.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(value))
            })
            && self.text.iter().all(|value| hit.text.contains(value))
            && self
                .line
                .iter()
                .all(|(start, end)| *start <= hit.line && hit.line <= *end)
    }
}

#[cfg(test)]
mod tests {
    use super::is_function_outline_line;

    #[test]
    fn context_target_outline_filter_accepts_only_function_rows() {
        assert!(is_function_outline_line("[3-5]        fn mark_args"));
        assert!(is_function_outline_line("[3-5]        def render"));
        assert!(is_function_outline_line("[3-5]        fun render"));
        assert!(!is_function_outline_line("[1-10]       class Handler"));
        assert!(!is_function_outline_line("[2]          let value"));
        assert!(!is_function_outline_line("[4-8]        section Usage"));
    }
}

impl Counts {
    fn from_hits(hits: &[AccessHit]) -> Self {
        let mut counts = Self::default();
        for hit in hits {
            counts.add(hit.kind);
        }
        counts
    }

    fn add(&mut self, kind: AccessKind) {
        match kind {
            AccessKind::Write => self.write += 1,
            AccessKind::Reset => self.reset += 1,
            AccessKind::Read => self.read += 1,
            AccessKind::Unknown => self.unknown += 1,
        }
    }
}
