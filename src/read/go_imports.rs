/// Extract Go import paths from single and parenthesized import declarations.
pub(crate) fn import_sources(content: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut in_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if in_block {
            if closes_block(trimmed) {
                in_block = false;
                continue;
            }
            if let Some(source) = import_spec_source(trimmed) {
                sources.push(source);
            }
            continue;
        }

        let Some(rest) = crate::read::keyword_rest(trimmed, "import") else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with('(') {
            in_block = true;
            continue;
        }
        if let Some(source) = import_spec_source(rest) {
            sources.push(source);
        }
    }

    sources
}

fn closes_block(line: &str) -> bool {
    line == ")"
        || line
            .strip_prefix(')')
            .is_some_and(|rest| rest.trim_start().starts_with("//"))
}

fn import_spec_source(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.starts_with("//") || spec.starts_with("/*") || spec.starts_with('*')
    {
        return None;
    }

    let (quote_pos, quote) = spec
        .char_indices()
        .find(|(_, ch)| matches!(*ch, '"' | '`'))?;
    let prefix = spec[..quote_pos].trim();
    if !prefix.is_empty() && !is_import_alias(prefix) {
        return None;
    }

    let quoted = &spec[quote_pos + quote.len_utf8()..];
    let end = quoted.find(quote)?;
    let source = &quoted[..end];
    (!source.is_empty()).then_some(source.to_string())
}

fn is_import_alias(value: &str) -> bool {
    value == "_"
        || value == "."
        || value.chars().enumerate().all(|(index, ch)| {
            ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::import_sources;

    #[test]
    fn extracts_single_grouped_and_mixed_imports() {
        let content = r#"
package main

import "single/pkg"
import (
    "fmt"
    alias "app/internal/util"
    _ "app/side"
)
import . `golang.org/x/tools`

var text = "import ("
var other = "outside/pkg"
"#;

        assert_eq!(
            import_sources(content),
            [
                "single/pkg",
                "fmt",
                "app/internal/util",
                "app/side",
                "golang.org/x/tools",
            ]
        );
    }

    #[test]
    fn ignores_import_like_text_outside_a_block() {
        let content = r#"
package main

var text = "import ("
var path = "fake/outside"
"#;

        assert!(import_sources(content).is_empty());
    }

    #[test]
    fn ignores_invalid_block_rows_and_accepts_close_comments() {
        let content =
            "import (\n    invalid = \"not/an/import\"\n    \"fmt\" // comment\n) // done\n";

        assert_eq!(import_sources(content), ["fmt"]);
    }
}
