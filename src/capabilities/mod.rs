//! Capability facade for removable source, artifact, and future provider modules.
//!
//! The default registry starts empty while preserving the shared facade used
//! by source, artifact, and future provider seams.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::types::{FileType, Lang, OutlineEntry};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportPathStyle {
    CInclude,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityId(&'static str);

#[allow(dead_code)]
impl CapabilityId {
    #[must_use]
    const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityKind {
    Language,
    Relation,
    Artifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityMetadata {
    id: CapabilityId,
    kind: CapabilityKind,
    evidence_source: &'static str,
    confidence_label: &'static str,
    supported_anchors: &'static [&'static str],
    supported_edges: &'static [&'static str],
    skip_text_prefilters: bool,
}

impl CapabilityMetadata {
    #[must_use]
    fn is_valid_for(self, kind: CapabilityKind) -> bool {
        self.kind == kind
            && !self.id.is_empty()
            && !self.evidence_source.is_empty()
            && !self.confidence_label.is_empty()
            && !self.supported_anchors.is_empty()
            && !self.supported_edges.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectCall {
    pub(crate) line: u32,
    pub(crate) target: String,
    pub(crate) target_name: Option<String>,
    pub(crate) mnemonic: String,
    pub(crate) call_text: String,
    pub(crate) caller: String,
    pub(crate) caller_range: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DescriptorRef {
    pub(crate) line: u32,
    pub(crate) target: String,
    pub(crate) caller: String,
}

trait LanguageProvider: Sync {
    fn metadata(&self) -> CapabilityMetadata;
    fn detect_file_type(&self, path: &Path) -> Option<FileType>;
    fn matches_file_type(&self, file_type: FileType) -> bool;

    fn is_private_text_file_type(&self, file_type: FileType) -> bool {
        self.metadata().skip_text_prefilters && self.matches_file_type(file_type)
    }

    fn outline_entries(&self, content: &str) -> Option<Vec<OutlineEntry>>;

    fn provides_outline_entries(&self) -> bool {
        false
    }

    fn import_source(&self, _trimmed: &str) -> Option<String> {
        None
    }

    fn import_path_style(&self) -> Option<ImportPathStyle> {
        None
    }

    fn supports_artifact_search(&self, _file_type: FileType) -> bool {
        false
    }
}

trait RelationProvider: Sync {
    fn metadata(&self) -> CapabilityMetadata;
    fn matches_file_type(&self, file_type: FileType) -> bool;

    fn direct_calls(
        &self,
        _content: &str,
        _def_range: Option<(u32, u32)>,
    ) -> Option<Vec<DirectCall>> {
        None
    }

    fn direct_call_matches_target(&self, call: &DirectCall, target: &str) -> bool {
        call.target == target
    }

    fn caller_context_kind(&self) -> Option<&'static str> {
        None
    }

    fn reverse_search_targets(&self, _entries: &[OutlineEntry]) -> Option<Vec<String>> {
        None
    }

    fn descriptor_dependent_targets(&self, _entries: &[OutlineEntry]) -> Option<Vec<String>> {
        None
    }

    fn local_deps(&self, _file_path: &Path, _content: &str) -> Vec<(PathBuf, String)> {
        Vec::new()
    }

    fn external_symbols(
        &self,
        _content: &str,
        _callee_names: &[String],
        _defined_names: &[String],
        _local_symbols: &HashSet<String>,
    ) -> Vec<String> {
        Vec::new()
    }

    fn descriptor_refs(
        &self,
        _content: &str,
        _def_range: Option<(u32, u32)>,
    ) -> Vec<DescriptorRef> {
        Vec::new()
    }
}

trait ArtifactProvider: Sync {
    fn metadata(&self) -> CapabilityMetadata;
    fn is_artifact_path(&self, path: &Path) -> bool;
    fn render(&self, path: &Path, bytes: &[u8]) -> Option<String>;
}

static LANGUAGE_PROVIDERS: [&dyn LanguageProvider; 0] = [];
static RELATION_PROVIDERS: [&dyn RelationProvider; 0] = [];
static ARTIFACT_PROVIDERS: [&dyn ArtifactProvider; 0] = [];

fn validate_registry_metadata() {
    debug_assert!(LANGUAGE_PROVIDERS
        .iter()
        .all(|provider| provider.metadata().is_valid_for(CapabilityKind::Language)));
    debug_assert!(RELATION_PROVIDERS
        .iter()
        .all(|provider| provider.metadata().is_valid_for(CapabilityKind::Relation)));
    debug_assert!(ARTIFACT_PROVIDERS
        .iter()
        .all(|provider| provider.metadata().is_valid_for(CapabilityKind::Artifact)));
}

fn language_provider_for_file_type(file_type: FileType) -> Option<&'static dyn LanguageProvider> {
    validate_registry_metadata();
    LANGUAGE_PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.matches_file_type(file_type))
}

fn language_provider_for_lang(lang: Lang) -> Option<&'static dyn LanguageProvider> {
    language_provider_for_file_type(FileType::Code(lang))
}

