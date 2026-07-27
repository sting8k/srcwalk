use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::cache::OutlineCache;
use crate::evidence::{
    bounded_line_range_indices, render_next_actions, Anchor, EvidenceSource, NextAction,
    NEXT_ACTION_LINE_CAP,
};
use crate::format;
use crate::types::SearchResult;

use super::semantic;

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
) -> bool {
    let targets = collect_structural_targets(result, cache);
    if targets.is_empty() {
        return false;
    }

    let actions = show_actions_for_targets(&targets, &result.scope);
    let rendered = render_next_actions(&actions);
    if rendered.is_empty() {
        let target = &targets[0];
        let width = target.end_line - target.start_line + 1;
        out.push_str("\n\n");
        let _ = write!(
            out,
            "> Caveat: confirmed structural target {} spans {width} lines, over the {NEXT_ACTION_LINE_CAP}-line next-action bound.",
            target.location_arg_relative_to(&result.scope)
        );
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

fn show_actions_for_targets(targets: &[StructuralTarget], scope: &Path) -> Vec<NextAction> {
    let ranges = targets
        .iter()
        .map(|target| (target.start_line, target.end_line))
        .collect::<Vec<_>>();
    let kept_targets = bounded_line_range_indices(&ranges)
        .into_iter()
        .map(|index| &targets[index])
        .collect::<Vec<_>>();

    if kept_targets.len() == 1 {
        let target = kept_targets[0];
        return vec![NextAction::from_evidence(
            show_command_for_target(target, scope),
            "read confirmed structural source target",
            10,
            EvidenceSource::Ast,
            target.anchor(),
        )];
    }

    if kept_targets
        .iter()
        .any(|target| target.requires_section_form(scope))
    {
        return kept_targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                NextAction::from_evidence(
                    show_command_for_target(target, scope),
                    "read confirmed structural source target",
                    10 + index as u16,
                    EvidenceSource::Ast,
                    target.anchor(),
                )
            })
            .collect();
    }

    if kept_targets.is_empty() {
        return Vec::new();
    }

    let joined = kept_targets
        .iter()
        .map(|target| target.location_arg_relative_to(scope))
        .collect::<Vec<_>>()
        .join(",");
    vec![NextAction::guidance(
        format!("srcwalk show {}", quote_or_placeholder(&joined)),
        "read confirmed structural source targets",
        10,
    )]
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
        let actions =
            show_actions_for_targets(&[target("a,file.rs", 1, 1), target("b.rs", 2, 2)], scope);
        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0].command(),
            "srcwalk show 'a,file.rs' --section 1-1"
        );
        assert_eq!(actions[1].command(), "srcwalk show b.rs:2-2");
    }

    #[test]
    fn over_cap_singleton_emits_no_action() {
        let actions = show_actions_for_targets(&[target("large.rs", 1, 201)], Path::new(""));
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
        );

        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0].command(),
            "srcwalk show 'a,file.rs' --section 1-150"
        );
        assert_eq!(actions[0].rank(), 10);
        assert_eq!(
            actions[1].command(),
            "srcwalk show 'c,file.rs' --section 1-50"
        );
        assert_eq!(actions[1].rank(), 11);
    }
}
