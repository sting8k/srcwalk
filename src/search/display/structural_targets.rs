use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::evidence::{
    bounded_line_range_indices, render_next_actions, Anchor, EvidenceSource, NextAction,
    NEXT_ACTION_LINE_CAP,
};
use crate::format;
use crate::precision;
use crate::types::SearchResult;

use super::semantic;
use super::RenderedSourceLines;

/// Header line for the confirmed-target block. It teaches the classification
/// (run the printed `Next` command; the canonical `<path>:<symbol>` target when
/// stable, numeric as a safe fallback) instead of claiming every target is
/// symbol-addressed.
const CONFIRMED_TARGETS_HEADER: &str =
    "## Confirmed structural targets - run the printed Next command (exact <path>:<symbol> when stable; numeric range as fallback)";

/// Single line emitted after any numeric-fallback targets. Kept at block level
/// so several fallbacks in one output share one explanation instead of
/// repeating it after every target.
const NUMERIC_FALLBACK_NOTE: &str =
    "> Note: numeric fallback - no unique symbol selector resolves to the exact body, so the printed range is the safe read.";

/// Single block-level line emitted when at least one canonical `<path>:<symbol>`
/// target was printed. It states the reuse contract (the same string is
/// accepted unchanged by all four exact-target commands) and keeps the caller
/// evidence bound honest.
const CANONICAL_TARGET_NOTE: &str =
    "> Note: <path>:<symbol> is the exact target; the same string is accepted unchanged by show, context, trace callers, and trace callees (keep any --scope printed with it). Caller matches remain by-name, so same-name definitions elsewhere may be included.";

struct StructuralTarget {
    path: PathBuf,
    start_line: u32,
    end_line: u32,
    /// Parser-backed symbol selector (bare name or `Type.method`) that resolves
    /// back to this target's range when `symbol_backed` is true.
    selector: String,
    /// True when the footer may emit `show path --section <selector>` safely
    /// (the selector round-trips to exactly this range in this file).
    symbol_backed: bool,
}

impl StructuralTarget {
    fn anchor(&self) -> Anchor {
        Anchor::lines(&self.path, self.start_line, self.end_line)
    }

    fn range_arg(&self) -> String {
        format!("{}-{}", self.start_line, self.end_line)
    }

    fn path_arg_relative_to(&self, scope: &Path) -> String {
        format::rel_nonempty(&self.path, scope)
    }

    fn location_arg_relative_to(&self, scope: &Path) -> String {
        self.anchor().display_relative_to(scope)
    }

    fn requires_section_form(&self, scope: &Path) -> bool {
        self.path_arg_relative_to(scope).contains(',')
    }

    /// Canonical exact target `<path>:<symbol>` — the one string every
    /// exact-target command accepts unchanged. Emitted only when:
    /// - the selector is parser-backed;
    /// - the displayed path holds no comma, because `show`/`context` read a
    ///   comma as a multi-target separator, so the inline form could not be
    ///   copied as a single target;
    /// - the displayed path actually addresses the file (CWD- or scope-relative,
    ///   not the shortened `dir/file` display `rel_nonempty` falls back to).
    fn canonical_target(&self, scope: &Path) -> Option<String> {
        if !self.symbol_backed || self.requires_section_form(scope) {
            return None;
        }
        let displayed = self.path_arg_relative_to(scope);
        if displayed != format::rel(&self.path, scope) {
            return None;
        }
        Some(format!("{displayed}:{}", self.selector))
    }
}

pub(in crate::search) fn has_confirmed_structural_targets(
    result: &SearchResult,
    cache: &OutlineCache,
) -> bool {
    result
        .matches
        .iter()
        .any(|m| semantic::context_target_for_match(m, cache).is_some())
}

