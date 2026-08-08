use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Extract static PHP namespace/use and require/include sources with source
/// line attribution. Dynamic expressions are intentionally ignored.
pub(crate) fn import_sources(content: &str) -> Vec<(String, u32)> {
    let Some(language) = crate::lang::outline::outline_language(crate::types::Lang::Php) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };

    let mut sources = Vec::new();
    collect_sources(tree.root_node(), content, &mut sources);
    sources
}

fn collect_sources(node: tree_sitter::Node<'_>, content: &str, out: &mut Vec<(String, u32)>) {
    match node.kind() {
        "namespace_use_declaration" => {
            collect_php_use(node, content, out);
            return;
        }
        "include_expression"
        | "include_once_expression"
        | "require_expression"
        | "require_once_expression" => {
            if let Some(source) = php_static_string(node, content) {
                out.push((source, node.start_position().row as u32 + 1));
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_sources(child, content, out);
    }
}

fn collect_php_use(node: tree_sitter::Node<'_>, content: &str, out: &mut Vec<(String, u32)>) {
    let line = node.start_position().row as u32 + 1;
    let prefix = node
        .named_children(&mut node.walk())
        .find(|child| child.kind() == "namespace_name")
        .and_then(|child| content.get(child.byte_range()))
        .map(str::to_string);
    if let Some(group) = node.child_by_field_name("body") {
        let mut cursor = group.walk();
        for clause in group.named_children(&mut cursor) {
            if let Some(name) = php_clause_source(clause, content) {
                let source = prefix
                    .as_deref()
                    .map_or(name.clone(), |prefix| format!("{prefix}\\{name}"));
                out.push((source, line));
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for clause in node.named_children(&mut cursor) {
        if let Some(source) = php_clause_source(clause, content) {
            out.push((source, line));
        }
    }
}

fn php_clause_source(node: tree_sitter::Node<'_>, content: &str) -> Option<String> {
    if node.kind() != "namespace_use_clause" {
        return None;
    }
    let mut cursor = node.walk();
    let source = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "name" | "qualified_name" | "relative_name"))
        .and_then(|child| content.get(child.byte_range()))
        .map(|source| source.trim().trim_matches('\\').to_string())
        .filter(|source| !source.is_empty());
    source
}

fn php_static_string(node: tree_sitter::Node<'_>, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    let string = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "string" | "encapsed_string"))
        .and_then(|child| content.get(child.byte_range()))?
        .trim();
    let string = string
        .strip_prefix("b'")
        .or_else(|| string.strip_prefix("B'"))
        .or_else(|| string.strip_prefix("b\""))
        .or_else(|| string.strip_prefix("B\""))
        .unwrap_or(string);
    let source = string.trim_matches(|ch| ch == '\'' || ch == '"');
    (!source.is_empty()).then_some(source.to_string())
}

/// Resolve a static PHP import using a relative file or the nearest Composer
/// `autoload.psr-4` map. Vendor/package guesses intentionally stay unresolved.
pub(crate) fn resolve(dir: &Path, source: &str, scope: Option<&Path>) -> Option<PathBuf> {
    if is_relative_path(source) {
        return existing_file(&dir.join(source), scope);
    }

    let (root, composer) = nearest_composer(dir)?;
    let mappings = composer.get("autoload")?.get("psr-4")?.as_object()?;
    let mut candidates = HashSet::new();

    for (prefix, dirs) in mappings {
        let Some(remainder) = strip_namespace_prefix(source, prefix) else {
            continue;
        };
        match dirs {
            Value::Array(dirs) => {
                for dir in dirs.iter().filter_map(Value::as_str) {
                    add_candidate(&root, dir, remainder, &mut candidates, scope);
                }
            }
            Value::String(dir) => add_candidate(&root, dir, remainder, &mut candidates, scope),
            _ => {}
        }
    }

    (candidates.len() == 1)
        .then(|| candidates.into_iter().next())
        .flatten()
}

fn is_relative_path(source: &str) -> bool {
    source == "."
        || source == ".."
        || source.starts_with("./")
        || source.starts_with("../")
        || source.starts_with(".\\")
        || source.starts_with("..\\")
}

fn strip_namespace_prefix<'a>(source: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim().trim_matches('\\');
    let source = source.trim().trim_matches('\\');
    if source == prefix {
        Some("")
    } else {
        source
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('\\'))
    }
}

fn add_candidate(
    root: &Path,
    base: &str,
    remainder: &str,
    candidates: &mut HashSet<PathBuf>,
    scope: Option<&Path>,
) {
    if remainder.is_empty() {
        return;
    }
    let rel = remainder.replace('\\', "/");
    let candidate = root.join(base).join(format!("{rel}.php"));
    if let Some(path) = existing_file(&candidate, scope) {
        candidates.insert(path);
    }
}

fn nearest_composer(start: &Path) -> Option<(PathBuf, Value)> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let path = dir.join("composer.json");
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str(&content) {
                return Some((dir.to_path_buf(), value));
            }
        }
        current = dir.parent();
    }
    None
}

fn existing_file(path: &Path, scope: Option<&Path>) -> Option<PathBuf> {
    let path = path
        .is_file()
        .then(|| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))?;
    scope
        .is_none_or(|scope| crate::read::imports::path_within_scope(&path, scope))
        .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::import_sources;

    #[test]
    fn extracts_php_use_groups_and_static_file_imports() {
        let code = r#"<?php
use App\Service\Runner;
use function App\Fns\run;
use App\{Entity\User, Support\Clock as Time};
require '../bootstrap.php';
include_once "../config.php";
require($dynamic);
"#;
        let sources: Vec<_> = import_sources(code)
            .into_iter()
            .map(|(source, _)| source)
            .collect();
        assert_eq!(
            sources,
            [
                "App\\Service\\Runner",
                "App\\Fns\\run",
                "App\\Entity\\User",
                "App\\Support\\Clock",
                "../bootstrap.php",
                "../config.php",
            ]
        );
    }
}
