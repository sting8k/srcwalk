use std::fs;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}
fn context_output(dir: &std::path::Path, file: &std::path::Path, symbol: &str) -> String {
    let target = format!("{}:{symbol}", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "context should succeed for {target}, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    stdout.into_owned()
}

fn assert_no_def_use_verdict_words(stdout: &str) {
    for forbidden in [
        "depends", "affects", "unsafe", "mismatch", "risk", "bug", "security",
    ] {
        assert!(
            !contains_word(stdout, forbidden),
            "local syntactic def-use must not emit verdict wording `{forbidden}`:\n{stdout}"
        );
    }
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .any(|word| word == needle)
}

fn assert_no_trace_routes(output: &str) {
    assert!(
        !output.contains("srcwalk trace callers") && !output.contains("srcwalk trace callees"),
        "context abstention must not emit source relation routes:\n{output}"
    );
}

#[test]
fn flow_filter_slices_ordered_calls_and_resolves_matching_callee() {
    let dir = temp_dir("flow_filter");
    fs::write(
        dir.join("lib.rs"),
        r#"
mod format;

fn entry() {
    let value = helper();
    noisy();
    format();
}

fn helper() -> i32 {
    1
}

fn noisy() {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["context", "entry", "--filter", "callee:helper", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow filter should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("### Callees (ordered, filtered callee:helper)"),
        "expected filtered callees header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("helper()"),
        "expected matching helper call, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("noisy()"),
        "filter should exclude non-matching call, got:\n{stdout}"
    );
    assert!(
        stdout.contains("filter matched 1/3 call sites"),
        "expected filter count footer, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[fn] helper"),
        "expected matching helper resolve, got:\n{stdout}"
    );
}

#[test]
fn flow_shows_call_arg_slots() {
    let dir = temp_dir("flow_arg_slots");
    fs::write(
        dir.join("lib.rs"),
        r#"
fn entry() {
    let value = helper(1, "two");
    finish(value);
}

fn helper(a: i32, b: &str) -> i32 { a }
fn finish(value: i32) {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["context", "entry", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("helper(arg1=1, arg2=\"two\")"),
        "expected arg slots for helper call, got:\n{stdout}"
    );
    assert!(
        stdout.contains("finish(arg1=value)"),
        "expected arg slot for finish call, got:\n{stdout}"
    );
}

#[test]
fn flow_resolves_skip_module_like_noise() {
    let dir = temp_dir("flow_resolve_noise");
    fs::write(
        dir.join("lib.rs"),
        r#"
mod format;

fn entry() {
    helper();
    format();
}

fn helper() {}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["context", "entry", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "flow should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("### Resolved local callees"),
        "expected local-helper resolves heading, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[fn] helper"),
        "expected helper resolve, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[fn] format"),
        "module-like format resolve should be skipped, got:\n{stdout}"
    );
}

#[test]
fn context_exact_file_symbol_renders_flow_map_and_neighborhood() {
    let dir = temp_dir("context_exact_file_symbol");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"fn entry() -> i32 {
    helper()
}

fn helper() -> i32 { 1 }
"#,
    )
    .unwrap();

    let target = format!("{}:entry", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context exact target should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("# Context Packet:"), "{stdout}");
    assert!(stdout.contains("## Flow Map"), "{stdout}");
    assert!(stdout.contains("## Call Neighborhood"), "{stdout}");
    assert!(stdout.contains("## Source Evidence"), "{stdout}");
    assert!(stdout.contains("   1| fn entry() -> i32 {"), "{stdout}");
    assert!(stdout.contains("### Callees"), "{stdout}");
    assert!(stdout.contains("### Callers"), "{stdout}");
    assert!(
        !stdout.contains("> Next: srcwalk show"),
        "exact context with complete source evidence should not suggest same-target show:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk trace callers entry"),
        "context should expose upstream drilldown, got:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk trace callees entry --detailed"),
        "context should expose downstream drilldown, got:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("> Next: srcwalk show").count(),
        0,
        "exact context should not include same-target show next action:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk trace callers entry")
            .count(),
        1,
        "context callers next action should not be duplicated:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk trace callees entry --detailed")
            .count(),
        1,
        "context callees next action should not be duplicated:\n{stdout}"
    );
}

#[test]
fn context_batches_comma_separated_exact_targets() {
    let dir = temp_dir("context_multi_exact_targets");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"fn first() -> i32 {
    1
}

