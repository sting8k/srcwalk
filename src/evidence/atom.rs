use crate::evidence::{Anchor, EvidenceSource};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EvidenceAtom {
    kind: EvidenceKind,
    role: Option<EvidenceRole>,
    anchor: Anchor,
    snippet: String,
    source: EvidenceSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceKind {
    Definition,
    NameOccurrence,
    Text,
    File,
    Write,
    Reset,
    Read,
    Call,
    Condition,
    Return,
    Dependency,
    UnknownAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EvidenceRole {
    AssignmentLhs,
    AssignmentRhs,
    Condition,
    Return,
    CallArg,
    IndexOrKey,
    Receiver,
    Initializer,
    Parameter,
    LocalDependency,
    ExternalDependency,
    Dependent,
    Expression,
}

impl EvidenceAtom {
    pub(crate) fn new(
        kind: EvidenceKind,
        role: Option<EvidenceRole>,
        anchor: Anchor,
        snippet: impl Into<String>,
        source: EvidenceSource,
    ) -> Self {
        Self {
            kind,
            role,
            anchor,
            snippet: snippet.into(),
            source,
        }
    }

    pub(crate) const fn kind(&self) -> EvidenceKind {
        self.kind
    }

    pub(crate) const fn role(&self) -> Option<EvidenceRole> {
        self.role
    }

    pub(crate) const fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    pub(crate) fn snippet(&self) -> &str {
        &self.snippet
    }

    pub(crate) const fn source(&self) -> EvidenceSource {
        self.source
    }
}

impl EvidenceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::NameOccurrence => "name occurrence",
            Self::Text => "text",
            Self::File => "file",
            Self::Write => "write",
            Self::Reset => "reset",
            Self::Read => "read",
            Self::Call => "call",
            Self::Condition => "condition",
            Self::Return => "return",
            Self::Dependency => "dependency",
            Self::UnknownAccess => "unknown",
        }
    }
}

impl EvidenceRole {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AssignmentLhs => "assignment_lhs",
            Self::AssignmentRhs => "assignment_rhs",
            Self::Condition => "condition",
            Self::Return => "return",
            Self::CallArg => "call_arg",
            Self::IndexOrKey => "index_or_key",
            Self::Receiver => "receiver",
            Self::Initializer => "initializer",
            Self::Parameter => "parameter",
            Self::LocalDependency => "local_dependency",
            Self::ExternalDependency => "external_dependency",
            Self::Dependent => "dependent",
            Self::Expression => "expression",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn atom_preserves_access_fact_fields() {
        let atom = EvidenceAtom::new(
            EvidenceKind::Write,
            Some(EvidenceRole::AssignmentLhs),
            Anchor::line(Path::new("src/lib.rs"), 12),
            "state.flag = true;",
            EvidenceSource::Ast,
        );

        assert_eq!(atom.kind().as_str(), "write");
        assert_eq!(
            atom.role().map(EvidenceRole::as_str),
            Some("assignment_lhs")
        );
        assert_eq!(atom.anchor().start_line(), 12);
        assert_eq!(atom.snippet(), "state.flag = true;");
        assert_eq!(atom.source(), EvidenceSource::Ast);
        assert_eq!(
            crate::evidence::confidence_label_for(atom.source()),
            "structural syntax"
        );
    }

    #[test]
    fn atom_preserves_text_provenance_confidence() {
        let atom = EvidenceAtom::new(
            EvidenceKind::UnknownAccess,
            None,
            Anchor::line(Path::new("src/lib.rs"), 30),
            "USE_FIELD(flag);",
            EvidenceSource::Text,
        );

        assert_eq!(atom.kind().as_str(), "unknown");
        assert_eq!(atom.role(), None);
        assert_eq!(atom.anchor().start_line(), 30);
        assert_eq!(atom.source(), EvidenceSource::Text);
        assert_eq!(
            crate::evidence::confidence_label_for(atom.source()),
            "text evidence"
        );
    }

    #[test]
    fn atom_names_compare_control_features() {
        assert_eq!(EvidenceKind::Condition.as_str(), "condition");
        assert_eq!(EvidenceKind::Return.as_str(), "return");
    }

    #[test]
    fn atom_names_dependency_features() {
        assert_eq!(EvidenceKind::Definition.as_str(), "definition");
        assert_eq!(EvidenceKind::NameOccurrence.as_str(), "name occurrence");
        assert_eq!(EvidenceKind::Text.as_str(), "text");
        assert_eq!(EvidenceKind::Dependency.as_str(), "dependency");
        assert_eq!(EvidenceKind::File.as_str(), "file");
        assert_eq!(EvidenceRole::Parameter.as_str(), "parameter");
        assert_eq!(EvidenceRole::LocalDependency.as_str(), "local_dependency");
        assert_eq!(
            EvidenceRole::ExternalDependency.as_str(),
            "external_dependency"
        );
        assert_eq!(EvidenceRole::Dependent.as_str(), "dependent");
    }
}
