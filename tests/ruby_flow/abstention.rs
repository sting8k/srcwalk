use super::*;

/// Every direct-flow construct the IR cannot represent must hard-abstain with
/// the exact construct, line, and edge reason, and emit no partial graph.
#[test]
fn ruby_flow_abstention_parameterized_never_partial_graph() {
    let dir = TempRepo::new("abstention_kinds");
    let src = r#"
class Kinds
  def with_rescue
    x = 1
    begin
      x = risky()
    rescue StandardError
      x = 0
    end
    x
  end

  def with_ensure
    x = 1
    begin
      x = risky()
    ensure
      x = 2
    end
  end

  def with_rescue_modifier
    x = risky() rescue 0
    x
  end

  def with_break
    i = 0
    while i < 3
      break if i == 2
      i += 1
    end
    i
  end

  def with_next
    i = 0
    while i < 3
      i += 1
      next if i == 2
    end
    i
  end

  def with_redo
    i = 0
    while i < 3
      redo if i == 2
      i += 1
    end
    i
  end

  def with_retry
    i = 0
    begin
      i += 1
      retry if i < 2
    end
    i
  end
end
"#;
    write_file(&dir.join("kinds.rb"), src);

    let cases = [
        (
            "with_rescue",
            "rescue StandardError",
            "rescue",
            "exception-handling edges",
        ),
        (
            "with_ensure",
            "    ensure",
            "ensure",
            "exception-ensure propagation",
        ),
        (
            "with_rescue_modifier",
            "x = risky() rescue 0",
            "rescue_modifier",
            "exception-handling edges",
        ),
        (
            "with_break",
            "break if i == 2",
            "break",
            "abrupt loop exit edges",
        ),
        (
            "with_next",
            "next if i == 2",
            "next",
            "abrupt-control edges",
        ),
        (
            "with_redo",
            "redo if i == 2",
            "redo",
            "abrupt loop restart edges",
        ),
        (
            "with_retry",
            "retry if i < 2",
            "retry",
            "exception-retry edges",
        ),
    ];

    for (method, needle, kind, reason) in cases {
        let line = line_of(src, needle);
        let (ok, stdout, stderr) = run(&dir, &["decision-flow", &format!("kinds.rb:{method}")]);
        assert!(!ok, "{method} must fail loudly");
        let expected =
            format!("Ruby Flow Map abstained: unsupported {kind} at line {line} requires {reason}");
        assert!(
            stderr.contains(&expected),
            "{method}: expected {expected:?} in stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("[flow]"),
            "{method}: must not emit a partial graph:\n{stdout}"
        );
    }
}

#[test]
fn ruby_flow_context_fallback_retains_packet_and_exact_caveat() {
    let dir = TempRepo::new("context_fallback");
    let src = r#"
class K
  def s(xs)
    out = []
    i = 0
    while i < xs.length
      next if xs[i] < 0
      out << xs[i]
      i += 1
    end
    out
  end
end
"#;
    write_file(&dir.join("k.rb"), src);

    let (ok, stdout, stderr) = run(&dir, &["context", "k.rb:s"]);
    assert!(ok, "context must fall back gracefully, stderr:\n{stderr}");
    assert!(stdout.contains("## Target"), "{stdout}");
    assert!(stdout.contains("## Flow Map"), "{stdout}");
    assert!(stdout.contains("## Exits"), "{stdout}");
    assert!(
        stdout.contains(
            "file-level evidence only; structural function map unavailable for this target"
        ),
        "{stdout}"
    );
    let next_line = line_of(src, "next if xs[i] < 0");
    let caveat = format!(
        "caveat: Ruby Flow Map abstained: unsupported next at line {next_line} requires abrupt-control edges"
    );
    assert!(
        stdout.contains(&caveat),
        "expected {caveat:?} in:\n{stdout}"
    );
    // No partial graph in the fallback.
    assert!(!stdout.contains("shape:"), "{stdout}");
    assert!(!stdout.contains("decision :"), "{stdout}");
    assert!(
        stdout.contains("not available from structural parser"),
        "{stdout}"
    );
    // Downstream packet sections still render after the Flow Map fallback.
    assert!(
        stdout.contains("## Call Neighborhood"),
        "packet must keep downstream sections after fallback: {stdout}"
    );
}

