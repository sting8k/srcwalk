use super::*;

#[test]
fn grammar_cache_keys_unique() {
    // Verify that (node_kind_count, field_count) is unique across all shipped grammars.
    // A collision would cause one language to serve another's cached query.
    let grammars: Vec<(&str, tree_sitter::Language)> = vec![
        ("rust", tree_sitter_rust::LANGUAGE.into()),
        (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        ("javascript", tree_sitter_javascript::LANGUAGE.into()),
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("go", tree_sitter_go::LANGUAGE.into()),
        ("java", tree_sitter_java::LANGUAGE.into()),
        ("c", tree_sitter_c::LANGUAGE.into()),
        ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        ("ruby", tree_sitter_ruby::LANGUAGE.into()),
        ("php", tree_sitter_php::LANGUAGE_PHP.into()),
        ("scala", tree_sitter_scala::LANGUAGE.into()),
        ("csharp", tree_sitter_c_sharp::LANGUAGE.into()),
        ("swift", tree_sitter_swift::LANGUAGE.into()),
        ("kotlin", tree_sitter_kotlin_ng::LANGUAGE.into()),
        ("elixir", tree_sitter_elixir::LANGUAGE.into()),
    ];
    let mut seen = std::collections::HashMap::new();
    for (name, lang) in &grammars {
        let key = lang_cache_key(lang);
        if let Some(prev) = seen.insert(key, name) {
            panic!("cache key collision: {prev} and {name} both produce {key:?}");
        }
    }
}

#[test]
fn kotlin_callee_query_compiles() {
    let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let query_str = callee_query_str(crate::types::Lang::Kotlin).unwrap();
    tree_sitter::Query::new(&lang, query_str).expect("kotlin callee query should compile");
}

#[test]
fn extract_kotlin_callee_names() {
    let kotlin = r#"fun example() {
    println("hello")
    val x = listOf(1, 2, 3)
    x.forEach { it.toString() }
}
"#;
    let names = extract_callee_names(kotlin, crate::types::Lang::Kotlin, None);

    assert!(
        names.contains(&"println".to_string()),
        "expected println, got: {names:?}"
    );
    assert!(
        names.contains(&"listOf".to_string()),
        "expected listOf, got: {names:?}"
    );
    assert!(
        names.contains(&"forEach".to_string()),
        "expected forEach, got: {names:?}"
    );
    assert!(
        names.contains(&"toString".to_string()),
        "expected toString, got: {names:?}"
    );
}

#[test]
fn extract_php_callee_names() {
    let php = r#"<?php
function run($svc): void {
    local_helper();
    Foo\Bar::staticCall();
    $svc->methodCall();
    $svc?->nullableCall();
}
"#;

    let names = extract_callee_names(php, Lang::Php, None);

    assert!(names.contains(&"local_helper".to_string()));
    assert!(names.contains(&"staticCall".to_string()));
    assert!(names.contains(&"methodCall".to_string()));
    assert!(names.contains(&"nullableCall".to_string()));
}

#[test]
fn elixir_callee_query_compiles() {
    let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
    let query_str = callee_query_str(crate::types::Lang::Elixir).unwrap();
    tree_sitter::Query::new(&lang, query_str).expect("elixir callee query should compile");
}

#[test]
fn extract_elixir_callee_names() {
    let elixir = r#"defmodule Example do
  def run(conn) do
    result = query(conn, "SELECT 1")
    Enum.map(result, &to_string/1)
    IO.puts("done")
    local_func()
  end
end
"#;
    let names = extract_callee_names(elixir, Lang::Elixir, None);

    assert!(
        names.contains(&"query".to_string()),
        "expected query, got: {names:?}"
    );
    assert!(
        names.contains(&"map".to_string()),
        "expected map (from Enum.map), got: {names:?}"
    );
    assert!(
        names.contains(&"puts".to_string()),
        "expected puts (from IO.puts), got: {names:?}"
    );
    assert!(
        names.contains(&"local_func".to_string()),
        "expected local_func, got: {names:?}"
    );

    // Definition keywords must NOT appear as callees
    assert!(
        !names.contains(&"def".to_string()),
        "definition keyword 'def' should be filtered, got: {names:?}"
    );
    assert!(
        !names.contains(&"defmodule".to_string()),
        "definition keyword 'defmodule' should be filtered, got: {names:?}"
    );
}

#[test]
fn extract_elixir_callee_names_pipes() {
    let elixir = r#"defmodule Pipes do
  def run(conn) do
    conn
    |> prepare("sql")
    |> execute()
    |> Enum.map(&transform/1)
  end
end
"#;
    let names = extract_callee_names(elixir, Lang::Elixir, None);

    // Pipe targets are regular call nodes — the callee query should find them
    assert!(
        names.contains(&"prepare".to_string()),
        "expected prepare from pipe, got: {names:?}"
    );
    assert!(
        names.contains(&"execute".to_string()),
        "expected execute from pipe, got: {names:?}"
    );
    assert!(
        names.contains(&"map".to_string()),
        "expected map from Enum.map pipe, got: {names:?}"
    );
}

#[test]
fn callee_queries_compile_for_all_supported_languages() {
    let langs = [
        Lang::Rust,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::JavaScript,
        Lang::Python,
        Lang::Go,
        Lang::Java,
        Lang::Scala,
        Lang::C,
        Lang::Cpp,
        Lang::Ruby,
        Lang::Php,
        Lang::Swift,
        Lang::Kotlin,
        Lang::CSharp,
        Lang::Elixir,
    ];

    for lang in langs {
        let Some(query) = callee_query_str(lang) else {
            continue;
        };
        let language = crate::lang::outline::outline_language(lang)
            .unwrap_or_else(|| panic!("missing parser for {lang:?}"));
        tree_sitter::Query::new(&language, query)
            .unwrap_or_else(|err| panic!("callee query failed for {lang:?}: {err:?}"));
    }
}