fn second() -> i32 {
    first() + 1
}

fn unrelated() -> i32 {
    3
}
"#,
    )
    .unwrap();

    let target = "lib.rs:first,lib.rs:second";
    let out = srcwalk()
        .current_dir(&dir)
        .args(["context", target, "--scope", ".", "--no-budget"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "multi-target context should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("# Context: 2 exact targets"),
        "expected multi-target header:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("# Context Packet:").count(),
        2,
        "expected one packet per exact target:\n{stdout}"
    );
    assert!(stdout.contains("## Target: lib.rs:first"), "{stdout}");
    assert!(stdout.contains("## Target: lib.rs:second"), "{stdout}");
    assert!(stdout.contains("fn first() -> i32"), "{stdout}");
    assert!(stdout.contains("fn second() -> i32"), "{stdout}");
    assert!(
        stdout.contains("multi-target context splits one global budget"),
        "expected budget caveat:\n{stdout}"
    );
}

#[test]
fn context_multi_target_rejects_same_file_range_shorthand() {
    let dir = temp_dir("context_multi_reject_shorthand");
    let file = dir.join("lib.rs");
    fs::write(&file, "fn first() {}\nfn second() {}\n").unwrap();

    let target = format!("{}:1-1,2-2", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "expected shorthand rejection");
    assert!(
        stderr.contains("repeat the file path for each range"),
        "expected exact-target guidance, got:\n{stderr}"
    );
}

#[test]
fn context_multi_target_rejects_more_than_three_targets() {
    let dir = temp_dir("context_multi_reject_cap");
    let file = dir.join("lib.rs");
    fs::write(&file, "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n").unwrap();

    let target = format!(
        "{}:a,{}:b,{}:c,{}:d",
        file.display(),
        file.display(),
        file.display(),
        file.display()
    );
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "expected multi-target cap rejection");
    assert!(
        stderr.contains("at most 3 comma-separated exact targets"),
        "expected cap guidance, got:\n{stderr}"
    );
}

#[test]
fn context_multi_target_rejects_empty_list_members() {
    let dir = temp_dir("context_multi_reject_empty_member");
    let file = dir.join("lib.rs");
    fs::write(&file, "fn first() {}\nfn second() {}\n").unwrap();

    let target = format!("{}:first,", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "expected empty member rejection");
    assert!(
        stderr.contains("empty context target in comma-separated list"),
        "expected empty-list guidance, got:\n{stderr}"
    );
}

#[test]
fn context_multi_target_rejects_budget_too_small_for_all_packets() {
    let dir = temp_dir("context_multi_reject_tiny_budget");
    fs::write(
        dir.join("lib.rs"),
        r#"fn first() -> i32 {
    1
}

fn second() -> i32 {
    first() + 1
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "context",
            "lib.rs:first,lib.rs:second",
            "--scope",
            ".",
            "--budget",
            "320",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "expected too-small budget rejection, stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("multi-target context --budget 320 is too small"),
        "expected budget-specific guidance, got:\n{stderr}"
    );
    assert!(stderr.contains("raise --budget"), "{stderr}");
    assert!(stderr.contains("run targets separately"), "{stderr}");
}

#[test]
fn context_multi_target_tight_boundary_keeps_target_identity_and_evidence() {
    let dir = temp_dir("context_multi_tight_boundary");
    fs::write(
        dir.join("lib.rs"),
        r#"fn first() -> i32 {
    1
}

fn second() -> i32 {
    first() + 1
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "context",
            "lib.rs:first,lib.rs:second",
            "--scope",
            ".",
            "--budget",
            "370",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "tight multi-target context should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("## Target: lib.rs:first"), "{stdout}");
    assert!(
        stdout.contains("# Context Packet: lib.rs:first"),
        "{stdout}"
    );
    assert!(stdout.contains("   1| fn first() -> i32 {"), "{stdout}");
    assert!(stdout.contains("## Target: lib.rs:second"), "{stdout}");
    assert!(
        stdout.contains("# Context Packet: lib.rs:second"),
        "{stdout}"
    );
    assert!(stdout.contains("   5| fn second() -> i32 {"), "{stdout}");
    assert!(
        stdout.trim_end().len() <= 370 * 4,
        "assembled packet should stay within approximate budget bytes, len={} stdout:\n{stdout}",
        stdout.trim_end().len()
    );
}

#[test]
fn context_multi_target_applies_global_budget_to_assembled_packet() {
    let dir = temp_dir("context_multi_global_budget");
    fs::write(
        dir.join("lib.rs"),
        r#"fn first() -> i32 {
    1
}

fn second() -> i32 {
    first() + 1
}
"#,
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "context",
            "lib.rs:first,lib.rs:second",
            "--scope",
            ".",
            "--budget",
            "370",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "multi-target budget context should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.trim_end().len() <= 370 * 4,
        "assembled packet should stay within approximate budget bytes, len={} stdout:\n{stdout}",
        stdout.trim_end().len()
    );
}