fn relation_provider_for_file_type(file_type: FileType) -> Option<&'static dyn RelationProvider> {
    validate_registry_metadata();
    RELATION_PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.matches_file_type(file_type))
}

fn relation_provider_for_lang(lang: Lang) -> Option<&'static dyn RelationProvider> {
    relation_provider_for_file_type(FileType::Code(lang))
}

pub(crate) fn detect_file_type(path: &Path) -> Option<FileType> {
    validate_registry_metadata();
    LANGUAGE_PROVIDERS
        .iter()
        .find_map(|provider| provider.detect_file_type(path))
}

pub(crate) fn matches_file_type(lang: Lang, file_type: FileType) -> bool {
    language_provider_for_lang(lang).is_some_and(|provider| provider.matches_file_type(file_type))
}

pub(crate) fn is_private_text_file_type(file_type: FileType) -> bool {
    language_provider_for_file_type(file_type)
        .is_some_and(|provider| provider.is_private_text_file_type(file_type))
}

pub(crate) fn import_path_style(lang: Lang) -> Option<ImportPathStyle> {
    language_provider_for_lang(lang).and_then(LanguageProvider::import_path_style)
}

pub(crate) fn supports_artifact_search(file_type: FileType) -> bool {
    language_provider_for_file_type(file_type)
        .is_some_and(|provider| provider.supports_artifact_search(file_type))
}

pub(crate) fn is_binary_artifact_path(path: &Path) -> bool {
    validate_registry_metadata();
    ARTIFACT_PROVIDERS
        .iter()
        .any(|provider| provider.is_artifact_path(path))
}

pub(crate) fn render_binary_artifact(path: &Path, bytes: &[u8]) -> Option<String> {
    validate_registry_metadata();
    ARTIFACT_PROVIDERS
        .iter()
        .find_map(|provider| provider.render(path, bytes))
}

pub(crate) fn outline_entries(lang: Lang, content: &str) -> Option<Vec<OutlineEntry>> {
    language_provider_for_lang(lang).and_then(|provider| provider.outline_entries(content))
}

pub(crate) fn provides_outline_entries(lang: Lang) -> bool {
    language_provider_for_lang(lang).is_some_and(LanguageProvider::provides_outline_entries)
}

pub(crate) fn import_source(lang: Lang, trimmed: &str) -> Option<String> {
    language_provider_for_lang(lang).and_then(|provider| provider.import_source(trimmed))
}

pub(crate) fn direct_calls(
    lang: Lang,
    content: &str,
    def_range: Option<(u32, u32)>,
) -> Option<Vec<DirectCall>> {
    relation_provider_for_lang(lang).and_then(|provider| provider.direct_calls(content, def_range))
}

pub(crate) fn direct_call_matches_target(lang: Lang, call: &DirectCall, target: &str) -> bool {
    relation_provider_for_lang(lang)
        .is_some_and(|provider| provider.direct_call_matches_target(call, target))
}

pub(crate) fn caller_context_kind(lang: Lang) -> Option<&'static str> {
    relation_provider_for_lang(lang).and_then(RelationProvider::caller_context_kind)
}

pub(crate) fn reverse_search_targets(lang: Lang, entries: &[OutlineEntry]) -> Option<Vec<String>> {
    relation_provider_for_lang(lang).and_then(|provider| provider.reverse_search_targets(entries))
}

pub(crate) fn descriptor_dependent_targets(
    lang: Lang,
    entries: &[OutlineEntry],
) -> Option<Vec<String>> {
    relation_provider_for_lang(lang)
        .and_then(|provider| provider.descriptor_dependent_targets(entries))
}

pub(crate) fn local_deps(lang: Lang, file_path: &Path, content: &str) -> Vec<(PathBuf, String)> {
    relation_provider_for_lang(lang)
        .map(|provider| provider.local_deps(file_path, content))
        .unwrap_or_default()
}

pub(crate) fn external_symbols(
    lang: Lang,
    content: &str,
    callee_names: &[String],
    defined_names: &[String],
    local_symbols: &HashSet<String>,
) -> Vec<String> {
    relation_provider_for_lang(lang)
        .map(|provider| {
            provider.external_symbols(content, callee_names, defined_names, local_symbols)
        })
        .unwrap_or_default()
}

pub(crate) fn descriptor_refs_for_lang(
    lang: Lang,
    content: &str,
    def_range: Option<(u32, u32)>,
) -> Vec<DescriptorRef> {
    relation_provider_for_lang(lang)
        .map(|provider| provider.descriptor_refs(content, def_range))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_metadata_is_empty_when_private_modules_are_disabled() {
        assert!(LANGUAGE_PROVIDERS.is_empty());
        assert!(RELATION_PROVIDERS.is_empty());
        assert!(ARTIFACT_PROVIDERS.is_empty());
    }
}
