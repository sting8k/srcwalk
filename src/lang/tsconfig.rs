//! US-054 P3 — tsconfig/jsconfig discovery and JSONC loading (no resolution yet).
//!
//! Provides the config-loading seam consumed by P4 alias resolution:
//!
//! - nearest `tsconfig.json` from the source directory up to and including the
//!   active analysis scope root; only when no `tsconfig.json` exists anywhere
//!   in that walk, fall back to the nearest `jsconfig.json`. Same-directory
//!   `tsconfig.json` beats `jsconfig.json`.
//! - no `package.json` boundary, no package/runtime semantics, no crossing of
//!   the active scope root;
//! - parses only the accepted JSONC shape (`compilerOptions.baseUrl` as a
//!   string and `compilerOptions.paths` as `key -> [target]`), allowing
//!   comments and trailing commas only — single-quoted strings, hexadecimal
//!   numbers, missing commas, loose property names, and unary-plus numbers are
//!   rejected (per US-054 Feature Shape Gate item 3);
//! - per-invocation memoization of positive and negative directory lookups
//!   (`directory -> Option<nearest_config_path>`, `OutlineCache` pattern).
//!
//! Malformed or unsupported config abstains: `load_config` returns `None` and
//! the import keeps its Phase-2 classification.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use jsonc_parser::{parse_to_value, JsonValue, ParseOptions};

/// Strict-loose JSONC options per US-054: comments + trailing commas allowed;
/// every documented loose form rejected.
fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    }
}

/// The accepted config shape exposed to P4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TsConfig {
    /// Config file path this mapping came from (for per-source provenance).
    pub config_path: PathBuf,
    /// `compilerOptions.baseUrl` when present and a string. Used by P4 as the
    /// base for `paths` targets; `baseUrl` alone never resolves bare imports.
    pub base_url: Option<PathBuf>,
    /// `compilerOptions.paths`: exact or single-trailing-`*` keys, each with an
    /// ordered target list. Order within a key is declaration order; object
    /// declaration order across keys is not used (resolved by longest prefix).
    pub paths: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathAliasMatch {
    Match { targets: Vec<String> },
    NoMatch,
    Abstain,
}

/// Match an import specifier against the accepted `compilerOptions.paths` subset.
/// This is deliberately pure: it does not inspect the filesystem, use `baseUrl`,
/// canonicalize paths, or apply compiler/runtime/package resolution semantics.
pub(crate) fn match_path_alias(specifier: &str, config: &TsConfig) -> PathAliasMatch {
    // Exact keys have precedence over every wildcard key, including unsupported
    // wildcard forms. This keeps an explicit mapping authoritative.
    if let Some((_, targets)) = config
        .paths
        .iter()
        .find(|(key, _)| !key.contains('*') && key == specifier)
    {
        return PathAliasMatch::Match {
            targets: targets.clone(),
        };
    }

    // An unsupported wildcard that matches is an intentional abstention. Do not
    // silently fall through to a broader supported wildcard mapping.
    if config.paths.iter().any(|(key, _)| {
        key.contains('*')
            && supported_wildcard_prefix(key).is_none()
            && wildcard_matches(key, specifier)
    }) {
        return PathAliasMatch::Abstain;
    }

    // Choose the supported wildcard with the longest literal prefix, independent
    // of the path-key declaration order. Preserve each target array's order.
    let mut best: Option<(usize, &[String], String)> = None;
    for (key, targets) in &config.paths {
        let Some(prefix) = supported_wildcard_prefix(key) else {
            continue;
        };
        let Some(capture) = specifier.strip_prefix(prefix) else {
            continue;
        };
        let prefix_len = prefix.chars().count();
        let is_more_specific = best
            .as_ref()
            .is_none_or(|(best_len, _, _)| prefix_len > *best_len);
        if is_more_specific {
            best = Some((prefix_len, targets.as_slice(), capture.to_owned()));
        }
    }

    let Some((_, targets, capture)) = best else {
        return PathAliasMatch::NoMatch;
    };
    PathAliasMatch::Match {
        targets: targets
            .iter()
            .map(|target| target.replace('*', &capture))
            .collect(),
    }
}

