use super::display::best_semantic_candidate;
use super::*;
use crate::types::{Match, OutlineEntry, OutlineKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Collect all file paths from a walker into a sorted Vec.
fn walk_paths(scope: &Path, glob: Option<&str>) -> Vec<PathBuf> {
    let w = walker(scope, glob).expect("walker failed");
    let paths: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    w.run(|| {
        let paths = &paths;
        Box::new(move |entry| {
            if let Ok(e) = entry {
                if e.file_type().is_some_and(|ft| ft.is_file()) {
                    paths.lock().unwrap().push(e.into_path());
                }
            }
            ignore::WalkState::Continue
        })
    });
    let mut v = paths.into_inner().unwrap();
    v.sort();
    v
}

fn extensions(paths: &[PathBuf]) -> HashSet<String> {
    paths
        .iter()
        .filter_map(|p| p.extension())
        .map(|e| e.to_string_lossy().to_string())
        .collect()
}

// ── walker unit tests ──

#[test]
fn walker_none_returns_all_file_types() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let all = walk_paths(&scope, None);
    let exts = extensions(&all);
    assert!(exts.contains("rs"), "expected .rs files, got {exts:?}");
    assert!(!all.is_empty());
}

#[test]
fn walker_whitelist_filters_to_matching_extension() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let filtered = walk_paths(&scope, Some("*.rs"));
    assert!(!filtered.is_empty(), "whitelist should find .rs files");
    for p in &filtered {
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("rs"),
            "non-.rs file leaked through whitelist: {}",
            p.display()
        );
    }
}

#[test]
fn walker_negation_excludes_matching_extension() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let without_rs = walk_paths(&scope, Some("!*.rs"));
    for p in &without_rs {
        assert_ne!(
            p.extension().and_then(|e| e.to_str()),
            Some("rs"),
            ".rs file leaked through negation: {}",
            p.display()
        );
    }
}

#[test]
fn walker_empty_string_equals_none() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let all = walk_paths(&scope, None);
    let empty = walk_paths(&scope, Some(""));
    assert_eq!(all.len(), empty.len(), "empty glob should behave like None");
}

#[test]
fn walker_invalid_glob_returns_error() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let result = walker(&scope, Some("[unclosed"));
    match result {
        Err(SrcwalkError::InvalidQuery { query, reason }) => {
            assert_eq!(query, "[unclosed");
            assert!(
                reason.contains("invalid glob"),
                "reason should mention 'invalid glob': {reason}"
            );
        }
        Err(other) => panic!("expected InvalidQuery, got {other}"),
        Ok(_) => panic!("expected Err for invalid glob, got Ok"),
    }
}

#[test]
fn walker_brace_expansion_matches_multiple_extensions() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR"));
    let filtered = walk_paths(&scope, Some("*.{rs,toml}"));
    let exts = extensions(&filtered);
    assert!(
        exts.contains("rs"),
        "brace expansion should include .rs: {exts:?}"
    );
    assert!(
        exts.contains("toml"),
        "brace expansion should include .toml: {exts:?}"
    );
    for ext in &exts {
        assert!(
            ext == "rs" || ext == "toml",
            "unexpected extension leaked: {ext}"
        );
    }
}

#[test]
fn walker_whitelist_fewer_than_unfiltered() {
    // Use project root (not src/) — project root has .toml, .md, .lock etc.
    // alongside .rs files, so *.rs is guaranteed to be a strict subset.
    let scope = Path::new(env!("CARGO_MANIFEST_DIR"));
    let all = walk_paths(&scope, None);
    let rs_only = walk_paths(&scope, Some("*.rs"));
    assert!(
        rs_only.len() < all.len(),
        "whitelist ({}) should find fewer files than unfiltered ({})",
        rs_only.len(),
        all.len()
    );
}

#[test]
fn walker_path_pattern_restricts_directory() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR"));
    let filtered = walk_paths(&scope, Some("src/**/*.rs"));
    assert!(!filtered.is_empty(), "path pattern should find files");
    let src_dir = scope.join("src");
    for p in &filtered {
        assert!(
            p.starts_with(&src_dir),
            "file outside src/ leaked: {}",
            p.display()
        );
    }
}

#[test]
fn walker_respects_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = tmp.path();
    std::fs::create_dir(scope.join(".git")).unwrap();
    std::fs::write(scope.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(scope.join("visible.rs"), "fn visible_symbol() {}\n").unwrap();
    std::fs::write(scope.join("ignored.rs"), "fn ignored_symbol() {}\n").unwrap();

    let paths = walk_paths(scope, None);

    assert!(
        paths.iter().any(|p| p.ends_with("visible.rs")),
        "visible file should be walked: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with("ignored.rs")),
        "gitignored file leaked into walker: {paths:?}"
    );
}

