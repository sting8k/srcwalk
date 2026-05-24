use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::evidence::{Anchor, EvidenceAtom, EvidenceKind, EvidenceSource};

macro_rules! define_id_type {
    ($name:ident) => {
        #[allow(dead_code)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub(crate) struct $name(&'static str);

        #[allow(dead_code)]
        impl $name {
            #[must_use]
            pub(crate) const fn new(value: &'static str) -> Self {
                Self(value)
            }

            #[must_use]
            pub(crate) const fn as_str(self) -> &'static str {
                self.0
            }

            #[must_use]
            pub(crate) fn is_empty(self) -> bool {
                self.0.is_empty()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0)
            }
        }
    };
}

define_id_type!(LangId);
define_id_type!(OutlineKindId);
define_id_type!(ViewModeId);

/// What kind of query the user issued.
#[derive(Debug)]
pub enum QueryType {
    FilePath(PathBuf),
    FilePathLine(PathBuf, usize),
    FilePathSection(PathBuf, String),
    Glob(String),
    SymbolGlob(String),
    Symbol(String),
    /// Broad concept query — single lowercase word or multi-word phrase
    /// that likely refers to a feature/module/flow rather than an exact symbol.
    Concept(String),
    /// Path-like or unclassified query — try symbol, then content as fallback.
    Fallthrough(String),
}

/// Provider-owned language identity for removable capability modules.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderLang {
    id: LangId,
    label: &'static str,
}

#[allow(dead_code)]
impl ProviderLang {
    #[must_use]
    pub const fn new(id: LangId, label: &'static str) -> Self {
        Self { id, label }
    }

    #[must_use]
    pub(crate) const fn id(self) -> LangId {
        self.id
    }

    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        self.label
    }
}

/// Programming language, carried through the type system so downstream
/// code never re-detects. Public built-ins stay closed; removable providers use
/// `Lang::Provider` while the compiler still checks built-in coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Java,
    Scala,
    C,
    Cpp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    CSharp,
    Elixir,
    Css,
    Scss,
    Less,
    Html,
    Markdown,
    #[allow(dead_code)]
    Provider(ProviderLang),
    Dockerfile,
    Make,
}

/// File type as detected by extension. Determines outline strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Code(Lang),
    Document(Lang),
    StructuredData,
    Tabular,
    Log,
    Other,
}

impl FileType {
    pub(crate) const fn structural_lang(self) -> Option<Lang> {
        match self {
            Self::Code(lang) | Self::Document(lang) => Some(lang),
            Self::StructuredData | Self::Tabular | Self::Log | Self::Other => None,
        }
    }

    pub(crate) const fn is_code(self) -> bool {
        matches!(self, Self::Code(_))
    }
}

/// What the output contains — shown in the header bracket.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderViewMode {
    id: ViewModeId,
    label: &'static str,
}

#[allow(dead_code)]
impl ProviderViewMode {
    #[must_use]
    pub(crate) const fn new(id: ViewModeId, label: &'static str) -> Self {
        Self { id, label }
    }

    #[must_use]
    pub(crate) const fn id(self) -> ViewModeId {
        self.id
    }

    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        self.label
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Full,
    Outline,
    /// Outline emitted because a `--full` request would exceed `--budget`.
    OutlineCascade,
    /// Top-level signatures only — second cascade step when even outline overflows.
    Signatures,
    Keys,
    #[allow(dead_code)]
    HeadTail,
    Empty,
    Generated,
    #[allow(dead_code)]
    Provider(ProviderViewMode),
    #[allow(dead_code)]
    Binary,
    #[allow(dead_code)]
    Error,
    Section,
    /// Outline of a section that exceeded the section token threshold.
    SectionOutline,
}

impl std::fmt::Display for ViewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Outline => write!(f, "outline"),
            Self::OutlineCascade => write!(f, "outline (full requested, over budget)"),
            Self::Signatures => write!(f, "signatures (full requested, over budget)"),
            Self::Keys => write!(f, "keys"),
            Self::HeadTail => write!(f, "head+tail"),
            Self::Empty => write!(f, "empty"),
            Self::Generated => write!(f, "generated — skipped"),
            Self::Provider(mode) => {
                debug_assert!(!mode.id().is_empty());
                f.write_str(mode.label())
            }
            Self::Binary => write!(f, "skipped"),
            Self::Error => write!(f, "error"),
            Self::Section => write!(f, "section"),
            Self::SectionOutline => write!(f, "section, outline (over limit)"),
        }
    }
}