/// Return the literal prefix for the only supported wildcard form: one `*` at
/// the end of the key. A key without `*` is an exact key, not a wildcard.
fn supported_wildcard_prefix(key: &str) -> Option<&str> {
    let prefix = key.strip_suffix('*')?;
    (!prefix.contains('*')).then_some(prefix)
}

/// Match a wildcard key for abstention detection. This general glob matcher is
/// used only for unsupported keys, so a matching unsupported shape is visible
/// instead of being mistaken for `NoMatch`.
fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && pattern[pattern_index] != '*'
            && pattern[pattern_index] == value[value_index]
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Per-invocation directory -> nearest-config lookup cache (`OutlineCache`
/// pattern): memoizes both positive and negative results. A fresh invocation
/// constructs a new cache, so added/removed/modified config files are observed.
#[derive(Default)]
pub(crate) struct ConfigCache {
    entries: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
}

impl ConfigCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Nearest tsconfig/jsconfig for `dir`, bounded by the active analysis
    /// scope root. Lookups are memoized per directory for one invocation.
    pub(crate) fn nearest_config(&self, dir: &Path, scope_root: &Path) -> Option<PathBuf> {
        let dir_owned = dir.to_path_buf();
        if let Some(hit) = self.entries.lock().unwrap().get(&dir_owned) {
            return hit.clone();
        }
        let found = find_nearest_config(dir, scope_root);
        self.entries
            .lock()
            .unwrap()
            .insert(dir_owned, found.clone());
        found
    }
}

/// Discovery walk: from `dir` upward to and including `scope_root`. Two-pass:
/// first find the nearest `tsconfig.json` anywhere in the walk; only when no
/// `tsconfig.json` exists does it fall back to the nearest `jsconfig.json` in
/// the same walk (maintainer-chosen precedence, story f0990da: tsconfig wins
/// across the whole walk, same-directory or not). Stops (inclusive) at
/// `scope_root` and never reads `package.json`.
///
/// Scope containment (R3): both paths are canonicalized first, so relative vs
/// absolute spellings and symlink paths compare as the same location. If either
/// canonicalization fails, discovery abstains rather than guessing a boundary.
/// If `dir` does not resolve inside `scope_root`, the walk does not start and
/// returns `None` — it can never climb out of the scope to read an unrelated
/// config.
pub(crate) fn find_nearest_config(dir: &Path, scope_root: &Path) -> Option<PathBuf> {
    let dir = std::fs::canonicalize(dir).ok()?;
    let scope_root = std::fs::canonicalize(scope_root).ok()?;
    if !dir.starts_with(&scope_root) {
        return None; // outside the active analysis scope: no config discovery
    }
    if let Some(ts) = walk_up(&dir, &scope_root, "tsconfig.json") {
        return Some(ts);
    }
    walk_up(&dir, &scope_root, "jsconfig.json")
}

