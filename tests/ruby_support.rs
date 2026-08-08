//! End-to-end proof that Ruby gets AST-backed structural navigation.
//!
//! Covers: outline (`show`), symbol search (`discover --as symbol`),
//! section drill-in (`show --section`), caller attribution (`trace callers`),
//! and callee resolution (`trace callees --detailed`). Also asserts that
//! metaprogrammed-only names (`attr_reader`, `define_method`) are never
//! labeled AST definitions.

use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn write_fixture(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

const ORDER_RB: &str = r#"module Store
  class Order < Record
    def total
      base() + tax()
    end

    def tax
      base() * 0.1
    end

    def base
      items.sum
    end
  end
end
"#;

const PERSON_RB: &str = r#"class Person
  attr_reader :name
  define_method(:greet) { "hi" }
end
"#;

#[test]
fn ruby_show_emits_bounded_structural_outline() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "order.rb", ORDER_RB);

    let out = srcwalk().arg("show").arg(&path).output().expect("run show");
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    assert!(out.status.success(), "show failed: {stdout}");
    // Module, class, and function rows with ranges; methods nested under class.
    assert!(
        stdout.contains("[1-15]       mod Store"),
        "missing mod row:\n{stdout}"
    );
    assert!(
        stdout.contains("[2-14]       class Order"),
        "missing class row:\n{stdout}"
    );
    assert!(
        stdout.contains("[3-5]        fn total"),
        "missing fn row:\n{stdout}"
    );
    assert!(
        stdout.contains("[7-9]        fn tax"),
        "missing fn row:\n{stdout}"
    );
    assert!(
        stdout.contains("[11-13]      fn base"),
        "missing fn row:\n{stdout}"
    );
    // Bounded: never leaks `<top-level>` or anonymous placeholders into Ruby outline.
    assert!(
        !stdout.contains("<top-level>"),
        "top-level leaked:\n{stdout}"
    );
}

#[test]
fn ruby_discover_class_and_method_are_ast_definitions() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "order.rb", ORDER_RB);

    let out = srcwalk()
        .args(["discover", "Order", "--as", "symbol", "--scope"])
        .arg(dir.path())
        .output()
        .expect("run discover");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(out.status.success(), "discover failed: {stdout}");
    assert!(
        stdout.contains("source: ast") && stdout.contains("structural syntax"),
        "class not an AST definition:\n{stdout}"
    );
    assert!(stdout.contains("[class]"), "missing class row:\n{stdout}");
    assert!(
        stdout.contains("order.rb:2"),
        "missing definition location:\n{stdout}"
    );

    let out = srcwalk()
        .args(["discover", "tax", "--as", "symbol", "--scope"])
        .arg(dir.path())
        .output()
        .expect("run discover method");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(out.status.success(), "method discover failed: {stdout}");
    assert!(
        stdout.contains("source: ast") && stdout.contains("structural syntax"),
        "method not an AST definition:\n{stdout}"
    );
    assert!(stdout.contains("[fn]"), "missing fn row:\n{stdout}");
    assert!(
        stdout.contains("order.rb:7"),
        "missing method location:\n{stdout}"
    );
}

#[test]
fn ruby_show_section_resolves_exact_method_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_fixture(dir.path(), "order.rb", ORDER_RB);

    let out = srcwalk()
        .arg("show")
        .arg(&path)
        .args(["--section", "tax"])
        .output()
        .expect("run show --section");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(out.status.success(), "show --section failed: {stdout}");
    assert!(
        stdout.contains("def tax") && stdout.contains("base() * 0.1") && stdout.contains("end"),
        "section did not resolve the exact method body:\n{stdout}"
    );
}

#[test]
fn ruby_callers_attribute_to_enclosing_type_and_method() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "order.rb", ORDER_RB);

    let out = srcwalk()
        .args(["trace", "callers", "base", "--scope"])
        .arg(dir.path())
        .output()
        .expect("run trace callers");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(out.status.success(), "trace callers failed: {stdout}");
    assert!(
        stdout.contains("Order.total") && stdout.contains("Order.tax"),
        "callers not attributed to enclosing type+method:\n{stdout}"
    );
    assert!(
        !stdout.contains("<top-level>"),
        "call attributed to top-level, should be method-owner:\n{stdout}"
    );
}

#[test]
fn ruby_callees_detailed_resolves_direct_calls() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "order.rb", ORDER_RB);

    let out = srcwalk()
        .args(["trace", "callees", "total", "--scope"])
        .arg(dir.path())
        .arg("--detailed")
        .output()
        .expect("run trace callees");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(out.status.success(), "trace callees failed: {stdout}");
    assert!(
        stdout.contains("base()") && stdout.contains("tax()"),
        "callees did not surface direct Ruby calls:\n{stdout}"
    );
}

#[test]
fn ruby_metaprogrammed_names_are_not_ast_definitions() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "person.rb", PERSON_RB);

    for symbol in ["name", "greet"] {
        let out = srcwalk()
            .args(["discover", symbol, "--as", "symbol", "--scope"])
            .arg(dir.path())
            .output()
            .expect("run discover");
        let stdout = String::from_utf8(out.stdout).expect("utf8");
        assert!(out.status.success(), "discover {symbol} failed: {stdout}");
        assert!(
            !stdout.contains("structural syntax") && !stdout.contains("source: ast"),
            "{symbol} (metaprogrammed) must not be an AST definition:\n{stdout}"
        );
        assert!(
            stdout.contains("text evidence"),
            "{symbol} should be labeled text-only evidence:\n{stdout}"
        );
    }
}
