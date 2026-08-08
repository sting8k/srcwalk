//! Shared JS/TS/TSX import classification and tsconfig-path resolution.
//!
//! This module owns the alias-aware decision seam used by dependency, callee, and
//! related-file consumers. It deliberately reuses the existing JS resolver and
//! only adds bounded static config-guided classification; it does not implement
//! compiler, runtime, package-export, or bundler resolution.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::lang::tsconfig::{self, ConfigCache, PathAliasMatch, TsConfig};

/// Historical related-file cap shared by read/callee import consumers.
pub(crate) const MAX_JS_RELATED_FILES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JsImportResolution {
    Local {
        path: PathBuf,
        via_tsconfig_paths: bool,
    },
    External,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JsImportDecision {
    pub(crate) source: String,
    pub(crate) opening_line: usize,
    pub(crate) resolution: JsImportResolution,
}

/// Classify a previously parsed JS/TS/TSX logical-import stream.
///
/// Relative/local-looking sources get the existing `resolve_js` winner first.
/// Only unresolved sources consult the nearest config and every source is then
/// classified as local, alias-local, external, or unresolved. The stream is
/// never parsed here, so callers can share one producer result across consumers.
pub(crate) fn classify_js_imports(
    source_path: &Path,
    logical_sources: &[(String, usize)],
    scope: &Path,
    config_cache: &ConfigCache,
) -> Vec<JsImportDecision> {
    classify_js_imports_with_resolver(
        source_path,
        logical_sources,
        scope,
        config_cache,
        resolve_in_scope,
    )
}

type CandidateResolver = fn(&Path, &str, Option<&Path>) -> Option<PathBuf>;

fn classify_js_imports_with_resolver(
    source_path: &Path,
    logical_sources: &[(String, usize)],
    scope: &Path,
    config_cache: &ConfigCache,
    resolve_candidate: CandidateResolver,
) -> Vec<JsImportDecision> {
    let Some(dir) = source_path.parent() else {
        return Vec::new();
    };

    let scope_canonical = fs::canonicalize(scope).ok();
    let config = config_cache
        .nearest_config(dir, scope)
        .and_then(|path| tsconfig::load_config(&path));

    logical_sources
        .iter()
        .filter(|(source, _)| !source.is_empty())
        .map(|(source, opening_line)| {
            let resolution = classify_one(
                dir,
                source,
                scope_canonical.as_deref(),
                config.as_ref(),
                resolve_candidate,
            );
            JsImportDecision {
                source: source.clone(),
                opening_line: *opening_line,
                resolution,
            }
        })
        .collect()
}

fn classify_one(
    dir: &Path,
    source: &str,
    scope: Option<&Path>,
    config: Option<&TsConfig>,
    resolve_candidate: CandidateResolver,
) -> JsImportResolution {
    if is_local_looking_source(source) {
        if let Some(path) = resolve_candidate(dir, source, scope) {
            return JsImportResolution::Local {
                path,
                via_tsconfig_paths: false,
            };
        }
    }

    let Some(config) = config else {
        return phase_two_classification(source);
    };

    match tsconfig::match_path_alias(source, config) {
        PathAliasMatch::Match { targets } => {
            let config_dir = config
                .config_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            let target_base = config
                .base_url
                .as_ref()
                .map_or_else(|| config_dir.to_path_buf(), |base| config_dir.join(base));

            for target in targets {
                if let Some(path) = resolve_candidate(&target_base, &target, scope) {
                    return JsImportResolution::Local {
                        path,
                        via_tsconfig_paths: true,
                    };
                }
            }

            // A selected alias with no candidate winner is unresolved, even for
            // bare/package-shaped specifiers. It must never become external.
            JsImportResolution::Unresolved
        }
        PathAliasMatch::Abstain => JsImportResolution::Unresolved,
        PathAliasMatch::NoMatch => phase_two_classification(source),
    }
}

fn phase_two_classification(source: &str) -> JsImportResolution {
    if is_local_looking_source(source) {
        JsImportResolution::Unresolved
    } else {
        JsImportResolution::External
    }
}

/// Resolve local paths with relative candidates first, preserving source order.
/// Alias paths fill remaining slots and never displace an earlier relative path.
/// Dedupe and provenance classification happen over the complete decision set
/// before applying the historical cap.
pub(crate) fn ordered_local_paths(
    decisions: &[JsImportDecision],
    limit: Option<usize>,
) -> Vec<PathBuf> {
    let mut relative = Vec::new();
    let mut aliases = Vec::new();
    let mut relative_seen = HashSet::new();
    let mut alias_seen = HashSet::new();

    for decision in decisions {
        let JsImportResolution::Local {
            path,
            via_tsconfig_paths,
        } = &decision.resolution
        else {
            continue;
        };
        let (paths, seen) = if *via_tsconfig_paths {
            (&mut aliases, &mut alias_seen)
        } else {
            (&mut relative, &mut relative_seen)
        };
        let identity = canonical_identity(path);
        if seen.insert(identity) {
            paths.push(path.clone());
        }
    }

    let cap = limit.unwrap_or(usize::MAX);
    if cap == 0 {
        return Vec::new();
    }
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for path in relative.into_iter().chain(aliases) {
        if seen.insert(canonical_identity(&path)) {
            ordered.push(path);
            if ordered.len() == cap {
                break;
            }
        }
    }
    ordered
}

fn canonical_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn is_local_looking_source(source: &str) -> bool {
    source.starts_with("./")
        || source.starts_with("../")
        || source.starts_with("@/")
        || source.starts_with("~/")
}

fn resolve_in_scope(base: &Path, target: &str, scope: Option<&Path>) -> Option<PathBuf> {
    let candidate = resolve_candidate(base, target)?;
    let scope = scope?;
    let canonical = fs::canonicalize(&candidate).ok()?;
    canonical.starts_with(scope).then_some(candidate)
}

fn resolve_candidate(base: &Path, target: &str) -> Option<PathBuf> {
    // A Windows drive spelling is not interpreted as a filename on Unix. Reject
    // it there rather than accidentally treating it as a child named
    // `C:\\...`; native Windows path handling remains in bounded containment.
    if !cfg!(windows) && has_windows_drive_prefix(target) {
        return None;
    }
    if !cfg!(windows) && target.starts_with("\\\\") {
        return None;
    }

    crate::read::imports::resolve_js(base, target)
}

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "srcwalk-js-alias-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn classify(root: &Path, source: &str) -> JsImportDecision {
        let file = root.join("src/main.ts");
        let cache = ConfigCache::new();
        classify_js_imports(&file, &[(source.to_string(), 1)], root, &cache)
            .pop()
            .unwrap()
    }

    #[test]
    fn relative_wins_over_matching_alias_without_marker() {
        let root = temp_dir("relative-wins");
        write(&root, "src/local.ts", "export const local = 1;\n");
        write(
            &root,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#,
        );
        let decision = classify(&root, "./local");
        assert_eq!(
            decision.resolution,
            JsImportResolution::Local {
                path: root.join("src/local.ts"),
                via_tsconfig_paths: false,
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bare_alias_resolves_and_zero_match_is_unresolved() {
        let root = temp_dir("bare-alias");
        write(&root, "src/util.ts", "export const util = 1;\n");
        write(
            &root,
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"utils/*":["src/*"]}}}"#,
        );
        let local = classify(&root, "utils/util");
        assert!(matches!(
            local.resolution,
            JsImportResolution::Local {
                via_tsconfig_paths: true,
                ..
            }
        ));
        let missing = classify(&root, "utils/missing");
        assert_eq!(missing.resolution, JsImportResolution::Unresolved);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn base_url_alone_keeps_bare_import_external() {
        let root = temp_dir("base-url-only");
        write(&root, "src/util.ts", "export const util = 1;\n");
        write(
            &root,
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":"src"}}"#,
        );
        assert_eq!(
            classify(&root, "util").resolution,
            JsImportResolution::External
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_matching_wildcard_abstains_without_fallback() {
        let root = temp_dir("unsupported-shadow");
        write(&root, "src/utils.ts", "export const utils = 1;\n");
        write(
            &root,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@app/*/utils":["src/*"],"@app/*":["src/*"]}}}"#,
        );
        assert_eq!(
            classify(&root, "@app/foo/utils").resolution,
            JsImportResolution::Unresolved
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ordered_paths_keep_relative_before_early_alias() {
        let decisions = vec![
            JsImportDecision {
                source: "alias".into(),
                opening_line: 1,
                resolution: JsImportResolution::Local {
                    path: PathBuf::from("alias.ts"),
                    via_tsconfig_paths: true,
                },
            },
            JsImportDecision {
                source: "./relative".into(),
                opening_line: 2,
                resolution: JsImportResolution::Local {
                    path: PathBuf::from("relative.ts"),
                    via_tsconfig_paths: false,
                },
            },
        ];
        assert_eq!(
            ordered_local_paths(&decisions, Some(1)),
            vec![PathBuf::from("relative.ts")]
        );
    }

    #[test]
    fn ordered_paths_dedupes_relative_and_alias_target() {
        let decisions = vec![
            JsImportDecision {
                source: "./util".into(),
                opening_line: 1,
                resolution: JsImportResolution::Local {
                    path: PathBuf::from("src/util.ts"),
                    via_tsconfig_paths: false,
                },
            },
            JsImportDecision {
                source: "@/util".into(),
                opening_line: 2,
                resolution: JsImportResolution::Local {
                    path: PathBuf::from("src/util.ts"),
                    via_tsconfig_paths: true,
                },
            },
        ];
        assert_eq!(
            ordered_local_paths(&decisions, None),
            vec![PathBuf::from("src/util.ts")]
        );
    }

    fn create_dir_symlink(target: &Path, link: &Path) -> bool {
        let result = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, link)
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(target, link)
            }
        };
        match result {
            Ok(()) => true,
            Err(err) if cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied => {
                false
            }
            Err(err) => panic!("failed to create directory symlink: {err}"),
        }
    }

    #[test]
    fn scoped_resolution_preserves_raw_symlink_spelling_and_dedupes_identity() {
        let root = temp_dir("raw-symlink-dedupe");
        write(&root, "real/util.ts", "export const util = 1;\n");
        write(
            &root,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["real/*"]}}}"#,
        );
        let alias = root.join("alias");
        if !create_dir_symlink(&root.join("real"), &alias) {
            let _ = fs::remove_dir_all(root);
            return;
        }
        let source_path = alias.join("main.ts");
        let cache = ConfigCache::new();
        let decisions = classify_js_imports(
            &source_path,
            &[("./util".to_string(), 1), ("@/util".to_string(), 2)],
            &root,
            &cache,
        );
        let raw_path = match &decisions[0].resolution {
            JsImportResolution::Local { path, .. } => path.clone(),
            other => panic!("expected relative local resolution, got {other:?}"),
        };
        assert_eq!(raw_path, alias.join("util.ts"));
        assert_ne!(raw_path, root.join("real/util.ts"));
        assert!(matches!(
            decisions[1].resolution,
            JsImportResolution::Local {
                via_tsconfig_paths: true,
                ..
            }
        ));
        assert_eq!(ordered_local_paths(&decisions, None), vec![raw_path]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn windows_drive_and_unc_prefixes_remain_detectable() {
        assert!(has_windows_drive_prefix("C:/outside/file.ts"));
        assert!(has_windows_drive_prefix(r"D:\\outside\\file.ts"));
        assert!(!has_windows_drive_prefix("./local.ts"));
        assert!(!has_windows_drive_prefix("@scope/pkg"));
    }
}
