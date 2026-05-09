use super::*;

fn sample_match() -> CallerMatch {
    CallerMatch {
        path: PathBuf::from("/repo/src/main.rs"),
        line: 42,
        calling_function: "main".to_string(),
        call_text: "client.start(1, monitor)".to_string(),
        caller_range: Some((40, 50)),
        receiver: Some("client".to_string()),
        prefix_kind: Some(PrefixKind::Variable),
        arg_count: Some(2),
        content: Arc::new(String::new()),
    }
}

#[test]
fn parse_callsite_filters_accepts_qualifiers() {
    let filters = parse_callsite_filters(Some("args:2 prefix:client receiver:client caller:main"))
        .expect("valid filters");
    assert_eq!(filters.len(), 4);
    assert_eq!(filters[0].field, "args");
    assert_eq!(filters[0].value, "2");
    assert_eq!(filters[1].field, "receiver");
    assert_eq!(filters[2].field, "receiver");
}

#[test]
fn parse_callsite_filters_rejects_unknown_fields() {
    let err = parse_callsite_filters(Some("unknown:x")).expect_err("invalid field");
    assert!(err.to_string().contains("unsupported filter field"));
}

#[test]
fn callsite_filters_match_semantic_fields() {
    let caller = sample_match();
    let scope = Path::new("/repo");
    let filters =
        parse_callsite_filters(Some("args:2 prefix:client caller:main path:src text:start"))
            .expect("valid filters");
    assert!(filters.iter().all(|f| f.matches(&caller, scope)));
}

#[test]
fn count_field_values_use_display_facts() {
    let caller = sample_match();
    let scope = Path::new("/repo");
    assert_eq!(callsite_field_value(&caller, scope, "args"), "2");
    assert_eq!(callsite_field_value(&caller, scope, "caller"), "main");
    assert_eq!(callsite_field_value(&caller, scope, "receiver"), "client");
    assert_eq!(callsite_field_value(&caller, scope, "path"), "src/main.rs");
    assert_eq!(callsite_field_value(&caller, scope, "file"), "main.rs");
}

#[test]
fn go_import_prefixes_classify_packages() {
    let content = r#"package main

import (
    sdktranslator "example.com/sdk/translator"
    "fmt"
    _ "example.com/sidefx"
    . "example.com/dot"
    json "encoding/json"
 )

func caller() {
    sdktranslator.TranslateRequest()
    fmt.Println()
    client.TranslateRequest()
}
"#;

    assert_eq!(
        classify_prefix_kind("sdktranslator", crate::types::Lang::Go, content),
        PrefixKind::Package
    );
    assert_eq!(
        classify_prefix_kind("fmt", crate::types::Lang::Go, content),
        PrefixKind::Package
    );
    assert_eq!(
        classify_prefix_kind("client", crate::types::Lang::Go, content),
        PrefixKind::Variable
    );
}

#[test]
fn rank_callers_prefers_named_contexts_over_top_level() {
    let scope = Path::new("/repo");
    let mut top_level = sample_match();
    top_level.path = PathBuf::from("/repo/a.ts");
    top_level.calling_function = TOP_LEVEL.to_string();
    let mut named = sample_match();
    named.path = PathBuf::from("/repo/deeper/path/b.ts");
    named.calling_function = "buildNotes".to_string();

    let mut callers = vec![top_level, named];
    rank_callers(&mut callers, scope, None);

    assert_eq!(callers[0].calling_function, "buildNotes");
}

#[test]
fn rank_callers_demotes_duplicate_context_calls() {
    let scope = Path::new("/repo");
    let mut duplicate_late = sample_match();
    duplicate_late.path = PathBuf::from("/repo/src/cache.php");
    duplicate_late.calling_function = "SmartyCustomCore.check".to_string();
    duplicate_late.line = 120;
    duplicate_late.receiver = Some("$this".to_string());

    let mut duplicate_first = sample_match();
    duplicate_first.path = PathBuf::from("/repo/src/cache.php");
    duplicate_first.calling_function = "SmartyCustomCore.check".to_string();
    duplicate_first.receiver = Some("$this".to_string());
    duplicate_first.line = 118;

    let mut different_context = sample_match();
    different_context.path = PathBuf::from("/repo/src/container.php");
    different_context.calling_function = "ContainerBuilder.build".to_string();
    different_context.line = 112;
    different_context.receiver = Some("$this->environment".to_string());

    let mut callers = vec![duplicate_late, duplicate_first, different_context];
    rank_callers(&mut callers, scope, None);

    assert_eq!(callers[0].calling_function, "ContainerBuilder.build");
    assert_eq!(callers[1].calling_function, "SmartyCustomCore.check");
    assert_eq!(callers[1].line, 118);
    assert_eq!(callers[2].line, 120);
}

#[test]
fn rank_callers_prefers_explicit_receivers_over_self_and_no_receiver() {
    let scope = Path::new("/repo");
    let mut no_receiver = sample_match();
    no_receiver.path = PathBuf::from("/repo/a.ts");
    no_receiver.receiver = None;

    let mut self_receiver = sample_match();
    self_receiver.path = PathBuf::from("/repo/b.ts");
    self_receiver.receiver = Some("$this".to_string());

    let mut explicit_receiver = sample_match();
    explicit_receiver.path = PathBuf::from("/repo/c.ts");
    explicit_receiver.receiver = Some("$kernel".to_string());

    let mut callers = vec![no_receiver, self_receiver, explicit_receiver];
    rank_callers(&mut callers, scope, None);

    assert_eq!(callers[0].receiver.as_deref(), Some("$kernel"));
    assert_eq!(callers[1].receiver.as_deref(), Some("$this"));
    assert_eq!(callers[2].receiver, None);
}