#[test]
fn walker_respects_dotignore() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = tmp.path();
    std::fs::write(scope.join(".ignore"), "ignored.rs\n").unwrap();
    std::fs::write(scope.join("visible.rs"), "fn visible_symbol() {}\n").unwrap();
    std::fs::write(scope.join("ignored.rs"), "fn ignored_symbol() {}\n").unwrap();

    let paths = walk_paths(scope, None);

    assert!(
        paths.iter().any(|p| p.ends_with("visible.rs")),
        "visible file should be walked: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with("ignored.rs")),
        ".ignore file leaked into walker: {paths:?}"
    );
}

// ── end-to-end through search functions ──

#[test]
fn discovery_searches_respect_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = tmp.path();
    std::fs::create_dir(scope.join(".git")).unwrap();
    std::fs::write(scope.join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(scope.join("visible.rs"), "fn visible_symbol() {}\n").unwrap();
    std::fs::write(scope.join("ignored.rs"), "fn ignored_symbol() {}\n").unwrap();

    let symbols =
        symbol::search("ignored_symbol", scope, None, None, None).expect("symbol search failed");
    let globs = glob::search("*.rs", scope, None, 0).expect("glob search failed");

    assert_eq!(
        symbols.total_found, 0,
        "symbol search should not return ignored files"
    );
    assert!(
        globs.files.iter().any(|f| f.path.ends_with("visible.rs")),
        "glob should include visible file"
    );
    let leaked_ignored = globs.files.iter().any(|f| f.path.ends_with("ignored.rs"));
    assert!(!leaked_ignored, "glob search leaked ignored file");
}

#[test]
fn discovery_searches_respect_dotignore() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = tmp.path();
    std::fs::write(scope.join(".ignore"), "ignored.rs\n").unwrap();
    std::fs::write(scope.join("visible.rs"), "fn visible_symbol() {}\n").unwrap();
    std::fs::write(scope.join("ignored.rs"), "fn ignored_symbol() {}\n").unwrap();

    let symbols =
        symbol::search("ignored_symbol", scope, None, None, None).expect("symbol search failed");
    let globs = glob::search("*.rs", scope, None, 0).expect("glob search failed");

    assert_eq!(
        symbols.total_found, 0,
        "symbol search should not return .ignore files"
    );
    assert!(
        globs.files.iter().any(|f| f.path.ends_with("visible.rs")),
        "glob should include visible file"
    );
    let leaked_ignored = globs.files.iter().any(|f| f.path.ends_with("ignored.rs"));
    assert!(!leaked_ignored, "glob search leaked .ignore file");
}

#[test]
fn content_search_glob_restricts_results() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let all = content::search("SrcwalkError", &scope, false, None, None).expect("search failed");
    let rs_only = content::search("SrcwalkError", &scope, false, None, Some("*.rs"))
        .expect("search with glob failed");
    let toml_only = content::search("SrcwalkError", &scope, false, None, Some("*.toml"))
        .expect("search with toml glob failed");

    assert!(all.total_found > 0, "unfiltered should find SrcwalkError");
    assert!(rs_only.total_found > 0, "*.rs should find SrcwalkError");
    assert_eq!(
        toml_only.total_found, 0,
        "*.toml should not find SrcwalkError in Rust source"
    );
    for m in &rs_only.matches {
        assert_eq!(
            m.path.extension().and_then(|e| e.to_str()),
            Some("rs"),
            "non-.rs match leaked: {}",
            m.path.display()
        );
    }
}

#[test]
fn symbol_search_glob_restricts_results() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let rs_result =
        symbol::search("walker", &scope, None, None, Some("*.rs")).expect("symbol search failed");
    let toml_result = symbol::search("walker", &scope, None, None, Some("*.toml"))
        .expect("symbol search with toml failed");

    assert!(rs_result.total_found > 0, "*.rs should find 'walker'");
    assert_eq!(
        toml_result.total_found, 0,
        "*.toml should not find 'walker'"
    );
    for m in &rs_result.matches {
        assert_eq!(
            m.path.extension().and_then(|e| e.to_str()),
            Some("rs"),
            "non-.rs match in symbol search: {}",
            m.path.display()
        );
    }
}

#[test]
fn callers_search_glob_restricts_results() {
    let scope = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let bloom = crate::index::bloom::BloomFilterCache::new();
    let rs_callers = callers::find_callers("walker", &scope, &bloom, Some("*.rs"), None)
        .expect("callers failed");
    let toml_callers = callers::find_callers("walker", &scope, &bloom, Some("*.toml"), None)
        .expect("callers toml failed");

    assert!(
        !rs_callers.is_empty(),
        "*.rs should find callers of 'walker'"
    );
    assert!(
        toml_callers.is_empty(),
        "*.toml should not find callers of 'walker'"
    );
    for c in &rs_callers {
        assert_eq!(
            c.path.extension().and_then(|e| e.to_str()),
            Some("rs"),
            "non-.rs caller leaked: {}",
            c.path.display()
        );
    }
}

