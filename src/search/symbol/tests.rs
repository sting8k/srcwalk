use super::*;
use std::time::SystemTime;

#[test]
fn rust_definitions_detected() {
    let code = r#"pub fn hello(name: &str) -> String {
    format!("Hello, {}", name)
}

pub struct Foo {
    bar: i32,
}

pub(crate) fn dispatch_tool(tool: &str) -> Result<String, String> {
    match tool {
        "read" => Ok("read".to_string()),
        _ => Err("unknown".to_string()),
    }
}
"#;
    let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::Rust).unwrap();

    let defs = find_defs_treesitter(
        std::path::Path::new("test.rs"),
        "hello",
        &ts_lang,
        Some(crate::types::Lang::Rust),
        code,
        15,
        SystemTime::now(),
        None,
    );
    assert!(!defs.is_empty(), "should find 'hello' definition");
    assert!(defs[0].is_definition);
    assert!(defs[0].def_range.is_some());

    let defs = find_defs_treesitter(
        std::path::Path::new("test.rs"),
        "Foo",
        &ts_lang,
        Some(crate::types::Lang::Rust),
        code,
        15,
        SystemTime::now(),
        None,
    );
    assert!(!defs.is_empty(), "should find 'Foo' definition");

    let defs = find_defs_treesitter(
        std::path::Path::new("test.rs"),
        "dispatch_tool",
        &ts_lang,
        Some(crate::types::Lang::Rust),
        code,
        15,
        SystemTime::now(),
        None,
    );
    assert!(!defs.is_empty(), "should find 'dispatch_tool' definition");
}

#[test]
fn c_declarator_definitions_detected() {
    let code = r"
static int normal_func(int x) { return x; }
char *make_name(void) { return 0; }
static int rust_demangle_callback(data, len)
  const char *data;
  int len;
{
  return 0;
}
";
    let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::C).unwrap();

    for name in ["normal_func", "make_name", "rust_demangle_callback"] {
        let defs = find_defs_treesitter(
            std::path::Path::new("test.c"),
            name,
            &ts_lang,
            Some(crate::types::Lang::C),
            code,
            code.lines().count() as u32,
            SystemTime::now(),
            None,
        );
        assert!(!defs.is_empty(), "should find C definition {name}");
        assert_eq!(defs[0].def_name.as_deref(), Some(name));
    }
}

#[test]
fn c_named_struct_body_definitions_detected() {
    let code = r"
typedef struct ngx_http_core_loc_conf_s  ngx_http_core_loc_conf_t;

struct ngx_http_core_loc_conf_s {
    int value;
};
";
    let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::C).unwrap();

    let defs = find_defs_treesitter(
        std::path::Path::new("test.h"),
        "ngx_http_core_loc_conf_s",
        &ts_lang,
        Some(crate::types::Lang::C),
        code,
        code.lines().count() as u32,
        SystemTime::now(),
        None,
    );

    assert_eq!(
        defs.len(),
        1,
        "should only report the struct body: {defs:?}"
    );
    assert_eq!(defs[0].line, 4);
    assert_eq!(
        defs[0].def_name.as_deref(),
        Some("ngx_http_core_loc_conf_s")
    );
    assert_eq!(defs[0].def_range, Some((4, 6)));
}

/// Helper: search for an Elixir definition by name in a code snippet.
fn elixir_find(code: &str, name: &str) -> Vec<Match> {
    let ts_lang = crate::lang::outline::outline_language(crate::types::Lang::Elixir).unwrap();
    let lines = code.lines().count() as u32;
    find_defs_treesitter(
        std::path::Path::new("test.ex"),
        name,
        &ts_lang,
        Some(crate::types::Lang::Elixir),
        code,
        lines,
        SystemTime::now(),
        None,
    )
}