#[test]
fn context_flow_map_includes_local_syntactic_def_use() {
    let dir = temp_dir("context_local_def_use");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"struct User { id: String }
fn handle(user: User, enabled: bool) -> Result<String, String> {
    let id = user.id;
    let normalized = normalize(id, enabled);
    if enabled && normalized.is_empty() {
        return Err(normalized);
    }
    Ok(normalized)
}
fn normalize(value: String, enabled: bool) -> String { value }
"#,
    )
    .unwrap();

    let target = format!("{}:handle", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context local def-use should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("definitions: user parameter :2; enabled parameter :2"),
        "expected parameter definitions:\n{stdout}"
    );
    assert!(
        stdout.contains("writes: id assignment_lhs :3"),
        "expected assignment lhs write:\n{stdout}"
    );
    assert!(
        stdout.contains("reads: user.id assignment_rhs :3"),
        "expected assignment rhs read:\n{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :4"),
        "expected call-assignment lhs write:\n{stdout}"
    );
    assert!(
        stdout.contains("calls: normalize :4"),
        "expected call annotation for assignment RHS call:\n{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :4; enabled call_arg :4"),
        "expected call assignment arguments as call_arg reads:\n{stdout}"
    );
    assert!(
        !stdout.contains("normalize assignment_rhs")
            && !stdout.contains("id assignment_rhs :4")
            && !stdout.contains("enabled assignment_rhs :4"),
        "call-assignment RHS must not duplicate call args or mark the callee as a data read:\n{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :5; normalized.is_empty condition :5"),
        "expected condition reads:\n{stdout}"
    );
    assert!(
        stdout.contains("reads: normalized call_arg :6"),
        "expected return call argument read:\n{stdout}"
    );
    for forbidden in [
        "depends", "affects", "unsafe", "mismatch", "risk", "bug", "security",
    ] {
        assert!(
            !stdout.contains(forbidden),
            "local syntactic def-use must not emit verdict wording `{forbidden}`:\n{stdout}"
        );
    }
}