#[test]
fn walker_follows_symlinked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real");
    std::fs::create_dir(&real_dir).unwrap();
    std::fs::write(real_dir.join("hello.rs"), "fn main() {}").unwrap();

    let link_dir = tmp.path().join("linked");
    std::fs::create_dir(&link_dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(real_dir.join("hello.rs"), link_dir.join("hello.rs")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(real_dir.join("hello.rs"), link_dir.join("hello.rs"))
        .unwrap();

    let paths = walk_paths(tmp.path(), None);
    let names: Vec<&str> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    // Should find hello.rs twice: once in real/, once via the symlink in linked/
    assert_eq!(
        names.iter().filter(|n| **n == "hello.rs").count(),
        2,
        "expected hello.rs from both real and symlinked dirs, got: {names:?}"
    );
}

#[test]
fn walker_follows_symlinked_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real_pkg");
    std::fs::create_dir(&real_dir).unwrap();
    std::fs::write(real_dir.join("lib.rs"), "pub fn add() {}").unwrap();
    std::fs::write(real_dir.join("util.rs"), "pub fn helper() {}").unwrap();

    // Symlink the entire directory
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_dir, tmp.path().join("deps_link")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real_dir, tmp.path().join("deps_link")).unwrap();

    let paths = walk_paths(tmp.path(), None);
    let link_files: Vec<_> = paths
        .iter()
        .filter(|p| p.starts_with(tmp.path().join("deps_link")))
        .collect();
    assert_eq!(
        link_files.len(),
        2,
        "expected 2 files via symlinked directory, got: {link_files:?}"
    );
}

#[test]
fn walker_survives_symlink_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("real.rs"), "fn main() {}").unwrap();

    // Create a symlink cycle: loop -> .
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path(), tmp.path().join("loop")).unwrap();

    // Should complete without hanging — ignore crate detects the cycle via inode tracking
    let paths = walk_paths(tmp.path(), None);
    let names: Vec<&str> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    assert!(
        names.contains(&"real.rs"),
        "should find real.rs despite cycle: {names:?}"
    );
}

#[test]
fn semantic_candidate_prefers_class_entry_for_generated_stub_range() {
    let entries = vec![OutlineEntry {
        kind: OutlineKind::Module,
        name: "Microsoft.UI.Xaml".to_string(),
        start_line: 4,
        end_line: 17,
        signature: None,
        children: vec![OutlineEntry {
            kind: OutlineKind::Class,
            name: "DependencyProperty".to_string(),
            start_line: 6,
            end_line: 16,
            signature: None,
            children: vec![OutlineEntry {
                kind: OutlineKind::Function,
                name: "DependencyProperty".to_string(),
                start_line: 9,
                end_line: 12,
                signature: Some("public DependencyProperty()".to_string()),
                children: Vec::new(),
                doc: None,
            }],
            doc: None,
        }],
        doc: None,
    }];
    let m = Match {
        path: std::path::PathBuf::from("DependencyProperty.cs"),
        line: 6,
        text: "#if false".to_string(),
        is_definition: true,
        exact: true,
        file_lines: 17,
        mtime: std::time::SystemTime::UNIX_EPOCH,
        def_range: Some((6, 16)),
        def_name: Some("DependencyProperty".to_string()),
        def_weight: 100,
        impl_target: None,
        base_target: None,
        in_comment: false,
    };

    let candidate = best_semantic_candidate(&entries, &m).expect("semantic candidate");
    assert_eq!(candidate.kind, OutlineKind::Class);
    assert_eq!(candidate.name, "DependencyProperty");
    assert_eq!(candidate.parents, vec!["Microsoft.UI.Xaml"]);
    assert_eq!((candidate.start_line, candidate.end_line), (6, 16));
    assert_eq!(candidate.children.len(), 1);
    assert_eq!(candidate.children[0].kind, OutlineKind::Function);
}

#[test]
fn content_search_finds_symbol_through_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real");
    std::fs::create_dir(&real_dir).unwrap();
    std::fs::write(
        real_dir.join("api.rs"),
        "pub fn unique_symlink_test_symbol() {}",
    )
    .unwrap();

    // Symlink the directory into the search scope
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_dir, tmp.path().join("linked")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real_dir, tmp.path().join("linked")).unwrap();

    let result =
        content::search("unique_symlink_test_symbol", tmp.path(), false, None, None).unwrap();
    // Should find the symbol in both real/api.rs and linked/api.rs
    assert!(
        result.total_found >= 2,
        "expected symbol found via both real and symlinked paths, got {}",
        result.total_found
    );
}