pub(super) fn append_structural_next_targets(
    out: &mut String,
    result: &SearchResult,
    cache: &OutlineCache,
    rendered_lines: &RenderedSourceLines,
) -> bool {
    let targets = collect_structural_targets(result, cache);
    if targets.is_empty() {
        return false;
    }

    let plan = show_actions_for_targets(&targets, &result.scope, rendered_lines);
    let actions = plan.actions;
    let rendered = render_next_actions(&actions);
    if rendered.is_empty() {
        let target = &targets[0];
        let width = target.end_line - target.start_line + 1;
        out.push_str("\n\n");
        if rendered_lines.contains_range(&target.path, target.start_line, target.end_line) {
            // US-063: the packet already rendered this target in full; the
            // offer was suppressed as redundant, not because of the line cap.
            let _ = write!(
                out,
                "> Caveat: confirmed structural target {} is already shown in full above.",
                target.location_arg_relative_to(&result.scope)
            );
        } else {
            let _ = write!(
                out,
                "> Caveat: confirmed structural target {} spans {width} lines, over the {NEXT_ACTION_LINE_CAP}-line next-action bound.",
                target.location_arg_relative_to(&result.scope)
            );
        }
        return true;
    }
    out.push('\n');
    out.push('\n');
    out.push_str(CONFIRMED_TARGETS_HEADER);
    out.push('\n');
    out.push_str(&rendered);
    if plan.canonical_count > 0 {
        out.push('\n');
        out.push_str(CANONICAL_TARGET_NOTE);
    }
    if plan.fallback_count > 0 {
        out.push('\n');
        out.push_str(NUMERIC_FALLBACK_NOTE);
    }
    true
}

fn collect_structural_targets(
    result: &SearchResult,
    cache: &OutlineCache,
) -> Vec<StructuralTarget> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for m in &result.matches {
        let Some(target) = semantic::context_target_for_match(m, cache) else {
            continue;
        };
        let key = (m.path.clone(), target.start_line, target.end_line);
        if !seen.insert(key) {
            continue;
        }

        targets.push(StructuralTarget {
            path: m.path.clone(),
            start_line: target.start_line,
            end_line: target.end_line,
            selector: target.selector,
            symbol_backed: target.symbol_backed,
        });
        if targets.len() == 3 {
            break;
        }
    }
    targets
}

/// Confirmed-target actions plus the block-level counts that decide which
/// single explanation lines follow them.
struct TargetActions {
    actions: Vec<NextAction>,
    /// Targets printed as a numeric `<path>:<start-end>` fallback.
    fallback_count: usize,
    /// Targets printed as a canonical `<path>:<symbol>` exact target.
    canonical_count: usize,
}

fn show_actions_for_targets(
    targets: &[StructuralTarget],
    scope: &Path,
    rendered: &RenderedSourceLines,
) -> TargetActions {
    let ranges = targets
        .iter()
        .map(|target| (target.start_line, target.end_line))
        .collect::<Vec<_>>();
    let kept_targets = bounded_line_range_indices(&ranges)
        .into_iter()
        .map(|index| &targets[index])
        // US-063: drop any target whose full range was already rendered
        // verbatim in this packet — offering it again is pure redundancy. The
        // whole range must be covered; a partial render keeps the offer.
        .filter(|target| !rendered.contains_range(&target.path, target.start_line, target.end_line))
        .collect::<Vec<_>>();

    if kept_targets.is_empty() {
        return TargetActions {
            actions: Vec::new(),
            fallback_count: 0,
            canonical_count: 0,
        };
    }

    // US-060b: split wide (>W-line) targets out so their numeric range anchors
    // to a bounded `> anchor:` evidence line instead of being offered bare.
    let (wide, narrow): (Vec<_>, Vec<_>) = kept_targets.into_iter().partition(|target| {
        precision::should_anchor_range((target.end_line - target.start_line + 1) as usize)
    });

    let mut actions = Vec::new();
    let mut fallback_count = 0;

    for target in &wide {
        if target.symbol_backed {
            // Stable parser-backed selector: the symbol command is the primary
            // action; the numeric range is non-action evidence metadata.
            actions.push(
                NextAction::guidance(
                    show_command_for_target(target, scope),
                    "read confirmed structural source target",
                    10,
                )
                .with_preamble([anchor_line(target, scope)]),
            );
        } else {
            // Ambiguous / non-round-tripping selector: numeric fallback stays
            // the primary action and is honestly labeled at block level.
            fallback_count += 1;
            actions.push(NextAction::guidance(
                show_command_for_target(target, scope),
                "read confirmed structural source target",
                10,
            ));
        }
    }

    // Each narrow target is offered as its own read command. Symbol-backed
    // targets use the canonical `<path>:<symbol>` exact target; the rest fall
    // back to a numeric range and are labeled at block level.
    for (index, target) in narrow.iter().enumerate() {
        if !target.symbol_backed {
            fallback_count += 1;
        }
        actions.push(NextAction::from_evidence(
            show_command_for_target(target, scope),
            "read confirmed structural source target",
            10 + index as u16,
            EvidenceSource::Ast,
            target.anchor(),
        ));
    }

    // The reuse note only holds for targets actually printed in canonical form.
    let canonical_count = wide
        .iter()
        .chain(narrow.iter())
        .filter(|target| target.canonical_target(scope).is_some())
        .count();

    TargetActions {
        actions,
        fallback_count,
        canonical_count,
    }
}