#[test]
fn context_flow_map_includes_local_def_use_for_typescript_javascript_and_go() {
    let dir = temp_dir("context_local_def_use_ts_js_go");

    let ts = dir.join("sample.ts");
    fs::write(
        &ts,
        r#"function handle(user: { id: string }, enabled: boolean): string {
  const id = user.id;
  const normalized = normalize(id, enabled);
  if (enabled && normalized.length === 0) {
    return fail(normalized);
  }
  return normalized;
}
function normalize(value: string, flag: boolean): string { return value; }
function fail(value: string): string { return value; }
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &ts, "handle");
    assert!(
        stdout.contains("definitions: user parameter :1; enabled parameter :1"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :2"), "{stdout}");
    assert!(
        stdout.contains("reads: user.id assignment_rhs :2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :3; enabled call_arg :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :4; normalized.length condition :4"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);

    let js = dir.join("sample.js");
    fs::write(
        &js,
        r#"function handle(user, enabled) {
  const id = user.id;
  const normalized = normalize(id, enabled);
  if (enabled && normalized.length === 0) {
    return fail(normalized);
  }
  return normalized;
}
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &js, "handle");
    assert!(
        stdout.contains("definitions: user parameter :1; enabled parameter :1"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :2"), "{stdout}");
    assert!(
        stdout.contains("reads: user.id assignment_rhs :2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :3; enabled call_arg :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :4; normalized.length condition :4"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);

    let go = dir.join("sample.go");
    fs::write(
        &go,
        r#"package sample
func handle(user User, enabled bool) string {
    id := user.ID
    normalized := normalize(id, enabled)
    if enabled && len(normalized) == 0 {
        return fail(normalized)
    }
    return normalized
}
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &go, "handle");
    assert!(
        stdout.contains("definitions: user parameter :2; enabled parameter :2"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :3"), "{stdout}");
    assert!(
        stdout.contains("reads: user.ID assignment_rhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :4"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :4; enabled call_arg :4"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :5; len condition :5; normalized condition :5"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);
}

#[test]
fn context_flow_map_includes_local_def_use_for_python_c_and_cpp() {
    let dir = temp_dir("context_local_def_use_py_c_cpp");

    let py = dir.join("sample.py");
    fs::write(
        &py,
        r#"def handle(user, enabled):
    id = user.id
    normalized = normalize(id, enabled)
    if enabled and len(normalized) == 0:
        return fail(normalized)
    return normalized
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &py, "handle");
    assert!(
        stdout.contains("definitions: user parameter :1; enabled parameter :1"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :2"), "{stdout}");
    assert!(
        stdout.contains("reads: user.id assignment_rhs :2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :3; enabled call_arg :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :4; len condition :4; normalized condition :4"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);

    let c = dir.join("sample.c");
    fs::write(
        &c,
        r#"int handle(struct User user, int enabled) {
    int id = user.id;
    int normalized = normalize(id, enabled);
    if (enabled && normalized == 0) {
        return fail(normalized);
    }
    return normalized;
}
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &c, "handle");
    assert!(
        stdout.contains("definitions: user parameter :1; enabled parameter :1"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :2"), "{stdout}");
    assert!(
        stdout.contains("reads: user.id assignment_rhs :2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :3; enabled call_arg :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :4; normalized condition :4"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);

    let cpp = dir.join("sample.cpp");
    fs::write(
        &cpp,
        r#"int handle(const User& user, bool enabled) {
    int id = user.id;
    int normalized = normalize(id, enabled);
    if (enabled && normalized == 0) {
        return fail(normalized);
    }
    return normalized;
}
"#,
    )
    .unwrap();
    let stdout = context_output(&dir, &cpp, "handle");
    assert!(
        stdout.contains("definitions: user parameter :1; enabled parameter :1"),
        "{stdout}"
    );
    assert!(stdout.contains("writes: id assignment_lhs :2"), "{stdout}");
    assert!(
        stdout.contains("reads: user.id assignment_rhs :2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("writes: normalized assignment_lhs :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: id call_arg :3; enabled call_arg :3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("reads: enabled condition :4; normalized condition :4"),
        "{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);
}

#[test]
fn context_flow_map_handles_tsx_react_component_jsx_callback_without_overclaiming() {
    let dir = temp_dir("context_tsx_react_component");
    let tsx = dir.join("UserCard.tsx");
    fs::write(
        &tsx,
        r#"type User = { id: string; name: string; enabled: boolean };
type Props = { user: User; onSave: (id: string) => void };

export function UserCard({ user, onSave }: Props) {
  const label = user.name.trim();
  const disabled = !user.enabled;
  return (
    <button disabled={disabled} onClick={() => onSave(user.id)}>
      {label}
    </button>
  );
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, &tsx, "UserCard");
    assert!(
        stdout.contains("confidence: structural syntax"),
        "TSX context should be structural when parser support is available:\n{stdout}"
    );
    assert!(
        stdout.contains("shape: linear structural flow; no branch nodes detected by supported parser"),
        "TSX React component should degrade to a bounded linear Flow Map when JSX has no branch nodes:\n{stdout}"
    );
    assert!(
        stdout.contains("L5 label = user.name.trim()"),
        "expected TSX member-call evidence in context callees:\n{stdout}"
    );
    assert!(
        stdout.contains("L8 onSave(arg1=user.id)"),
        "expected JSX callback call argument evidence without runtime/dataflow claim:\n{stdout}"
    );
    assert!(
        stdout.contains("return ( <button disabled={disabled} onClick={() => onSave(user.id)}> {label} </button> );"),
        "expected JSX return to stay source evidence in exits:\n{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);
}

#[test]
fn context_flow_map_handles_java_and_csharp_member_chains_without_runtime_claims() {
    let dir = temp_dir("context_java_csharp_member_chains");
    let java = dir.join("OrderService.java");
    fs::write(
        &java,
        r#"class Order { Customer customer; boolean enabled; String id; }
class Customer { String name; }
class OrderService {
  String label(Order order) {
    String name = order.customer.name.trim();
    if (order.enabled) {
      return format(name, order.id);
    }
    return name;
  }
  String format(String name, String id) { return name + id; }
}
"#,
    )
    .unwrap();
    let csharp = dir.join("OrderService.cs");
    fs::write(
        &csharp,
        r#"class Order { public Customer Customer { get; set; } public bool Enabled { get; set; } public string Id { get; set; } }
class Customer { public string Name { get; set; } }
class OrderService {
  string Label(Order order) {
    var name = order.Customer.Name.Trim();
    if (order.Enabled) {
      return Format(name, order.Id);
    }
    return name;
  }
  string Format(string name, string id) { return name + id; }
}
"#,
    )
    .unwrap();

    let java_stdout = context_output(&dir, &java, "label");
    assert!(
        java_stdout.contains("confidence: structural syntax"),
        "Java context should stay structural when parser support is available:\n{java_stdout}"
    );
    assert!(
        java_stdout.contains("shape: 1 entry, 1 decision, 0 loops, 2 exits, 1 action"),
        "Java member-chain fixture should preserve bounded control-flow shape:\n{java_stdout}"
    );
    assert!(
        java_stdout.contains("definitions: order parameter :4")
            && java_stdout.contains("writes: name = order.customer.name.trim() assignment_lhs :5")
            && java_stdout.contains("order.enabled condition :6")
            && java_stdout.contains("order.id call_arg :7"),
        "Java context should expose parameter, assignment, condition, and call_arg member evidence:\n{java_stdout}"
    );
    assert!(
        java_stdout.contains("L5 name = order.customer.name.trim()")
            && java_stdout.contains("L7 ->ret format(arg1=name, arg2=order.id)"),
        "Java context should expose member-chain and local-call evidence:\n{java_stdout}"
    );
    assert_no_def_use_verdict_words(&java_stdout);

    let csharp_stdout = context_output(&dir, &csharp, "Label");
    assert!(
        csharp_stdout.contains("confidence: structural syntax"),
        "C# context should stay structural when parser support is available:\n{csharp_stdout}"
    );
    assert!(
        csharp_stdout.contains("shape: 1 entry, 1 decision, 0 loops, 2 exits, 1 action"),
        "C# property-chain fixture should preserve bounded control-flow shape:\n{csharp_stdout}"
    );
    assert!(
        csharp_stdout.contains("definitions: order parameter :4")
            && csharp_stdout.contains("order.Enabled condition :6")
            && csharp_stdout.contains("order.Id call_arg :7"),
        "C# context should expose parameter, condition, and call_arg property evidence without runtime claims:\n{csharp_stdout}"
    );
    assert!(
        csharp_stdout.contains("L5 name = order.Customer.Name.Trim()")
            && csharp_stdout.contains("L7 ->ret Format(arg1=name, arg2=order.Id)"),
        "C# context should expose property-chain and local-call evidence:\n{csharp_stdout}"
    );
    assert_no_def_use_verdict_words(&csharp_stdout);
}

#[test]
fn context_linear_flow_map_includes_entry_parameter_definitions() {
    let dir = temp_dir("context_linear_entry_params");
    let file = dir.join("output.ts");
    fs::write(
        &file,
        r#"function prettyJson(data: unknown): string {
  return JSON.stringify(data, null, 2);
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, &file, "prettyJson");
    assert!(
        stdout.contains("shape: linear structural flow"),
        "fixture should exercise linear fallback:\n{stdout}"
    );
    assert!(
        stdout.contains("entry: N1 entry :1-3 entry"),
        "linear fallback should render structurally confirmed entry node when annotated:\n{stdout}"
    );
    assert!(
        stdout.contains("definitions: data parameter :1"),
        "linear fallback should keep entry parameter definitions:\n{stdout}"
    );
    assert!(
        stdout.contains("L2 ->ret JSON.stringify(arg1=data"),
        "linear fallback should preserve existing call-neighborhood argument evidence:\n{stdout}"
    );
    assert_no_def_use_verdict_words(&stdout);
}

#[test]
fn context_bare_file_error_uses_target_language() {
    let dir = temp_dir("context_bare_file_error");
    fs::write(dir.join("lib.rs"), "fn entry() {}\n").unwrap();

    let out = srcwalk()
        .args(["context", "lib.rs", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "bare file context should fail");
    assert!(
        stderr.contains("target needs a symbol, line, or range"),
        "expected target guidance, got:\n{stderr}"
    );
    assert!(
        stderr.contains("read the file with `srcwalk") && stderr.contains("lib.rs:<symbol>"),
        "expected read and exact target suggestions, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("decision-flow"),
        "context error must not leak legacy command name:\n{stderr}"
    );
}

#[test]
fn context_line_range_fallback_does_not_emit_symbol_trace_tips() {
    let dir = temp_dir("context_range_fallback_no_trace_tips");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"pub struct Config {
    value: i32,
}

fn entry() -> i32 { helper() }
fn helper() -> i32 { 1 }
"#,
    )
    .unwrap();

    let target = format!("{}:1-3", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context range fallback should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("file-level evidence only; structural function map unavailable"),
        "expected file-level fallback:\n{stdout}"
    );
    assert!(
        stdout.contains("### Callers\n- not available for non-symbol range targets"),
        "expected caller lookup to be skipped:\n{stdout}"
    );
    assert!(stdout.contains("## Source Evidence"), "{stdout}");
    assert!(stdout.contains("   1| pub struct Config {"), "{stdout}");
    assert_eq!(
        stdout.matches("> Next: srcwalk show").count(),
        0,
        "range context with complete source evidence should not suggest same-target show:\n{stdout}"
    );
    assert!(
        !stdout.contains("trace callers 1-3") && !stdout.contains("trace callees 1-3"),
        "range target must not leak into trace tips:\n{stdout}"
    );
}

#[test]
fn context_unresolved_file_symbol_fallback_emits_resolution_caveat() {
    let dir = temp_dir("context unresolved symbol fallback caveat");
    let file = dir.join("lib file.rs");
    fs::write(
        &file,
        r#"pub struct Config {
    value: i32,
}

fn entry() -> i32 { helper() }
fn helper() -> i32 { 1 }
"#,
    )
    .unwrap();

    let target = format!("{}:MissingSymbol", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context unresolved symbol fallback should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("caveat: requested symbol selector was not resolved to a structural function range; packet is file-level only"),
        "expected unresolved-symbol fallback caveat:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "## Call Neighborhood\n- unavailable until the requested symbol resolves to a structural function target"
        ),
        "unresolved symbol fallback should not scan unrelated file-wide callees:\n{stdout}"
    );
    assert!(
        !stdout.contains("### Callees (ordered)") && !stdout.contains("helper"),
        "unresolved symbol fallback should not show unrelated file-wide call sites:\n{stdout}"
    );
    assert!(
        stdout.contains("srcwalk discover MissingSymbol --as symbol --scope"),
        "expected discover retry guidance for unresolved symbol:\n{stdout}"
    );
    assert!(
        stdout.contains(" --scope '")
            && stdout.contains("context unresolved symbol fallback caveat"),
        "spaced scope in discover retry guidance should be shell-quoted:\n{stdout}"
    );
    assert!(
        !stdout.contains("> Next: srcwalk show"),
        "unresolved symbol fallback should not emit generic file-wide show guidance:\n{stdout}"
    );
    assert!(
        !stdout.contains("srcwalk trace callers MissingSymbol")
            && !stdout.contains("srcwalk trace callees MissingSymbol"),
        "unresolved symbol fallback should not suggest trace routes before resolution:\n{stdout}"
    );
}

#[test]
fn context_exact_file_symbol_with_tight_budget_keeps_show_next() {
    let dir = temp_dir("context_exact_budget_show_next");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"fn entry() -> i32 {
    helper()
}

fn helper() -> i32 { 1 }
"#,
    )
    .unwrap();

    let target = format!("{}:entry", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .args(["--budget", "40"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context budget target should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("... truncated"), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk show") && stdout.contains(":1-3 -C 20"),
        "tight budget may omit source evidence, so exact show route must remain:\n{stdout}"
    );
}

#[test]
fn context_focused_range_inside_long_function_exposes_structural_completion() {
    let dir = temp_dir("context_focused_range_selected_source");
    let file = dir.join("lib.rs");
    let mut content = String::from("fn entry() -> i32 {\n");
    for line in 2..=100 {
        content.push_str(&format!("    let v{line} = {line};\n"));
    }
    content.push_str("    0\n}\n");
    fs::write(&file, content).unwrap();

    let target = format!("{}:50-52", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context focused range should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("## Source Evidence"), "{stdout}");
    assert!(stdout.contains("  50|     let v50 = 50;"), "{stdout}");
    assert!(stdout.contains("  52|     let v52 = 52;"), "{stdout}");
    assert!(
        !stdout.contains("   1| fn entry() -> i32"),
        "focused range source evidence should not replay the whole containing function:\n{stdout}"
    );
    assert!(
        stdout.contains("partial inside structural function 1-102")
            && stdout.contains("omitted lines 1-49,53-102"),
        "partial exact range should expose omitted structural lines:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show") && stdout.contains("--section 1-49,53-102"),
        "partial exact range should route to missing structural lines only:\n{stdout}"
    );
    assert!(
        !stdout.contains(":50-52 -C 20"),
        "complete selected source range should not suggest rereading the selected target:\n{stdout}"
    );
}

#[test]
fn context_exact_range_with_omitted_source_keeps_show_next() {
    let dir = temp_dir("context_long_range_show_next");
    let file = dir.join("lib.rs");
    let mut content = String::new();
    for line in 1..=90 {
        content.push_str(&format!("// line {line}\n"));
    }
    fs::write(&file, content).unwrap();

    let target = format!("{}:1-90", file.display());
    let out = srcwalk()
        .args(["context", &target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "context long range should succeed, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(stdout.contains("## Source Evidence"), "{stdout}");
    assert!(
        stdout.contains("shown: 1-80; omitted lines after 80: 10"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show") && stdout.contains(":1-90 -C 20"),
        "omitted source evidence should keep exact show next read:\n{stdout}"
    );
}

#[test]
fn context_bare_c_named_struct_resolves_body() {
    let dir = temp_dir("context_c_named_struct");
    fs::write(
        dir.join("core.h"),
        r#"
typedef struct ngx_http_core_loc_conf_s  ngx_http_core_loc_conf_t;

struct ngx_http_core_loc_conf_s {
    int value;
};
"#,
    )
    .unwrap();

    let out = srcwalk()
        .args(["context", "ngx_http_core_loc_conf_s", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "bare C struct context should resolve, stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("core.h:4-6"),
        "context should resolve the named struct body range, got:\n{stdout}"
    );
}

#[test]
fn context_ambiguous_symbol_does_not_emit_trace_routes() {
    let dir = temp_dir("context_ambiguous_no_trace_routes");
    fs::create_dir_all(dir.join("a")).unwrap();
    fs::create_dir_all(dir.join("b")).unwrap();
    fs::write(dir.join("a/lib.rs"), "fn same() {}\n").unwrap();
    fs::write(dir.join("b/lib.rs"), "fn same() {}\n").unwrap();

    let out = srcwalk()
        .args(["context", "same", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "ambiguous context should fail");
    assert!(stderr.contains("ambiguous symbol target"), "{stderr}");
    assert_no_trace_routes(&stderr);
}

#[test]
fn context_unresolved_symbol_does_not_emit_trace_routes() {
    let dir = temp_dir("context_unresolved_no_trace_routes");
    fs::write(dir.join("lib.rs"), "fn real() {}\n").unwrap();

    let out = srcwalk()
        .args(["context", "missing", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "unresolved context should fail");
    assert!(stderr.contains("no matches for \"missing\""), "{stderr}");
    assert_no_trace_routes(&stderr);
}

#[test]
fn context_text_document_and_unsupported_targets_do_not_emit_trace_routes() {
    let dir = temp_dir("context_non_code_no_trace_routes");
    let doc = dir.join("guide.md");
    let text = dir.join("notes.txt");
    let css = dir.join("style.css");
    fs::write(&doc, "# Guide\n").unwrap();
    fs::write(&text, "plain text\n").unwrap();
    fs::write(&css, "a { color: red; }\n").unwrap();

    for path in [&doc, &text] {
        let target = format!("{}:1", path.display());
        let out = srcwalk()
            .args(["context", &target, "--scope"])
            .arg(&dir)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "non-code context should degrade:\n{stdout}"
        );
        assert!(stdout.contains("(not a code file)"), "{stdout}");
        assert_no_trace_routes(&stdout);
    }

    let css_target = format!("{}:1", css.display());
    let out = srcwalk()
        .args(["context", &css_target, "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "unsupported CSS context should fail");
    assert!(stderr.contains("Css is not supported"), "{stderr}");
    assert_no_trace_routes(&stderr);
}