/// A single search match, carrying enough context for ranking and display.
#[derive(Debug, Clone)]
pub struct Match {
    pub path: PathBuf,
    pub line: u32,
    pub text: String,
    pub is_definition: bool,
    pub exact: bool,
    pub file_lines: u32,
    pub mtime: SystemTime,
    /// Line range of the enclosing definition node (for expand).
    /// Populated by tree-sitter for definitions; None for usages.
    pub def_range: Option<(u32, u32)>,
    /// The defined symbol name (populated from AST during definition detection).
    pub def_name: Option<String>,
    /// Semantic weight for definition kinds. 0 for usages.
    pub def_weight: u16,
    /// For impl/implements matches: the trait or interface being implemented.
    /// None for primary definitions and plain usages.
    pub impl_target: Option<String>,
    /// For neutral base-list matches such as C# `class X : Y`, where `Y` may be
    /// a base class or an interface. None for primary definitions and usages.
    pub base_target: Option<String>,
    /// Whether this match sits inside a comment or doc-comment node.
    /// Populated by tree-sitter post-processing on usage matches.
    pub in_comment: bool,
}

impl Match {
    pub(crate) fn to_evidence_atom(&self) -> EvidenceAtom {
        let kind = if self.is_definition {
            EvidenceKind::Definition
        } else if self.exact {
            EvidenceKind::Usage
        } else {
            EvidenceKind::Text
        };
        let source = if self.is_definition {
            EvidenceSource::Ast
        } else {
            EvidenceSource::Text
        };
        let anchor = if self.is_definition {
            self.def_range.map_or_else(
                || Anchor::line(&self.path, self.line),
                |(start, end)| Anchor::lines(&self.path, start, end),
            )
        } else {
            Anchor::line(&self.path, self.line)
        };

        EvidenceAtom::new(kind, None, anchor, self.text.clone(), source)
    }
}

/// Assembled search results before formatting.
#[derive(Debug)]
pub struct SearchResult {
    pub query: String,
    pub scope: PathBuf,
    pub matches: Vec<Match>,
    pub total_found: usize,
    pub definitions: usize,
    pub usages: usize,
    pub comments: usize,
    /// Whether more results exist beyond the current page.
    pub has_more: bool,
    /// Current offset (0-based) into the full result set.
    pub offset: usize,
}

/// A single entry in a code outline.
#[derive(Debug)]
pub struct OutlineEntry {
    pub kind: OutlineKind,
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Option<String>,
    pub children: Vec<OutlineEntry>,
    pub doc: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderOutlineKind {
    id: OutlineKindId,
    outline_label: &'static str,
    semantic_label: &'static str,
    definition_weight: u16,
}

#[allow(dead_code)]
impl ProviderOutlineKind {
    #[must_use]
    pub(crate) const fn new(
        id: OutlineKindId,
        outline_label: &'static str,
        semantic_label: &'static str,
        definition_weight: u16,
    ) -> Self {
        Self {
            id,
            outline_label,
            semantic_label,
            definition_weight,
        }
    }

    #[must_use]
    pub(crate) const fn id(self) -> OutlineKindId {
        self.id
    }

    #[must_use]
    pub(crate) fn outline_label(self) -> &'static str {
        debug_assert!(!self.id().is_empty());
        self.outline_label
    }

    #[must_use]
    pub(crate) fn semantic_label(self) -> &'static str {
        debug_assert!(!self.id().is_empty());
        self.semantic_label
    }

    #[must_use]
    pub(crate) fn definition_weight(self) -> u16 {
        debug_assert!(!self.id().is_empty());
        self.definition_weight
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    Import,
    Function,
    Class,
    Struct,
    Interface,
    TypeAlias,
    Enum,
    Constant,
    Variable,
    ImmutableVariable,
    Export,
    #[allow(dead_code)]
    Provider(ProviderOutlineKind),
    Selector,
    AtRule,
    Section,
    Element,
    CodeBlock,
    Mixin,
    #[allow(dead_code)]
    Property,
    Module,
    #[allow(dead_code)]
    TestSuite,
    #[allow(dead_code)]
    TestCase,
}

/// Detect test files by path patterns.
pub(crate) fn is_test_file(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.contains(".test.") || s.contains(".spec.") || s.contains("__tests__/")
}

/// Tokens ≈ bytes / 4. Ceiling division, no float.
#[must_use]
pub fn estimate_tokens(byte_len: u64) -> u64 {
    byte_len.div_ceil(4)
}

/// UTF-8 safe string truncation. Never panics on multi-byte characters.
#[must_use]
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_lang_exposes_id_and_label() {
        let lang = ProviderLang::new(LangId::new("lang.example"), "Example");

        assert_eq!(lang.id(), LangId::new("lang.example"));
        assert_eq!(lang.label(), "Example");
        assert_ne!(Lang::Provider(lang), Lang::Rust);
    }
}