/// US-060b: keep the numeric range of a wide target visible as evidence while
/// making clear it is a bounded preview, not the body address. Rendered as a
/// plain (non-action) line using only the START line, so it never looks like a
/// hint that could be re-read as a numeric range. The symbol command on the
/// `> Next:` line is the recommended action.
fn anchor_line(target: &StructuralTarget, scope: &Path) -> String {
    format!(
        "  evidence anchor: {} (bounded preview; not the body address)",
        Anchor::line(&target.path, target.start_line).display_relative_to(scope)
    )
}

fn show_command_for_target(target: &StructuralTarget, scope: &Path) -> String {
    let scope_suffix = scope_suffix(target, scope);

    // Canonical exact target: one `<path>:<symbol>` string that show, context,
    // trace callers, and trace callees all accept unchanged.
    if let Some(canonical) = target.canonical_target(scope) {
        return format!(
            "srcwalk show {}{scope_suffix}",
            quote_or_placeholder(&canonical)
        );
    }

    // Parser-backed selector inside a comma-bearing path: the inline form would
    // be split into multiple targets, so keep the separately quoted `--section`
    // command. It reads the same body but is not the canonical reusable target.
    if target.symbol_backed {
        return format!(
            "srcwalk show {} --section {}{scope_suffix}",
            quote_or_placeholder(&target.path_arg_relative_to(scope)),
            quote_or_placeholder(&target.selector)
        );
    }

    // Numeric fallback for exceptional targets with no stable symbol selector.
    if target.requires_section_form(scope) {
        return format!(
            "srcwalk show {} --section {}{scope_suffix}",
            quote_or_placeholder(&target.path_arg_relative_to(scope)),
            quote_or_placeholder(&target.range_arg())
        );
    }

    format!(
        "srcwalk show {}{scope_suffix}",
        quote_or_placeholder(&target.location_arg_relative_to(scope))
    )
}

/// The `--scope <dir>` a printed command needs to resolve its target.
///
/// Paths are displayed relative to the process CWD whenever the file lives
/// under it, so the usual command runs verbatim with no suffix. When `--scope`
/// points outside the CWD the display is scope-relative, and the printed
/// command must carry the same `--scope` or the copied target addresses nothing.
fn scope_suffix(target: &StructuralTarget, scope: &Path) -> String {
    if format::is_cwd_relative(&target.path) {
        return String::new();
    }
    let scope = format::display_path(scope);
    // A CWD or empty scope adds no resolution information; printing it would
    // only produce a flag that cannot help.
    if scope.is_empty() || scope == "." {
        return String::new();
    }
    format!(" --scope {}", quote_or_placeholder(&scope))
}

