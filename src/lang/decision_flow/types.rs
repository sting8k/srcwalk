use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::evidence::{Anchor, EvidenceAtom, EvidenceKind, EvidenceRole, EvidenceSource};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlowTarget {
    pub(crate) path: PathBuf,
    pub(crate) display_target: String,
    pub(crate) selector: TargetSelector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TargetSelector {
    Symbol(String),
    LineRange { start: u32, end: u32 },
    FocusedLineRange { start: u32, end: u32 },
}

#[derive(Clone, Debug)]
pub(super) struct FlowGraph {
    pub(super) target: String,
    pub(super) path: PathBuf,
    pub(super) entry_label: String,
    pub(super) entry_start: u32,
    pub(super) entry_end: u32,
    pub(super) nodes: Vec<FlowNode>,
    pub(super) edges: Vec<FlowEdge>,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug)]
pub(super) struct FlowNode {
    pub(super) id: usize,
    pub(super) kind: FlowNodeKind,
    pub(super) label: String,
    pub(super) start_line: u32,
    pub(super) end_line: u32,
    pub(super) annotations: Vec<FlowAnnotation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FlowAnnotation {
    atom: EvidenceAtom,
}

impl FlowAnnotation {
    pub(super) fn definition(path: &Path, text: String, role: EvidenceRole, line: u32) -> Self {
        Self {
            atom: EvidenceAtom::new(
                EvidenceKind::Definition,
                Some(role),
                Anchor::line(path, line),
                text,
                EvidenceSource::Ast,
            ),
        }
    }

    pub(super) fn call(path: &Path, text: String, line: u32) -> Self {
        Self {
            atom: EvidenceAtom::new(
                EvidenceKind::Call,
                None,
                Anchor::line(path, line),
                text,
                EvidenceSource::Ast,
            ),
        }
    }

    pub(super) fn read(path: &Path, text: String, role: EvidenceRole, line: u32) -> Self {
        Self {
            atom: EvidenceAtom::new(
                EvidenceKind::Read,
                Some(role),
                Anchor::line(path, line),
                text,
                EvidenceSource::Ast,
            ),
        }
    }

    pub(super) fn write(path: &Path, text: String, role: EvidenceRole, line: u32) -> Self {
        Self {
            atom: EvidenceAtom::new(
                EvidenceKind::Write,
                Some(role),
                Anchor::line(path, line),
                text,
                EvidenceSource::Ast,
            ),
        }
    }

    pub(super) const fn kind(&self) -> EvidenceKind {
        self.atom.kind()
    }

    pub(super) const fn role(&self) -> Option<EvidenceRole> {
        self.atom.role()
    }

    pub(super) const fn line(&self) -> u32 {
        self.atom.anchor().start_line()
    }

    pub(super) fn text(&self) -> &str {
        self.atom.snippet()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FlowNodeKind {
    Entry,
    Decision,
    Call,
    Return,
    Throw,
    Loop,
    Summary,
}

#[derive(Clone, Debug)]
pub(super) struct FlowEdge {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) label: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct IncomingEdge {
    pub(super) from: usize,
    pub(super) label: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Branch<'tree> {
    pub(super) label: String,
    pub(super) body: Vec<Node<'tree>>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn flow_annotations_expose_atom_fields_without_renderer_specific_storage() {
        let call = FlowAnnotation::call(Path::new("src/lib.rs"), "load_config".to_string(), 42);
        assert_eq!(call.kind(), EvidenceKind::Call);
        assert_eq!(call.role(), None);
        assert_eq!(call.text(), "load_config");
        assert_eq!(call.line(), 42);

        let read = FlowAnnotation::read(
            Path::new("src/lib.rs"),
            "state.flag".to_string(),
            EvidenceRole::Condition,
            44,
        );
        assert_eq!(read.kind(), EvidenceKind::Read);
        assert_eq!(read.role(), Some(EvidenceRole::Condition));
        assert_eq!(read.text(), "state.flag");
        assert_eq!(read.line(), 44);

        let parameter = FlowAnnotation::definition(
            Path::new("src/lib.rs"),
            "request".to_string(),
            EvidenceRole::Parameter,
            40,
        );
        assert_eq!(parameter.kind(), EvidenceKind::Definition);
        assert_eq!(parameter.role(), Some(EvidenceRole::Parameter));
        assert_eq!(parameter.text(), "request");
        assert_eq!(parameter.line(), 40);

        let write = FlowAnnotation::write(
            Path::new("src/lib.rs"),
            "state.flag".to_string(),
            EvidenceRole::AssignmentLhs,
            45,
        );
        assert_eq!(write.kind(), EvidenceKind::Write);
        assert_eq!(write.role(), Some(EvidenceRole::AssignmentLhs));
        assert_eq!(write.text(), "state.flag");
        assert_eq!(write.line(), 45);
    }
}