#[test]
fn elixir_definitions_detected() {
    let code = r#"defmodule MyApp.Greeter do
  @type t :: %{name: String.t()}

  def hello(name) do
    "Hello, #{name}!"
  end

  defp private_helper(x), do: x + 1

  defmacro my_macro(expr) do
    quote do: unquote(expr)
  end
end
"#;
    // Dotted module name
    let defs = elixir_find(code, "MyApp.Greeter");
    assert!(!defs.is_empty(), "should find 'MyApp.Greeter' module def");
    assert!(defs[0].is_definition);

    // Public function (block form with parens)
    assert!(
        !elixir_find(code, "hello").is_empty(),
        "should find 'hello'"
    );

    // Private function (keyword form: `, do:`)
    assert!(
        !elixir_find(code, "private_helper").is_empty(),
        "should find 'private_helper'"
    );

    // Macro
    assert!(
        !elixir_find(code, "my_macro").is_empty(),
        "should find 'my_macro'"
    );
}

#[test]
fn elixir_guard_clause_definitions() {
    let code = r"defmodule Guards do
  def safe_div(a, b) when b != 0 do
    a / b
  end

  defp checked(x) when is_integer(x), do: x

  defguard is_positive(x) when x > 0
end
";
    // Guard clause with `when` — block form
    assert!(
        !elixir_find(code, "safe_div").is_empty(),
        "should find 'safe_div' with guard clause"
    );

    // Guard clause with `when` — keyword form
    assert!(
        !elixir_find(code, "checked").is_empty(),
        "should find 'checked' with guard clause"
    );

    // defguard
    assert!(
        !elixir_find(code, "is_positive").is_empty(),
        "should find 'is_positive' defguard"
    );
}

#[test]
fn elixir_multi_clause_and_no_arg() {
    let code = r#"defmodule Dispatch do
  def handle(:ok), do: :success
  def handle(:error), do: :failure

  def version, do: "1.0"
end
"#;
    // Multi-clause: both clauses should be found
    let defs = elixir_find(code, "handle");
    assert!(
        defs.len() >= 2,
        "should find both 'handle' clauses, got {}: {defs:?}",
        defs.len()
    );

    // No-arg function (bare identifier, no parens)
    assert!(
        !elixir_find(code, "version").is_empty(),
        "should find no-arg 'version'"
    );
}

#[test]
fn elixir_protocol_impl_exception() {
    let code = r"defprotocol Printable do
  @callback format(t) :: String.t()
  def to_string(data)
end

defimpl Printable, for: User do
  def to_string(user), do: user.name
end

defmodule MyError do
  defexception [:message, :code]
end
";
    // Protocol + defimpl: both indexed under the protocol name "Printable"
    let defs = elixir_find(code, "Printable");
    assert!(
        defs.len() >= 2,
        "should find both defprotocol and defimpl for 'Printable', got {}",
        defs.len()
    );

    // defexception
    assert!(
        !elixir_find(code, "defexception").is_empty(),
        "should find 'defexception'"
    );

    // Module containing exception
    assert!(
        !elixir_find(code, "MyError").is_empty(),
        "should find 'MyError' module"
    );
}

#[test]
fn elixir_delegate_and_nested_modules() {
    let code = r"defmodule Outer do
  defdelegate count(list), to: Enum

  defmodule Inner do
    def nested_func, do: :ok
  end
end
";
    // defdelegate
    assert!(
        !elixir_find(code, "count").is_empty(),
        "should find 'count' defdelegate"
    );

    // Nested module
    assert!(
        !elixir_find(code, "Inner").is_empty(),
        "should find nested 'Inner' module"
    );
}