fn quote_or_placeholder(value: &str) -> String {
    format::shell_quote_arg(value).unwrap_or_else(|| "<path>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, start_line: u32, end_line: u32) -> StructuralTarget {
        StructuralTarget {
            path: PathBuf::from(path),
            start_line,
            end_line,
            selector: String::new(),
            symbol_backed: false,
        }
    }

    fn symbol_target(
        path: &str,
        start_line: u32,
        end_line: u32,
        selector: &str,
    ) -> StructuralTarget {
        StructuralTarget {
            path: PathBuf::from(path),
            start_line,
            end_line,
            selector: selector.to_string(),
            symbol_backed: true,
        }
    }

    #[test]
    fn show_command_quotes_space_paths() {
        let scope = Path::new("");
        let command = show_command_for_target(&target("a file.rs", 1, 3), scope);
        assert_eq!(command, "srcwalk show 'a file.rs:1-3'");
    }

    #[test]
    fn show_command_uses_section_for_comma_paths() {
        let scope = Path::new("");
        let command = show_command_for_target(&target("a,file.rs", 1, 3), scope);
        assert_eq!(command, "srcwalk show 'a,file.rs' --section 1-3");
    }

    #[test]
    fn batched_show_quotes_combined_space_targets() {
        let scope = Path::new("");
        let TargetActions { actions, .. } = show_actions_for_targets(
            &[target("a file.rs", 1, 1), target("b file.rs", 2, 2)],
            scope,
            &RenderedSourceLines::default(),
        );
        // Distinct narrow numeric targets stay on their own lines.
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].command(), "srcwalk show 'a file.rs:1-1'");
        assert_eq!(actions[1].command(), "srcwalk show 'b file.rs:2-2'");
    }

    #[test]
    fn symbol_target_renders_canonical_path_symbol_command() {
        let scope = Path::new("");
        let command = show_command_for_target(&symbol_target("lib.rs", 1, 3, "target"), scope);
        assert_eq!(command, "srcwalk show lib.rs:target");
    }

    #[test]
    fn symbol_target_preserves_qualified_selector() {
        let scope = Path::new("");
        let command = show_command_for_target(&symbol_target("batch.go", 3, 3, "Batch.Set"), scope);
        assert_eq!(command, "srcwalk show batch.go:Batch.Set");
    }

    #[test]
    fn symbol_target_quotes_space_path_and_selector() {
        let scope = Path::new("");
        let command = show_command_for_target(&symbol_target("a file.rs", 1, 3, "my fn"), scope);
        assert_eq!(command, "srcwalk show 'a file.rs:my fn'");
    }

    #[test]
    fn symbol_target_keeps_comma_path_quoted() {
        let scope = Path::new("");
        let command = show_command_for_target(&symbol_target("a,file.rs", 1, 3, "target"), scope);
        assert_eq!(command, "srcwalk show 'a,file.rs' --section target");
    }

    #[test]
    fn batched_show_splits_comma_paths() {
        let scope = Path::new("");
        let TargetActions { actions, .. } = show_actions_for_targets(
            &[target("a,file.rs", 1, 1), target("b.rs", 2, 2)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0].command(),
            "srcwalk show 'a,file.rs' --section 1-1"
        );
        assert_eq!(actions[1].command(), "srcwalk show b.rs:2-2");
    }

    #[test]
    fn over_cap_singleton_emits_no_action() {
        let TargetActions { actions, .. } = show_actions_for_targets(
            &[target("large.rs", 1, 201)],
            Path::new(""),
            &RenderedSourceLines::default(),
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn over_cap_ranges_skip_and_keep_contiguous_priorities() {
        let scope = Path::new("");
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[
                target("a,file.rs", 1, 150),
                target("b,file.rs", 1, 51),
                target("c,file.rs", 1, 50),
            ],
            scope,
            &RenderedSourceLines::default(),
        );

        // The 201-line `b` range is dropped by the line cap; the surviving `a`
        // (150) and `c` (50) are both >W and non-symbol-backed, so they become
        // numeric fallback commands (section form for comma paths).
        assert_eq!(actions.len(), 2);
        assert_eq!(fallbacks, 2);
        assert_eq!(
            actions[0].command(),
            "srcwalk show 'a,file.rs' --section 1-150"
        );
        assert_eq!(actions[0].rank(), 10);
        assert_eq!(
            actions[1].command(),
            "srcwalk show 'c,file.rs' --section 1-50"
        );
        assert_eq!(actions[1].rank(), 10);
    }

    #[test]
    fn wide_boundary_stays_plain_offer() {
        let scope = Path::new("");
        // Width W (40) is the boundary: exactly 40 lines stays a plain offer.
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[target("p.rs", 1, 40)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(fallbacks, 1);
        assert_eq!(actions[0].command(), "srcwalk show p.rs:1-40");
    }

    #[test]
    fn wide_numeric_fallback_uses_numeric_command_primary() {
        let scope = Path::new("");
        // Width W+1 (41) >W, non-symbol-backed: numeric command is the primary
        // action (no separate anchor, no `> expand:`).
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[target("p.rs", 1, 41)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(fallbacks, 1);
        assert_eq!(actions[0].command(), "srcwalk show p.rs:1-41");
        assert!(
            !actions[0].command().contains("expand:"),
            "{:?}",
            actions[0].command()
        );
    }

    #[test]
    fn wide_symbol_backed_promotes_symbol_command_to_primary() {
        let scope = Path::new("");
        // Wide symbol-backed target: the symbol command is the primary action
        // and the numeric range is non-action evidence metadata.
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[symbol_target(
                "semantic.rs",
                156,
                272,
                "format_definition_semantic_match_with_path",
            )],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(fallbacks, 0);
        assert_eq!(
            actions[0].command(),
            "srcwalk show semantic.rs:format_definition_semantic_match_with_path"
        );
        assert_eq!(
            actions[0].reason(),
            "read confirmed structural source target"
        );
    }

    #[test]
    fn wide_symbol_backed_anchor_is_non_action_metadata() {
        let scope = Path::new("");
        let TargetActions { actions, .. } = show_actions_for_targets(
            &[symbol_target("semantic.rs", 156, 272, "Type.method")],
            scope,
            &RenderedSourceLines::default(),
        );
        let preamble = &actions[0];
        // The action's command is the symbol command; the anchor is plain
        // metadata using the START line only, never a competing action or a
        // repeat of the full body range.
        assert_eq!(preamble.command(), "srcwalk show semantic.rs:Type.method");
        let rendered = render_next_actions(std::slice::from_ref(preamble));
        assert_eq!(
            rendered,
            "  evidence anchor: semantic.rs:156 (bounded preview; not the body address)\n> Next: srcwalk show semantic.rs:Type.method"
        );
        assert!(!rendered.contains("expand:"), "{rendered}");
        assert!(!rendered.contains("> Next: semantic.rs"), "{rendered}");
        assert!(
            !rendered.contains("156-272"),
            "anchor must not repeat the full range: {rendered}"
        );
        assert!(
            !rendered.starts_with('>'),
            "anchor must be plain metadata, not action-shaped: {rendered}"
        );
    }

    #[test]
    fn narrow_symbol_backed_has_exactly_one_symbol_next() {
        let scope = Path::new("");
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[symbol_target("lib.rs", 1, 3, "helper")],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(fallbacks, 0);
        let rendered = render_next_actions(&actions);
        assert_eq!(rendered, "> Next: srcwalk show lib.rs:helper");
        assert!(!rendered.contains("expand:"), "{rendered}");
        assert!(!rendered.contains(":1-3"), "{rendered}");
    }

    #[test]
    fn header_teaches_classification() {
        assert!(CONFIRMED_TARGETS_HEADER.contains("run the printed Next command"));
        assert!(CONFIRMED_TARGETS_HEADER.contains("exact <path>:<symbol> when stable"));
        assert!(CONFIRMED_TARGETS_HEADER.contains("numeric range as fallback"));
    }

    #[test]
    fn canonical_note_names_all_four_commands_and_keeps_caller_bound() {
        for command in ["show", "context", "trace callers", "trace callees"] {
            assert!(CANONICAL_TARGET_NOTE.contains(command), "{command}");
        }
        // The reuse claim must not upgrade caller evidence.
        assert!(CANONICAL_TARGET_NOTE.contains("by-name"));
        assert!(CANONICAL_TARGET_NOTE.contains("same-name definitions"));
    }

    #[test]
    fn canonical_count_covers_only_targets_printed_as_path_symbol() {
        let scope = Path::new("");
        let plan = show_actions_for_targets(
            &[
                // canonical: parser-backed, comma-free path
                symbol_target("a.rs", 1, 3, "Type.method"),
                // parser-backed but comma path -> `--section`, not canonical
                symbol_target("b,file.rs", 1, 3, "helper"),
                // not parser-backed -> numeric fallback
                target("c.rs", 1, 3),
            ],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(plan.canonical_count, 1);
        assert_eq!(plan.fallback_count, 1);
        assert_eq!(plan.actions[0].command(), "srcwalk show a.rs:Type.method");
        assert_eq!(
            plan.actions[1].command(),
            "srcwalk show 'b,file.rs' --section helper"
        );
        assert_eq!(plan.actions[2].command(), "srcwalk show c.rs:1-3");
    }

    #[test]
    fn scope_suffix_is_printed_only_when_the_display_needs_it() {
        // A path under the process CWD is displayed CWD-relative, so the
        // printed command runs verbatim with no suffix.
        let cwd = std::env::current_dir().unwrap();
        let under_cwd = symbol_target(
            cwd.join("src").join("lib.rs").to_str().unwrap(),
            1,
            3,
            "helper",
        );
        assert_eq!(scope_suffix(&under_cwd, &cwd), "");

        // A path outside the CWD is displayed scope-relative, so the command
        // must carry the scope that makes the copied target resolve.
        let outside = symbol_target("/elsewhere/pkg/lib.rs", 1, 3, "helper");
        let scope = Path::new("/elsewhere/pkg");
        assert_eq!(scope_suffix(&outside, scope), " --scope /elsewhere/pkg");
        assert_eq!(
            show_command_for_target(&outside, scope),
            "srcwalk show lib.rs:helper --scope /elsewhere/pkg"
        );
    }

    #[test]
    fn numeric_fallback_note_is_single_block_level() {
        // Two non-symbol-backed wide targets -> one shared fallback note at
        // block level, not repeated per target.
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[target("a.rs", 1, 41), target("b.rs", 1, 41)],
            Path::new(""),
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 2);
        assert_eq!(fallbacks, 2);
        assert!(NUMERIC_FALLBACK_NOTE.starts_with("> Note:"));
        assert!(NUMERIC_FALLBACK_NOTE.contains("numeric fallback"));
    }

    #[test]
    fn fully_rendered_target_is_suppressed_but_partial_is_kept() {
        let scope = Path::new("");
        let mut rendered = RenderedSourceLines::default();
        // Render lines 1-6 of p.rs via a code block.
        rendered.record_code_block(
            Path::new("p.rs"),
            "   1 │ a\n   2 │ b\n   3 │ c\n   4 │ d\n   5 │ e\n   6 │ f\n",
        );

        // Full containment (1-6) -> offer suppressed.
        let TargetActions { actions: full, .. } =
            show_actions_for_targets(&[target("p.rs", 1, 6)], scope, &rendered);
        assert!(
            full.is_empty(),
            "fully-rendered target must be suppressed: {full:?}"
        );

        // Partial overlap (1-3 all rendered, 4-8 not) -> offer kept.
        let TargetActions {
            actions: partial, ..
        } = show_actions_for_targets(&[target("p.rs", 1, 8)], scope, &rendered);
        assert_eq!(partial.len(), 1, "partial overlap must keep the offer");
        assert!(
            partial[0].command().contains('8'),
            "{:?}",
            partial[0].command()
        );

        // No overlap (20-25) -> offer kept.
        let TargetActions { actions: none, .. } =
            show_actions_for_targets(&[target("p.rs", 20, 25)], scope, &rendered);
        assert_eq!(none.len(), 1);
    }

    #[test]
    fn mixed_symbol_and_fallback_emit_one_primary_action_each() {
        let scope = Path::new("");
        let TargetActions {
            actions,
            fallback_count: fallbacks,
            ..
        } = show_actions_for_targets(
            &[
                symbol_target("a.rs", 1, 3, "stableOne"),
                target("b.rs", 1, 41),
            ],
            scope,
            &RenderedSourceLines::default(),
        );
        // One canonical primary action per emitted target.
        assert_eq!(actions.len(), 2);
        assert_eq!(fallbacks, 1);
        let rendered = render_next_actions(&actions);
        assert_eq!(rendered.matches("> Next:").count(), 2, "{rendered}");
        assert!(!rendered.contains("expand:"), "{rendered}");
    }
}
