use std::path::Path;

use crate::evidence::{
    bounded_line_range_indices, render_next_actions, Anchor, EvidenceSource, NextAction,
};
use crate::lang::outline::get_outline_entries;
use crate::types::{FileType, OutlineEntry, OutlineKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PartialFunctionBoundary {
    span: (u32, u32),
    missing: (u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StructuralFrame {
    name: String,
    span: (u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StructuralReadFrame {
    Enclosed(Vec<StructuralFrame>),
    Spans(usize),
    OutsideAnyFunction,
}

pub(crate) fn partial_function_completion(
    path: &Path,
    content: &str,
    file_type: FileType,
    selected_start: u32,
    selected_end: u32,
) -> Option<String> {
    if !file_type.is_code() {
        return None;
    }
    let lang = file_type.structural_lang()?;
    crate::lang::outline::outline_language(lang)?;

    let entries = get_outline_entries(content, lang);
    let partial_boundaries = partial_function_boundaries(&entries, selected_start, selected_end);
    if partial_boundaries.is_empty() {
        return None;
    }

    let missing_ranges = missing_function_ranges(&partial_boundaries);
    if missing_ranges.is_empty() {
        return None;
    }
    let span_description = structural_spans_text(&partial_boundaries);
    let all_range_text = missing_ranges
        .iter()
        .map(|(start, end)| format_line_range(*start, *end))
        .collect::<Vec<_>>()
        .join(",");
    let kept_indices = bounded_line_range_indices(&missing_ranges);
    let kept_ranges = kept_indices
        .iter()
        .map(|&index| missing_ranges[index])
        .collect::<Vec<_>>();
    let kept_range_text = kept_ranges
        .iter()
        .map(|(start, end)| format_line_range(*start, *end))
        .collect::<Vec<_>>()
        .join(",");
    path.to_str()?;
    let caveat = format!(
        "> Caveat: selected lines {selected_start}-{selected_end} are partial inside {span_description}; omitted lines {all_range_text}."
    );
    if kept_ranges.is_empty() {
        return Some(caveat);
    }

    let display_path = crate::format::display_path(path);
    let (anchor_start, anchor_end) = kept_ranges[0];
    let anchor = Anchor::lines(path, anchor_start, anchor_end);
    let command = if kept_ranges.len() == 1 {
        let target = crate::format::shell_quote_arg(&format!("{display_path}:{kept_range_text}"))?;
        format!("srcwalk show {target}")
    } else {
        let path_arg = crate::format::shell_quote_arg(&display_path)?;
        format!("srcwalk show {path_arg} --section {kept_range_text}")
    };
    let next = render_next_actions(&[NextAction::from_evidence(
        command,
        "read omitted structural function lines",
        10,
        EvidenceSource::Ast,
        anchor,
    )]);
    Some(format!("{caveat}\n{next}"))
}

pub(crate) fn structural_read_frame(
    content: &str,
    file_type: FileType,
    requested_start: u32,
    requested_end: u32,
    displayed_start: u32,
    displayed_end: u32,
) -> Option<String> {
    if !file_type.is_code() {
        return None;
    }
    let lang = file_type.structural_lang()?;
    crate::lang::outline::outline_language(lang)?;

    let entries = get_outline_entries(content, lang);
    let frame = structural_read_frame_for_range(&entries, requested_start, requested_end)?;

    Some(render_structural_read_frame(
        frame,
        requested_start,
        requested_end,
        displayed_start,
        displayed_end,
    ))
}

pub(crate) fn structural_read_frame_from_entries(
    entries: &[OutlineEntry],
    requested_start: u32,
    requested_end: u32,
    displayed_start: u32,
    displayed_end: u32,
) -> Option<String> {
    let frame = structural_read_frame_for_range(entries, requested_start, requested_end)?;
    Some(render_structural_read_frame(
        frame,
        requested_start,
        requested_end,
        displayed_start,
        displayed_end,
    ))
}

fn missing_function_ranges(partial_boundaries: &[PartialFunctionBoundary]) -> Vec<(u32, u32)> {
    let mut ranges = partial_boundaries
        .iter()
        .map(|boundary| boundary.missing)
        .filter(|(start, end)| start <= end)
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some((_, last_end)) = merged.last_mut() {
            if start <= last_end.saturating_add(1) {
                *last_end = (*last_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn render_structural_read_frame(
    frame: StructuralReadFrame,
    requested_start: u32,
    requested_end: u32,
    displayed_start: u32,
    displayed_end: u32,
) -> String {
    let requested = format_line_range(requested_start, requested_end);
    let displayed = format_line_range(displayed_start, displayed_end);
    let suffix = match frame {
        StructuralReadFrame::Enclosed(frames) => {
            let scope = structural_frames_text(&frames);
            let coverage = if frames.len() == 1
                && requested_start <= frames[0].span.0
                && requested_end >= frames[0].span.1
            {
                "complete"
            } else {
                "partial"
            };
            format!("within {scope}; {coverage}")
        }
        StructuralReadFrame::Spans(count) => {
            let noun = if count == 1 {
                "structural function"
            } else {
                "structural functions"
            };
            format!("spans {count} {noun}; not enclosed")
        }
        StructuralReadFrame::OutsideAnyFunction => "outside any function span".to_string(),
    };

    format!("> Source frame: requested {requested}; displayed {displayed}; {suffix}.")
}

fn structural_spans_text(partial_boundaries: &[PartialFunctionBoundary]) -> String {
    let mut spans = partial_boundaries
        .iter()
        .map(|boundary| boundary.span)
        .collect::<Vec<_>>();
    spans.sort_unstable();
    spans.dedup();
    let span_text = spans
        .iter()
        .map(|(start, end)| format!("{start}-{end}"))
        .collect::<Vec<_>>()
        .join(",");
    if spans.len() == 1 {
        format!("structural function {span_text}")
    } else {
        format!("structural functions {span_text}")
    }
}

fn format_line_range(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}
fn partial_function_boundaries(
    entries: &[OutlineEntry],
    selected_start: u32,
    selected_end: u32,
) -> Vec<PartialFunctionBoundary> {
    let mut boundaries = Vec::new();
    if let Some(span) = smallest_partial_function_for_start(entries, selected_start) {
        boundaries.push(PartialFunctionBoundary {
            span,
            missing: (span.0, selected_start - 1),
        });
    }
    if let Some(span) = smallest_partial_function_for_end(entries, selected_end) {
        let boundary = PartialFunctionBoundary {
            span,
            missing: (selected_end + 1, span.1),
        };
        if !boundaries.contains(&boundary) {
            boundaries.push(boundary);
        }
    }
    boundaries
}

fn structural_read_frame_for_range(
    entries: &[OutlineEntry],
    requested_start: u32,
    requested_end: u32,
) -> Option<StructuralReadFrame> {
    if !contains_function(entries) {
        return None;
    }

    let frames = structural_overlapping_frames(entries, requested_start, requested_end);
    let containing = frames
        .iter()
        .filter(|frame| frame.span.0 <= requested_start && requested_end <= frame.span.1)
        .cloned()
        .collect::<Vec<_>>();
    if !containing.is_empty() {
        return Some(StructuralReadFrame::Enclosed(smallest_frames(containing)));
    }

    if frames.is_empty() {
        return Some(StructuralReadFrame::OutsideAnyFunction);
    }

    Some(StructuralReadFrame::Spans(outermost_span_count(&frames)))
}

fn structural_overlapping_frames(
    entries: &[OutlineEntry],
    requested_start: u32,
    requested_end: u32,
) -> Vec<StructuralFrame> {
    fn visit(
        entries: &[OutlineEntry],
        requested_start: u32,
        requested_end: u32,
        frames: &mut Vec<StructuralFrame>,
    ) {
        for entry in entries {
            if is_valid_function(entry)
                && entry.start_line <= requested_end
                && entry.end_line >= requested_start
            {
                frames.push(StructuralFrame {
                    name: entry.name.clone(),
                    span: (entry.start_line, entry.end_line),
                });
            }
            visit(&entry.children, requested_start, requested_end, frames);
        }
    }

    let mut frames = Vec::new();
    visit(entries, requested_start, requested_end, &mut frames);
    frames.sort_by_key(|frame| (frame.span.0, frame.span.1, frame.name.clone()));
    frames.dedup_by(|a, b| a.span == b.span && a.name == b.name);
    frames
}

fn contains_function(entries: &[OutlineEntry]) -> bool {
    entries
        .iter()
        .any(|entry| is_valid_function(entry) || contains_function(&entry.children))
}

fn is_valid_function(entry: &OutlineEntry) -> bool {
    entry.kind == OutlineKind::Function
        && entry.start_line > 0
        && entry.end_line >= entry.start_line
}

fn outermost_span_count(frames: &[StructuralFrame]) -> usize {
    let mut spans = frames.iter().map(|frame| frame.span).collect::<Vec<_>>();
    spans.sort_unstable();
    spans.dedup();
    spans
        .iter()
        .filter(|span| {
            !spans
                .iter()
                .any(|other| other != *span && other.0 <= span.0 && span.1 <= other.1)
        })
        .count()
}

fn smallest_frames(frames: Vec<StructuralFrame>) -> Vec<StructuralFrame> {
    let min_span = frames
        .iter()
        .map(|frame| frame.span.1.saturating_sub(frame.span.0))
        .min()
        .unwrap_or(u32::MAX);
    frames
        .into_iter()
        .filter(|frame| frame.span.1.saturating_sub(frame.span.0) == min_span)
        .collect()
}

fn structural_frames_text(frames: &[StructuralFrame]) -> String {
    let parts = frames
        .iter()
        .map(|frame| {
            let span = format_line_range(frame.span.0, frame.span.1);
            let name = frame.name.trim();
            if name.is_empty() {
                format!("structural function {span}")
            } else {
                format!("fn {name} {span}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if frames.len() == 1 {
        parts
    } else {
        format!("structural functions {parts}")
    }
}

fn smallest_partial_function_for_start(entries: &[OutlineEntry], line: u32) -> Option<(u32, u32)> {
    smallest_function_matching_boundary(entries, line, |entry| entry.start_line < line)
}

fn smallest_partial_function_for_end(entries: &[OutlineEntry], line: u32) -> Option<(u32, u32)> {
    smallest_function_matching_boundary(entries, line, |entry| line < entry.end_line)
}

fn smallest_function_matching_boundary(
    entries: &[OutlineEntry],
    line: u32,
    is_partial: impl Fn(&OutlineEntry) -> bool + Copy,
) -> Option<(u32, u32)> {
    fn visit(
        entries: &[OutlineEntry],
        line: u32,
        is_partial: impl Fn(&OutlineEntry) -> bool + Copy,
        best: &mut Option<(u32, u32)>,
    ) {
        for entry in entries {
            if is_valid_function(entry)
                && entry.start_line <= line
                && line <= entry.end_line
                && is_partial(entry)
            {
                let candidate = (entry.start_line, entry.end_line);
                let candidate_span = entry.end_line.saturating_sub(entry.start_line);
                let best_span = best.map_or(u32::MAX, |(start, end)| end.saturating_sub(start));
                if candidate_span < best_span {
                    *best = Some(candidate);
                }
            }
            visit(&entry.children, line, is_partial, best);
        }
    }

    let mut best = None;
    visit(entries, line, is_partial, &mut best);
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn function(start_line: u32, end_line: u32, children: Vec<OutlineEntry>) -> OutlineEntry {
        named_function("", start_line, end_line, children)
    }

    fn named_function(
        name: &str,
        start_line: u32,
        end_line: u32,
        children: Vec<OutlineEntry>,
    ) -> OutlineEntry {
        OutlineEntry {
            kind: OutlineKind::Function,
            name: name.to_string(),
            start_line,
            end_line,
            signature: None,
            children,
            doc: None,
        }
    }

    #[test]
    fn shell_quote_arg_preserves_safe_arguments_and_rejects_controls() {
        assert_eq!(
            crate::format::shell_quote_arg("src/read/file.rs:4-6"),
            Some("src/read/file.rs:4-6".to_string())
        );
        assert_eq!(crate::format::shell_quote_arg("bad\npath:4-6"), None);
    }

    #[test]
    fn shell_quote_arg_quotes_spaces_and_apostrophes() {
        let raw = "dir with space/file's.rs:4-6";
        let quoted = crate::format::shell_quote_arg(raw).unwrap();

        #[cfg(windows)]
        assert_eq!(quoted, "'dir with space/file''s.rs:4-6'");
        #[cfg(not(windows))]
        assert_eq!(quoted, "'dir with space/file'\\''s.rs:4-6'");
    }

    #[cfg(unix)]
    #[test]
    fn shell_quote_arg_is_executable_by_posix_shell() {
        let raw = "dir with space/file's.rs:4-6";
        let quoted = crate::format::shell_quote_arg(raw).unwrap();
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("set -- {quoted}; printf %s \"$1\""))
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), raw);
    }

    #[test]
    fn structural_frame_prefers_nested_function_for_inner_range() {
        let entries = vec![named_function(
            "outer",
            1,
            10,
            vec![named_function("inner", 3, 5, Vec::new())],
        )];

        assert_eq!(
            structural_read_frame_for_range(&entries, 4, 4),
            Some(StructuralReadFrame::Enclosed(vec![StructuralFrame {
                name: "inner".to_string(),
                span: (3, 5)
            }]))
        );
    }

    #[test]
    fn structural_frame_keeps_parent_for_full_parent_range() {
        let entries = vec![named_function(
            "outer",
            1,
            10,
            vec![named_function("inner", 3, 5, Vec::new())],
        )];

        assert_eq!(
            structural_read_frame_for_range(&entries, 1, 10),
            Some(StructuralReadFrame::Enclosed(vec![StructuralFrame {
                name: "outer".to_string(),
                span: (1, 10)
            }]))
        );
    }

    #[test]
    fn structural_frame_reports_non_enclosed_ranges() {
        let entries = vec![named_function("only", 3, 5, Vec::new())];

        assert_eq!(
            structural_read_frame_for_range(&entries, 2, 5),
            Some(StructuralReadFrame::Spans(1))
        );
        assert_eq!(
            structural_read_frame_for_range(&entries, 3, 6),
            Some(StructuralReadFrame::Spans(1))
        );
    }

    #[test]
    fn structural_frame_reports_outside_any_function_span() {
        let entries = vec![named_function("only", 3, 5, Vec::new())];

        assert_eq!(
            structural_read_frame_for_range(&entries, 1, 2),
            Some(StructuralReadFrame::OutsideAnyFunction)
        );
    }

    #[test]
    fn structural_frame_abstains_when_outline_has_no_functions() {
        let entries = vec![OutlineEntry {
            kind: OutlineKind::Struct,
            name: "Container".to_string(),
            start_line: 1,
            end_line: 5,
            signature: None,
            children: Vec::new(),
            doc: None,
        }];

        assert_eq!(structural_read_frame_for_range(&entries, 2, 4), None);
        assert_eq!(structural_read_frame_for_range(&[], 2, 4), None);
    }

    #[test]
    fn structural_frame_counts_outermost_overlapping_spans() {
        let entries = vec![
            named_function(
                "outer",
                1,
                10,
                vec![named_function("inner", 3, 5, Vec::new())],
            ),
            named_function("sibling", 12, 14, Vec::new()),
        ];

        assert_eq!(
            structural_read_frame_for_range(&entries, 4, 13),
            Some(StructuralReadFrame::Spans(2))
        );
    }

    #[test]
    fn structural_frame_property_never_emits_behavior_claims() {
        let entries = vec![
            named_function(
                "outer",
                3,
                10,
                vec![named_function("inner", 5, 7, Vec::new())],
            ),
            named_function("sibling", 13, 15, Vec::new()),
        ];
        let allowed = [
            "Source",
            "frame",
            "requested",
            "displayed",
            "within",
            "fn",
            "spans",
            "structural",
            "function",
            "functions",
            "outside",
            "any",
            "span",
            "not",
            "enclosed",
            "complete",
            "partial",
            "outer",
            "inner",
            "sibling",
        ];
        let banned = [
            "calls",
            "returns",
            "depends",
            "runtime",
            "owns",
            "implements",
            "invokes",
            "because",
        ];

        for start in 1..=17 {
            for end in start..=17 {
                let Some(frame) = structural_read_frame_for_range(&entries, start, end) else {
                    continue;
                };
                let is_enclosed = matches!(frame, StructuralReadFrame::Enclosed(_));
                let line = render_structural_read_frame(frame, start, end, start, end);
                let lower = line.to_ascii_lowercase();

                for word in banned {
                    assert!(
                        !lower.contains(word),
                        "frame must not emit behavior word `{word}` for {start}-{end}: {line}"
                    );
                }
                if !is_enclosed {
                    assert!(
                        !line.contains("complete"),
                        "non-enclosed frame must not claim complete for {start}-{end}: {line}"
                    );
                    for name in ["outer", "inner", "sibling"] {
                        assert!(
                            !line.contains(name),
                            "non-enclosed frame must not name functions for {start}-{end}: {line}"
                        );
                    }
                }

                for token in line.split(|ch: char| !ch.is_ascii_alphanumeric()) {
                    if token.is_empty() || token.chars().all(|ch| ch.is_ascii_digit()) {
                        continue;
                    }
                    assert!(
                        allowed.contains(&token),
                        "unexpected token `{token}` for {start}-{end}: {line}"
                    );
                }
            }
        }
    }

    #[test]
    fn boundary_function_span_prefers_nested_function() {
        let entries = vec![function(1, 10, vec![function(3, 5, Vec::new())])];

        assert_eq!(
            partial_function_boundaries(&entries, 4, 4),
            vec![
                PartialFunctionBoundary {
                    span: (3, 5),
                    missing: (3, 3)
                },
                PartialFunctionBoundary {
                    span: (3, 5),
                    missing: (5, 5)
                }
            ]
        );
    }

    #[test]
    fn boundary_function_span_uses_partial_ancestor_when_nested_span_is_complete() {
        let entries = vec![function(1, 10, vec![function(3, 5, Vec::new())])];

        assert_eq!(
            partial_function_boundaries(&entries, 3, 10),
            vec![PartialFunctionBoundary {
                span: (1, 10),
                missing: (1, 2)
            }]
        );
        assert_eq!(
            partial_function_boundaries(&entries, 1, 5),
            vec![PartialFunctionBoundary {
                span: (1, 10),
                missing: (6, 10)
            }]
        );
    }

    #[test]
    fn missing_function_ranges_are_sorted_and_merged() {
        let boundaries = [
            PartialFunctionBoundary {
                span: (3, 5),
                missing: (3, 3),
            },
            PartialFunctionBoundary {
                span: (1, 10),
                missing: (1, 3),
            },
            PartialFunctionBoundary {
                span: (1, 10),
                missing: (9, 10),
            },
        ];

        assert_eq!(missing_function_ranges(&boundaries), vec![(1, 3), (9, 10)]);
    }

    fn rust_function(line_count: u32) -> String {
        let mut content = String::from("fn target() {\n");
        for line in 2..line_count {
            writeln!(content, "    let v{line} = {line};").unwrap();
        }
        content.push_str("}\n");
        content
    }

    #[test]
    fn completion_skips_wide_first_range_and_anchors_surviving_singleton() {
        let output = partial_function_completion(
            Path::new("lib.rs"),
            &rust_function(300),
            FileType::Code(crate::types::Lang::Rust),
            250,
            260,
        )
        .unwrap();

        assert!(output.contains("omitted lines 1-249,261-300"), "{output}");
        assert!(
            output.contains("> Next: srcwalk show lib.rs:261-300"),
            "{output}"
        );
        assert!(!output.contains("--section"), "{output}");
    }

    #[test]
    fn completion_keeps_caveat_when_all_ranges_exceed_cap() {
        let output = partial_function_completion(
            Path::new("lib.rs"),
            &rust_function(500),
            FileType::Code(crate::types::Lang::Rust),
            251,
            251,
        )
        .unwrap();

        assert!(output.contains("omitted lines 1-250,252-500"), "{output}");
        assert!(!output.contains("> Next:"), "{output}");
    }
}