#[test]
fn suggest_finds_case_variant() {
    let dir = std::env::temp_dir().join(format!("srcwalk_p13_suggest_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("foo.rs");
    std::fs::write(
        &path,
        "pub fn orderExists() -> bool { true }\nfn other() {}\n",
    )
    .unwrap();

    let hits = suggest("OrderExists", &dir, None, 3);
    assert!(
        hits.iter().any(|(s, _, _)| s == "orderExists"),
        "expected case-variant suggestion, got: {hits:?}"
    );

    let no_match = suggest("CompletelyUnrelatedXyz", &dir, None, 3);
    assert!(
        no_match.is_empty(),
        "no fuzzy hit expected, got: {no_match:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn suggest_crosses_naming_convention() {
    let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_conv_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("foo.rs");
    std::fs::write(
        &path,
        "pub fn search_symbol() -> bool { true }\npub fn HotReloadProcessor() {}\n",
    )
    .unwrap();

    // camelCase query → snake_case symbol
    let hits = suggest("searchSymbol", &dir, None, 3);
    assert!(
        hits.iter().any(|(s, _, _)| s == "search_symbol"),
        "expected snake_case suggestion for camelCase query, got: {hits:?}"
    );

    // lowercase query → PascalCase symbol
    let hits2 = suggest("hotreloadprocessor", &dir, None, 3);
    assert!(
        hits2.iter().any(|(s, _, _)| s == "HotReloadProcessor"),
        "expected PascalCase suggestion for lowercase query, got: {hits2:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn suggest_handles_lev1_typo() {
    let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_typo_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("foo.rs");
    std::fs::write(&path, "pub fn run_inner() {}\n").unwrap();

    let hits = suggest("run_iner", &dir, None, 3);
    assert!(
        hits.iter().any(|(s, _, _)| s == "run_inner"),
        "expected typo-tolerant suggestion, got: {hits:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn suggest_skips_non_source_files() {
    let dir = std::env::temp_dir().join(format!("srcwalk_p13fix_skip_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // JSON i18n bundle — must NOT match
    std::fs::write(dir.join("es.json"), r#"{"sesion": "iniciar"}"#).unwrap();
    // SOURCES.txt build artifact — must NOT match
    std::fs::write(dir.join("SOURCES.txt"), "src/foo/sesion.py\n").unwrap();
    // Real source — should match
    std::fs::write(dir.join("real.py"), "def session(): pass\n").unwrap();

    let hits = suggest("Sesion", &dir, None, 5);
    assert!(
        hits.iter().all(|(_, p, _)| {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            n != "es.json" && n != "SOURCES.txt"
        }),
        "expected non-source files filtered out, got: {hits:?}"
    );
    assert!(
        hits.iter().any(|(s, _, _)| s == "session"),
        "expected real .py hit, got: {hits:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tree_sitter_definition_metadata_preserves_impl_and_base_targets() {
    let rust = r"trait Matcher {
    fn find(&self);
}
struct RegexMatcher;
impl Matcher for RegexMatcher {
    fn find(&self) {}
}
";
    let rust_lang = crate::lang::outline::outline_language(crate::types::Lang::Rust).unwrap();
    let defs = find_defs_treesitter(
        std::path::Path::new("lib.rs"),
        "Matcher",
        &rust_lang,
        Some(crate::types::Lang::Rust),
        rust,
        rust.lines().count() as u32,
        SystemTime::now(),
        None,
    );
    assert!(
        defs.iter().any(|m| m.def_name.as_deref() == Some("Matcher")
            && m.def_weight >= 90
            && m.impl_target.is_none()
            && m.base_target.is_none()),
        "expected primary trait definition metadata, got: {defs:?}"
    );
    assert!(
        defs.iter().any(
            |m| m.def_name.as_deref() == Some("impl Matcher for RegexMatcher")
                && m.def_weight == 80
                && m.impl_target.as_deref() == Some("Matcher")
                && m.base_target.is_none()
        ),
        "expected Rust impl relationship metadata, got: {defs:?}"
    );

    let csharp = "interface IMatcher { void Find(); }\nclass RegexMatcher : IMatcher { public void Find() {} }\n";
    let csharp_lang = crate::lang::outline::outline_language(crate::types::Lang::CSharp).unwrap();
    let defs = find_defs_treesitter(
        std::path::Path::new("A.cs"),
        "IMatcher",
        &csharp_lang,
        Some(crate::types::Lang::CSharp),
        csharp,
        csharp.lines().count() as u32,
        SystemTime::now(),
        None,
    );
    assert!(
        defs.iter()
            .any(|m| m.def_name.as_deref() == Some("RegexMatcher : IMatcher")
                && m.def_weight == 70
                && m.impl_target.is_none()
                && m.base_target.as_deref() == Some("IMatcher")),
        "expected C# base relationship metadata, got: {defs:?}"
    );
}

#[test]
fn artifact_anchor_definitions_preserve_exact_metadata() {
    let content = "module.exports.MyBundle=function(){};\n";
    let defs = find_artifact_anchor_defs(
        std::path::Path::new("dist/app.min.js"),
        "MyBundle",
        content,
        content.lines().count() as u32,
        SystemTime::now(),
    );
    assert_eq!(
        defs.len(),
        1,
        "expected one artifact anchor def, got: {defs:?}"
    );
    let def = &defs[0];
    assert_eq!(def.line, 1);
    assert!(def.is_definition);
    assert!(def.exact);
    assert_eq!(def.def_name.as_deref(), Some("export MyBundle"));
    assert_eq!(def.def_weight, 95);
    assert!(def.def_range.is_none());
}

#[test]
fn batch_search_matches_single_search_for_defs_usages_and_comments() {
    use std::collections::BTreeSet;

    fn keys(result: &SearchResult) -> BTreeSet<(String, u32, bool, bool, Option<String>)> {
        result
            .matches
            .iter()
            .map(|m| {
                (
                    m.path.file_name().unwrap().to_string_lossy().into_owned(),
                    m.line,
                    m.is_definition,
                    m.in_comment,
                    m.def_name.clone(),
                )
            })
            .collect()
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        r"fn alpha() {
    beta();
}

fn beta() {}

// beta appears in a comment-only usage
",
    )
    .unwrap();

    let queries = ["alpha", "beta"];
    let batch = search_batch(&queries, dir.path(), None, None, Some("*.rs")).unwrap();
    assert_eq!(batch.len(), queries.len());

    for (idx, query) in queries.iter().enumerate() {
        let single = search(query, dir.path(), None, None, Some("*.rs")).unwrap();
        assert_eq!(
            keys(&batch[idx]),
            keys(&single),
            "batch result diverged from single-symbol search for {query}\nbatch: {:?}\nsingle: {:?}",
            batch[idx],
            single
        );
        assert_eq!(
            batch[idx].definitions, single.definitions,
            "definitions diverged for {query}"
        );
        assert_eq!(
            batch[idx].usages, single.usages,
            "usages diverged for {query}"
        );
        assert_eq!(
            batch[idx].comments, single.comments,
            "comments diverged for {query}"
        );
    }

    assert!(
        batch[1]
            .matches
            .iter()
            .any(|m| m.in_comment && !m.is_definition),
        "expected beta comment usage to be tagged, got: {:?}",
        batch[1]
    );
}

#[test]
fn split_dot_symbol_query_accepts_single_dot_pairs() {
    assert_eq!(split_dot_symbol_query("Batch.Set"), Some(("Batch", "Set")));
    assert_eq!(
        split_dot_symbol_query("Config.load"),
        Some(("Config", "load"))
    );
    assert_eq!(
        split_dot_symbol_query("syncQueue.pop"),
        Some(("syncQueue", "pop"))
    );
}

#[test]
fn split_dot_symbol_query_rejects_multi_dot_and_empty_sides() {
    assert_eq!(split_dot_symbol_query("a.b.c"), None);
    assert_eq!(split_dot_symbol_query(".Set"), None);
    assert_eq!(split_dot_symbol_query("Batch."), None);
    assert_eq!(split_dot_symbol_query(""), None);
    assert_eq!(split_dot_symbol_query("Set"), None);
}

/// US-075: the batch definition prefilter must admit every file that the
/// single-symbol prefilter would admit. Overlapping needles rely on
/// `find_overlapping_iter` reporting every pattern id, including duplicates.
#[test]
fn aho_corasick_overlapping_iter_reports_duplicate_and_contained_patterns() {
    let ac = aho_corasick::AhoCorasick::new(["Set", "Set", "helper", "helper_extra"]).unwrap();
    let mut ids: Vec<usize> = ac
        .find_overlapping_iter(&b"func (b *Batch) Set(v int) {}\nfn helper_extra() {}\n"[..])
        .map(|m| m.pattern().as_usize())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids, vec![0, 1, 2, 3]);
}

/// US-075 definition-identity oracle: for every accepted batch term, the batch
/// must yield the same definition candidates as the single-symbol search.
#[test]
fn batch_definitions_match_single_search_for_qualified_and_overlapping_queries() {
    struct Case {
        name: &'static str,
        files: &'static [(&'static str, &'static str)],
        glob: &'static str,
        queries: &'static [&'static str],
        expected_defs: &'static [usize],
    }

    const GO: &[(&str, &str)] = &[(
        "sample.go",
        "package sample\n\ntype Batch struct{}\n\nfunc (b *Batch) Set(v int) {}\n\ntype Other struct{}\n\nfunc (o *Other) Set(v int) {}\n\nfunc helper() {}\n",
    )];
    const RUST: &[(&str, &str)] = &[(
        "sample.rs",
        "pub struct Batch;\n\nimpl Batch {\n    pub fn set(&self, v: i32) -> i32 {\n        v\n    }\n}\n\npub struct Other;\n\nimpl Other {\n    pub fn set(&self, v: i32) -> i32 {\n        v\n    }\n}\n\npub fn helper() {}\n",
    )];
    const PY: &[(&str, &str)] = &[(
        "sample.py",
        "class Batch:\n    def set(self, v):\n        return v\n\n\nclass Other:\n    def set(self, v):\n        return v\n\n\ndef helper():\n    return None\n",
    )];
    // The long name lives in a file that does not contain the short name, so a
    // masked needle cannot be rescued by the other query's admission.
    const OVERLAP: &[(&str, &str)] = &[
        ("only_extra.rs", "pub fn helper_extra() -> u8 {\n    1\n}\n"),
        ("short.rs", "pub fn helper() -> u8 {\n    2\n}\n"),
    ];

    let cases = [
        Case {
            name: "go_qualified",
            files: GO,
            glob: "*.go",
            queries: &["Batch.Set", "helper"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "go_qualified_reversed",
            files: GO,
            glob: "*.go",
            queries: &["helper", "Batch.Set"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "rust_qualified",
            files: RUST,
            glob: "*.rs",
            queries: &["Batch.set", "helper"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "python_qualified",
            files: PY,
            glob: "*.py",
            queries: &["Batch.set", "helper"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "prefix_overlap",
            files: OVERLAP,
            glob: "*.rs",
            queries: &["helper", "helper_extra"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "prefix_overlap_reversed",
            files: OVERLAP,
            glob: "*.rs",
            queries: &["helper_extra", "helper"],
            expected_defs: &[1, 1],
        },
        Case {
            name: "shared_needle",
            files: GO,
            glob: "*.go",
            queries: &["Batch.Set", "Other.Set", "Set"],
            expected_defs: &[1, 1, 2],
        },
        Case {
            name: "shared_needle_reversed",
            files: GO,
            glob: "*.go",
            queries: &["Set", "Other.Set", "Batch.Set"],
            expected_defs: &[2, 1, 1],
        },
        Case {
            name: "max_five_terms",
            files: GO,
            glob: "*.go",
            queries: &["Batch.Set", "Other.Set", "Set", "helper", "Nope.Set"],
            expected_defs: &[1, 1, 2, 1, 0],
        },
    ];

    for case in &cases {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in case.files {
            std::fs::write(dir.path().join(rel), content).unwrap();
        }

        let batch = search_batch(case.queries, dir.path(), None, None, Some(case.glob)).unwrap();
        assert_eq!(batch.len(), case.queries.len());

        for (idx, query) in case.queries.iter().enumerate() {
            let single = search(query, dir.path(), None, None, Some(case.glob)).unwrap();
            let batch_defs = definition_keys(&batch[idx], dir.path());
            let single_defs = definition_keys(&single, dir.path());
            assert_eq!(
                batch_defs, single_defs,
                "{}: batch definitions diverged from single search for '{query}'",
                case.name
            );
            assert_eq!(
                batch_defs.len(),
                case.expected_defs[idx],
                "{}: unexpected definition count for '{query}' (got {batch_defs:?})",
                case.name
            );
        }
    }
}

/// US-075 AC-5: a literal comment occurrence may add occurrences but must never
/// change which definitions the batch finds.
#[test]
fn batch_definition_anchors_are_independent_of_comment_occurrences() {
    const BASE: &str = "package sample\n\ntype Batch struct{}\n\nfunc (b *Batch) Set(v int) {}\n\nfunc helper() {}\n";
    let queries = ["Batch.Set", "helper"];

    let anchors = |source: &str| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sample.go"), source).unwrap();
        let batch = search_batch(&queries, dir.path(), None, None, Some("*.go")).unwrap();
        let single = search(queries[0], dir.path(), None, None, Some("*.go")).unwrap();
        (
            definition_keys(&batch[0], dir.path()),
            definition_keys(&single, dir.path()),
        )
    };

    let (without_comment, single_without) = anchors(BASE);
    let (with_comment, single_with) = anchors(&format!("{BASE}\n// Batch.Set\n"));

    assert_eq!(without_comment.len(), 1, "{without_comment:?}");
    assert_eq!(without_comment, with_comment);
    assert_eq!(single_without, single_with);
    assert_eq!(without_comment, single_without);
}

/// US-075 AC-6: same-name definitions in different files stay separate, and a
/// wrong qualifier never acquires a definition because a sibling query admitted
/// the file.
#[test]
fn batch_shared_needle_keeps_qualifier_ownership_and_file_identity() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(
        dir.path().join("a/sample.go"),
        "package a\n\ntype Batch struct{}\n\nfunc (b *Batch) Set(v int) {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b/sample.go"),
        "package b\n\ntype Other struct{}\n\nfunc (o *Other) Set(v int) {}\n",
    )
    .unwrap();

    let queries = ["Batch.Set", "Other.Set", "Nope.Set"];
    let batch = search_batch(&queries, dir.path(), None, None, Some("*.go")).unwrap();

    let batch_first = definition_keys(&batch[0], dir.path());
    let batch_second = definition_keys(&batch[1], dir.path());
    assert_eq!(batch_first.len(), 1, "{batch_first:?}");
    assert_eq!(batch_second.len(), 1, "{batch_second:?}");
    assert!(batch_first.iter().all(|k| k.0 == "a/sample.go"));
    assert!(batch_second.iter().all(|k| k.0 == "b/sample.go"));
    assert!(
        definition_keys(&batch[2], dir.path()).is_empty(),
        "wrong qualifier must not acquire a definition"
    );

    for (idx, query) in queries.iter().enumerate() {
        let single = search(query, dir.path(), None, None, Some("*.go")).unwrap();
        assert_eq!(
            definition_keys(&batch[idx], dir.path()),
            definition_keys(&single, dir.path()),
            "batch definitions diverged from single search for '{query}'"
        );
    }
}

/// Normalized definition identity: scope-relative path, anchor, range, name and
/// kind weight. Occurrences are excluded so a downgrade cannot pass as a match.
fn definition_keys(
    result: &SearchResult,
    root: &std::path::Path,
) -> std::collections::BTreeSet<(String, u32, Option<(u32, u32)>, Option<String>, u16)> {
    result
        .matches
        .iter()
        .filter(|m| m.is_definition)
        .map(|m| {
            (
                m.path
                    .strip_prefix(root)
                    .unwrap_or(&m.path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                m.line,
                m.def_range,
                m.def_name.clone(),
                m.def_weight,
            )
        })
        .collect()
}
