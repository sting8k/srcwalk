//! P1.2 — bare filename + --section disambiguation.

use std::fs;

fn setup_repo() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tilth_p12_{}_{}",
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
    let cache = tilth::cache::OutlineCache::new();
    let out = tilth::run("Cart.php", &dir, Some("10-15"), None, None, 0, None, &cache).unwrap();

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
    let cache = tilth::cache::OutlineCache::new();
    let out = tilth::run("Cart.php", &dir, None, None, None, 0, None, &cache).unwrap();

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
        "tilth_p12_amb_{}_{}",
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

    let cache = tilth::cache::OutlineCache::new();
    let result = tilth::run("Cart.php", &dir, Some("1-1"), None, None, 0, None, &cache);

    assert!(result.is_err(), "expected error for ambiguous prod paths");
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("exactly one"), "expected disambig error, got: {msg}");
    assert!(msg.contains("Cart.php"), "expected candidate listing, got: {msg}");

    let _ = fs::remove_dir_all(&dir);
}
