//! P1.2 — bare filename + --section disambiguation.

use std::fs;

fn setup_repo() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_p12_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(dir.join("classes")).unwrap();
    fs::create_dir_all(dir.join("tests/Resources/modules_tests/override/classes")).unwrap();
    fs::create_dir_all(dir.join("vendor/acme/classes")).unwrap();

    let body = (1..=30)
        .map(|i| format!("// line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("classes/Cart.php"), &body).unwrap();
    fs::write(
        dir.join("tests/Resources/modules_tests/override/classes/Cart.php"),
        "// test copy\n",
    )
    .unwrap();
    fs::write(dir.join("vendor/acme/classes/Cart.php"), "// vendor copy\n").unwrap();
    dir
}

#[test]
fn bare_filename_with_section_auto_resolves_to_prod() {
    let dir = setup_repo();
    let cache = srcwalk::cache::OutlineCache::new();
    let out = srcwalk::run("Cart.php", &dir, Some("10-15"), None, None, 0, None, &cache).unwrap();

    assert!(
        out.contains("Resolved 'Cart.php'"),
        "expected resolution note, got: {out}"
    );
    assert!(
        out.contains("classes/Cart.php"),
        "expected prod path in output, got: {out}"
    );
    assert!(
        out.contains("line 10") && out.contains("line 15"),
        "expected section lines 10–15, got: {out}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_no_section_unchanged() {
    let dir = setup_repo();
    let cache = srcwalk::cache::OutlineCache::new();
    let out = srcwalk::run("Cart.php", &dir, None, None, None, 0, None, &cache).unwrap();

    // Glob output, not a section view.
    assert!(
        out.contains("Glob:") || out.contains("files"),
        "expected glob listing, got: {out}"
    );
    assert!(
        !out.contains("Resolved 'Cart.php'"),
        "resolution note should only fire with --section, got: {out}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_with_section_ambiguous_prod_fails_loud() {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_p12_amb_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(dir.join("src/a")).unwrap();
    fs::create_dir_all(dir.join("src/b")).unwrap();
    fs::write(dir.join("src/a/Cart.php"), "// a\n").unwrap();
    fs::write(dir.join("src/b/Cart.php"), "// b\n").unwrap();

    let cache = srcwalk::cache::OutlineCache::new();
    let result = srcwalk::run("Cart.php", &dir, Some("1-1"), None, None, 0, None, &cache);

    assert!(result.is_err(), "expected error for ambiguous prod paths");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("exactly one"),
        "expected disambig error, got: {msg}"
    );
    assert!(
        msg.contains("Cart.php"),
        "expected candidate listing, got: {msg}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_respects_gitignore_for_disambig() {
    // Repo with a bespoke "benchmark/" dir NOT in NON_PROD_DIR_SEGMENTS.
    // Only .gitignore marks it as non-primary.
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_p12fix_gi_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("benchmark/nested")).unwrap();
    fs::write(dir.join(".gitignore"), "benchmark/\n").unwrap();
    fs::write(dir.join("src/lib.rs"), "// main\n").unwrap();
    fs::write(dir.join("benchmark/nested/lib.rs"), "// bench copy\n").unwrap();

    let cache = srcwalk::cache::OutlineCache::new();
    let out = srcwalk::run("lib.rs", &dir, Some("1-1"), None, None, 0, None, &cache).unwrap();

    assert!(
        out.contains("Resolved 'lib.rs'") && out.contains("src/lib.rs"),
        "gitignore-marked benchmark should be non-primary, got: {out}"
    );
    assert!(
        out.contains("non-primary"),
        "wording should say 'non-primary', got: {out}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_filename_depth_rank_picks_shallowest() {
    // Two primary candidates (no .gitignore filter). Depth-rank tiebreaker
    // should pick the shallowest.
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_p12fix_depth_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("pkg/deep/nested")).unwrap();
    fs::write(dir.join("src/main.rs"), "// shallow\n").unwrap();
    fs::write(dir.join("pkg/deep/nested/main.rs"), "// deep\n").unwrap();

    let cache = srcwalk::cache::OutlineCache::new();
    let out = srcwalk::run("main.rs", &dir, Some("1-1"), None, None, 0, None, &cache).unwrap();

    assert!(
        out.contains("Resolved 'main.rs'"),
        "expected resolution via depth-rank, got: {out}"
    );
    let picked_line = out.lines().find(|l| l.contains("Resolved")).unwrap_or("");
    assert!(
        picked_line.contains("→ src/main.rs"),
        "expected shallowest path picked, got: {picked_line}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn no_match_concept_query_offers_symbol_suggestion() {
    // P1.3.fix-edge — concept/fallthrough miss should suggest cross-convention
    // symbol via search::symbol::suggest, not just file-name lookup.
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_nomatch_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("api.go"),
        "package main\nfunc ProcessRequest() {}\n",
    )
    .unwrap();

    let cache = srcwalk::cache::OutlineCache::new();
    // bare lowercase token → Concept path → no symbol/content hit → must
    // surface the cross-convention suggestion.
    let err = srcwalk::run("processrequst", &dir, None, None, None, 0, None, &cache)
        .expect_err("expected NoMatches");
    let msg = format!("{err}");
    assert!(
        msg.contains("no matches for \"processrequst\""),
        "expected NoMatches wording, got: {msg}"
    );
    assert!(
        msg.contains("Did you mean: ProcessRequest"),
        "expected symbol suggestion, got: {msg}"
    );
}
