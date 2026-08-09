use super::*;

#[test]
fn ruby_flow_method_singleton_and_endless_resolution() {
    let dir = TempRepo::new("method_singleton_endless");
    write_file(
        &dir.join("multi.rb"),
        r#"
class Multi
  def self.build(x)
    if x > 0
      "pos"
    else
      "nonpos"
    end
  end

  def plain(x)
    if x > 0
      small(x)
    else
      big(x)
    end
  end

  def shout(x) = loud(x)
end
"#,
    );

    // Instance method symbol resolves and renders a decision.
    let (ok, stdout, stderr) = run(&dir, &["context", "multi.rb:plain"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(stdout.contains("## Flow Map"), "{stdout}");
    assert!(stdout.contains("if x > 0"), "{stdout}");
    assert!(stdout.contains("small(x)"), "{stdout}");
    assert!(stdout.contains("big(x)"), "{stdout}");

    // Singleton method resolves via its display name and renders a decision.
    let (ok, stdout, _) = run(&dir, &["context", "multi.rb:build"]);
    assert!(ok);
    assert!(
        stdout.contains("if x > 0"),
        "singleton must render: {stdout}"
    );
    assert!(stdout.contains("decision :"), "{stdout}");

    // Endless method body call is traced as an action.
    let (ok, stdout, _) = run(&dir, &["context", "multi.rb:shout"]);
    assert!(ok);
    assert!(
        stdout.contains("loud(x)"),
        "endless body call missing: {stdout}"
    );

    // review renders the same flow map through the shared engine.
    let (ok, stdout, _) = run(&dir, &["review", "multi.rb:plain"]);
    assert!(ok, "review must succeed");
    assert!(
        stdout.contains("## flow map"),
        "review must render a flow map section: {stdout}"
    );
    assert!(stdout.contains("small(x)"), "{stdout}");
}

#[test]
fn ruby_flow_if_elsif_else_and_modifier_if() {
    let dir = TempRepo::new("if_modifier");
    write_file(
        &dir.join("guard.rb"),
        r#"
class Guard
  def classify(x)
    if x > 0
      positive(x)
    elsif x < 0
      negative(x)
    else
      zero()
    end
  end

  def check(x)
    return 0 if x.nil?
    raise ArgumentError, "bad" if x < 0
    x * 2
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "guard.rb:check"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(stdout.contains("if x.nil?"), "{stdout}");
    assert!(stdout.contains("return 0"), "{stdout}");
    assert!(stdout.contains("if x < 0"), "{stdout}");
    // Modifier-if orientation: return executes on the TRUE edge of `if x.nil?`.
    let return_line = line_containing(&stdout, "return 0");
    assert!(
        return_line.contains("true ->"),
        "modifier if must fire on true: {return_line}"
    );
    let raise_line = line_containing(&stdout, "raise ArgumentError, \"bad\"");
    assert!(
        raise_line.contains("true ->"),
        "modifier if must fire on true: {raise_line}"
    );

    // Normal if/elsif/else: all three branches render as actions in source order.
    let (ok, stdout, _) = run(&dir, &["context", "guard.rb:classify"]);
    assert!(ok);
    assert!(stdout.contains("if x > 0"), "{stdout}");
    assert!(
        stdout.contains("if x < 0"),
        "elsif must render as a decision: {stdout}"
    );
    assert!(stdout.contains("positive(x)"), "{stdout}");
    assert!(stdout.contains("negative(x)"), "{stdout}");
    assert!(stdout.contains("zero()"), "{stdout}");
    let pos = line_containing(&stdout, "positive(x)");
    assert!(pos.contains("true ->"), "if branch fires on true: {pos}");
    let neg = line_containing(&stdout, "negative(x)");
    assert!(
        neg.contains("true ->"),
        "elsif branch fires on the nested if true edge: {neg}"
    );
    let zero = line_containing(&stdout, "zero()");
    assert!(
        zero.contains("false ->"),
        "else branch fires on the nested if false edge: {zero}"
    );
    let pos_i = stdout.find("positive(x)").unwrap();
    let neg_i = stdout.find("negative(x)").unwrap();
    let zero_i = stdout.find("zero()").unwrap();
    assert!(
        pos_i < neg_i && neg_i < zero_i,
        "branches must render in source order: {stdout}"
    );
}

#[test]
fn ruby_flow_unless_body_via_no_and_alternative_via_yes() {
    let dir = TempRepo::new("unless_orientation");
    write_file(
        &dir.join("u.rb"),
        r#"
class U
  def check(x)
    unless x.valid?
      fix(x)
    else
      ok()
    end
  end

  def guard_mod(x)
    cleanup(x) unless x.ready?
    x
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "u.rb:check"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(stdout.contains("unless x.valid?"), "{stdout}");
    // Body is reached when the condition is FALSE ("no"); alternative via "yes".
    let false_idx = stdout.find("false ->").unwrap();
    let true_idx = stdout.find("true ->").unwrap();
    assert!(
        false_idx < true_idx,
        "body edge must precede alternative edge"
    );
    assert!(
        stdout[false_idx..true_idx].contains("fix(x)"),
        "unless body must be reached via no: {stdout}"
    );
    assert!(
        stdout[true_idx..].contains("ok()"),
        "unless alternative must be reached via yes: {stdout}"
    );

    // unless_modifier: the body also executes when the condition is FALSE.
    let (ok, stdout, _) = run(&dir, &["context", "u.rb:guard_mod"]);
    assert!(ok);
    assert!(stdout.contains("unless x.ready?"), "{stdout}");
    assert!(stdout.contains("cleanup(x)"), "{stdout}");
    let cleanup = line_containing(&stdout, "cleanup(x)");
    assert!(
        cleanup.contains("false ->"),
        "unless_modifier body must fire on the false/no edge: {cleanup}"
    );
}

#[test]
fn ruby_flow_case_patterns_are_labels_not_actions() {
    let dir = TempRepo::new("case_patterns");
    write_file(
        &dir.join("casey.rb"),
        r#"
class Casey
  def decide(x)
    case x
    when 1, 2
      small()
    when 3
      mid()
    else
      big()
    end
  end

  def classify(v)
    case
    when v.nil?
      missing()
    when v > 0
      positive()
    else
      unknown()
    end
  end
end
"#,
    );

    // Value case: patterns are edge labels, actions are the method calls.
    let (ok, stdout, stderr) = run(&dir, &["context", "casey.rb:decide"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(stdout.contains("case x"), "case condition label: {stdout}");
    assert!(
        stdout.contains("1, 2 ->"),
        "multi-pattern must be an edge label: {stdout}"
    );
    assert!(
        stdout.contains("3 ->"),
        "single pattern must be an edge label: {stdout}"
    );
    assert!(
        stdout.contains("default ->"),
        "else must be labeled default: {stdout}"
    );
    assert!(stdout.contains("small()"), "{stdout}");
    assert!(stdout.contains("mid()"), "{stdout}");
    assert!(stdout.contains("big()"), "{stdout}");

    // Value-less case: no condition on the decision; patterns carry conditions.
    let (ok, stdout, _) = run(&dir, &["context", "casey.rb:classify"]);
    assert!(ok);
    assert!(
        !stdout.contains("case v"),
        "value-less case must not bind a condition: {stdout}"
    );
    assert!(stdout.contains("v.nil? ->"), "{stdout}");
    assert!(stdout.contains("v > 0 ->"), "{stdout}");
    assert!(stdout.contains("missing()"), "{stdout}");
    assert!(stdout.contains("positive()"), "{stdout}");
}

#[test]
fn ruby_flow_case_without_else_keeps_fallthrough_reachable() {
    // When every `when` branch terminates and there is no `else`, an
    // unmatched value still falls through with no exception in real Ruby;
    // the statement after `case` must remain in the Flow Map instead of
    // silently disappearing because the branches all terminated.
    let dir = TempRepo::new("case_no_else_fallthrough");
    write_file(
        &dir.join("router.rb"),
        r#"
class Router
  def label(v)
    case v
    when 1
      return "one"
    when 2
      return "two"
    end
    unmatched_default()
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "router.rb:label"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(
        stdout.contains("unmatched_default()"),
        "statement after an else-less case must stay reachable: {stdout}"
    );
    let fallthrough = line_containing(&stdout, "unmatched_default()");
    assert!(
        fallthrough.contains("no match ->"),
        "fallthrough edge must be honestly labeled: {fallthrough}"
    );
}

#[test]
fn ruby_flow_loop_labels_while_until_for() {
    let dir = TempRepo::new("loop_labels");
    write_file(
        &dir.join("loopy.rb"),
        r#"
class Loopy
  def run(xs)
    while xs.length > 0
      process(xs.shift)
    end
    until done?
      tick()
    end
    for item in xs
      use(item)
    end
    work 5 while more?
    save(x, y) until done?
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "loopy.rb:run"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(stdout.contains("while xs.length > 0"), "{stdout}");
    assert!(stdout.contains("until done?"), "{stdout}");
    assert!(stdout.contains("for item in xs"), "{stdout}");
    assert!(stdout.contains("loop_back"), "{stdout}");
    assert!(stdout.contains("process"), "{stdout}");
    assert!(stdout.contains("tick"), "{stdout}");
    assert!(stdout.contains("use"), "{stdout}");
    // Modifier loops: label + body action render for while/until modifiers.
    assert!(
        stdout.contains("while more?"),
        "modifier while label: {stdout}"
    );
    assert!(
        stdout.contains("until done?"),
        "modifier until label: {stdout}"
    );
    assert!(stdout.contains("work 5"), "modifier while body: {stdout}");
    assert!(
        stdout.contains("save(x, y)"),
        "modifier until body: {stdout}"
    );
}

#[test]
fn ruby_flow_return_and_receiverless_throws_but_obj_raise_is_call() {
    let dir = TempRepo::new("throw_vs_call");
    write_file(
        &dir.join("excl.rb"),
        r#"
class Excl
  def check(v)
    raise "boom" if v.nil?
    fail "nope" if v == 0
    obj.raise if v < 0
    loud(v)
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "excl.rb:check"]);
    assert!(ok, "stderr:\n{stderr}");
    // Receiverless raise/fail are throws.
    let boom = line_containing(&stdout, "raise \"boom\"");
    assert!(
        boom.contains("throw"),
        "receiverless raise must be a throw: {boom}"
    );
    let fail = line_containing(&stdout, "fail \"nope\"");
    assert!(
        fail.contains("throw"),
        "receiverless fail must be a throw: {fail}"
    );
    // obj.raise stays a plain call/action, never a throw.
    let obj = line_containing(&stdout, "obj.raise");
    assert!(obj.contains("action"), "obj.raise must be an action: {obj}");
    assert!(
        !obj.contains("throw"),
        "obj.raise must not be a throw: {obj}"
    );
    assert!(stdout.contains("loud(v)"), "{stdout}");
}

#[test]
fn ruby_flow_opaque_block_header_only_no_abstention() {
    let dir = TempRepo::new("opaque_block");
    write_file(
        &dir.join("blky.rb"),
        r#"
class Blky
  def collect(xs)
    out = []
    xs.each do |x|
      next if x < 0
      out << x if x > 0
    end
    out
  end
end
"#,
    );

    let (ok, stdout, stderr) = run(&dir, &["context", "blky.rb:collect"]);
    assert!(ok, "stderr:\n{stderr}");
    assert!(
        stdout.contains("xs.each"),
        "call header must render: {stdout}"
    );
    assert!(
        !stdout.contains("abstained"),
        "block must not abstain: {stdout}"
    );
    let flow = &stdout[stdout.find("## Flow Map").unwrap()..stdout.find("## Exits").unwrap()];
    assert!(flow.contains("xs.each"), "{flow}");
    assert!(
        !flow.contains("next if x < 0"),
        "block body must not be traced: {flow}"
    );
    assert!(
        !flow.contains("out << x"),
        "block body must not be traced: {flow}"
    );
}
