use super::*;
use crate::search::callees::extract_call_sites;

fn build(
    path: &Path,
    content: &str,
    lang: Lang,
    caller_range: (u32, u32),
) -> DirectCallEvidenceIndex {
    let sites = extract_call_sites(content, lang, Some(caller_range));
    build_direct_call_evidence_index(path, content, lang, Some(caller_range), &sites)
}

#[test]
fn maps_same_file_rust_and_javascript_positional_arguments() {
    let rust = r#"
fn helper(user: User, path: &str) {}
fn caller(user: User, path: &str) {
    helper(user, path);
}
"#;
    let rust_index = build(Path::new("src/lib.rs"), rust, Lang::Rust, (3, 5));
    assert_eq!(rust_index.edges().len(), 1);
    let rust_edge = &rust_index.edges()[0];
    assert_eq!(
        rust_edge
            .arg_param_mappings()
            .iter()
            .map(|mapping| {
                (
                    mapping.arg_index(),
                    mapping.arg_display(),
                    mapping.param_index(),
                    mapping.param_name(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(0, "user", 0, "user"), (1, "path", 1, "path")]
    );
    assert_eq!(
        rust_edge.confidence(),
        DirectCallResolutionConfidence::SameFileStructural
    );

    let javascript = r#"
function helper(user, path) {}
function caller(user, path) {
  helper(user, path);
}
"#;
    let js_index = build(
        Path::new("src/lib.js"),
        javascript,
        Lang::JavaScript,
        (3, 5),
    );
    assert_eq!(js_index.edges().len(), 1);
    assert_eq!(js_index.edges()[0].arg_param_mappings().len(), 2);
}

#[test]
fn retains_edge_but_labels_unreliable_mapping_inputs() {
    let arity = r#"
fn helper(user: User, path: &str) {}
fn caller(user: User) {
    helper(user);
}
"#;
    let arity_index = build(Path::new("src/lib.rs"), arity, Lang::Rust, (3, 5));
    assert_eq!(arity_index.edges().len(), 1);
    assert_eq!(
        arity_index.edges()[0].mapping_unknown(),
        Some(ArgParamMappingUnknownReason::ArityMismatch)
    );
    assert_eq!(arity_index.edges()[0].omitted_arg_param_mappings(), 0);

    let spread = r#"
function helper(value) {}
function caller(values) {
  helper(...values);
}
"#;
    let spread_index = build(Path::new("src/lib.js"), spread, Lang::JavaScript, (3, 5));
    assert_eq!(spread_index.edges().len(), 1);
    assert_eq!(
        spread_index.edges()[0].mapping_unknown(),
        Some(ArgParamMappingUnknownReason::NonPositionalArguments)
    );
    assert!(spread_index.edges()[0].arg_param_mappings().is_empty());
}

#[test]
fn ambiguous_and_self_recursive_targets_abstain_from_edges() {
    let ambiguous = r#"
fn helper(value: i32) {}
fn helper(value: i64) {}
fn caller(value: i32) {
    helper(value);
}
"#;
    let ambiguous_index = build(Path::new("src/lib.rs"), ambiguous, Lang::Rust, (4, 6));
    assert!(ambiguous_index.edges().is_empty());
    assert_eq!(ambiguous_index.unknowns().len(), 1);
    assert_eq!(
        ambiguous_index.unknowns()[0].reason(),
        DirectCallUnknownReason::AmbiguousTarget
    );
    assert_eq!(ambiguous_index.unknowns()[0].candidates().len(), 2);

    let recursive = r#"
fn caller(value: i32) {
    caller(value);
}
"#;
    let recursive_index = build(Path::new("src/lib.rs"), recursive, Lang::Rust, (2, 4));
    assert!(recursive_index.edges().is_empty());
    assert_eq!(
        recursive_index.unknowns()[0].reason(),
        DirectCallUnknownReason::SelfRecursiveCall
    );
}

#[test]
fn maps_explicit_related_file_target() {
    let root = std::env::temp_dir().join(format!(
        "srcwalk_direct_call_related_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let caller_path = root.join("lib.rs");
    let related_path = root.join("service.rs");
    let caller = r#"
mod service;
use self::service::apply_update;
fn caller(record_id: i32) {
    apply_update(record_id);
}
"#;
    std::fs::write(&caller_path, caller).unwrap();
    std::fs::write(&related_path, "pub fn apply_update(record_id: i32) {}\n").unwrap();

    let index = build(&caller_path, caller, Lang::Rust, (4, 6));
    assert_eq!(index.edges().len(), 1);
    let edge = &index.edges()[0];
    assert_eq!(
        edge.confidence(),
        DirectCallResolutionConfidence::ExplicitRelatedFileStructural
    );
    assert!(
        edge.target_anchor().display().contains("service.rs"),
        "expected related target anchor, got {}",
        edge.target_anchor().display()
    );
    assert_eq!(edge.arg_param_mappings()[0].param_name(), "record_id");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn duplicate_same_and_related_file_targets_abstain() {
    let root = std::env::temp_dir().join(format!(
        "srcwalk_direct_call_duplicate_related_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let caller_path = root.join("lib.rs");
    let related_path = root.join("service.rs");
    let caller = r#"
mod service;
use self::service::apply_update;
fn apply_update(record_id: i32) {}
fn caller(record_id: i32) {
    apply_update(record_id);
}
"#;
    std::fs::write(&caller_path, caller).unwrap();
    std::fs::write(&related_path, "pub fn apply_update(record_id: i32) {}\n").unwrap();

    let index = build(&caller_path, caller, Lang::Rust, (5, 7));
    assert!(index.edges().is_empty());
    assert_eq!(index.unknowns().len(), 1);
    assert_eq!(
        index.unknowns()[0].reason(),
        DirectCallUnknownReason::AmbiguousTarget
    );
    assert_eq!(index.unknowns()[0].candidates().len(), 2);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn qualified_calls_and_unresolved_calls_abstain_without_hiding_actionable_unknowns() {
    let qualified = r#"
fn save(value: i32) {}
fn caller(client: Client, value: i32) {
    client.save(value);
}
"#;
    let qualified_index = build(Path::new("src/lib.rs"), qualified, Lang::Rust, (3, 5));
    assert!(qualified_index.edges().is_empty());
    assert!(qualified_index.unknowns().is_empty());

    let mut crowded = String::from(
        "fn helper(value: i32) {}\nfn helper(value: i64) {}\nfn caller(value: i32) {\n",
    );
    for index in 0..(MAX_DIRECT_CALL_UNKNOWNS + 4) {
        crowded.push_str(&format!("    external{index}(value);\n"));
    }
    crowded.push_str("    helper(value);\n}\n");
    let end_line = crowded.lines().count() as u32;
    let crowded_index = build(Path::new("src/lib.rs"), &crowded, Lang::Rust, (3, end_line));
    assert!(crowded_index.edges().is_empty());
    assert_eq!(crowded_index.unknowns().len(), 1);
    assert_eq!(
        crowded_index.unknowns()[0].reason(),
        DirectCallUnknownReason::AmbiguousTarget
    );
    assert_eq!(crowded_index.omitted_unknowns(), 0);
}

#[test]
fn mapping_parser_supports_go_and_name_last_signatures() {
    assert_eq!(
        parse_function_parameters(
            Path::new("src/lib.go"),
            "func (s *Service) Save(ctx context.Context, id string) error"
        ),
        Some(vec!["ctx".to_string(), "id".to_string()])
    );
    assert_eq!(
        parse_function_parameters(
            Path::new("src/lib.c"),
            "void save(const char *path, unsigned int mode)"
        ),
        Some(vec!["path".to_string(), "mode".to_string()])
    );
    assert!(parse_function_parameters(
        Path::new("src/lib.rs"),
        "fn helper(values: impl Iterator<Item = String>)"
    )
    .is_some());
}

#[test]
fn arg_param_mapping_is_capped_with_omitted_count() {
    let target = DirectCallTarget {
        name: "helper".to_string(),
        path: PathBuf::from("src/lib.rs"),
        start_line: 1,
        end_line: 2,
        signature: Some(
            "fn helper(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32, g: i32, h: i32, i: i32)"
                .to_string(),
        ),
        confidence: DirectCallResolutionConfidence::SameFileStructural,
    };
    let args = ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let (mappings, omitted, unknown) = arg_param_mappings(Path::new("src/lib.rs"), &args, &target);
    assert_eq!(mappings.len(), MAX_ARG_PARAM_MAPPINGS_PER_EDGE);
    assert_eq!(omitted, 1);
    assert_eq!(unknown, None);
    assert_eq!(
        ArgParamMapping::confidence(),
        "syntactic positional arg/param"
    );
}
