//! Shared owner-region model and selection semantics for multilanguage owner
//! attribution (US-067 phase 1).
//!
//! Each supported language adapter parses a file into a flat list of nested
//! callable `OwnerRegion`s. A region is either a `Named` owner (an `OwnerAnchor`)
//! or an `AnonymousBarrier` (an anonymous callable that must not let a hit fall
//! through to an enclosing named owner). For a given hit line, the shared
//! `narrowest_named_owner` selects the unique narrowest containing region and
//! returns its anchor only when that region is `Named`; a barrier or an
//! equal-width tie abstains.
//!
//! Language-specific traversal and container logic stay in each adapter
//! (`python.rs`, `rust.rs`, `js_ts.rs`); this module owns only the shared region
//! model, the unique-narrowest selection, and local-`ERROR` primitives.

pub(crate) mod python;
pub(crate) mod rust;

use crate::evidence::owner_links::OwnerAnchor;

/// A callable region in a source file. `Named` regions carry an owner anchor
/// (a supported named callable); `AnonymousBarrier` regions are anonymous
/// callables that must abstain so a hit inside them never falls through to an
/// enclosing named owner.
#[derive(Debug, Clone)]
pub(crate) enum OwnerRegion {
    Named(OwnerAnchor),
    AnonymousBarrier { start_line: u32, end_line: u32 },
}

impl OwnerRegion {
    pub(crate) fn start_line(&self) -> u32 {
        match self {
            OwnerRegion::Named(a) => a.start_line,
            OwnerRegion::AnonymousBarrier { start_line, .. } => *start_line,
        }
    }

    pub(crate) fn end_line(&self) -> u32 {
        match self {
            OwnerRegion::Named(a) => a.end_line,
            OwnerRegion::AnonymousBarrier { end_line, .. } => *end_line,
        }
    }

    fn contains(&self, line: u32) -> bool {
        self.start_line() <= line && line <= self.end_line()
    }

    fn span(&self) -> u32 {
        self.end_line().saturating_sub(self.start_line())
    }
}

/// Select the unique narrowest containing region for `line` and return its
/// anchor only when that region is `Named`. Abstains when no region contains
/// the line, when two or more regions tie for the narrowest span, or when the
/// narrowest region is an `AnonymousBarrier`.
pub(crate) fn narrowest_named_owner(regions: &[OwnerRegion], line: u32) -> Option<&OwnerAnchor> {
    let containing = regions
        .iter()
        .filter(|region| region.contains(line))
        .collect::<Vec<_>>();
    let min_span = containing.iter().map(|region| region.span()).min()?;
    let mut narrowest = containing.iter().filter(|region| region.span() == min_span);
    let first = narrowest.next()?;
    if narrowest.next().is_some() {
        return None;
    }
    match first {
        OwnerRegion::Named(anchor) => Some(anchor),
        OwnerRegion::AnonymousBarrier { .. } => None,
    }
}

/// A byte span plus its inclusive 1-indexed line span, used to record a parser
/// `ERROR` node or a zero-width missing token so attribution can abstain around
/// it. `is_point` marks a zero-width missing token; its byte overlap semantics
/// are boundary-inclusive (see [`ErrorRange::overlaps_bytes`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ErrorRange {
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    /// `true` for a zero-width missing token (a single byte position).
    pub(crate) is_point: bool,
}

impl ErrorRange {
    /// `true` when this error range overlaps `[byte_start, byte_end)`.
    ///
    /// For a non-empty `ERROR` span this is the standard half-open overlap.
    /// For a zero-width missing token (`is_point`) it is boundary-inclusive
    /// (`start_byte <= byte_start..=byte_end`), so a missing token sitting at a
    /// callable's end/EOF boundary still degrades that callable rather than
    /// slipping just outside its byte span.
    pub(crate) fn overlaps_bytes(&self, byte_start: usize, byte_end: usize) -> bool {
        if self.is_point {
            self.start_byte >= byte_start && self.start_byte <= byte_end
        } else {
            self.start_byte < byte_end && byte_start < self.end_byte
        }
    }

    /// `true` when this error range's inclusive line range contains `line`.
    pub(crate) fn contains_line(&self, line: u32) -> bool {
        self.start_line <= line && line <= self.end_line
    }
}

/// Convert a tree-sitter node's exclusive end position into an inclusive,
/// 1-indexed end line. tree-sitter end positions are exclusive, so an end
/// column of `0` means the node ends at the start of a row and its last content
/// line is the previous row; this must not accidentally claim the next line.
fn inclusive_end_line(node: tree_sitter::Node<'_>) -> u32 {
    let pos = node.end_position();
    if pos.column == 0 {
        pos.row as u32
    } else {
        pos.row as u32 + 1
    }
}

/// Collect every `ERROR` or missing node's byte + line span in a tree.
/// Line numbers are 1-indexed and inclusive.
///
/// Walks ALL children (named and unnamed), because tree-sitter missing tokens
/// are frequently unnamed and zero-width (e.g. a missing `)`). A zero-width
/// missing token is recorded as a single-line point with `is_point = true`.
/// Non-empty `ERROR` spans clamp their end byte to the content length and
/// compute their inclusive end line via [`inclusive_end_line`].
pub(crate) fn collect_error_ranges(tree: &tree_sitter::Tree, content: &[u8]) -> Vec<ErrorRange> {
    let mut ranges = Vec::new();
    let mut stack: Vec<tree_sitter::Node> = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
        if node.kind() == "ERROR" || node.is_missing() {
            let start = node.start_byte();
            let end = node.end_byte();
            let start_line = node.start_position().row as u32 + 1;
            if node.is_missing() {
                // Zero-width missing token: a single-line point. `end` equals
                // `start`; the line is the missing token's own line.
                ranges.push(ErrorRange {
                    start_byte: start,
                    end_byte: end,
                    start_line,
                    end_line: start_line,
                    is_point: true,
                });
            } else if end > start {
                ranges.push(ErrorRange {
                    start_byte: start,
                    end_byte: end.min(content.len()),
                    start_line,
                    end_line: inclusive_end_line(node),
                    is_point: false,
                });
            }
        }
    }
    ranges
}

