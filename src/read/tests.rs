use super::*;

#[test]
fn heading_found() {
    let input = b"# Title\nSome content\n## Section\nSection content\n";
    let result = resolve_heading(input, "## Section");

    assert_eq!(result, Some((3, 4)));
}

#[test]
fn heading_not_found() {
    let input = b"# Title\nContent\n";
    let result = resolve_heading(input, "## Missing");

    assert_eq!(result, None);
}

#[test]
fn heading_in_code_block() {
    let input = b"# Real\n```\n## Fake\n```\n";
    let result = resolve_heading(input, "## Fake");

    // Heading inside code block should be skipped
    assert_eq!(result, None);
}

#[test]
fn duplicate_headings() {
    let input = b"## First\ntext\n## First\ntext\n";
    let result = resolve_heading(input, "## First");

    // Should return the first occurrence
    assert_eq!(result, Some((1, 2)));
}

#[test]
fn last_heading_to_eof() {
    let input = b"# Start\ntext\n## End\nfinal line\n";
    let result = resolve_heading(input, "## End");

    // Last heading should extend to total_lines (4)
    assert_eq!(result, Some((3, 4)));
}

#[test]
fn nested_sections() {
    let input = b"## A\ncontent\n### B\nmore\n## C\ntext\n";
    let result = resolve_heading(input, "## A");

    // ## A should include ### B, ending when ## C starts (line 5)
    // So range is [1, 4]
    assert_eq!(result, Some((1, 4)));
}

#[test]
fn no_hashes() {
    let input = b"# Heading\ntext\n";

    // Empty string
    assert_eq!(resolve_heading(input, ""), None);

    // String without hashes
    assert_eq!(resolve_heading(input, "hello"), None);
}

