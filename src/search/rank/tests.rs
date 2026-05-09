use super::sort;
use crate::types::Match;
use std::path::PathBuf;
use std::time::SystemTime;

fn make_match(path: &str, text: &str, is_definition: bool, def_name: Option<&str>) -> Match {
    Match {
        path: PathBuf::from(path),
        line: 1,
        text: text.to_string(),
        is_definition,
        exact: true,
        file_lines: 40,
        mtime: SystemTime::now(),
        def_range: None,
        def_name: def_name.map(ToString::to_string),
        def_weight: if is_definition { 80 } else { 0 },
        impl_target: None,
        base_target: None,
        in_comment: false,
    }
}

#[test]
fn prefers_exact_definition_name_over_usage() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match("/repo/src/auth.rs", "handleAuth(user)", false, None),
        make_match(
            "/repo/src/auth.rs",
            "pub fn handleAuth(req: Request) -> Response {",
            true,
            Some("handleAuth"),
        ),
    ];

    sort(&mut matches, "handleAuth", &scope, None);

    assert!(matches[0].is_definition);
    assert_eq!(matches[0].def_name.as_deref(), Some("handleAuth"));
}

#[test]
fn prefers_non_test_match_for_non_test_query() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match(
            "/repo/src/__tests__/auth.test.ts",
            "export function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
        make_match(
            "/repo/src/auth.ts",
            "export function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
    ];

    sort(&mut matches, "handleAuth", &scope, None);

    assert_eq!(matches[0].path, PathBuf::from("/repo/src/auth.ts"));
}

#[test]
fn prefers_same_subtree_as_context() {
    let scope = PathBuf::from("/repo/src");
    let context = PathBuf::from("/repo/src/auth/controller.rs");
    let mut matches = vec![
        make_match(
            "/repo/src/payments/service.rs",
            "pub fn handleAuth() {",
            true,
            Some("handleAuth"),
        ),
        make_match(
            "/repo/src/auth/service.rs",
            "pub fn handleAuth() {",
            true,
            Some("handleAuth"),
        ),
    ];

    sort(&mut matches, "handleAuth", &scope, Some(&context));

    assert_eq!(matches[0].path, PathBuf::from("/repo/src/auth/service.rs"));
}

#[test]
fn prefers_exported_api_over_local_definition() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match(
            "/repo/src/internal/auth.ts",
            "function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
        make_match(
            "/repo/src/public/auth.ts",
            "export function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
    ];

    sort(&mut matches, "handleAuth", &scope, None);

    assert_eq!(matches[0].path, PathBuf::from("/repo/src/public/auth.ts"));
}

#[test]
fn prefers_real_definition_over_fixture_match() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match(
            "/repo/src/fixtures/auth-fixture.ts",
            "export function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
        make_match(
            "/repo/src/auth.ts",
            "export function handleAuth() {",
            true,
            Some("handleAuth"),
        ),
    ];

    sort(&mut matches, "handleAuth", &scope, None);

    assert_eq!(matches[0].path, PathBuf::from("/repo/src/auth.ts"));
}

#[test]
fn prefers_thinking_logic_over_schema_for_concept_query() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match(
            "/repo/src/internal/interfaces/client_models.go",
            "ThinkingConfig *GenerationConfigThinkingConfig `json:\"thinkingConfig,omitempty\"`",
            false,
            None,
        ),
        make_match(
            "/repo/src/internal/util/thinking.go",
            "func NormalizeThinkingBudget(model string, requested int) int {",
            true,
            Some("NormalizeThinkingBudget"),
        ),
    ];

    sort(&mut matches, "thinking", &scope, None);

    assert!(
        matches[0].path.to_string_lossy().contains("thinking.go"),
        "expected thinking.go first, got {:?}",
        matches[0].path,
    );
}

#[test]
fn prefers_model_mapping_logic_over_docs_for_alias_query() {
    let scope = PathBuf::from("/repo/src");
    let mut matches = vec![
        make_match(
            "/repo/src/docs/FORCE_HANDLER_GUIDE.md",
            "Alias routing example",
            false,
            None,
        ),
        make_match(
            "/repo/src/internal/api/modules/amp/model_mapping.go",
            "func (m *DefaultModelMapper) MapModel(requestedModel string) string {",
            true,
            Some("MapModel"),
        ),
    ];

    sort(&mut matches, "alias", &scope, None);

    assert!(
        matches[0].path.to_string_lossy().contains("model_mapping"),
        "expected model_mapping.go first, got {:?}",
        matches[0].path,
    );
}

// --- Unit tests for individual penalty/boost functions ---

#[test]
fn non_code_penalty_docs_positive() {
    // Docs get penalized (positive return value, subtracted by caller)
    let path = PathBuf::from("/repo/docs/guide.md");
    assert!(super::non_code_penalty(&path) > 0);
}

#[test]
fn non_code_penalty_no_double_penalty_for_dist() {
    // dist/ should NOT be penalized here — VENDOR_DIRS handles it
    let path = PathBuf::from("/repo/dist/bundle.js");
    assert_eq!(super::non_code_penalty(&path), 0);
}

