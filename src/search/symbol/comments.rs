use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::lang::detect_file_type;
use crate::lang::outline::outline_language;
use crate::types::{FileType, Match};

type CommentFileData = (Vec<(usize, usize)>, Vec<usize>);

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

/// Parse one file and retain comment ranges and line starts for tagging.
fn parse_comment_file(path: &Path) -> CommentFileData {
    let ts_lang = match detect_file_type(path) {
        FileType::Code(lang) => outline_language(lang),
        _ => None,
    };
    let Some(ts_lang) = ts_lang else {
        return (Vec::new(), Vec::new());
    };

    let Ok(content) = std::fs::read_to_string(path) else {
        return (Vec::new(), Vec::new());
    };
    let line_starts = collect_line_starts(&content);

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return (Vec::new(), line_starts);
    }
    let Some(tree) = parser.parse(&content, None) else {
        return (Vec::new(), line_starts);
    };

    (collect_comment_ranges(tree.root_node()), line_starts)
}

fn collect_line_starts(content: &str) -> Vec<usize> {
    let mut line_starts = vec![0];
    line_starts.extend(
        content
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    line_starts
}

/// Tag `in_comment` on usage matches by parsing each file with tree-sitter.
/// Only files that have at least one usage match are parsed.
pub(super) fn tag_comment_matches(buckets: &mut [Vec<Match>]) {
    let mut seen = HashSet::new();
    let mut file_paths = Vec::new();
    for bucket in buckets.iter() {
        for m in bucket {
            if !m.is_definition && seen.insert(m.path.clone()) {
                file_paths.push(m.path.clone());
            }
        }
    }

    let file_data: HashMap<PathBuf, CommentFileData> = file_paths
        .par_iter()
        .map(|path| (path.clone(), parse_comment_file(path)))
        .collect();

    for bucket in buckets.iter_mut() {
        for m in bucket.iter_mut() {
            if m.is_definition {
                continue;
            }
            let Some((ranges, line_starts)) = file_data.get(&m.path) else {
                continue;
            };
            if ranges.is_empty() {
                continue;
            }
            let Some(line_index) = (m.line as usize).checked_sub(1) else {
                continue;
            };
            let Some(byte_offset) = line_starts.get(line_index).copied() else {
                continue;
            };
            m.in_comment = is_in_comment(byte_offset, ranges);
        }
    }
}
