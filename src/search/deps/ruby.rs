//! Ruby-specific reverse dependency traversal.
//!
//! Answers "what `require`s this file?" using the same parser-backed resolver
//! that drives forward deps, so reverse edges are structurally consistent with
//! forward edges. This is a focused child module because `deps.rs` is a
//! mega-file over 800 LOC and the reverse-import concern is Ruby-only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::SrcwalkError;
use crate::lang::ruby::{require_refs, RubyRequireKind};
use crate::read::imports::{
    is_absolute_source, resolve_all_related_files_with_content, resolve_ruby_require,
    resolve_ruby_require_relative,
};
use crate::types::{FileType, Lang};

/// File-level label used for import dependents so a `require` edge is never
/// rendered as if it were a method call.
const FILE_LEVEL_CALLER: &str = "<file>";

/// A Ruby require source is an external gem candidate only when it is a plain
/// bare name. Local-intent relative forms (`./`, `../`, `.name`), absolute
/// paths, whitespace garbage, and native non-`.rb` targets are never external;
/// dynamic or interpolated forms never reach here (parser rejects them).
fn is_external_source(source: &str) -> bool {
    // Absolute (Unix or Windows drive) and local-intent relative forms are
    // never gem names; whitespace garbage is never a real module path.
    if source.is_empty()
        || is_absolute_source(source)
        || source.starts_with('.')
        || source.starts_with('/')
        || source.starts_with('\\')
        || source.contains(' ')
    {
        return false;
    }
    // Real module paths start with an alphanumeric or `@` (e.g. `@scope/gem`).
    if !source
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '@')
    {
        return false;
    }
    match Path::new(source).extension().and_then(|ext| ext.to_str()) {
        // Bare gem name (e.g. `json`) or explicit `.rb`; anything else (e.g.
        // `native.so`) is an unsupported native/dynamic target.
        None | Some("rb") => true,
        Some(_) => false,
    }
}

/// Static bare `require` sources that did not resolve locally, i.e. external
/// references relative to the repo. `require_relative` is never external, and
/// dynamic/receiver-scoped forms are absent from the parser output, so they can
/// never be mislabeled. Returns sorted, deduplicated sources.
pub(crate) fn external_requires(content: &str, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for reference in require_refs(content) {
        if reference.kind != RubyRequireKind::Require {
            continue;
        }
        let source = reference.source;
        if !is_external_source(&source) {
            continue;
        }
        // Resolved locally? Then it is already a local edge; never external too.
        if resolve_ruby_require(dir, &source).is_some() {
            continue;
        }
        if !out.contains(&source) {
            out.push(source);
        }
    }
    out.sort();
    out
}

/// Resolve a Ruby file's static `require`/`require_relative` targets through
/// the shared parser-backed resolver. These are the only local dependency
/// edges that earn `Ast` provenance; generic text-only imports never appear
/// here because the resolver never promotes dynamic or receiver-scoped forms.
/// Returns the resolved files plus the same paths as a set for `Ast`-provenance
/// labeling in `deps.rs`.
pub(crate) fn resolved_import_files(
    path: &Path,
    content: &str,
) -> (Vec<PathBuf>, HashSet<PathBuf>) {
    let files = resolve_all_related_files_with_content(path, content);
    let ast_paths = files.iter().cloned().collect();
    (files, ast_paths)
}