#[test]
fn extract_call_sites_populates_callee_name() {
    let rust = r#"
fn run(client: &Client) {
    let value = client.fetch(1);
    finish(value);
}
"#;
    let sites = extract_call_sites(rust, Lang::Rust, None);

    let fetch = sites
        .iter()
        .find(|site| site.callee == "fetch")
        .expect("expected fetch callsite");
    assert_eq!(fetch.args, vec!["1"]);
    let finish = sites
        .iter()
        .find(|site| site.callee == "finish")
        .expect("expected finish callsite");
    assert_eq!(finish.args, vec!["value"]);
}

#[test]
fn extract_simple_call_site_across_languages() {
    let cases = [
        (Lang::Rust, "fn run() { helper(1); }"),
        (Lang::TypeScript, "function run() { helper(1); }"),
        (Lang::Tsx, "function Run() { helper(1); return <div />; }"),
        (Lang::JavaScript, "function run() { helper(1); }"),
        (Lang::Python, "def run():\n    helper(1)\n"),
        (Lang::Go, "package p\nfunc run() { helper(1) }\n"),
        (Lang::Java, "class C { void run() { helper(1); } }"),
        (Lang::Scala, "object C { def run() = { helper(1) } }"),
        (Lang::C, "void run() { helper(1); }"),
        (Lang::Cpp, "void run() { helper(1); }"),
        (Lang::Ruby, "def run\n  helper(1)\nend\n"),
        (Lang::Php, "<?php function run() { helper(1); }"),
        (Lang::Swift, "func run() { helper(1) }"),
        (Lang::Kotlin, "fun run() { helper(1) }"),
        (Lang::CSharp, "class C { void Run() { helper(1); } }"),
        (
            Lang::Elixir,
            "defmodule M do\n  def run do\n    helper(1)\n  end\nend\n",
        ),
    ];

    for (lang, code) in cases {
        let sites = extract_call_sites(code, lang, None);
        assert!(
            sites.iter().any(|site| site.callee == "helper"),
            "expected helper call for {lang:?}, got: {sites:?}"
        );
    }
}

#[test]
fn extract_javascript_call_sites() {
    let javascript = r#"
async function downloadBinary(url, dest) {
    const res = await fetch(url);
    const fileStream = createWriteStream(dest, { flags: "w" });
    fileStream.on("finish", () => resolve());
    chmodSync(dest, 0o755);
}
"#;
    let sites = extract_call_sites(javascript, Lang::JavaScript, None);

    let fetch = sites
        .iter()
        .find(|site| site.callee == "fetch")
        .expect("expected fetch callsite");
    assert_eq!(fetch.args, vec!["url"]);
    let create = sites
        .iter()
        .find(|site| site.callee == "createWriteStream")
        .expect("expected createWriteStream callsite");
    assert_eq!(create.args, vec!["dest", "{ flags: \"w\" }"]);
    let chmod = sites
        .iter()
        .find(|site| site.callee == "chmodSync")
        .expect("expected chmodSync callsite");
    assert_eq!(chmod.args, vec!["dest", "0o755"]);
}

#[test]
fn extract_csharp_optional_invocation_call_sites() {
    let csharp = r#"
class C {
    void Run() {
        _options.OnRegistered?.Invoke(ev);
        pending.RetryTimer?.Dispose();
    }
}
"#;
    let sites = extract_call_sites(csharp, Lang::CSharp, None);

    let invoke = sites
        .iter()
        .find(|site| site.callee == "Invoke")
        .expect("expected optional Invoke callsite");
    assert_eq!(
        invoke.call_prefix.as_deref(),
        Some("_options.OnRegistered?.Invoke")
    );
    assert_eq!(invoke.args, vec!["ev"]);
    let dispose = sites
        .iter()
        .find(|site| site.callee == "Dispose")
        .expect("expected optional Dispose callsite");
    assert_eq!(
        dispose.call_prefix.as_deref(),
        Some("pending.RetryTimer?.Dispose")
    );
    assert!(dispose.args.is_empty());
}

#[test]
fn filter_call_sites_matches_exact_callee() {
    let sites = vec![
        CallSite {
            line: 1,
            callee: "fetch".to_string(),
            call_text: "client.fetch(1)".to_string(),
            call_prefix: Some("client.fetch".to_string()),
            args: vec!["1".to_string()],
            return_var: Some("value".to_string()),
            is_return: false,
            call_byte_range: None,
        },
        CallSite {
            line: 2,
            callee: "finish".to_string(),
            call_text: "finish(value)".to_string(),
            call_prefix: Some("finish".to_string()),
            args: vec!["value".to_string()],
            return_var: None,
            is_return: false,
            call_byte_range: None,
        },
    ];

    let filtered = filter_call_sites(sites, Some("callee:fetch")).expect("valid filter");

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].callee, "fetch");
}

#[test]
fn filter_call_sites_rejects_unknown_fields() {
    let err = filter_call_sites(Vec::new(), Some("receiver:client"))
        .expect_err("unsupported field should fail");

    assert!(err.to_string().contains("unsupported callee filter field"));
}