fn walk_up(dir: &Path, scope_root: &Path, config_name: &str) -> Option<PathBuf> {
    let mut current = dir;
    loop {
        let candidate = current.join(config_name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if current == scope_root {
            return None;
        }
        current = current.parent()?;
    }
}

/// Load and parse a config file into the accepted shape. Returns `None` on
/// malformed/unsupported JSONC, missing fields, wrong types, or I/O errors
/// (abstention: leave imports in their Phase-2 classification).
pub(crate) fn load_config(path: &Path) -> Option<TsConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = parse_to_value(&text, &parse_options()).ok()??;
    let JsonValue::Object(root) = value else {
        return None;
    };
    let compiler_options = root.get("compilerOptions")?.clone();
    let JsonValue::Object(options) = compiler_options else {
        return None;
    };

    // baseUrl: must be a string when present.
    let base_url = match options.get("baseUrl") {
        None => None,
        Some(JsonValue::String(s)) => Some(PathBuf::from(s.as_ref())),
        Some(_) => return None, // present but not a string -> abstain
    };

    // paths: must be an object when present; each key -> array of strings.
    let paths = match options.get("paths") {
        None => Vec::new(),
        Some(JsonValue::Object(obj)) => {
            let mut collected = Vec::new();
            for (key, value) in obj.clone() {
                let JsonValue::Array(targets) = value else {
                    return None; // non-array target -> abstain
                };
                let mut list = Vec::new();
                for t in targets {
                    let JsonValue::String(s) = t else {
                        return None; // non-string target -> abstain
                    };
                    list.push(s.to_string());
                }
                collected.push((key.to_string(), list));
            }
            collected
        }
        Some(_) => return None, // paths present but not an object -> abstain
    };

    Some(TsConfig {
        config_path: path.to_path_buf(),
        base_url,
        paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "srcwalk-tsconfig-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // Discovery now returns canonical paths (R3); resolve the root the same
        // way so assertions compare like-for-like (e.g. /var -> /private/var
        // on macOS).
        fs::canonicalize(&root).unwrap()
    }

    #[test]
    fn nested_config_found_from_child_dir() {
        let root = tmp("nested");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let found = find_nearest_config(&root.join("a/b"), &root).unwrap();
        assert_eq!(found, root.join("tsconfig.json"));
    }

    #[test]
    fn tsconfig_wins_over_jsconfig_fallback() {
        // Only jsconfig in tree -> jsconfig used.
        let root = tmp("fallback");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("jsconfig.json"), "{}").unwrap();
        let found = find_nearest_config(&root.join("a"), &root).unwrap();
        assert_eq!(found, root.join("jsconfig.json"));
        // tsconfig added at root -> nearest tsconfig wins above jsconfig.
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let found2 = find_nearest_config(&root.join("a"), &root).unwrap();
        assert_eq!(found2, root.join("tsconfig.json"));
    }

    #[test]
    fn same_dir_tsconfig_beats_jsconfig() {
        let root = tmp("samedir");
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        fs::write(root.join("jsconfig.json"), "{}").unwrap();
        let found = find_nearest_config(&root, &root).unwrap();
        assert_eq!(found, root.join("tsconfig.json"));
    }

    #[test]
    fn scope_root_bounds_discovery() {
        // Config EXISTS above scope root -> must NOT be found.
        let outer = tmp("outer");
        let inner = outer.join("project");
        fs::create_dir_all(&inner).unwrap();
        fs::write(outer.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(find_nearest_config(&inner, &inner), None);
        // And config INSIDE the scope root IS found.
        fs::write(inner.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            find_nearest_config(&inner, &inner),
            Some(inner.join("tsconfig.json"))
        );
    }

    #[test]
    fn no_config_returns_none() {
        let root = tmp("noconfig");
        fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(find_nearest_config(&root.join("src"), &root), None);
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse() {
        let root = tmp("jsonc");
        let cfg = root.join("tsconfig.json");
        fs::write(
            &cfg,
            "{\n  // allow comments\n  \"compilerOptions\": {\n    \"baseUrl\": \"./src\",\n    \"paths\": {\n      \"@/*\": [\"*\"], // trailing comma\n    },\n  },\n}\n",
        )
        .unwrap();
        let loaded = load_config(&cfg).unwrap();
        assert_eq!(loaded.base_url, Some(PathBuf::from("./src")));
        assert_eq!(
            loaded.paths,
            vec![("@/*".to_string(), vec!["*".to_string()])]
        );
    }

    #[test]
    fn malformed_config_abstains() {
        let root = tmp("malformed");
        let cfg = root.join("tsconfig.json");
        fs::write(&cfg, "{ not valid json !!").unwrap();
        assert!(load_config(&cfg).is_none());
        fs::write(&cfg, "").unwrap(); // empty -> None too
        assert!(load_config(&cfg).is_none());
    }

    #[test]
    fn rejected_loose_forms_abstain() {
        let root = tmp("rejected");
        let cases = [
            "{ 'compilerOptions': { 'paths': {} } }",
            "{ \"x\": 0x1F }",
            "{ \"a\": 1 \"b\": 2 }",
            "{ compilerOptions: {} }",
            "{ \"x\": +1 }",
        ];
        for (i, text) in cases.iter().enumerate() {
            let cfg = root.join(format!("r{i}.json"));
            fs::write(&cfg, text).unwrap();
            assert!(load_config(&cfg).is_none(), "form must be rejected: {text}");
        }
    }

    #[test]
    fn wrong_types_abstain() {
        let root = tmp("wtypes");
        let cfg = root.join("tsconfig.json");
        // baseUrl as number and paths value as string -> both abstain.
        fs::write(&cfg, r#"{"compilerOptions":{"baseUrl":42}}"#).unwrap();
        assert!(load_config(&cfg).is_none());
        fs::write(&cfg, r#"{"compilerOptions":{"paths":{"@/*":"*"}}}"#).unwrap();
        assert!(load_config(&cfg).is_none());
    }

    #[test]
    fn memoized_positive_and_negative_lookups() {
        let root = tmp("memo");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let cache = ConfigCache::new();
        // Positive: same dir twice -> same answer.
        let d = root.join("a/b");
        assert_eq!(
            cache.nearest_config(&d, &root),
            Some(root.join("tsconfig.json"))
        );
        assert_eq!(
            cache.nearest_config(&d, &root),
            Some(root.join("tsconfig.json"))
        );
        // Negative at a scope where no config exists in the walk -> memoized None.
        let root2 = tmp("memo2");
        fs::create_dir_all(root2.join("a/b")).unwrap();
        let cache2 = ConfigCache::new();
        assert_eq!(cache2.nearest_config(&root2.join("a/b"), &root2), None);
        assert_eq!(cache2.nearest_config(&root2.join("a/b"), &root2), None);
    }

    #[test]
    fn fresh_invocation_sees_config_changes() {
        let root = tmp("fresh");
        let cache1 = ConfigCache::new();
        assert_eq!(cache1.nearest_config(&root, &root), None);
        // New invocation == new cache: added config is observed.
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let cache2 = ConfigCache::new();
        assert_eq!(
            cache2.nearest_config(&root, &root),
            Some(root.join("tsconfig.json"))
        );
        // Removed config observed by yet another fresh cache.
        fs::remove_file(root.join("tsconfig.json")).unwrap();
        let cache3 = ConfigCache::new();
        assert_eq!(cache3.nearest_config(&root, &root), None);
    }

    #[test]
    fn path_spaces_and_host_independent_separators() {
        let root = tmp("spaces");
        let spaced = root.join("my project").join("α dir");
        fs::create_dir_all(&spaced).unwrap();
        fs::write(spaced.join("tsconfig.json"), "{}").unwrap();
        let found = find_nearest_config(&spaced, &root);
        assert_eq!(found, Some(spaced.join("tsconfig.json")));
    }

    #[test]
    fn tsconfig_anywhere_beats_child_jsconfig() {
        // Maintainer-chosen precedence (story f0990da): tsconfig wins across
        // the WHOLE walk. A jsconfig.json in the child dir must NOT shadow a
        // tsconfig.json in the parent dir.
        let root = tmp("twopass");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/jsconfig.json"), "{}").unwrap();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let found = find_nearest_config(&root.join("a/b"), &root).unwrap();
        assert_eq!(found, root.join("tsconfig.json"));
        // Without the parent tsconfig, the child jsconfig is the answer.
        fs::remove_file(root.join("tsconfig.json")).unwrap();
        let found2 = find_nearest_config(&root.join("a/b"), &root).unwrap();
        assert_eq!(found2, root.join("a/jsconfig.json"));
    }

    #[test]
    fn relative_vs_absolute_spelling_resolves_same_config() {
        // Same location spelled relative to the CWD and absolute must produce
        // the same canonical answer (R3 containment relies on canonical forms).
        // Use a dir under the CWD so a relative spelling exists.
        let base = std::env::current_dir().unwrap().join("target/tmp-relabs");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        fs::write(base.join("src/tsconfig.json"), "{}").unwrap();
        let rel = PathBuf::from("target/tmp-relabs/src");
        let abs = base.join("src");
        let abs_found = find_nearest_config(&abs, &base);
        let rel_found = find_nearest_config(&rel, &base);
        assert!(abs_found.is_some());
        assert_eq!(abs_found, rel_found);
    }

    #[test]
    fn unrelated_dir_outside_scope_returns_none() {
        let root = tmp("outside");
        let other = tmp("other-scope");
        fs::write(other.join("tsconfig.json"), "{}").unwrap();
        // A config exists, but outside the active analysis scope: discovery
        // must NOT climb out of scope to read it (R3 containment).
        assert_eq!(find_nearest_config(&root, &root), None);
        assert_eq!(find_nearest_config(&other, &root), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_scope_resolves_to_real_config() {
        let root = tmp("symlink");
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("tsconfig.json"), "{}").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Discovery through the symlink lands on the real config; canonical
        // containment keeps the walk inside the scope either way.
        let found = find_nearest_config(&link, &root).unwrap();
        assert_eq!(found, real.join("tsconfig.json"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_scope_to_outside_is_rejected() {
        let root = tmp("symlink-boundary");
        let outside = tmp("symlink-outside");
        fs::write(outside.join("tsconfig.json"), "{}").unwrap();
        let link = root.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        // The lexical path is under root, but canonicalization resolves it
        // outside the active scope: reject before either two-pass walk.
        assert_eq!(find_nearest_config(&link, &root), None);
    }

    #[test]
    fn canonicalization_failure_abstains() {
        let root = tmp("canonical-failure");
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        // Neither unresolved boundary may fall back to a lexical walk.
        assert_eq!(find_nearest_config(&root.join("missing-dir"), &root), None);
        assert_eq!(
            find_nearest_config(&root, &root.join("missing-scope-root")),
            None
        );
    }
}

#[cfg(test)]
mod path_match_tests {
    use super::*;
    use std::path::PathBuf;

    fn config(paths: &[(&str, &[&str])]) -> TsConfig {
        TsConfig {
            config_path: PathBuf::from("tsconfig.json"),
            base_url: None,
            paths: paths
                .iter()
                .map(|(key, targets)| {
                    (
                        (*key).to_string(),
                        targets.iter().map(|target| (*target).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn exact_key_beats_wildcard_key() {
        let config = config(&[("@/*", &["wild/*"]), ("@/exact", &["exact.ts"])]);
        assert_eq!(
            match_path_alias("@/exact", &config),
            PathAliasMatch::Match {
                targets: vec!["exact.ts".to_string()],
            }
        );
    }

    #[test]
    fn longest_wildcard_prefix_wins_regardless_of_declaration_order() {
        let config = config(&[("@/*", &["broad/*"]), ("@/lib/*", &["specific/*"])]);
        assert_eq!(
            match_path_alias("@/lib/button", &config),
            PathAliasMatch::Match {
                targets: vec!["specific/button".to_string()],
            }
        );
    }

    #[test]
    fn wildcard_target_order_is_preserved() {
        let config = config(&[("@/*", &["src/*", "generated/*", "fallback/*"])]);
        assert_eq!(
            match_path_alias("@/button", &config),
            PathAliasMatch::Match {
                targets: vec![
                    "src/button".to_string(),
                    "generated/button".to_string(),
                    "fallback/button".to_string(),
                ],
            }
        );
    }

    #[test]
    fn wildcard_substitution_supports_bare_style_keys_and_exact_keys() {
        let config = config(&[
            ("utils/*", &["src/utils/*"]),
            ("react", &["vendor/react.js"]),
        ]);
        assert_eq!(
            match_path_alias("utils/button", &config),
            PathAliasMatch::Match {
                targets: vec!["src/utils/button".to_string()],
            }
        );
        assert_eq!(
            match_path_alias("react", &config),
            PathAliasMatch::Match {
                targets: vec!["vendor/react.js".to_string()],
            }
        );
    }

    #[test]
    fn matching_unsupported_wildcard_abstains_instead_of_falling_back() {
        let config = config(&[
            ("@app/*", &["broad/*"]),
            ("@app/*/utils", &["unsupported/*"]),
        ]);
        assert_eq!(
            match_path_alias("@app/core/utils", &config),
            PathAliasMatch::Abstain
        );
    }

    #[test]
    fn nonmatching_unsupported_wildcard_does_not_block_supported_match() {
        let config = config(&[("@app/*/utils", &["unsupported/*"]), ("@app/*", &["src/*"])]);
        assert_eq!(
            match_path_alias("@app/core", &config),
            PathAliasMatch::Match {
                targets: vec!["src/core".to_string()],
            }
        );
    }

    #[test]
    fn no_match_includes_base_url_alone_without_paths() {
        let base_url_config = TsConfig {
            config_path: PathBuf::from("tsconfig.json"),
            base_url: Some(PathBuf::from("src")),
            paths: Vec::new(),
        };
        assert_eq!(
            match_path_alias("utils", &base_url_config),
            PathAliasMatch::NoMatch
        );

        let config = config(&[("@/*", &["src/*"])]);
        assert_eq!(
            match_path_alias("package/name", &config),
            PathAliasMatch::NoMatch
        );
    }
}
