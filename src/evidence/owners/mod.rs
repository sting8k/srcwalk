//! Shared owner-region model and selection semantics for multilanguage owner
//! attribution (US-067 phase 1).
//!
//! Each supported language adapter parses a file into a flat list of nested
//! callable `OwnerRegion`s. A region is either a `Named` owner (an `OwnerAnchor`)
//! or an `AnonymousBarrier` (an anonymous callable that must not let a hit fall
//! through to an enclosing named owner). For a given hit line, shared
//! `attribute_line` returns one typed `OwnerAttribution`: a unique narrowest
//! `Named` anchor or the explicit `ErrorLine`, `TopLevel`, `Tie`, or `Barrier`
//! reason. Whole-file `ParseFailed` is recorded at the dispatch boundary.
//!
//! Language-specific traversal and container logic stay in each adapter
//! (`python.rs`, `rust.rs`, `js_ts.rs`); this module owns only the shared region
//! model, the unique-narrowest selection, and local-`ERROR` primitives.

pub(crate) mod c_cpp;
pub(crate) mod csharp;
pub(crate) mod java;
pub(crate) mod js_ts;
pub(crate) mod kotlin;
pub(crate) mod php;
pub(crate) mod python;
pub(crate) mod ruby;
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

/// One typed owner-attribution decision for a shown hit line: either the
/// unique narrowest named owner, or the parser-known reason attribution
/// conservatively stopped (US-073). Unsupported languages never reach this
/// type; they produce no attribution record at all.
#[derive(Debug, Clone, Copy)]
pub(crate) enum OwnerAttribution<'a> {
    Named(&'a OwnerAnchor),
    Abstained(OwnerAbstentionReason),
}

/// Projection helpers for asserting one half of the decision. Production code
/// consumes `OwnerAttribution` by exhaustive `match` (so a future reason cannot
/// be silently dropped); these accessors exist for the adapter-level tests that
/// only care about one side of the decision.
#[cfg(test)]
impl<'a> OwnerAttribution<'a> {
    /// The named owner, or `None` for any abstention.
    pub(crate) fn named(self) -> Option<&'a OwnerAnchor> {
        match self {
            OwnerAttribution::Named(anchor) => Some(anchor),
            OwnerAttribution::Abstained(_) => None,
        }
    }

    /// The abstention reason, or `None` when a named owner was selected.
    pub(crate) fn abstained(self) -> Option<OwnerAbstentionReason> {
        match self {
            OwnerAttribution::Named(_) => None,
            OwnerAttribution::Abstained(reason) => Some(reason),
        }
    }
}

/// Why structural owner attribution stopped. These describe only the parser's
/// attribution decision; they never rank a hit's relevance (in particular
/// `TopLevel` is a lexical-location fact, not an importance judgment).
///
/// Declaration order IS the canonical render order, and the derived `Ord` is
/// relied on by the renderer's ordered collections.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum OwnerAbstentionReason {
    /// The language is owner-supported but its parser/setup/tree acceptance
    /// contract returned no usable analysis. Recorded at the dispatch boundary,
    /// never by an adapter.
    ParseFailed,
    /// A collected parser `ERROR`/missing-token line range contains the hit.
    ErrorLine,
    /// The unique narrowest containing region is an `AnonymousBarrier`. V1
    /// intentionally combines true anonymous callables with named regions
    /// degraded to barriers by a local error.
    Barrier,
    /// Analysis succeeded and no owner region contains the hit line.
    TopLevel,
    /// Two or more containing regions share the narrowest line span, so
    /// line-only evidence cannot choose safely.
    Tie,
}

impl OwnerAbstentionReason {
    /// The rendered lowercase label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            OwnerAbstentionReason::ParseFailed => "parse-failed",
            OwnerAbstentionReason::ErrorLine => "error-line",
            OwnerAbstentionReason::Barrier => "barrier",
            OwnerAbstentionReason::TopLevel => "top-level",
            OwnerAbstentionReason::Tie => "tie",
        }
    }
}

