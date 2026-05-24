#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceSource {
    Ast,
    Text,
    Document,
    Artifact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confidence {
    StructuralSyntax,
    TextEvidence,
    DocumentNavigation,
    ArtifactLevel,
}

impl EvidenceSource {
    const fn confidence(self) -> Confidence {
        match self {
            Self::Ast => Confidence::StructuralSyntax,
            Self::Text => Confidence::TextEvidence,
            Self::Document => Confidence::DocumentNavigation,
            Self::Artifact => Confidence::ArtifactLevel,
        }
    }
}

impl Confidence {
    const fn as_str(self) -> &'static str {
        match self {
            Self::StructuralSyntax => "structural syntax",
            Self::TextEvidence => "text evidence",
            Self::DocumentNavigation => "document navigation",
            Self::ArtifactLevel => "artifact-level",
        }
    }
}

pub(crate) const fn evidence_source_label_for(source: EvidenceSource) -> &'static str {
    match source {
        EvidenceSource::Ast => "ast",
        EvidenceSource::Text => "text",
        EvidenceSource::Document => "document",
        EvidenceSource::Artifact => "artifact",
    }
}

pub(crate) const fn evidence_source_caveat_for(source: EvidenceSource) -> Option<&'static str> {
    match source {
        EvidenceSource::Document => {
            Some("document evidence is navigation evidence, not rendered DOM/browser behavior.")
        }
        EvidenceSource::Artifact => Some(
            "artifact evidence is artifact-level unless a provider proves source-level semantics.",
        ),
        EvidenceSource::Ast | EvidenceSource::Text => None,
    }
}

pub(crate) fn evidence_packet_label_for(source: EvidenceSource, kind: &str) -> String {
    match evidence_source_caveat_for(source) {
        Some(caveat) => format!(
            "source: {} · kind: {kind} · confidence: {}\ncaveat: {caveat}",
            evidence_source_label_for(source),
            confidence_label_for(source)
        ),
        None => format!(
            "source: {} · kind: {kind} · confidence: {}",
            evidence_source_label_for(source),
            confidence_label_for(source)
        ),
    }
}

pub(crate) const fn confidence_label_for(source: EvidenceSource) -> &'static str {
    source.confidence().as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_sources_map_to_confidence_and_display_labels() {
        assert_eq!(
            confidence_label_for(EvidenceSource::Ast),
            "structural syntax"
        );
        assert_eq!(confidence_label_for(EvidenceSource::Text), "text evidence");
        assert_eq!(
            confidence_label_for(EvidenceSource::Document),
            "document navigation"
        );
        assert_eq!(
            confidence_label_for(EvidenceSource::Artifact),
            "artifact-level"
        );
        assert_eq!(
            evidence_source_label_for(EvidenceSource::Document),
            "document"
        );
        assert_eq!(
            evidence_source_label_for(EvidenceSource::Artifact),
            "artifact"
        );
        assert_eq!(
            evidence_source_caveat_for(EvidenceSource::Document),
            Some("document evidence is navigation evidence, not rendered DOM/browser behavior.")
        );
        assert_eq!(
            evidence_source_caveat_for(EvidenceSource::Artifact),
            Some("artifact evidence is artifact-level unless a provider proves source-level semantics.")
        );
        assert_eq!(
            evidence_packet_label_for(EvidenceSource::Document, "section"),
            "source: document · kind: section · confidence: document navigation\ncaveat: document evidence is navigation evidence, not rendered DOM/browser behavior."
        );
        assert_eq!(
            evidence_packet_label_for(EvidenceSource::Artifact, "outline"),
            "source: artifact · kind: outline · confidence: artifact-level\ncaveat: artifact evidence is artifact-level unless a provider proves source-level semantics."
        );
    }
}