/// Merge reverse import dependents for a Ruby `target` into `by_file`.
///
/// Runs even when the target exports zero symbols (e.g. a data/config file).
/// Walks `scope` with the existing ignore-aware walker, considers only Ruby
/// files, skips the target, prefilters content that has no `require`, then
/// AST-parses and resolves each reference through the same resolver used for
/// forward deps. A dependent is recorded when a resolved path canonicalizes to
/// the target. Labels preserve the exact source text (`require_relative
/// <source>` / `require <source>`) and the exact 1-based line, with a
/// file-level caller instead of a fabricated method owner.
pub(crate) fn merge_reverse_import_dependents(
    target: &Path,
    scope: &Path,
    by_file: &mut HashMap<PathBuf, Vec<(String, String, u32)>>,
) -> Result<(), SrcwalkError> {
    let target_canonical = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let found = std::sync::Mutex::new(Vec::<(PathBuf, String, u32)>::new());

    let walker = crate::search::walker(scope, None)?;
    walker.run(|| {
        let target_canonical = &target_canonical;
        let found = &found;
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            let is_target = path == target
                || path
                    .canonicalize()
                    .is_ok_and(|canonical| canonical == *target_canonical);
            if is_target {
                return ignore::WalkState::Continue;
            }
            if !matches!(
                crate::lang::detect_file_type(path),
                FileType::Code(Lang::Ruby)
            ) {
                return ignore::WalkState::Continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return ignore::WalkState::Continue;
            };
            if !content.contains("require") {
                return ignore::WalkState::Continue;
            }
            let Some(dir) = path.parent() else {
                return ignore::WalkState::Continue;
            };
            for reference in require_refs(&content) {
                let resolved = match reference.kind {
                    RubyRequireKind::RequireRelative => {
                        resolve_ruby_require_relative(dir, &reference.source)
                    }
                    RubyRequireKind::Require => resolve_ruby_require(dir, &reference.source),
                };
                let Some(resolved_path) = resolved else {
                    continue;
                };
                let resolved_canonical = resolved_path.canonicalize().unwrap_or(resolved_path);
                if resolved_canonical != *target_canonical {
                    continue;
                }
                let label = match reference.kind {
                    RubyRequireKind::RequireRelative => {
                        format!("require_relative {}", reference.source)
                    }
                    RubyRequireKind::Require => format!("require {}", reference.source),
                };
                found
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((path.to_path_buf(), label, reference.line));
            }
            ignore::WalkState::Continue
        })
    });

    // Deterministic dedupe: one edge per (file, label) keeping the earliest
    // line (a file `require_relative`d twice for the same source must render
    // once), matching the descriptor-dependent precedent in `deps.rs`.
    let found = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut deduped = HashMap::<(PathBuf, String), u32>::new();
    for (path, label, line) in found {
        let key = (path, label);
        deduped
            .entry(key)
            .and_modify(|existing| *existing = (*existing).min(line))
            .or_insert(line);
    }
    let mut found: Vec<(PathBuf, String, u32)> = deduped
        .into_iter()
        .map(|((path, label), line)| (path, label, line))
        .collect();
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    for (path, label, line) in found {
        by_file
            .entry(path)
            .or_default()
            .push((FILE_LEVEL_CALLER.to_string(), label, line));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// Unique temp dir per test so parallel tests never share a fixture.
    fn temp_dir() -> PathBuf {
        let n = DIR_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("srcwalk-ruby-deps-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn external_requires_rejects_relative_native_and_garbage() {
        let dir = temp_dir();
        // Gemfile makes this a package root, so `lib/` is a bounded load root:
        // a bare require that resolves locally there is never external.
        fs::write(dir.join("Gemfile"), "source :rubygems\n").unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/json.rb"), "module JSON; end\n").unwrap();

        let content = r#"
require 'json'
require 'native.so'
require './missing'
require '../missing'
require "./also_missing"
require 'bad name'
require 'C:/abs/path'
require 'json/sub'
"#;
        let external = external_requires(content, &dir);

        // `json` resolves locally (lib/json.rb) -> not external.
        // `json/sub` does NOT resolve (no such local file) -> external bare name.
        // native `.so`, relative `./`/`../`, Windows absolute, and whitespace
        // garbage never are.
        assert_eq!(external, vec!["json/sub"]);
    }

    #[test]
    fn external_requires_explicit_rb_source_is_kept() {
        let dir = temp_dir();
        // No local lib/foo.rb, so `foo.rb` is an unresolved bare external.
        let content = "require 'foo.rb'\n";
        assert_eq!(external_requires(content, &dir), vec!["foo.rb"]);
    }
}