/// Select the unique narrowest containing region for `line` and classify the
/// outcome. Abstains as `TopLevel` when no region contains the line, `Tie` when
/// two or more regions share the narrowest span, and `Barrier` when the unique
/// narrowest region is an `AnonymousBarrier`.
fn narrowest_region_decision(regions: &[OwnerRegion], line: u32) -> OwnerAttribution<'_> {
    let containing = regions
        .iter()
        .filter(|region| region.contains(line))
        .collect::<Vec<_>>();
    let Some(min_span) = containing.iter().map(|region| region.span()).min() else {
        return OwnerAttribution::Abstained(OwnerAbstentionReason::TopLevel);
    };
    let mut narrowest = containing.iter().filter(|region| region.span() == min_span);
    let Some(first) = narrowest.next() else {
        return OwnerAttribution::Abstained(OwnerAbstentionReason::TopLevel);
    };
    if narrowest.next().is_some() {
        return OwnerAttribution::Abstained(OwnerAbstentionReason::Tie);
    }
    match first {
        OwnerRegion::Named(anchor) => OwnerAttribution::Named(anchor),
        OwnerRegion::AnonymousBarrier { .. } => {
            OwnerAttribution::Abstained(OwnerAbstentionReason::Barrier)
        }
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

/// Attribute a hit line to one typed decision. Fixed precedence: a local error
/// range containing the line wins over every region decision (line inputs
/// cannot be narrowed by parser columns), then the unique-narrowest region
/// classification applies. `ParseFailed` is not decided here; it is a
/// dispatch-boundary fact about the whole file.
pub(crate) fn attribute_line<'a>(
    regions: &'a [OwnerRegion],
    errors: &[ErrorRange],
    line: u32,
) -> OwnerAttribution<'a> {
    if any_error_contains_line(errors, line) {
        return OwnerAttribution::Abstained(OwnerAbstentionReason::ErrorLine);
    }
    narrowest_region_decision(regions, line)
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

    fn error_line(line: u32) -> ErrorRange {
        ErrorRange {
            start_byte: 0,
            end_byte: 1,
            start_line: line,
            end_line: line,
            is_point: false,
        }
    }

    #[test]
    fn picks_unique_narrowest_named_region() {
        let regions = vec![named("outer", 1, 10), named("inner", 3, 5)];
        let owner = attribute_line(&regions, &[], 4).named().unwrap();
        assert_eq!(owner.name, "inner");
    }

    #[test]
    fn barrier_abstains_instead_of_falling_through_to_outer_named() {
        // A lambda (barrier) nested inside a named function: a hit inside the
        // lambda must not fall through to the enclosing named owner.
        let regions = vec![named("outer", 1, 10), barrier(3, 5)];
        assert_eq!(
            attribute_line(&regions, &[], 4).abstained(),
            Some(OwnerAbstentionReason::Barrier)
        );
    }

    #[test]
    fn equal_span_tie_abstains() {
        let regions = vec![named("a", 1, 4), named("b", 2, 5)];
        // Both span 3; a hit on line 3 is contained by both -> tie -> abstain.
        assert_eq!(
            attribute_line(&regions, &[], 3).abstained(),
            Some(OwnerAbstentionReason::Tie)
        );
    }

    #[test]
    fn named_inside_barrier_still_eligible() {
        // A named def nested inside a lambda remains eligible within its own
        // narrower range.
        let regions = vec![named("outer", 1, 20), barrier(3, 10), named("inner", 4, 6)];
        let owner = attribute_line(&regions, &[], 5).named().unwrap();
        assert_eq!(owner.name, "inner");
    }

    // ---- US-073: typed attribution decisions and fixed precedence ----

    #[test]
    fn no_containing_region_is_top_level() {
        let regions = vec![named("f", 5, 9)];
        assert_eq!(
            attribute_line(&regions, &[], 2).abstained(),
            Some(OwnerAbstentionReason::TopLevel)
        );
        // An empty region list is still an analyzed file: top-level, never
        // parse-failed (parse failure is a dispatch-level fact).
        assert_eq!(
            attribute_line(&[], &[], 1).abstained(),
            Some(OwnerAbstentionReason::TopLevel)
        );
    }

    #[test]
    fn error_line_wins_over_every_region_decision() {
        // Precedence 3 beats 4/5/6/7: the same line would otherwise be named,
        // top-level, tie, or barrier.
        let errors = [error_line(4)];
        let named_regions = vec![named("outer", 1, 10), named("inner", 3, 5)];
        assert_eq!(
            attribute_line(&named_regions, &errors, 4).abstained(),
            Some(OwnerAbstentionReason::ErrorLine)
        );
        let tie_regions = vec![named("a", 2, 5), named("b", 4, 7)];
        assert_eq!(
            attribute_line(&tie_regions, &errors, 4).abstained(),
            Some(OwnerAbstentionReason::ErrorLine)
        );
        let barrier_regions = vec![barrier(3, 5)];
        assert_eq!(
            attribute_line(&barrier_regions, &errors, 4).abstained(),
            Some(OwnerAbstentionReason::ErrorLine)
        );
        assert_eq!(
            attribute_line(&[], &errors, 4).abstained(),
            Some(OwnerAbstentionReason::ErrorLine)
        );
    }

    #[test]
    fn tie_wins_over_barrier_when_narrowest_span_is_shared() {
        // Precedence 5 beats 6: a barrier tied with a named region of equal
        // span is a tie, not a barrier.
        let regions = vec![named("a", 2, 5), barrier(4, 7)];
        assert_eq!(
            attribute_line(&regions, &[], 4).abstained(),
            Some(OwnerAbstentionReason::Tie)
        );
    }

    #[test]
    fn named_decision_carries_the_anchor_and_no_reason() {
        let regions = vec![named("inner", 3, 5)];
        let decision = attribute_line(&regions, &[], 4);
        assert_eq!(
            decision.named().map(|a| a.name.clone()),
            Some("inner".into())
        );
        assert_eq!(decision.abstained(), None);
    }

    #[test]
    fn reason_labels_are_the_approved_lowercase_strings() {
        assert_eq!(OwnerAbstentionReason::ParseFailed.label(), "parse-failed");
        assert_eq!(OwnerAbstentionReason::ErrorLine.label(), "error-line");
        assert_eq!(OwnerAbstentionReason::Barrier.label(), "barrier");
        assert_eq!(OwnerAbstentionReason::TopLevel.label(), "top-level");
        assert_eq!(OwnerAbstentionReason::Tie.label(), "tie");
    }

    #[test]
    fn reason_sort_order_is_the_canonical_render_order() {
        // Rendering groups reasons through ordered collections, so the derived
        // `Ord` must equal the canonical render order.
        let mut reasons = [
            OwnerAbstentionReason::Tie,
            OwnerAbstentionReason::TopLevel,
            OwnerAbstentionReason::Barrier,
            OwnerAbstentionReason::ErrorLine,
            OwnerAbstentionReason::ParseFailed,
        ];
        reasons.sort();
        let labels = reasons.iter().map(|r| r.label()).collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec!["parse-failed", "error-line", "barrier", "top-level", "tie"]
        );
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