/// Whether any error range overlaps the given byte span.
pub(crate) fn any_error_overlaps_bytes(
    ranges: &[ErrorRange],
    byte_start: usize,
    byte_end: usize,
) -> bool {
    ranges
        .iter()
        .any(|range| range.overlaps_bytes(byte_start, byte_end))
}

/// Whether any error range's inclusive line range contains `line`.
pub(crate) fn any_error_contains_line(ranges: &[ErrorRange], line: u32) -> bool {
    ranges.iter().any(|range| range.contains_line(line))
}

/// Attribute a hit line: abstain if it intersects a local error range (line
/// inputs cannot be narrowed by parser columns), otherwise select the unique
/// narrowest named owner.
pub(crate) fn attribute_line<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> Option<&'a OwnerAnchor> {
    if any_error_contains_line(errors, line) {
        return None;
    }
    narrowest_named_owner(regions, line)
}

/// A `Named` region whose byte span overlaps an error range is degraded to an
/// `AnonymousBarrier` covering the full callable, per the local-error contract.
pub(crate) fn degrade_named_on_error(
    region: OwnerRegion,
    errors: &[ErrorRange],
    byte_start: usize,
    byte_end: usize,
) -> OwnerRegion {
    match region {
        OwnerRegion::Named(anchor) if any_error_overlaps_bytes(errors, byte_start, byte_end) => {
            OwnerRegion::AnonymousBarrier {
                start_line: anchor.start_line,
                end_line: anchor.end_line,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, s: u32, e: u32) -> OwnerRegion {
        let anchor = crate::evidence::owner_links::OwnerAnchor {
            path: std::path::PathBuf::from("x.py"),
            name: name.to_string(),
            receiver_var: None,
            receiver_type: None,
            package_dir: std::path::PathBuf::from("."),
            start_line: s,
            end_line: e,
            language: crate::types::Lang::Python,
            display_name: name.to_string(),
        };
        OwnerRegion::Named(anchor)
    }

    fn barrier(s: u32, e: u32) -> OwnerRegion {
        OwnerRegion::AnonymousBarrier {
            start_line: s,
            end_line: e,
        }
    }

    #[test]
    fn picks_unique_narrowest_named_region() {
        let regions = vec![named("outer", 1, 10), named("inner", 3, 5)];
        let owner = narrowest_named_owner(&regions, 4).unwrap();
        assert_eq!(owner.name, "inner");
    }

    #[test]
    fn barrier_abstains_instead_of_falling_through_to_outer_named() {
        // A lambda (barrier) nested inside a named function: a hit inside the
        // lambda must not fall through to the enclosing named owner.
        let regions = vec![named("outer", 1, 10), barrier(3, 5)];
        assert!(narrowest_named_owner(&regions, 4).is_none());
    }

    #[test]
    fn equal_span_tie_abstains() {
        let regions = vec![named("a", 1, 4), named("b", 2, 5)];
        // Both span 3; a hit on line 3 is contained by both -> tie -> abstain.
        assert!(narrowest_named_owner(&regions, 3).is_none());
    }

    #[test]
    fn named_inside_barrier_still_eligible() {
        // A named def nested inside a lambda remains eligible within its own
        // narrower range.
        let regions = vec![named("outer", 1, 20), barrier(3, 10), named("inner", 4, 6)];
        let owner = narrowest_named_owner(&regions, 5).unwrap();
        assert_eq!(owner.name, "inner");
    }

    #[test]
    fn error_range_lines_abstain_and_overlapping_named_degrades() {
        let tree_less = vec![ErrorRange {
            start_byte: 0,
            end_byte: 10,
            start_line: 2,
            end_line: 2,
            is_point: false,
        }];
        assert!(any_error_contains_line(&tree_less, 2));
        assert!(!any_error_contains_line(&tree_less, 3));
        assert!(any_error_overlaps_bytes(&tree_less, 5, 6));
        let degraded = degrade_named_on_error(named("f", 1, 5), &tree_less, 0, 10);
        assert!(matches!(degraded, OwnerRegion::AnonymousBarrier { .. }));
        let kept = degrade_named_on_error(named("f", 1, 5), &tree_less, 20, 30);
        assert!(matches!(kept, OwnerRegion::Named(_)));
    }

    #[test]
    fn missing_point_overlap_is_boundary_inclusive() {
        // A zero-width missing token at a callable's exact end/EOF byte must
        // still overlap it (boundary-inclusive), so it degrades rather than
        // slipping just outside the half-open span.
        let point = ErrorRange {
            start_byte: 6,
            end_byte: 6,
            start_line: 1,
            end_line: 1,
            is_point: true,
        };
        // Callable [0,6) => point at 6 == end -> overlap.
        assert!(point.overlaps_bytes(0, 6));
        // Callable [0,5) => point 6 outside -> no overlap.
        assert!(!point.overlaps_bytes(0, 5));
        // Callable [6,10) => point 6 == start -> overlap (boundary-inclusive).
        assert!(point.overlaps_bytes(6, 10));
        // A non-point ERROR span uses strict half-open overlap (end exclusive).
        let span = ErrorRange {
            start_byte: 0,
            end_byte: 6,
            start_line: 1,
            end_line: 1,
            is_point: false,
        };
        assert!(span.overlaps_bytes(3, 4));
        assert!(!span.overlaps_bytes(6, 7));
    }
}
