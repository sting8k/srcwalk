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

struct StructuralTarget {
    path: PathBuf,
    start_line: u32,
    end_line: u32,
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

    let actions = show_actions_for_targets(&targets, &result.scope, rendered_lines);
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
    out.push_str("\n\n## Confirmed structural targets\n");
    out.push_str(&rendered);
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
        });
        if targets.len() == 3 {
            break;
        }
    }
    targets
}

fn show_actions_for_targets(
    targets: &[StructuralTarget],
    scope: &Path,
    rendered: &RenderedSourceLines,
) -> Vec<NextAction> {
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
        return Vec::new();
    }

    // US-060b: split wide (>W-line) targets out of every offer path so each
    // anchors to `path:line (label)` + an `expand: srcwalk show path:A-B`
    // command instead of being offered bare. The expand command preserves the
    // exact range (section form for comma paths), so the full range is always
    // exactly one printed command away.
    let (wide, narrow): (Vec<_>, Vec<_>) = kept_targets.into_iter().partition(|target| {
        precision::should_anchor_range((target.end_line - target.start_line + 1) as usize)
    });

    let mut actions = Vec::new();
    for target in &wide {
        actions.push(NextAction::guidance(
            anchored_offer(target, scope, "read confirmed structural source target"),
            "read confirmed structural source target",
            10,
        ));
    }

    if narrow.len() == 1 {
        let target = narrow[0];
        actions.push(NextAction::from_evidence(
            show_command_for_target(target, scope),
            "read confirmed structural source target",
            10,
            EvidenceSource::Ast,
            target.anchor(),
        ));
    } else if !narrow.is_empty()
        && narrow
            .iter()
            .any(|target| target.requires_section_form(scope))
    {
        for (index, target) in narrow.iter().enumerate() {
            actions.push(NextAction::from_evidence(
                show_command_for_target(target, scope),
                "read confirmed structural source target",
                10 + index as u16,
                EvidenceSource::Ast,
                target.anchor(),
            ));
        }
    } else if !narrow.is_empty() {
        let joined = narrow
            .iter()
            .map(|target| target.location_arg_relative_to(scope))
            .collect::<Vec<_>>()
            .join(",");
        actions.push(NextAction::guidance(
            format!("srcwalk show {}", quote_or_placeholder(&joined)),
            "read confirmed structural source targets",
            10,
        ));
    }
    actions
}

/// US-060b: build the anchored offer for a wide target — a single-line
/// `path:start (label)` anchor plus an `> expand:` command that retrieves the
/// full range in exactly one printed command (preserving section form for comma
/// paths). Rendered as two `> `-prefixed lines by `render_next_actions`.
fn anchored_offer(target: &StructuralTarget, scope: &Path, reason: &str) -> String {
    let label = truncate_label(reason);
    let path = format::rel_nonempty(&target.path, scope);
    format!(
        "{}:{} ({label})\n> expand: {}",
        path,
        target.start_line,
        show_command_for_target(target, scope)
    )
}

/// Keep the anchor label short enough for a single-line `path:line (label)`.
fn truncate_label(reason: &str) -> &str {
    let trimmed = reason.trim();
    if trimmed.len() <= 40 {
        trimmed
    } else {
        &trimmed[..trimmed.floor_char_boundary(40)]
    }
}

fn show_command_for_target(target: &StructuralTarget, scope: &Path) -> String {
    if target.requires_section_form(scope) {
        return format!(
            "srcwalk show {} --section {}",
            quote_or_placeholder(&target.path_arg_relative_to(scope)),
            quote_or_placeholder(&target.range_arg())
        );
    }

    format!(
        "srcwalk show {}",
        quote_or_placeholder(&target.location_arg_relative_to(scope))
    )
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
        let actions = show_actions_for_targets(
            &[target("a file.rs", 1, 1), target("b file.rs", 2, 2)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].command(),
            "srcwalk show 'a file.rs:1-1,b file.rs:2-2'"
        );
    }

    #[test]
    fn batched_show_splits_comma_paths() {
        let scope = Path::new("");
        let actions = show_actions_for_targets(
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
        let actions = show_actions_for_targets(
            &[target("large.rs", 1, 201)],
            Path::new(""),
            &RenderedSourceLines::default(),
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn over_cap_ranges_skip_and_keep_contiguous_priorities() {
        let scope = Path::new("");
        let actions = show_actions_for_targets(
            &[
                target("a,file.rs", 1, 150),
                target("b,file.rs", 1, 51),
                target("c,file.rs", 1, 50),
            ],
            scope,
            &RenderedSourceLines::default(),
        );

        // The 201-line `b` range is dropped by the line cap; the surviving `a`
        // (150) and `c` (50) are both >W so they anchor to path:line + expand.
        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0].command(),
            "a,file.rs:1 (read confirmed structural source target)\n> expand: srcwalk show 'a,file.rs' --section 1-150"
        );
        assert_eq!(actions[0].rank(), 10);
        assert_eq!(
            actions[1].command(),
            "c,file.rs:1 (read confirmed structural source target)\n> expand: srcwalk show 'c,file.rs' --section 1-50"
        );
        assert_eq!(actions[1].rank(), 10);
    }

    #[test]
    fn wide_range_offered_anchors_but_w_boundary_does_not() {
        let scope = Path::new("");
        // Width W (40) is the boundary: exactly 40 lines stays a plain offer.
        let boundary = show_actions_for_targets(
            &[target("p.rs", 1, 40)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(boundary.len(), 1);
        assert!(
            !boundary[0].command().contains("expand:"),
            "width-40 target must not anchor: {:?}",
            boundary[0].command()
        );
        assert_eq!(boundary[0].command(), "srcwalk show p.rs:1-40");

        // Width W+1 (41) must anchor to `path:line (label)` + `> expand:`.
        let wide = show_actions_for_targets(
            &[target("p.rs", 1, 41)],
            scope,
            &RenderedSourceLines::default(),
        );
        assert_eq!(wide.len(), 1);
        assert!(
            wide[0]
                .command()
                .contains("> expand: srcwalk show p.rs:1-41"),
            "width-41 target must anchor with an expand command: {:?}",
            wide[0].command()
        );
        assert!(
            wide[0].command().starts_with("p.rs:1 ("),
            "{:?}",
            wide[0].command()
        );
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
        let full = show_actions_for_targets(&[target("p.rs", 1, 6)], scope, &rendered);
        assert!(
            full.is_empty(),
            "fully-rendered target must be suppressed: {full:?}"
        );

        // Partial overlap (1-3 all rendered, 4-8 not) -> offer kept.
        let partial = show_actions_for_targets(&[target("p.rs", 1, 8)], scope, &rendered);
        assert_eq!(partial.len(), 1, "partial overlap must keep the offer");
        assert!(
            partial[0].command().contains('8'),
            "{:?}",
            partial[0].command()
        );

        // No overlap (20-25) -> offer kept.
        let none = show_actions_for_targets(&[target("p.rs", 20, 25)], scope, &rendered);
        assert_eq!(none.len(), 1);
    }
}
