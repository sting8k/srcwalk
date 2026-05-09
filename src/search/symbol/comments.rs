use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::types::{FileType, Match};

/// Collect sorted byte-offset ranges of all comment nodes in a tree-sitter tree.
/// Works across all supported languages — tree-sitter grammars universally use
/// node kinds containing "comment" for line, block, and doc comments.
fn collect_comment_ranges(root: tree_sitter::Node) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    collect_comment_ranges_recursive(&mut cursor, &mut ranges);
    ranges
}

fn collect_comment_ranges_recursive(
    cursor: &mut tree_sitter::TreeCursor,
    ranges: &mut Vec<(usize, usize)>,
) {
    loop {
        let node = cursor.node();
        if node.kind().contains("comment") {
            ranges.push((node.start_byte(), node.end_byte()));
        } else if cursor.goto_first_child() {
            collect_comment_ranges_recursive(cursor, ranges);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Check whether a byte offset falls inside any comment range (binary search).
fn is_in_comment(offset: usize, comment_ranges: &[(usize, usize)]) -> bool {
    comment_ranges
        .binary_search_by(|&(start, end)| {
            if offset < start {
                std::cmp::Ordering::Greater
            } else if offset >= end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Tag `in_comment` on usage matches by parsing each file with tree-sitter.
/// Only files that have at least one usage match are parsed.
pub(super) fn tag_comment_matches(buckets: &mut [Vec<Match>]) {
    use std::collections::HashMap;

    // Collect all unique file paths that need comment-checking.
    let mut file_paths: HashMap<std::path::PathBuf, Vec<(usize, usize)>> = HashMap::new();
    for bucket in buckets.iter() {
        for m in bucket {
            if !m.is_definition {
                file_paths.entry(m.path.clone()).or_default();
            }
        }
    }

    // Parse each file once, collect comment ranges.
    for (path, ranges) in &mut file_paths {
        let lang = detect_file_type(path);
        let ts_lang = match lang {
            FileType::Code(l) => outline_language(l),
            _ => None,
        };
        let Some(ts_lang) = ts_lang else { continue };
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&ts_lang).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&content, None) else {
            continue;
        };
        *ranges = collect_comment_ranges(tree.root_node());
    }

    // Tag each usage match.
    for bucket in buckets.iter_mut() {
        for m in bucket.iter_mut() {
            if m.is_definition {
                continue;
            }
            if let Some(ranges) = file_paths.get(&m.path) {
                if ranges.is_empty() {
                    continue;
                }
                // Convert line number to byte offset: read file and find line start.
                // We need the byte offset of the match line to check against comment ranges.
                // Since we already read the file above, re-read is cached by OS.
                if let Ok(content) = std::fs::read_to_string(&m.path) {
                    if let Some(byte_offset) = line_to_byte_offset(&content, m.line as usize) {
                        m.in_comment = is_in_comment(byte_offset, ranges);
                    }
                }
            }
        }
    }
}

/// Convert 1-based line number to byte offset of that line's start.
fn line_to_byte_offset(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    let mut current_line = 1usize;
    if line == 1 {
        return Some(0);
    }
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(i + 1);
            }
        }
    }
    None
}
