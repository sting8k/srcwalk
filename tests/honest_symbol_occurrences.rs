use std::fs;
use std::path::{Path, PathBuf};
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
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn run_discover(dir: &Path, args: &[&str]) -> String {
    let output = srcwalk()
        .current_dir(dir)
        .args(["discover"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .replace('\\', "/")
}

#[test]
fn repeated_definitions_label_name_occurrences_and_keep_ambiguity_on_later_pages() {
    let dir = temp_repo("honest_symbol_occurrences");
    write_file(
        &dir.join("repeated.rs"),
        "pub mod left { pub fn token() {} }\n\
         pub mod right { pub fn token() {} }\n\
         pub fn first() { left::token(); }\n\
         pub fn second() { right::token(); }\n\
         // token in a comment\n",
    );

    let full = run_discover(&dir, &["token", "--scope", "repeated.rs"]);
    assert!(
        full.contains("5 matches (2 definitions, 2 name occurrences, 1 in comments)"),
        "candidate coverage changed:\n{full}"
    );
    assert_eq!(
        full.matches("text-matched name occurrences are not binding-resolved")
            .count(),
        1,
        "ambiguity caveat must render once:\n{full}"
    );
    assert!(
        full.contains("source: ast · kind: definition · confidence: structural syntax"),
        "definition provenance changed:\n{full}"
    );
    assert!(
        full.contains("[2 name occurrences]")
            && full.contains("source: text · kind: name occurrence · confidence: text evidence"),
        "word-boundary hits must be honest text-backed candidates:\n{full}"
    );
    assert!(
        full.contains("[comment occurrence]")
            && full.contains("source: text · kind: comment occurrence · confidence: text evidence"),
        "comment occurrence provenance must stay explicit:\n{full}"
    );
    assert!(!full.contains("kind: usage"), "{full}");

    let later_page = run_discover(
        &dir,
        &[
            "token",
            "--scope",
            "repeated.rs",
            "--limit",
            "1",
            "--offset",
            "2",
        ],
    );
    assert!(
        later_page.contains("repeated.rs:3 [name occurrence]"),
        "{later_page}"
    );
    assert_eq!(
        later_page
            .matches("text-matched name occurrences are not binding-resolved")
            .count(),
        1,
        "ambiguity must survive pagination:\n{later_page}"
    );
    assert!(
        later_page.contains("Continue with --offset 3 --limit 1"),
        "pagination guidance changed:\n{later_page}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn one_definition_uses_honest_labels_without_ambiguity_wording() {
    let dir = temp_repo("single_symbol_occurrence");
    write_file(
        &dir.join("single.rs"),
        "pub fn single() {}\npub fn call() { single(); }\n",
    );

    let stdout = run_discover(&dir, &["single", "--scope", "single.rs"]);
    assert!(
        stdout.contains("1 definitions, 1 name occurrences"),
        "{stdout}"
    );
    assert!(stdout.contains("[name occurrence]"), "{stdout}");
    assert!(
        !stdout.contains("definition candidates share this name"),
        "one definition must not get ambiguity wording:\n{stdout}"
    );

    let text = run_discover(&dir, &["single", "--as", "text", "--scope", "single.rs"]);
    assert!(
        text.contains("[2 text matches]")
            && text.contains("source: text · kind: text · confidence: text evidence"),
        "literal text discovery must remain text evidence:\n{text}"
    );
    assert!(!text.contains("name occurrence"), "{text}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn artifact_symbol_search_uses_the_shared_occurrence_contract() {
    let dir = temp_repo("artifact_symbol_occurrences");
    write_file(
        &dir.join("bundle.min.js"),
        "function token() {}\n\
         function wrapper() { token(); }\n\
         function token() {}\n\
         function wrapper2() { token(); }\n",
    );

    let stdout = run_discover(&dir, &["token", "--scope", "bundle.min.js", "--artifact"]);
    assert!(
        stdout.contains("4 matches (2 definitions, 2 name occurrences)"),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("text-matched name occurrences are not binding-resolved")
            .count(),
        1,
        "{stdout}"
    );
    assert!(
        stdout.contains("source: artifact · kind: definition · confidence: artifact-level")
            && stdout
                .contains("source: artifact · kind: name occurrence · confidence: artifact-level"),
        "artifact provenance must not be upgraded to source semantics:\n{stdout}"
    );
    assert!(!stdout.contains("kind: usage"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn heuristic_fallback_stays_text_evidence() {
    let dir = temp_repo("heuristic_fallback_evidence");
    write_file(&dir.join("fallback.txt"), "function token() {}\ntoken();\n");

    let stdout = run_discover(&dir, &["token", "--scope", "fallback.txt"]);
    assert!(stdout.contains("[2 text matches]"), "{stdout}");
    assert!(
        stdout.contains("source: text · kind: text · confidence: text evidence"),
        "{stdout}"
    );
    assert!(!stdout.contains("source: ast"), "{stdout}");
    assert!(!stdout.contains("kind: definition"), "{stdout}");

    write_file(
        &dir.join("Dockerfile"),
        "function token() {}\nfunction token() {}\ntoken\nother\n",
    );
    let dockerfile = run_discover(&dir, &["token", "--scope", "Dockerfile"]);
    assert!(!dockerfile.contains("kind: definition"), "{dockerfile}");
    assert!(
        !dockerfile.contains("definition candidates share this name"),
        "heuristic candidates must not create a repeated-definition caveat:\n{dockerfile}"
    );
    let dockerfile_batch = run_discover(&dir, &["token,other", "--scope", "Dockerfile"]);
    assert!(
        !dockerfile_batch.contains("definition candidates share this name"),
        "batch heuristic candidates must not create a repeated-definition caveat:\n{dockerfile_batch}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_search_keeps_repeated_definition_ambiguity() {
    let dir = temp_repo("multi_symbol_ambiguity");
    write_file(
        &dir.join("symbols.rs"),
        "pub mod left { pub fn token() {} }\n\
         pub mod right { pub fn token() {} }\n\
         pub fn call() { left::token(); }\n\
         pub fn other() {}\n",
    );

    let stdout = run_discover(&dir, &["token,other", "--scope", "symbols.rs"]);
    assert_eq!(
        stdout
            .matches("text-matched name occurrences are not binding-resolved")
            .count(),
        1,
        "multi-symbol output must retain the token ambiguity caveat:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expanded_discover_compacts_rendered_source_occurrences_without_hiding_recursive_calls() {
    let dir = temp_repo("expanded_compact_rendered_occurrences");
    write_file(
        &dir.join("recur.rs"),
        "pub fn recur(n: u32) {\n    if n > 0 { recur(n - 1); }\n}\npub fn outside() { recur(1); }\n",
    );

    let stdout = run_discover(&dir, &["recur", "--scope", "recur.rs", "--expand=1"]);

    assert!(
        stdout.contains("3 matches (1 definitions, 2 name occurrences)"),
        "counts must not change:\n{stdout}"
    );
    assert!(
        stdout.contains("   2 │     if n > 0 { recur(n - 1); }"),
        "recursive call source must still be visible in expanded definition:\n{stdout}"
    );
    assert!(
        stdout.contains("- :2      [name occurrence · source shown above] ← recur"),
        "recursive occurrence anchor must remain, compacted only because source is shown above:\n{stdout}"
    );
    assert!(
        !stdout.contains("inside shown definitions"),
        "old omission wording must not return:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compacted_single_occurrence_does_not_repeat_context_or_source_expansion() {
    let dir = temp_repo("expanded_compact_single_no_repeat");
    let mut source = String::from("pub fn recur(n: u32) {\n    if n > 0 { recur(n - 1); }\n");
    for i in 1..=55 {
        source.push_str(&format!("    let filler_{i:02} = {i};\n"));
    }
    source.push_str("}\n");
    write_file(&dir.join("recur.rs"), &source);

    let stdout = run_discover(&dir, &["recur", "--scope", "recur.rs", "--expand=2"]);

    assert!(
        stdout.contains("→ [2]   [name occurrence · source shown above]"),
        "fixture must compact the recursive occurrence:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("   2 │     if n > 0 { recur(n - 1); }")
            .count(),
        1,
        "compacted occurrence must not repeat its source expansion:\n{stdout}"
    );
    assert!(
        !stdout.contains("\n→ [1-58]"),
        "compacted occurrence must not repeat outline context:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expanded_discover_keeps_unshown_external_occurrence_snippet() {
    let dir = temp_repo("expanded_unshown_external_occurrence");
    let mut source = String::from("pub fn target() {\n    let value = 1;\n}\n");
    for i in 0..260 {
        source.push_str(&format!(
            "// filler {i:03} to force def-range expansion instead of full-file expansion\n"
        ));
    }
    source.push_str("pub fn outside() { target(); }\n");
    write_file(&dir.join("large.rs"), &source);

    let stdout = run_discover(&dir, &["target", "--scope", "large.rs", "--expand=1"]);

    assert!(
        stdout.contains("pub fn outside() { target(); }"),
        "unshown external occurrence must keep its source snippet:\n{stdout}"
    );
    assert!(
        !stdout.contains("[name occurrence · source shown above]"),
        "unshown occurrence must not be compacted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expanded_discover_keeps_smart_truncated_occurrence_snippet() {
    let dir = temp_repo("expanded_smart_truncated_occurrence");
    let mut source = String::from("pub fn needle() {\n");
    for i in 0..90 {
        source.push_str(&format!("    let filler_{i:03} = {i};\n"));
    }
    source.push_str("    let captured = needle;\n");
    for i in 90..130 {
        source.push_str(&format!("    let tail_{i:03} = {i};\n"));
    }
    source.push_str("}\n");
    write_file(&dir.join("long.rs"), &source);

    let stdout = run_discover(&dir, &["needle", "--scope", "long.rs", "--expand=1"]);

    assert!(
        stdout.contains("> Caveat: expanded source truncated."),
        "fixture should exercise smart truncation:\n{stdout}"
    );
    assert!(
        stdout.contains("let captured = needle;"),
        "occurrence line stripped from expanded source must still render as hit evidence:\n{stdout}"
    );
    assert!(
        !stdout.contains("[name occurrence · source shown above]"),
        "stripped occurrence line must not be compacted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expanded_text_search_does_not_use_name_occurrence_compaction() {
    let dir = temp_repo("expanded_text_no_name_compaction");
    write_file(&dir.join("text.rs"), "pub fn token() {\n    token();\n}\n");

    let stdout = run_discover(
        &dir,
        &["token", "--as", "text", "--scope", "text.rs", "--expand=1"],
    );

    assert!(
        stdout.contains("[2 text matches]") || stdout.contains("[3 text matches]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("token();") && !stdout.contains("[name occurrence · source shown above]"),
        "literal text search must keep text evidence, not symbol compaction:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
