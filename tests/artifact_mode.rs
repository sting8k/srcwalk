use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "srcwalk_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn artifact_flag_includes_minified_js_symbol_evidence() {
    let dir = temp_repo("artifact_js_find");
    fs::write(
        dir.join("bundle.min.js"),
        "function login(){return fetch('/api/login')}function boot(){return login()}",
    )
    .unwrap();

    let default = srcwalk()
        .args(["login", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        !default.status.success(),
        "default source mode should skip minified artifact:\n{}",
        String::from_utf8_lossy(&default.stdout)
    );

    let artifact = srcwalk()
        .args(["login", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "artifact search failed:\n{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    assert!(stdout.contains("bundle.min.js"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
    assert!(stdout.contains("AST cap 25MB"), "{stdout}");
}

#[test]
fn artifact_path_read_labels_artifact_level_output() {
    let dir = temp_repo("artifact_js_read");
    let file = dir.join("bundle.min.js");
    fs::write(
        &file,
        "function login(){return fetch('/api/login')}function boot(){return login()}",
    )
    .unwrap();

    let output = srcwalk()
        .arg(file.to_string_lossy().as_ref())
        .arg("--artifact")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact read failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[artifact outline]"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
    assert!(stdout.contains("AST cap 25MB"), "{stdout}");
    assert!(stdout.contains("drill into artifact symbols"), "{stdout}");
    assert!(
        !stdout.contains("srcwalk deps <file>"),
        "artifact reads should not suggest source deps:\n{stdout}"
    );
}

#[test]
fn artifact_read_surfaces_safe_export_anchors() {
    let dir = temp_repo("artifact_export_anchors");
    let cjs = dir.join("cjs.min.js");
    fs::write(
        &cjs,
        "exports.Widget=function(){};module.exports.Helper=class{};j6.exports.internal=1;",
    )
    .unwrap();

    let cjs_output = srcwalk()
        .arg(cjs.to_string_lossy().as_ref())
        .arg("--artifact")
        .output()
        .unwrap();
    assert!(
        cjs_output.status.success(),
        "cjs artifact read failed:\n{}",
        String::from_utf8_lossy(&cjs_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&cjs_output.stdout);
    assert!(stdout.contains("Artifact anchors:"), "{stdout}");
    assert!(stdout.contains("export Widget"), "{stdout}");
    assert!(stdout.contains("export Helper"), "{stdout}");
    assert!(
        !stdout.contains("internal"),
        "internal object exports should not become artifact anchors:\n{stdout}"
    );

    let umd = dir.join("umd.min.js");
    fs::write(
        &umd,
        "!function(t,e){\"object\"==typeof exports&&\"undefined\"!=typeof module?module.exports=e():\"function\"==typeof define&&define.amd?define(e):(t=\"undefined\"!=typeof globalThis?globalThis:t||self).bootstrap=e()}(this,function(){return {}})",
    )
    .unwrap();
    let umd_output = srcwalk()
        .arg(umd.to_string_lossy().as_ref())
        .arg("--artifact")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&umd_output.stdout);
    assert!(stdout.contains("export bootstrap"), "{stdout}");
    assert!(!stdout.contains("export amd"), "{stdout}");

    let es = dir.join("es.js");
    fs::write(
        &es,
        "export function alpha(){}\nexport { beta as gamma, delta };\n",
    )
    .unwrap();
    let es_output = srcwalk()
        .arg(es.to_string_lossy().as_ref())
        .arg("--artifact")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&es_output.stdout);
    assert!(stdout.contains("export alpha"), "{stdout}");
    assert!(stdout.contains("export gamma"), "{stdout}");
    assert!(stdout.contains("export delta"), "{stdout}");

    let amd = dir.join("amd.js");
    let modules = (0..25)
        .map(|i| format!("ace.define(\"ace/lib/module{i}\",[],function(){{}});"))
        .collect::<Vec<_>>()
        .join("");
    fs::write(
        &amd,
        format!("define(function(){{}});define(123,function(){{}});{modules}"),
    )
    .unwrap();
    let amd_output = srcwalk()
        .arg(amd.to_string_lossy().as_ref())
        .arg("--artifact")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&amd_output.stdout);
    assert!(stdout.contains("mod ace/lib/module0"), "{stdout}");
    assert!(stdout.contains("mod ace/lib/module19"), "{stdout}");
    assert!(
        !stdout.contains("mod 123"),
        "numeric/anonymous modules should not become anchors:\n{stdout}"
    );
    assert!(
        stdout.contains("more artifact anchors omitted"),
        "module anchors should be capped:\n{stdout}"
    );
}

