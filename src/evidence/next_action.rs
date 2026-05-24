use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::evidence::{Anchor, EvidenceSource};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NextAction {
    command: String,
    reason: String,
    rank: u16,
    confidence: NextActionConfidence,
    source_anchor: Option<Anchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NextActionConfidence {
    StructuralSyntax,
    TextEvidence,
    OutputMetadata,
    Guidance,
}

impl NextAction {
    pub(crate) fn new(
        command: impl Into<String>,
        reason: impl Into<String>,
        rank: u16,
        confidence: NextActionConfidence,
        source_anchor: Option<Anchor>,
    ) -> Self {
        Self {
            command: command.into(),
            reason: reason.into(),
            rank,
            confidence,
            source_anchor,
        }
    }

    pub(crate) fn from_evidence(
        command: impl Into<String>,
        reason: impl Into<String>,
        rank: u16,
        source: EvidenceSource,
        source_anchor: Anchor,
    ) -> Self {
        Self::new(
            command,
            reason,
            rank,
            NextActionConfidence::from(source),
            Some(source_anchor),
        )
    }

    pub(crate) fn metadata(
        command: impl Into<String>,
        reason: impl Into<String>,
        rank: u16,
    ) -> Self {
        Self::new(
            command,
            reason,
            rank,
            NextActionConfidence::OutputMetadata,
            None,
        )
    }

    pub(crate) fn guidance(
        command: impl Into<String>,
        reason: impl Into<String>,
        rank: u16,
    ) -> Self {
        Self::new(command, reason, rank, NextActionConfidence::Guidance, None)
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) const fn rank(&self) -> u16 {
        self.rank
    }

    pub(crate) const fn confidence(&self) -> NextActionConfidence {
        self.confidence
    }

    pub(crate) fn source_anchor(&self) -> Option<&Anchor> {
        self.source_anchor.as_ref()
    }

    fn sort_key(&self) -> (u16, u8, u32, &str, &str) {
        (
            self.rank(),
            self.confidence().sort_rank(),
            self.source_anchor().map_or(u32::MAX, Anchor::start_line),
            self.reason(),
            self.command(),
        )
    }
}

impl NextActionConfidence {
    const fn sort_rank(self) -> u8 {
        match self {
            Self::StructuralSyntax => 0,
            Self::TextEvidence => 1,
            Self::OutputMetadata => 2,
            Self::Guidance => 3,
        }
    }
}

impl From<EvidenceSource> for NextActionConfidence {
    fn from(source: EvidenceSource) -> Self {
        match source {
            EvidenceSource::Ast => Self::StructuralSyntax,
            EvidenceSource::Text | EvidenceSource::Document | EvidenceSource::Artifact => {
                Self::TextEvidence
            }
        }
    }
}

pub(crate) fn render_next_actions(actions: &[NextAction]) -> String {
    let actions = ordered_unique(actions);
    let mut out = String::new();
    for action in actions {
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = write!(out, "> Next: {}", action.command());
    }
    out
}

fn ordered_unique(actions: &[NextAction]) -> Vec<NextAction> {
    let mut by_command = BTreeMap::<String, NextAction>::new();
    for action in actions {
        by_command
            .entry(action.command.clone())
            .and_modify(|existing| {
                if action.sort_key() < existing.sort_key() {
                    *existing = action.clone();
                }
            })
            .or_insert_with(|| action.clone());
    }

    let mut actions: Vec<_> = by_command.into_values().collect();
    actions.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    actions
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn render_orders_by_rank_then_dedupes_by_command() {
        let actions = vec![
            NextAction::guidance("srcwalk context src/lib.rs:1-3", "late duplicate", 50),
            NextAction::metadata("srcwalk show src/lib.rs:1-3 -C 20", "primary read", 10),
            NextAction::from_evidence(
                "srcwalk context src/lib.rs:1-3",
                "confirmed structural target",
                20,
                EvidenceSource::Ast,
                Anchor::lines(Path::new("src/lib.rs"), 1, 3),
            ),
        ];

        assert_eq!(
            render_next_actions(&actions),
            "> Next: srcwalk show src/lib.rs:1-3 -C 20\n> Next: srcwalk context src/lib.rs:1-3"
        );
    }

    #[test]
    fn duplicate_commands_keep_best_rank() {
        let actions = vec![
            NextAction::guidance("srcwalk show src/lib.rs -C 20", "fallback", 80),
            NextAction::metadata("srcwalk show src/lib.rs -C 20", "pagination", 30),
        ];

        assert_eq!(
            render_next_actions(&actions),
            "> Next: srcwalk show src/lib.rs -C 20"
        );
    }
}
