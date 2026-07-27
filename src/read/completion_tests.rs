use super::*;
use std::path::Path;

fn expected_show_next(path: &Path, range: &str) -> String {
    let display_path = crate::format::display_path(path);
    let target = crate::format::shell_quote_arg(&format!("{display_path}:{range}"))
        .expect("test path should be shell-quotable");
    format!("> Next: srcwalk show {target}")
}

fn expected_show_sections_next(path: &Path, sections: &str) -> String {
    let display_path = crate::format::display_path(path);
    let target =
        crate::format::shell_quote_arg(&display_path).expect("test path should be shell-quotable");
    format!("> Next: srcwalk show {target} --section {sections}")
}

#[test]
fn partial_numeric_range_exposes_exact_structural_completion() {
    let path = std::env::temp_dir().join("srcwalk_partial_range_completion.rs");
    std::fs::write(
        &path,
        "fn outer() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    println!(\"{a}{b}{c}\");\n}\n",
    )
    .unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, Some("1-3"), false, &cache).unwrap();
    let expected_next = expected_show_next(&path, "4-6");

    assert!(
        out.contains("> Caveat: selected lines 1-3 are partial inside structural function 1-6; omitted lines 4-6."),
        "expected partial structural cue: {out}"
    );
    assert!(
        out.contains(&expected_next),
        "expected exact completion command: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn partial_numeric_range_completion_is_multilanguage() {
    let fixtures = [
        (
            "srcwalk_partial_range_completion.py",
            "def outer():\n    a = 1\n    b = 2\n    c = 3\n    return a + b + c\n",
            "1-3",
            "1-5",
            "4-5",
        ),
        (
            "srcwalk_partial_range_completion.go",
            "package sample\n\nfunc Outer() int {\n    a := 1\n    b := 2\n    c := 3\n    return a + b + c\n}\n",
            "3-5",
            "3-8",
            "6-8",
        ),
    ];

    for (name, source, selected, target, omitted) in fixtures {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, source).unwrap();

        let cache = OutlineCache::new();
        let out = read_file(&path, Some(selected), false, &cache).unwrap();

        assert!(
            out.contains(&format!("partial inside structural function {target}")),
            "expected {name} structural completion: {out}"
        );
        assert!(
            out.contains(&expected_show_next(&path, omitted)),
            "expected {name} exact completion command: {out}"
        );

        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn partial_numeric_range_completion_can_target_head_and_tail_only() {
    let path = std::env::temp_dir().join("srcwalk_middle_range_completion.rs");
    std::fs::write(
        &path,
        "fn outer() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    println!(\"{a}{b}{c}\");\n}\n",
    )
    .unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, Some("3-4"), false, &cache).unwrap();
    let expected_next = expected_show_sections_next(&path, "1-2,5-6");

    assert!(
        out.contains("omitted lines 1-2,5-6"),
        "expected omitted head and tail cue: {out}"
    );
    assert!(
        out.contains(&expected_next),
        "expected exact missing-only command: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn partial_numeric_range_completion_handles_boundary_overlap_suffix() {
    let path = std::env::temp_dir().join("srcwalk_boundary_suffix_completion.rs");
    std::fs::write(
        &path,
        "fn first() {\n    let done = 1;\n}\n\nfn second() {\n    let a = 1;\n    let b = 2;\n    println!(\"{a}{b}\");\n}\n",
    )
    .unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, Some("4-7"), false, &cache).unwrap();
    let expected_next = expected_show_next(&path, "8-9");

    assert!(
        out.contains("> Source frame: requested 4-7; displayed 4-7; spans 1 structural function; not enclosed."),
        "expected non-enclosed frame for suffix boundary-overlap range: {out}"
    );
    assert!(
        !out.contains("within fn second") && !out.contains("complete"),
        "non-enclosed frame should not name second or claim complete: {out}"
    );
    assert_eq!(
        out.matches("> Source frame:").count(),
        1,
        "boundary-overlap suffix should emit one source frame: {out}"
    );

    assert!(
        out.contains("> Caveat: selected lines 4-7 are partial inside structural function 5-9; omitted lines 8-9."),
        "expected suffix completion for boundary-overlap range: {out}"
    );
    assert!(
        out.contains(&expected_next),
        "expected exact suffix command: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn partial_numeric_range_completion_handles_boundary_overlap_prefix() {
    let path = std::env::temp_dir().join("srcwalk_boundary_prefix_completion.go");
    std::fs::write(
        &path,
        "package sample\n\nfunc First() int {\n    a := 1\n    b := 2\n    return a + b\n}\n\nfunc Second() int {\n    return 2\n}\n",
    )
    .unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, Some("5-8"), false, &cache).unwrap();
    let expected_next = expected_show_next(&path, "3-4");

    assert!(
        out.contains("> Source frame: requested 5-8; displayed 5-8; spans 1 structural function; not enclosed."),
        "expected non-enclosed frame for prefix boundary-overlap range: {out}"
    );
    assert!(
        !out.contains("within fn First") && !out.contains("complete"),
        "non-enclosed frame should not name First or claim complete: {out}"
    );
    assert_eq!(
        out.matches("> Source frame:").count(),
        1,
        "boundary-overlap prefix should emit one source frame: {out}"
    );

    assert!(
        out.contains("> Caveat: selected lines 5-8 are partial inside structural function 3-7; omitted lines 3-4."),
        "expected prefix completion for boundary-overlap range: {out}"
    );
    assert!(
        out.contains(&expected_next),
        "expected exact prefix command: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn complete_structural_reads_do_not_emit_partial_completion() {
    let path = std::env::temp_dir().join("srcwalk_complete_range_no_completion.rs");
    std::fs::write(&path, "fn outer() {\n    let a = 1;\n}\n").unwrap();
    let cache = OutlineCache::new();

    let range = read_file(&path, Some("1-3"), false, &cache).unwrap();
    let symbol = read_file(&path, Some("outer"), false, &cache).unwrap();

    assert!(
        !range.contains("partial inside structural function"),
        "{range}"
    );
    assert!(
        !symbol.contains("partial inside structural function"),
        "{symbol}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn non_structural_ranges_abstain_from_function_completion() {
    for name in [
        "srcwalk_partial_completion_notes.txt",
        "srcwalk_partial_completion_notes.md",
    ] {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, "# Heading\n\none\n\ntwo\n").unwrap();
        let cache = OutlineCache::new();

        let out = read_file(&path, Some("1-3"), false, &cache).unwrap();

        assert!(!out.contains("partial inside structural function"), "{out}");
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn partial_range_uses_available_enclosing_function() {
    let path = std::env::temp_dir().join("srcwalk_partial_completion.py");
    std::fs::write(&path, "def outer():\n    value = 1\n    return value\n").unwrap();
    let cache = OutlineCache::new();

    let out = read_file(&path, Some("2-2"), false, &cache).unwrap();

    assert!(
        out.contains("partial inside structural function 1-3"),
        "expected enclosing function: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn partial_completion_quotes_unsafe_path_target() {
    let path = std::env::temp_dir().join("srcwalk partial's range.rs");
    std::fs::write(&path, "fn outer() {\n    let value = 1;\n}\n").unwrap();
    let cache = OutlineCache::new();

    let out = read_file(&path, Some("1-2"), false, &cache).unwrap();

    assert!(
        out.contains("> Next: srcwalk show '") && out.contains(":3'"),
        "expected quoted full path:range target: {out}"
    );

    let _ = std::fs::remove_file(&path);
}