#[test]
fn artifact_find_surfaces_export_anchor_results() {
    let dir = temp_repo("artifact_export_anchor_find");
    fs::write(
        dir.join("bootstrap.min.js"),
        "!function(t,e){\"object\"==typeof exports&&\"undefined\"!=typeof module?module.exports=e():\"function\"==typeof define&&define.amd?define(e):(t=\"undefined\"!=typeof globalThis?globalThis:t||self).bootstrap=e()}(this,function(){return {}})",
    )
    .unwrap();

    let default = srcwalk()
        .args(["find", "bootstrap", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&default.stdout).contains("[anchor] export bootstrap"),
        "source mode should not surface artifact anchors:\n{}",
        String::from_utf8_lossy(&default.stdout)
    );

    let artifact = srcwalk()
        .args(["find", "bootstrap", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "artifact anchor search failed:\n{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    assert!(stdout.contains("[anchor] export bootstrap"), "{stdout}");
    assert!(stdout.contains("bootstrap.min.js:1"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
}

#[test]
fn artifact_find_surfaces_amd_module_anchor_results() {
    let dir = temp_repo("artifact_module_anchor_find");
    fs::write(
        dir.join("ace.min.js"),
        "ace.define(\"ace/lib/lang\",[],function(){});ace.define(\"ace/config\",[],function(){});",
    )
    .unwrap();

    let output = srcwalk()
        .args(["find", "ace/lib/lang", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact module anchor search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[anchor] mod ace/lib/lang"), "{stdout}");
    assert!(stdout.contains("ace.min.js:1"), "{stdout}");
    assert!(!stdout.contains("ace/config"), "{stdout}");
}

#[test]
fn artifact_callers_include_minified_js_bundle_calls() {
    let dir = temp_repo("artifact_js_callers");
    fs::write(
        dir.join("bundle.min.js"),
        "function login(){return fetch('/api/login')}function boot(){return login()}",
    )
    .unwrap();

    let output = srcwalk()
        .args(["callers", "login", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact callers failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("bundle.min.js"), "{stdout}");
    assert!(stdout.contains("boot"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
    assert!(stdout.contains("direct calls"), "{stdout}");
    assert!(stdout.contains("AST cap 25MB"), "{stdout}");
    assert!(stdout.contains("no transitive impact"), "{stdout}");
}

#[test]
fn artifact_callees_include_same_file_minified_js_calls() {
    let dir = temp_repo("artifact_js_callees");
    fs::write(
        dir.join("bundle.min.js"),
        "function login(){return fetch('/api/login')}function boot(){return login()}",
    )
    .unwrap();

    let output = srcwalk()
        .args(["callees", "boot", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact callees failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("login"), "{stdout}");
    assert!(stdout.contains("bundle.min.js"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
    assert!(stdout.contains("same-file calls"), "{stdout}");
    assert!(stdout.contains("AST cap 25MB"), "{stdout}");
    assert!(stdout.contains("no transitive depth"), "{stdout}");
}

#[test]
fn artifact_callees_resolve_nested_umd_functions_by_byte_range() {
    let dir = temp_repo("artifact_nested_umd_callees");
    fs::write(
        dir.join("umd.min.js"),
        "!function(t,e){\"object\"==typeof exports&&\"undefined\"!=typeof module?module.exports=e():\"function\"==typeof define&&define.amd?define(e):(t=\"undefined\"!=typeof globalThis?globalThis:t||self).demo=e()}(this,function(){function login(){return fetch('/api/login')}function boot(){return login()}return {boot:boot}})",
    )
    .unwrap();

    let find = srcwalk()
        .args(["find", "login", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        find.status.success(),
        "nested artifact find failed:\n{}",
        String::from_utf8_lossy(&find.stderr)
    );
    let stdout = String::from_utf8_lossy(&find.stdout);
    assert!(stdout.contains("login"), "{stdout}");
    assert!(
        !stdout.contains("<iife"),
        "nested artifact search should not report the enclosing IIFE as the symbol:\n{stdout}"
    );

    let callees = srcwalk()
        .args(["callees", "boot", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        callees.status.success(),
        "nested artifact callees failed:\n{}",
        String::from_utf8_lossy(&callees.stderr)
    );
    let stdout = String::from_utf8_lossy(&callees.stdout);
    assert!(stdout.contains("login"), "{stdout}");
    assert!(
        !stdout.contains("(unresolved):"),
        "nested artifact callees should use byte ranges, not whole minified lines:\n{stdout}"
    );

    let detailed = srcwalk()
        .args(["callees", "boot", "--artifact", "--detailed", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        detailed.status.success(),
        "nested artifact detailed callees failed:\n{}",
        String::from_utf8_lossy(&detailed.stderr)
    );
    let stdout = String::from_utf8_lossy(&detailed.stdout);
    assert!(stdout.contains("login()"), "{stdout}");
    assert!(!stdout.contains("define"), "{stdout}");
    assert!(!stdout.contains("fetch"), "{stdout}");
}

#[test]
fn artifact_search_centers_long_one_line_usage_snippets() {
    let dir = temp_repo("artifact_centered_snippet");
    fs::write(
        dir.join("app.min.js"),
        format!(
            "var a='{}';function boot(){{return targetCall()}}var b='{}';",
            "x".repeat(800),
            "y".repeat(800)
        ),
    )
    .unwrap();

    let output = srcwalk()
        .args(["find", "targetCall", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("targetCall()"), "{stdout}");
    assert!(stdout.contains('…'), "{stdout}");
    assert!(
        !stdout.contains(&"x".repeat(300)),
        "artifact snippet should not dump long minified prefix:\n{stdout}"
    );
    assert!(
        !stdout.contains(&"y".repeat(300)),
        "artifact snippet should not dump long minified suffix:\n{stdout}"
    );
}

#[test]
fn artifact_relation_depth_is_rejected_until_supported() {
    let output = srcwalk()
        .args(["callers", "login", "--artifact", "--depth", "2"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--artifact callers currently supports direct call sites only"));
}

#[test]
fn artifact_search_skips_non_js_text_even_with_artifact_flag() {
    let dir = temp_repo("artifact_text_export");
    let file = dir.join("droid.strings.txt");
    let mut content = String::new();
    content.push_str(&"runtime filler line\n".repeat(40_000));
    content.push_str("Bun is a fast JavaScript runtime\n");
    fs::write(&file, content).unwrap();

    let default = srcwalk()
        .args(["Bun", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let default_stdout = String::from_utf8_lossy(&default.stdout);
    assert!(
        default_stdout.contains("0 matches") && !default_stdout.contains("droid.strings.txt"),
        "default source mode should keep large text-export guardrail:\n{default_stdout}"
    );

    let artifact = srcwalk()
        .args(["Bun", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!artifact.status.success());
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    let stderr = String::from_utf8_lossy(&artifact.stderr);
    assert!(
        !stdout.contains("droid.strings.txt") && !stderr.contains("droid.strings.txt"),
        "artifact search should stay scoped to JS/TS artifacts:\nstdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn artifact_search_enters_dist_dirs_when_explicitly_enabled() {
    let dir = temp_repo("artifact_dist_dir");
    let dist = dir.join("dist");
    fs::create_dir_all(&dist).unwrap();
    fs::write(
        dist.join("app.min.js"),
        "function distOnlyTarget(){return 1}function boot(){return distOnlyTarget()}",
    )
    .unwrap();

    let default = srcwalk()
        .args(["distOnlyTarget", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let default_stdout = String::from_utf8_lossy(&default.stdout);
    assert!(
        default_stdout.contains("0 matches") || !default.status.success(),
        "default source mode should not enter dist artifacts:\n{default_stdout}"
    );

    let artifact = srcwalk()
        .args(["distOnlyTarget", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "artifact search failed:\n{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    assert!(stdout.contains("dist/app.min.js"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");
}

#[test]
fn artifact_search_includes_large_minified_js_under_artifact_cap() {
    let dir = temp_repo("artifact_large_js");
    let file = dir.join("large.min.js");
    let mut content =
        String::from("function liveLargeTarget(){return helper()}function helper(){return 1}");
    content.push_str("/*");
    content.push_str(&"x".repeat(600_000));
    content.push_str("*/");
    fs::write(&file, content).unwrap();

    let artifact = srcwalk()
        .args(["liveLargeTarget", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "large artifact search failed:\n{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let stdout = String::from_utf8_lossy(&artifact.stdout);
    assert!(stdout.contains("large.min.js"), "{stdout}");
    assert!(stdout.contains("Artifact mode:"), "{stdout}");

    let callees = srcwalk()
        .args(["callees", "liveLargeTarget", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        callees.status.success(),
        "large artifact callees failed:\n{}",
        String::from_utf8_lossy(&callees.stderr)
    );
    let callees_stdout = String::from_utf8_lossy(&callees.stdout);
    assert!(callees_stdout.contains("helper"), "{callees_stdout}");
    assert!(
        callees_stdout.contains("Artifact mode:"),
        "{callees_stdout}"
    );
}

#[test]
fn artifact_search_skips_raw_binary_even_with_text_extension() {
    let dir = temp_repo("artifact_text_binary_skip");
    fs::write(
        dir.join("binary.strings.txt"),
        b"hello\0Bun hidden in binary sample\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["Bun", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no matches for"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn artifact_symbol_search_does_not_return_source_definitions() {
    let dir = temp_repo("artifact_no_source_defs");
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("cache.rs"), "pub struct OutlineCache;\n").unwrap();

    let output = srcwalk()
        .args(["find", "OutlineCache", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("src/cache.rs") && !stdout.contains("[struct] OutlineCache"),
        "artifact definition search should not return source definitions:\n{stdout}"
    );
}

#[test]
fn artifact_name_glob_search_includes_synthetic_export_anchors() {
    let dir = temp_repo("artifact_anchor_glob_find");
    let dist = dir.join("dist");
    fs::create_dir_all(&dist).unwrap();
    fs::write(
        dist.join("app.min.js"),
        "module.exports.MyBundle=function(){};\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["find", "My*", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "artifact glob anchor search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[anchor] export MyBundle"), "{stdout}");
    assert!(stdout.contains("dist/app.min.js:1"), "{stdout}");
}

#[test]
fn artifact_mode_reenables_only_artifact_output_dirs() {
    let dir = temp_repo("artifact_dir_whitelist");
    let dist = dir.join("dist");
    let dependency = dir.join("node_modules/pkg");
    fs::create_dir_all(&dist).unwrap();
    fs::create_dir_all(&dependency).unwrap();
    fs::write(
        dist.join("app.min.js"),
        "module.exports.AppBundle=function(){};\n",
    )
    .unwrap();
    fs::write(
        dependency.join("index.js"),
        "module.exports.DependencyBundle=function(){};\n",
    )
    .unwrap();

    let map = srcwalk()
        .args(["map", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        map.status.success(),
        "artifact map failed:\n{}",
        String::from_utf8_lossy(&map.stderr)
    );
    let stdout = String::from_utf8_lossy(&map.stdout);
    assert!(stdout.contains("dist/"), "{stdout}");
    assert!(stdout.contains("export AppBundle"), "{stdout}");
    assert!(
        !stdout.contains("node_modules") && !stdout.contains("DependencyBundle"),
        "artifact map should not re-enable dependency trees:\n{stdout}"
    );

    let search = srcwalk()
        .args(["find", "DependencyBundle", "--artifact", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(
        !stdout.contains("node_modules") && !stdout.contains("DependencyBundle"),
        "artifact search should not re-enable dependency trees:\n{stdout}"
    );
}