#[test]
fn non_code_penalty_no_double_penalty_for_build() {
    let path = PathBuf::from("/repo/build/output.js");
    assert_eq!(super::non_code_penalty(&path), 0);
}

#[test]
fn non_code_penalty_generated_without_dist() {
    let path = PathBuf::from("/repo/src/generated/types.ts");
    assert!(super::non_code_penalty(&path) > 0);
}

#[test]
fn non_code_penalty_normal_code_zero() {
    let path = PathBuf::from("/repo/src/auth.rs");
    assert_eq!(super::non_code_penalty(&path), 0);
}

#[test]
fn fixture_penalty_capped_at_200() {
    // A path hitting multiple needles should be capped
    let m = make_match(
        "/repo/src/fixtures/mock_stub_fake.ts",
        "example fixture mock stub fake",
        false,
        None,
    );
    let penalty = super::fixture_penalty(&m);
    assert!(
        penalty <= 200,
        "fixture_penalty was {penalty}, expected <= 200"
    );
    assert!(penalty > 0);
}

#[test]
fn fixture_penalty_zero_for_normal_code() {
    let m = make_match(
        "/repo/src/auth.ts",
        "export function handleAuth() {",
        true,
        Some("handleAuth"),
    );
    assert_eq!(super::fixture_penalty(&m), 0);
}

#[test]
fn incidental_text_penalty_comment_line() {
    // Lines starting with // should be penalized
    let m = make_match(
        "/repo/src/lib.rs",
        "// handleAuth is deprecated",
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "handleAuth"), 150);
}

#[test]
fn incidental_text_penalty_no_hash_false_positive() {
    // # in C/Rust files should NOT trigger comment penalty
    let m = make_match("/repo/src/main.c", "#include <stdio.h>", false, None);
    assert_eq!(super::incidental_text_penalty(&m, "stdio"), 0);
}

#[test]
fn incidental_text_penalty_hash_comment_in_python() {
    // # in .py files IS a comment — should be penalized
    let m = make_match(
        "/repo/src/main.py",
        "# handle_auth is deprecated",
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "handle_auth"), 150);
}

#[test]
fn incidental_text_penalty_no_star_false_positive() {
    // * should NOT trigger comment penalty
    let m = make_match("/repo/src/main.c", "*ptr = value;", false, None);
    assert_eq!(super::incidental_text_penalty(&m, "ptr"), 0);
}

#[test]
fn incidental_text_penalty_no_string_literal_heuristic() {
    // String literals should NOT be penalized (fragile heuristic removed)
    let m = make_match(
        "/repo/src/lib.rs",
        r#"let msg = "handleAuth error";"#,
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "handleAuth"), 0);
}

#[test]
fn incidental_text_penalty_trailing_comment() {
    // Query only in trailing comment should be penalized
    let m = make_match(
        "/repo/src/lib.rs",
        "let x = 1; // handleAuth workaround",
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "handleAuth"), 100);
}

#[test]
fn incidental_text_penalty_url_not_comment() {
    // :// is a URL scheme — should NOT be treated as trailing comment
    let m = make_match(
        "/repo/src/lib.rs",
        r#"let url = "https://handleAuth.example.com";"#,
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "handleAuth"), 0);
}

#[test]
fn incidental_text_penalty_skip_definitions() {
    // Definitions should never be penalized
    let m = make_match(
        "/repo/src/lib.rs",
        "// handleAuth docs",
        true,
        Some("handleAuth"),
    );
    assert_eq!(super::incidental_text_penalty(&m, "handleAuth"), 0);
}

#[test]
fn incidental_text_penalty_doc_comment_exempt() {
    // /// doc comments should NOT be penalized — they provide useful symbol context
    let m = make_match(
        "/repo/src/lib.rs",
        "/// Handles auth validation for incoming requests",
        false,
        None,
    );
    assert_eq!(super::incidental_text_penalty(&m, "auth"), 0);
}

#[test]
fn sign_convention_all_penalties_positive() {
    // All penalty functions should return >= 0 (positive values, subtracted by score())
    let doc_path = PathBuf::from("/repo/docs/guide.md");
    assert!(super::non_code_penalty(&doc_path) >= 0);

    let fixture = make_match("/repo/fixtures/mock.ts", "mock data", false, None);
    assert!(super::fixture_penalty(&fixture) >= 0);

    let comment = make_match("/repo/src/lib.rs", "// TODO fix auth", false, None);
    assert!(super::incidental_text_penalty(&comment, "auth") >= 0);
}

#[test]
fn vendor_path_detects_dist_and_build() {
    // dist/ and build/ are in VENDOR_DIRS — this is where the penalty comes from
    assert!(super::is_vendor_path(&PathBuf::from(
        "/repo/dist/bundle.js"
    )));
    assert!(super::is_vendor_path(&PathBuf::from(
        "/repo/build/output.js"
    )));
    assert!(!super::is_vendor_path(&PathBuf::from("/repo/src/auth.rs")));
}
