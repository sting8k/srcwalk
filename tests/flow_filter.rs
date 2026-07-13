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
    assert!(stdout.contains("### Callees"), "{stdout}");
    assert!(stdout.contains("### Callers"), "{stdout}");
    assert!(stdout.contains("> Next: srcwalk show"), "{stdout}");
    assert_eq!(
        stdout.matches("> Next: srcwalk show").count(),
        1,
        "context show next action should not be duplicated:\n{stdout}"
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

fn entry() -> i32 { 1 }
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
    assert!(
        stdout.contains("> Next: srcwalk show") && stdout.contains(":1-3 -C 20"),
        "expected exact show next read:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("> Next: srcwalk show").count(),
        1,
        "range fallback show next action should not be duplicated:\n{stdout}"
    );
    assert!(
        !stdout.contains("trace callers 1-3") && !stdout.contains("trace callees 1-3"),
        "range target must not leak into trace tips:\n{stdout}"
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
fn context_renders_bounded_rust_local_structural_links() {
    let dir = temp_dir("context_rust_local_links");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"
struct Request { path: String }

fn open(_: String) {}

fn handle(req: Request) {
    let path = req.path;
    let alias = path;
    open(alias);
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, &file, "handle");
    assert!(
        stdout.contains("### Local structural links"),
        "expected local structural link section:\n{stdout}"
    );
    assert!(stdout.contains("req.path -> path [field_read]"));
    assert!(
        stdout.contains("path -> alias [assignment/alias]"),
        "expected alias predecessor link:\n{stdout}"
    );
    assert!(stdout.contains("alias -> open(alias) [argument_use]"));
    assert!(stdout.contains("confidence: local structural syntax"));
    assert!(stdout.contains("not runtime dataflow"));
    assert_no_def_use_verdict_words(&stdout);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_renders_javascript_and_multiline_call_result_links() {
    let dir = temp_dir("context_js_local_links");
    let js_file = dir.join("lib.js");
    fs::write(
        &js_file,
        r#"
function handle(req) {
  const path = req.path;
  const alias = path;
  open(alias);
}
"#,
    )
    .unwrap();

    let js_stdout = context_output(&dir, &js_file, "handle");
    assert!(js_stdout.contains("req.path -> path [field_read]"));
    assert!(js_stdout.contains("alias -> open(alias) [argument_use]"));

    let rust_file = dir.join("call_result.rs");
    fs::write(
        &rust_file,
        r#"
fn load_config(_: &str) -> String { String::new() }
fn open(_: String) {}

fn handle(path: &str) {
    let config = load_config(
        path,
    );
    open(config);
}
"#,
    )
    .unwrap();

    let rust_stdout = context_output(&dir, &rust_file, "handle");
    assert!(
        rust_stdout.contains("-> config [call_result]"),
        "multiline call result identity must connect to its binding:\n{rust_stdout}"
    );
    assert!(rust_stdout.contains("config -> open(config) [argument_use]"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_abstains_on_ambiguous_local_predecessors() {
    let dir = temp_dir("context_ambiguous_local_links");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"
struct Request { safe: String, user: String }
fn open(_: String) {}

fn handle(req: Request) {
    let path = req.safe;
    let path = req.user;
    open(path);
}
"#,
    )
    .unwrap();

    let stdout = context_output(&dir, &file, "handle");
    assert!(
        !stdout.contains("### Local structural links"),
        "ambiguous predecessor must abstain instead of choosing a chain:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_caps_local_structural_link_rows_with_omitted_count() {
    let dir = temp_dir("context_capped_local_links");
    let file = dir.join("lib.rs");
    let mut source = String::from("struct Request { value: i32 }\n");
    for index in 0..13 {
        source.push_str(&format!("fn sink{index}(_: i32) {{}}\n"));
    }
    source.push_str("fn handle(req: Request) {\n");
    for index in 0..13 {
        source.push_str(&format!(
            "    let value{index} = req.value;\n    sink{index}(value{index});\n"
        ));
    }
    source.push_str("}\n");
    fs::write(&file, source).unwrap();

    let stdout = context_output(&dir, &file, "handle");
    assert!(stdout.contains("### Local structural links"));
    assert!(
        stdout.contains("more local structural links omitted"),
        "expected deterministic omitted count after row cap:\n{stdout}"
    );
    let rendered_rows = stdout
        .lines()
        .filter(|line| line.starts_with("- ") && line.contains("] ") && !line.contains("omitted"))
        .count();
    assert_eq!(
        rendered_rows, 12,
        "local-link rows must respect cap:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_filter_limits_local_links_to_visible_calls() {
    let dir = temp_dir("context_filtered_local_links");
    let file = dir.join("lib.rs");
    fs::write(
        &file,
        r#"
struct Request { first: i32, second: i32 }
fn one(_: i32) {}
fn two(_: i32) {}

fn handle(req: Request) {
    let first = req.first;
    one(first);
    let second = req.second;
    two(second);
}
"#,
    )
    .unwrap();

    let target = format!("{}:handle", file.display());
    let output = srcwalk()
        .args(["context", &target, "--filter", "callee:two", "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let local_section = stdout
        .split("### Local structural links")
        .nth(1)
        .and_then(|rest| rest.split("### Resolved local callees").next())
        .expect("filtered call should retain one local-link section");
    assert!(local_section.contains("req.second -> second [field_read]"));
    assert!(local_section.contains("second -> two(second) [argument_use]"));
    assert!(
        !local_section.contains("req.first") && !local_section.contains("one(first)"),
        "filtered local-link section leaked hidden call evidence:\n{local_section}"
    );
    let direct_section = stdout
        .split("### Direct-call evidence")
        .nth(1)
        .and_then(|rest| rest.split("### Resolved local callees").next())
        .expect("filtered call should retain one direct-call evidence section");
    assert!(
        direct_section.contains("two(second)"),
        "filtered direct-call evidence should include the visible call:\n{direct_section}"
    );
    assert!(
        !direct_section.contains("one(first)"),
        "filtered direct-call evidence leaked hidden call evidence:\n{direct_section}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_direct_call_evidence_is_limited_to_visible_call_rows() {
    let dir = temp_dir("context_capped_direct_calls");
    let file = dir.join("lib.rs");
    let mut source = String::new();
    for index in 0..13 {
        source.push_str(&format!("fn helper{index}(value: i32) {{}}\n"));
    }
    source.push_str("fn handle(value: i32) {\n");
    for index in 0..13 {
        source.push_str(&format!("    helper{index}(value);\n"));
    }
    source.push_str("}\n");
    fs::write(&file, source).unwrap();

    let stdout = context_output(&dir, &file, "handle");
    let direct_section = stdout
        .split("### Direct-call evidence")
        .nth(1)
        .and_then(|rest| rest.split("### Resolved local callees").next())
        .expect("context should render direct-call evidence");
    let rendered_rows = direct_section
        .lines()
        .filter(|line| line.starts_with("- L") && line.contains("helper"))
        .count();
    assert_eq!(
        rendered_rows, 12,
        "direct-call evidence rows must follow the visible call-site cap:\n{direct_section}"
    );
    assert!(
        stdout.contains("... 1 more call sites"),
        "context should still report capped call-site rows:\n{stdout}"
    );
    assert!(
        direct_section.contains("helper0(value)")
            && direct_section.contains("arg0 `value` -> param0 `value`"),
        "expected resolved mapping for a visible direct call:\n{direct_section}"
    );
    assert!(
        !direct_section.contains("helper12(value)")
            && !direct_section.contains("direct-call edges omitted"),
        "direct-call evidence should be built only from visible call rows:\n{direct_section}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_local_links_ignore_calls_beyond_visible_neighborhood() {
    let dir = temp_dir("context_hidden_call_local_links");
    let file = dir.join("lib.rs");
    let mut source = String::from(
        "struct Request { hidden: i32 }\nfn sink(_: i32) {}\nfn handle(req: Request) {\n",
    );
    for index in 0..12 {
        source.push_str(&format!("    sink({index});\n"));
    }
    source.push_str("    let hidden = req.hidden;\n    sink(hidden);\n}\n");
    fs::write(&file, source).unwrap();

    let stdout = context_output(&dir, &file, "handle");
    assert!(stdout.contains("... 1 more call sites"));
    assert!(
        !stdout.contains("### Local structural links")
            && !stdout.contains("req.hidden -> hidden")
            && !stdout.contains("hidden -> sink(hidden)"),
        "hidden call must not influence visible local-link evidence:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_local_links_reject_hidden_call_result_predecessors() {
    let dir = temp_dir("context_hidden_call_result_predecessor");
    let file = dir.join("lib.rs");
    let mut source = String::from(
        "fn build() -> i32 { 1 }\nfn sink(_: i32) {}\nfn handle() {\n    sink(value);\n",
    );
    for index in 0..11 {
        source.push_str(&format!("    sink({index});\n"));
    }
    source.push_str("    let value = build();\n}\n");
    fs::write(&file, source).unwrap();

    let stdout = context_output(&dir, &file, "handle");
    assert!(stdout.contains("... 1 more call sites"));
    assert!(
        !stdout.contains("### Local structural links")
            && !stdout.contains("build() -> value [call_result]"),
        "hidden call result must not enter a visible call predecessor chain:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
