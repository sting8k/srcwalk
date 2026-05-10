use crate::{budget, index, session};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArtifactMode {
    #[default]
    Source,
    Artifact,
}

impl ArtifactMode {
    #[must_use]
    pub const fn enabled(self) -> bool {
        match self {
            Self::Source => false,
            Self::Artifact => true,
        }
    }

    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Source => None,
            Self::Artifact => Some("Artifact mode: JS/TS anchors, binaries skipped, AST cap 25MB."),
        }
    }

    #[must_use]
    pub const fn callers_note(self) -> Option<&'static str> {
        match self {
            Self::Source => None,
            Self::Artifact => Some(
                "Artifact mode: JS/TS direct calls, binaries skipped, AST cap 25MB, no transitive impact.",
            ),
        }
    }

    #[must_use]
    pub const fn callees_note(self) -> Option<&'static str> {
        match self {
            Self::Source => None,
            Self::Artifact => Some(
                "Artifact mode: JS/TS same-file calls, binaries skipped, AST cap 25MB, no transitive depth.",
            ),
        }
    }
}

impl From<bool> for ArtifactMode {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Artifact
        } else {
            Self::Source
        }
    }
}

/// Holds expanded search dependencies, allocated once.
/// Avoids scattered `Option<T>` + `unwrap()` throughout dispatch.
pub(crate) struct ExpandedCtx {
    pub(crate) session: session::Session,
    pub(crate) sym_index: index::SymbolIndex,
    pub(crate) bloom: index::bloom::BloomFilterCache,
    pub(crate) expand: usize,
    pub(crate) budget_tokens: Option<u64>,
}

pub(crate) fn apply_optional_budget(output: String, budget_tokens: Option<u64>) -> String {
    match budget_tokens {
        Some(b) => budget::apply_preserving_footer(&output, b),
        None => output,
    }
}

pub(crate) fn with_artifact_read_label(output: String, artifact: ArtifactMode) -> String {
    if !artifact.enabled() {
        return output;
    }

    let output = if let Some(line_end) = output.find('\n') {
        let (header, rest) = output.split_at(line_end);
        format!("{}{rest}", relabel_artifact_header(header.to_string()))
    } else {
        relabel_artifact_header(output)
    };

    output.replace(
        "> Next: drill into a symbol with --section <name> or a line range",
        "> Next: drill into artifact symbols with --section <name> or a line range",
    )
}

fn relabel_artifact_header(header: String) -> String {
    let replacements = [
        (" [outline]", " [artifact outline]"),
        (
            " [outline (full requested, over budget)]",
            " [artifact outline (full requested, over budget)]",
        ),
        (
            " [signatures (full requested, over budget)]",
            " [artifact signatures (full requested, over budget)]",
        ),
        (" [full]", " [artifact full]"),
        (" [section]", " [artifact section]"),
        (
            " [section, outline (over limit)]",
            " [artifact section, outline (over limit)]",
        ),
    ];
    for (old, new) in replacements {
        if header.ends_with(old) {
            return format!("{}{}", header.trim_end_matches(old), new);
        }
    }
    header
}

pub(crate) fn with_artifact_note(output: String, artifact: ArtifactMode) -> String {
    let Some(note) = artifact.note() else {
        return output;
    };
    let output = output.replace(
        "> Next: drill into any hit with `srcwalk <path>:<line>`.",
        "> Next: drill artifact hits with `srcwalk <path> --artifact --section <symbol|bytes:start-end>`.",
    );
    format!("{output}\n\n> {note}")
}