#[test]
fn default_path_read_returns_outline_not_full() {
    let path = std::env::temp_dir().join("srcwalk_default_outline.rs");
    std::fs::write(&path, b"fn alpha() {}\nfn beta() {}\n").unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, None, false, &cache).unwrap();

    assert!(out.contains("[outline]"), "expected outline header: {out}");
    assert!(
        !out.contains("[full]"),
        "default read must not be full: {out}"
    );
    assert!(
        out.contains("alpha"),
        "outline should include symbols: {out}"
    );
    assert!(
        out.contains("retry with --full"),
        "outline footer should mention --full for raw text: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn explicit_full_fits_raw_caps() {
    let path = std::env::temp_dir().join("srcwalk_full_fits.rs");
    std::fs::write(&path, b"fn alpha() {}\nfn beta() {}\n").unwrap();

    let cache = OutlineCache::new();
    let out = read_file(&path, None, true, &cache).unwrap();

    assert!(
        out.contains("[full]"),
        "explicit full should be full: {out}"
    );
    assert!(
        out.contains("1  fn alpha()"),
        "full body should be numbered: {out}"
    );
    assert!(
        !out.contains("full capped"),
        "small full should not cap: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn explicit_full_caps_after_raw_line_limit() {
    use std::io::Write;

    let path = std::env::temp_dir().join("srcwalk_full_line_cap.rs");
    let mut f = std::fs::File::create(&path).unwrap();
    for i in 0..250 {
        writeln!(f, "fn func_{i}() {{}}").unwrap();
    }
    drop(f);

    let cache = OutlineCache::new();
    let out = read_file(&path, None, true, &cache).unwrap();

    assert!(
        out.contains("full capped — tokens ~"),
        "expected cap warning: {out}"
    );
    assert!(
        out.contains("lines 200/251"),
        "expected 200-line page: {out}"
    );
    assert!(
        out.contains("--section 201-<end>"),
        "expected next-page hint: {out}"
    );
    assert!(
        out.contains("func_0"),
        "expected first page body/outline: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn long_section_over_200_lines_returns_source_when_within_token_limit() {
    use std::io::Write;

    let path = std::env::temp_dir().join("srcwalk_section_long_fn.rs");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "fn long_fn() {{").unwrap();
    for i in 0..220 {
        writeln!(f, "    let value_{i} = {i};").unwrap();
    }
    writeln!(f, "}}").unwrap();
    drop(f);

    let cache = OutlineCache::new();
    let out = read_file(&path, Some("long_fn"), false, &cache).unwrap();

    assert!(out.contains("[section]"), "expected raw section: {out}");
    assert!(
        out.contains("let value_219 = 219;"),
        "expected full long function source: {out}"
    );
    assert!(
        !out.contains("[section, outline (over limit)]"),
        "line count alone should not force an outline: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn section_budget_controls_token_degradation() {
    let path = std::env::temp_dir().join("srcwalk_section_budget.rs");
    let mut body = String::from("fn noisy() {\n");
    for i in 0..80 {
        body.push_str(&format!(
                "    let value_{i} = \"padding padding padding padding padding padding padding padding\";\n"
            ));
    }
    body.push_str("}\n");
    std::fs::write(&path, body).unwrap();

    let cache = OutlineCache::new();
    let low_budget = read_file_with_budget(&path, Some("noisy"), false, Some(100), &cache).unwrap();
    assert!(
        low_budget.contains("[section, outline (over limit)]"),
        "expected low budget to outline: {low_budget}"
    );
    assert!(
        low_budget.contains("section cap ~") && low_budget.contains("/100 tokens"),
        "expected budget limit in footer: {low_budget}"
    );

    assert!(
        low_budget.contains("use narrower --section or --budget <N>"),
        "non-artifact source files should keep generic over-limit advice: {low_budget}"
    );

    let high_budget =
        read_file_with_budget(&path, Some("noisy"), false, Some(5_000), &cache).unwrap();
    assert!(
        high_budget.contains("[section]") && high_budget.contains("padding padding"),
        "expected high budget to return source: {high_budget}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn minified_js_section_over_limit_suggests_artifact_mode() {
    let path = std::env::temp_dir().join("srcwalk_minified_section_hint.js");
    let padding = "x".repeat(4_000);
    std::fs::write(&path, format!("function target(){{return '{padding}'}}")).unwrap();

    let cache = OutlineCache::new();
    let out = read_file_with_budget(&path, Some("target"), false, Some(100), &cache).unwrap();
    assert!(out.contains("[section, outline (over limit)]"), "{out}");
    assert!(out.contains("minified artifact?"), "{out}");
    assert!(out.contains("--artifact --section target"), "{out}");
    assert!(
        out.contains("--artifact --section bytes:<start>-<end>"),
        "{out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn budget_cascade_full_to_outline() {
    // Build a file large enough that --full would emit ~5k tokens.
    let mut body = String::from("<?php\nclass Big {\n");
    for i in 0..120 {
        body.push_str(&format!(
                "    public function method_{i}() {{\n        $x = {i}; // padding line {i}\n        return $x * 2;\n    }}\n"
            ));
    }
    body.push_str("}\n");
    let path = std::env::temp_dir().join("srcwalk_p11_cascade.php");
    std::fs::write(&path, body.as_bytes()).unwrap();

    let cache = OutlineCache::new();
    let out = read_file_with_budget(&path, None, true, Some(800), &cache).unwrap();

    // Budget honored.
    let tokens = estimate_tokens(out.len() as u64);
    assert!(tokens <= 800, "cascade overshot budget: {tokens} tokens");
    // Header relabelled, not [full].
    assert!(
        out.contains("[outline (full requested, over budget)]") || out.contains("[signatures"),
        "expected cascade header label, got: {}",
        &out[..out.len().min(200)]
    );
    // Cascade note present.
    assert!(
        out.contains("budget ~") && out.contains("compacted outline"),
        "missing cascade note: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn budget_cascade_passthrough_when_fits() {
    // Tiny file fits in budget → unchanged behavior (full content).
    let path = std::env::temp_dir().join("srcwalk_p11_tiny.php");
    std::fs::write(&path, b"<?php\nclass Tiny { public function f() {} }\n").unwrap();

    let cache = OutlineCache::new();
    let out = read_file_with_budget(&path, None, true, Some(2000), &cache).unwrap();

    assert!(
        out.contains("[full]"),
        "expected [full] label, got header in: {out}"
    );
    assert!(
        !out.contains("downgraded"),
        "no cascade note for fitting file"
    );

    let _ = std::fs::remove_file(&path);
}

// --- suggest_symbols tests ---

#[test]
fn suggest_symbols_prefix_match() {
    let code = b"fn collect_ranges() {}\nfn collect_names() {}\nfn parse_input() {}\n";
    let path = std::env::temp_dir().join("srcwalk_suggest_prefix.rs");
    std::fs::write(&path, code).unwrap();

    let suggestions = suggest_symbols(code, &path, "collect", 3);
    assert!(
        suggestions.len() >= 2,
        "expected at least 2 prefix matches: {suggestions:?}"
    );
    // Prefix matches should come first (distance 0)
    assert!(
        suggestions[0].starts_with("collect_"),
        "first should be prefix match: {}",
        suggestions[0]
    );
    assert!(
        suggestions[1].starts_with("collect_"),
        "second should be prefix match: {}",
        suggestions[1]
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn suggest_symbols_edit_distance_fallback() {
    let code = b"fn tag_comment_matches() {}\nfn find_symbol() {}\n";
    let path = std::env::temp_dir().join("srcwalk_suggest_edit.rs");
    std::fs::write(&path, code).unwrap();

    let suggestions = suggest_symbols(code, &path, "tag_comment", 3);
    assert!(!suggestions.is_empty(), "should have suggestions");
    assert!(
        suggestions[0].contains("tag_comment_matches"),
        "closest should be tag_comment_matches: {}",
        suggestions[0]
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn suggest_symbols_includes_line_ranges() {
    let code = b"fn alpha() {}\nfn beta() {}\n";
    let path = std::env::temp_dir().join("srcwalk_suggest_ranges.rs");
    std::fs::write(&path, code).unwrap();

    let suggestions = suggest_symbols(code, &path, "alph", 3);
    assert!(!suggestions.is_empty());
    // Format should be "name [start-end]"
    assert!(
        suggestions[0].contains('[') && suggestions[0].contains(']'),
        "should include line range: {}",
        suggestions[0]
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn suggest_symbols_empty_for_non_code() {
    let md = b"# Heading\nSome text\n";
    let path = std::env::temp_dir().join("srcwalk_suggest_md.md");
    std::fs::write(&path, md).unwrap();

    let suggestions = suggest_symbols(md, &path, "foo", 3);
    assert!(
        suggestions.is_empty(),
        "non-code file should return empty suggestions"
    );

    let _ = std::fs::remove_file(&path);
}

// --- symbol suggest on miss integration ---

#[test]
fn section_symbol_miss_shows_suggestions() {
    let code = "fn resolve_heading() {}\nfn resolve_symbol() {}\nfn resolve_range() {}\n";
    let path = std::env::temp_dir().join("srcwalk_section_miss.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let err = read_section(&path, "resolve_sym", None, &cache).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("symbol not found. Closest:"),
        "should show suggestions: {msg}"
    );
    assert!(
        msg.contains("resolve_symbol"),
        "should suggest resolve_symbol: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}

// --- multi-symbol section tests ---

#[test]
fn c_kr_function_section_resolves_by_name() {
    let code = r#"static int rust_demangle_callback(data, len)
  const char *data;
  int len;
{
  return 0;
}
"#;
    let path = std::env::temp_dir().join("srcwalk_kr_section.c");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "rust_demangle_callback", None, &cache).unwrap();
    assert!(
        out.contains("rust_demangle_callback(data, len)") && out.contains("return 0;"),
        "K&R C function should resolve as a named section: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_symbol_section_returns_all_bodies() {
    let code = "fn aaa() {\n    1\n}\nfn bbb() {\n    2\n}\nfn ccc() {\n    3\n}\n";
    let path = std::env::temp_dir().join("srcwalk_multi_sym.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "aaa,ccc", None, &cache).unwrap();
    assert!(
        out.contains("2 symbols, section"),
        "header should show symbol count: {out}"
    );
    assert!(out.contains("aaa()"), "should contain aaa body");
    assert!(out.contains("ccc()"), "should contain ccc body");
    assert!(!out.contains("bbb()"), "should NOT contain bbb body");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_symbol_section_sorted_by_line_order() {
    let code = "fn first() {\n    1\n}\nfn second() {\n    2\n}\n";
    let path = std::env::temp_dir().join("srcwalk_multi_order.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    // Request in reverse order
    let out = read_section(&path, "second,first", None, &cache).unwrap();
    let pos_first = out.find("first()").unwrap();
    let pos_second = out.find("second()").unwrap();
    assert!(
        pos_first < pos_second,
        "should be sorted by line order, not request order"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_symbol_section_partial_miss_returns_found() {
    let code = "fn real_fn() {}\nfn other_fn() {}\n";
    let path = std::env::temp_dir().join("srcwalk_multi_miss.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "real_fn,nope_fn", None, &cache).unwrap();
    assert!(
        out.contains("real_fn()"),
        "should contain found symbol: {out}"
    );
    assert!(
        out.contains("Missing symbols"),
        "should note missing: {out}"
    );
    assert!(out.contains("nope_fn"), "should name missing symbol: {out}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_symbol_section_all_miss_errors() {
    let code = "fn real_fn() {}\n";
    let path = std::env::temp_dir().join("srcwalk_multi_all_miss.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let err = read_section(&path, "zzz_fake,yyy_fake", None, &cache).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("symbols not found"),
        "all-miss should error: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_section_line_ranges_return_all_blocks() {
    let code = "l1\nl2\nl3\nl4\nl5\nl6\n";
    let path = std::env::temp_dir().join("srcwalk_multi_ranges.txt");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "5-6,2-3", None, &cache).unwrap();
    assert!(
        out.contains("2 sections, section"),
        "header should show section count: {out}"
    );
    let pos_l2 = out.find("l2").unwrap();
    let pos_l5 = out.find("l5").unwrap();
    assert!(
        pos_l2 < pos_l5,
        "blocks should be sorted by line order: {out}"
    );
    assert!(out.contains("---"), "blocks should be separated: {out}");
    assert!(
        !out.contains("l4"),
        "unrequested line should be omitted: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_section_mixes_symbol_and_line_range() {
    let code = "fn first() {\n    1\n}\nlet outside = 9;\nfn second() {\n    2\n}\n";
    let path = std::env::temp_dir().join("srcwalk_multi_mixed.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "second,4-4", None, &cache).unwrap();
    assert!(
        out.contains("2 sections, section"),
        "mixed list should use sections wording: {out}"
    );
    assert!(out.contains("outside = 9"), "should contain range: {out}");
    assert!(
        out.contains("second()"),
        "should contain symbol body: {out}"
    );
    assert!(
        !out.contains("first()"),
        "unrequested symbol should be omitted: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn over_budget_multi_section_returns_compact_bodies_and_missing_notes() {
    let mut code = String::from("fn first() {\n");
    for i in 0..40 {
        code.push_str(&format!(
            "    let a_{i} = \"padding padding padding padding\";\n"
        ));
    }
    code.push_str("}\nfn second() {\n");
    for i in 0..40 {
        code.push_str(&format!(
            "    let b_{i} = \"padding padding padding padding\";\n"
        ));
    }
    code.push_str("}\n");
    let path = std::env::temp_dir().join("srcwalk_multi_section_budget.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_file_with_budget(
        &path,
        Some("first,missing_fn,second"),
        false,
        Some(100),
        &cache,
    )
    .unwrap();
    assert!(
        out.contains("compact (over limit)"),
        "expected compact over-budget mode: {out}"
    );
    assert!(
        out.contains("## section: first") && out.contains("## section: second"),
        "compact output should keep requested section labels: {out}"
    );
    assert!(
        out.contains("let a_0") && out.contains("let b_0"),
        "compact output should include useful code from each section: {out}"
    );
    assert!(
        out.contains("lines omitted") && out.contains("compacted ~"),
        "compact output should include concise budget/omission metrics: {out}"
    );
    assert!(
        out.contains("Missing symbols") && out.contains("missing_fn"),
        "compact output should preserve missing symbol notes: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn compact_merged_symbol_and_range_keeps_range_anchor() {
    let mut code = String::from("fn big() {\n");
    for i in 0..80 {
        code.push_str(&format!("    let value_{i} = {i};\n"));
    }
    code.push_str("}\n");
    let path = std::env::temp_dir().join("srcwalk_compact_range_anchor.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_file_with_budget(&path, Some("big,40-42"), false, Some(100), &cache).unwrap();
    assert!(
        out.contains("fn big()") && out.contains("► 40") && out.contains("value_38"),
        "compact merged output should keep signature and requested range anchor: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_section_partial_miss_returns_found() {
    let code = "fn real_fn() {}\nlet kept = true;\n";
    let path = std::env::temp_dir().join("srcwalk_multi_section_miss.rs");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "real_fn,nope_fn,2-2", None, &cache).unwrap();
    assert!(
        out.contains("real_fn()"),
        "should contain found symbol: {out}"
    );
    assert!(
        out.contains("kept = true"),
        "should contain found range: {out}"
    );
    assert!(
        out.contains("Missing sections"),
        "should note missing: {out}"
    );
    assert!(
        out.contains("nope_fn"),
        "should name missing section: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_section_labels_and_merges_overlapping_ranges() {
    let code = "l1\nl2\nl3\nl4\nl5\n";
    let path = std::env::temp_dir().join("srcwalk_multi_section_overlap.txt");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let out = read_section(&path, "2-4,3-5", None, &cache).unwrap();
    assert!(
        out.contains("1 section, section"),
        "overlap should merge into one rendered block: {out}"
    );
    assert!(
        out.contains("## section: 2-4, 3-5 [2-5]"),
        "merged block should keep requested labels and final range: {out}"
    );
    assert_eq!(
        out.matches("l3").count(),
        1,
        "overlap should not duplicate lines: {out}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn multi_section_all_invalid_ranges_error() {
    let code = "one\ntwo\n";
    let path = std::env::temp_dir().join("srcwalk_multi_section_oob.txt");
    std::fs::write(&path, code).unwrap();

    let cache = OutlineCache::new();
    let err = read_section(&path, "10-12,20-21", None, &cache).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("sections not found"),
        "all-invalid should error: {msg}"
    );
    assert!(
        msg.contains("range out of bounds"),
        "should explain bounds: {msg}"
    );

    let _ = std::fs::remove_file(&path);
}