#[test]
fn ruby_flow_unrelated_invalid_query_is_not_swallowed() {
    let dir = TempRepo::new("unrelated_error");
    write_file(&dir.join("txt.txt"), "plain text, no code\n");

    let (ok, stdout, stderr) = run(&dir, &["context", "txt.txt"]);
    assert!(!ok, "unrelated InvalidQuery must propagate, not fall back");
    assert!(
        stderr.contains("target needs a symbol, line, or range"),
        "stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("## Flow Map"),
        "must not render a fallback packet:\n{stdout}"
    );
}

#[test]
fn ruby_flow_nested_defs_comments_strings_do_not_poison_outer() {
    let dir = TempRepo::new("isolation");
    write_file(
        &dir.join("iso.rb"),
        r#"
class Iso
  # next and rescue in a comment: # next if x; rescue StandardError
  def outer(x)
    class Inner
      def hidden
        next if true
      end
    end
    inner = "next in string"
    y = x + 1
    z = y * 2
    z
  end

  def inner_hidden
    next if true
    rescue StandardError
      1
    end
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "iso.rb:outer"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(
        !stdout.contains("abstained"),
        "outer must not abstain: {stdout}"
    );
    assert!(
        stdout.contains("y = x + 1"),
        "outer body must render: {stdout}"
    );
    assert!(
        stdout.contains("z = y * 2"),
        "outer body must render: {stdout}"
    );
    // The nested class body must not leak into the outer Flow Map.
    let flow = &stdout[stdout.find("## Flow Map").unwrap()..stdout.find("## Exits").unwrap()];
    assert!(
        !flow.contains("if true"),
        "nested class body must not leak into outer: {flow}"
    );
}

#[test]
fn ruby_flow_line_range_target_and_deterministic_output() {
    let dir = TempRepo::new("line_range_determinism");
    write_file(
        &dir.join("multi.rb"),
        r#"
class Multi
  def decide(x)
    if x > 0
      small(x)
    else
      big(x)
    end
  end

  def other
    plain()
  end
end
"#,
    );

    // A line and a range inside `decide` both resolve to it.
    let (ok, by_line, _) = run(&dir, &["context", "multi.rb:5"]);
    assert!(ok);
    assert!(
        by_line.contains("small(x)"),
        "line target must resolve: {by_line}"
    );
    let (ok, by_range, _) = run(&dir, &["context", "multi.rb:4-9"]);
    assert!(ok);
    assert!(
        by_range.contains("small(x)"),
        "range target must resolve: {by_range}"
    );

    // Deterministic: two runs produce byte-identical packets.
    let (ok1, first, _) = run(&dir, &["context", "multi.rb:decide"]);
    let (ok2, second, _) = run(&dir, &["context", "multi.rb:decide"]);
    assert!(ok1 && ok2);
    assert_eq!(first, second, "output must be deterministic");
}

/// Regression: the generic `branch_body_nodes` change must keep an
/// expression-bodied (non-block) case branch whole, not split return from expr.
#[test]
fn ruby_flow_js_expression_bodied_case_branch_regression() {
    let dir = TempRepo::new("js_expression_bodied");
    write_file(
        &dir.join("js.js"),
        r#"
function route(mode) {
  switch (mode) {
    case "text":
      return runText();
    case "bin":
      return runBin();
    default:
      return runDefault();
  }
}
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["decision-flow", "js.js:route"]);
    assert!(ok, "stderr:\n{stderr}");
    for needle in [
        "\"text\" =>",
        "\"bin\" =>",
        "default =>",
        "runText",
        "runBin",
        "runDefault",
    ] {
        assert!(stdout.contains(needle), "missing {needle:?} in:\n{stdout}");
    }
    let text_line = line_containing(&stdout, "runText");
    assert!(
        text_line.contains("[return]"),
        "expression-bodied case must stay one return node: {text_line}"
    );
}
