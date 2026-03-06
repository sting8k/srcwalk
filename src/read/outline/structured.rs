use std::path::Path;

/// Depth-limited outline for JSON, YAML, TOML.
pub fn outline(path: &Path, content: &str, max_lines: usize) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => json_outline(content, max_lines),
        Some("yaml" | "yml") => yaml_outline(content, max_lines),
        Some("toml") => toml_outline(content, max_lines),
        _ => key_value_outline(content, max_lines),
    }
}

fn json_outline(content: &str, max_lines: usize) -> String {
    let value: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => return format!("[parse error: {e}]"),
    };
    let mut lines = Vec::new();
    walk_json(&value, "", 0, 2, max_lines, &mut lines);
    lines.join("\n")
}

fn walk_json(
    value: &serde_json::Value,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    max_lines: usize,
    lines: &mut Vec<String>,
) {
    if lines.len() >= max_lines {
        return;
    }

    match value {
        serde_json::Value::Object(map) => {
            if depth >= max_depth {
                if !prefix.is_empty() {
                    lines.push(format!("{prefix}: {{{} keys}}", map.len()));
                }
                return;
            }
            for (key, val) in map {
                if lines.len() >= max_lines {
                    return;
                }
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match val {
                    serde_json::Value::Object(inner) => {
                        if depth + 1 >= max_depth {
                            let keys: Vec<&String> = inner.keys().take(5).collect();
                            let key_list = keys
                                .iter()
                                .map(|k| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            let suffix = if inner.len() > 5 { ", ..." } else { "" };
                            lines.push(format!(
                                "{full_key}: {{{} keys}} [{key_list}{suffix}]",
                                inner.len()
                            ));
                        } else {
                            walk_json(val, &full_key, depth + 1, max_depth, max_lines, lines);
                        }
                    }
                    serde_json::Value::Array(arr) => {
                        let preview = if arr.is_empty() {
                            "[]".to_string()
                        } else {
                            let first = truncate_json_value(&arr[0], 40);
                            format!("[{} items] [{first}]", arr.len())
                        };
                        lines.push(format!("{full_key}: {preview}"));
                    }
                    _ => {
                        let val_str = truncate_json_value(val, 40);
                        let type_name = json_type_name(val);
                        lines.push(format!("{full_key}: {val_str} ({type_name})"));
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            lines.push(format!("{prefix}: [{} items]", arr.len()));
        }
        _ => {
            let val_str = truncate_json_value(value, 40);
            lines.push(format!("{prefix}: {val_str}"));
        }
    }
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "string",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Null => "null",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn truncate_json_value(v: &serde_json::Value, max: usize) -> String {
    let s = match v {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.len() > max {
        format!(
            "{}...",
            crate::types::truncate_str(&s, max.saturating_sub(3))
        )
    } else {
        s
    }
}

/// YAML outline via line scan — no parser needed.
/// Detect keys by: optional whitespace, then a word, then `: ` or `:`+EOL.
/// Indentation level = nesting depth (2-space standard).
fn yaml_outline(content: &str, max_lines: usize) -> String {
    let mut entries = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if entries.len() >= max_lines {
            break;
        }
        let trimmed = line.trim_start();
        // Skip comments, blank lines, and list items
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        // Look for key: value or key: (block)
        if let Some(colon) = trimmed.find(':') {
            let key = &trimmed[..colon];
            // Keys shouldn't contain spaces (that would be a value line)
            if key.contains(' ') {
                continue;
            }
            let indent = line.len() - trimmed.len();
            let depth = indent / 2;
            if depth <= 2 {
                let prefix = "  ".repeat(depth);
                let after_colon = trimmed[colon + 1..].trim();
                if after_colon.is_empty() {
                    // Block mapping — just show key
                    entries.push(format!("[{}] {prefix}{key}:", i + 1));
                } else {
                    let val = if after_colon.len() > 40 {
                        format!("{}...", crate::types::truncate_str(after_colon, 37))
                    } else {
                        after_colon.to_string()
                    };
                    entries.push(format!("[{}] {prefix}{key}: {val}", i + 1));
                }
            }
        }
    }
    entries.join("\n")
}

fn toml_outline(content: &str, max_lines: usize) -> String {
    let value: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(e) => return format!("[parse error: {e}]"),
    };
    let mut lines = Vec::new();
    walk_toml(&value, 0, 2, max_lines, &mut lines);
    lines.join("\n")
}

fn walk_toml(
    value: &toml::Value,
    depth: usize,
    max_depth: usize,
    max_lines: usize,
    lines: &mut Vec<String>,
) {
    if lines.len() >= max_lines {
        return;
    }
    let indent = "  ".repeat(depth);

    if let toml::Value::Table(table) = value {
        for (key, val) in table {
            if lines.len() >= max_lines {
                return;
            }
            match val {
                toml::Value::Table(inner) if depth < max_depth => {
                    lines.push(format!("{indent}[{key}]"));
                    walk_toml(val, depth + 1, max_depth, max_lines, lines);
                }
                toml::Value::Table(inner) => {
                    lines.push(format!("{indent}{key}: {{{} keys}}", inner.len()));
                }
                toml::Value::Array(arr) => {
                    lines.push(format!("{indent}{key}: [{} items]", arr.len()));
                }
                _ => {
                    let val_str = val.to_string();
                    let truncated = if val_str.len() > 40 {
                        format!("{}...", crate::types::truncate_str(&val_str, 37))
                    } else {
                        val_str
                    };
                    lines.push(format!("{indent}{key}: {truncated}"));
                }
            }
        }
    }
}

fn key_value_outline(content: &str, max_lines: usize) -> String {
    content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}
