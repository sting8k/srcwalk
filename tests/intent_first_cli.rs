use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OWNER_LINK_ZERO_EDGE: &str = "No direct name-level call evidence among hit owners. Dynamic dispatch, DI, callbacks, and protocol wiring are not ruled out.";

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
fn json_flag_is_removed_from_public_cli() {
    let help = srcwalk()
        .args(["trace", "callers", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(!stdout.contains("--json"), "{stdout}");

    let output = srcwalk()
        .args(["discover", "anything", "--json"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "discover --json unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--json'"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn discover_symbol_space_separated_scopes_names_repeated_flags() {
    let dir = temp_repo("discover_symbol_space_separated_scopes");
    fs::create_dir_all(dir.join("source root")).unwrap();
    fs::create_dir_all(dir.join("test root")).unwrap();
    fs::create_dir_all(dir.join("bench root")).unwrap();
    fs::write(
        dir.join("source root/lib.rs"),
        "pub fn shared_target() {}\n",
    )
    .unwrap();
    fs::write(dir.join("test root/lib.rs"), "pub fn shared_target() {}\n").unwrap();

    let output = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "shared_target",
            "--as",
            "symbol",
            "--scope",
            "source root",
            "test root",
            "bench root",
            "--limit",
            "2",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument 'test root'")
            && stderr.contains(
                "hint: `discover --as symbol` requires `--scope` before each search root:"
            )
            && stderr.contains(
                "srcwalk discover shared_target --as symbol --scope 'source root' --scope 'test root' --scope 'bench root' --limit 2"
            ),
        "expected a dynamic repeated-scope correction:\n{stderr}"
    );

    let corrected = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "shared_target",
            "--as",
            "symbol",
            "--scope",
            "source root",
            "--scope",
            "test root",
            "--scope",
            "bench root",
            "--limit",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "corrected command failed:\n{}",
        String::from_utf8_lossy(&corrected.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_scope_hint_abstains_outside_explicit_symbol_roots() {
    let dir = temp_repo("discover_scope_hint_abstains");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();

    for args in [
        vec![
            "discover",
            "shared_target",
            "--as",
            "text",
            "--scope",
            "src",
            "tests",
        ],
        vec![
            "discover",
            "shared_target",
            "--as",
            "symbol",
            "--scope",
            "src",
            "missing",
        ],
    ] {
        let output = srcwalk().current_dir(&dir).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("unexpected argument"), "{stderr}");
        assert!(
            !stderr.contains("requires `--scope` before each search root"),
            "unexpected scope correction:\n{stderr}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_space_separated_targets_names_the_comma_form() {
    let dir = temp_repo("show_space_separated_targets");
    fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    fs::write(dir.join("b.rs"), "fn b() {}\n").unwrap();

    let output = srcwalk()
        .current_dir(&dir)
        .args(["show", "a.rs:1", "b.rs:1"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument 'b.rs:1'")
            && stderr.contains("pass multiple read targets as one comma-separated argument")
            && stderr.contains("srcwalk show 'a.rs:1,b.rs:1'"),
        "expected clap error plus corrective hint:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_space_separated_target_groups_expand_ranges_and_keep_options() {
    let dir = temp_repo("show_space_separated_target_groups");
    let lines = (1..=10)
        .map(|line| format!("fn line_{line}() {{}}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join("a.rs"), format!("{lines}\n")).unwrap();
    fs::write(dir.join("b.rs"), format!("{lines}\n")).unwrap();

    let output = srcwalk()
        .current_dir(&dir)
        .args(["show", "a.rs:1-2,3-4", "b.rs:5-6,7-8", "-C", "2"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument 'b.rs:5-6,7-8'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("srcwalk show 'a.rs:1-2,a.rs:3-4,b.rs:5-6,b.rs:7-8' -C 2"),
        "expected a complete runnable correction:\n{stderr}"
    );

    let corrected = srcwalk()
        .current_dir(&dir)
        .args(["show", "a.rs:1-2,a.rs:3-4,b.rs:5-6,b.rs:7-8", "-C", "2"])
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "corrected command failed:\n{}",
        String::from_utf8_lossy(&corrected.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_space_separated_targets_do_not_suggest_an_over_limit_command() {
    let mut args = vec!["show".to_string()];
    args.extend((1..=9).map(|index| format!("file{index}.rs:1")));
    let output = srcwalk().args(args).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");

    let reversed = srcwalk()
        .args(["show", "file1.rs:9-2", "file2.rs:1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&reversed.stderr);
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn show_space_separated_drive_path_groups_keep_the_drive_prefix() {
    let output = srcwalk()
        .args(["show", r"C:\src\a.rs:1-2,3-4", r"D:\src\b.rs:5-6,7-8"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(r"C:\src\a.rs:1-2,C:\src\a.rs:3-4")
            && stderr.contains(r"D:\src\b.rs:5-6,D:\src\b.rs:7-8"),
        "drive-qualified targets were not preserved:\n{stderr}"
    );
}

#[test]
fn show_context_lines_expands_focused_line_window() {
    let dir = temp_repo("show_context_lines");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\nfn fourth() {}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:3", "-C", "1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show -C failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 │ fn second() {}"), "{stdout}");
    assert!(stdout.contains("►    3 │ fn third() {}"), "{stdout}");
    assert!(stdout.contains("4 │ fn fourth() {}"), "{stdout}");
    assert!(
        !stdout.contains("1: fn first() {}"),
        "context should be exactly one line around focus:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_numeric_range_renders_structural_source_frame() {
    let dir = temp_repo("show_source_frame");
    fs::write(
        dir.join("lib.rs"),
        "fn outer() {\n    let a = 1;\n    let b = 2;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:2", "-C", "1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .contains("> Source frame: requested 2; displayed 1-3; within fn outer 1-4; partial."),
        "expected structural source frame:\n{stdout}"
    );
    assert!(stdout.contains("►    2 │     let a = 1;"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_gap_range_renders_bounds_only_source_frame() {
    let dir = temp_repo("show_gap_source_frame");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:4", "-C", "1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("> Source frame: requested 4; displayed 3-5; outside any function span."),
        "expected gap source frame:\n{stdout}"
    );
    assert!(
        !stdout.contains("within fn"),
        "gap frame must not name a function:\n{stdout}"
    );
    assert!(
        !stdout.contains("complete"),
        "gap frame must not claim completeness:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_cross_boundary_range_renders_non_enclosed_source_frame() {
    let dir = temp_repo("show_cross_boundary_source_frame");
    fs::write(
        dir.join("lib.rs"),
        "const VALUE: i32 = 1;\n\nfn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:2-9", "-C", "0", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("> Source frame: requested 2-9; displayed 2-9; spans 2 structural functions; not enclosed."),
        "expected non-enclosed source frame:\n{stdout}"
    );
    assert!(
        !stdout.contains("within fn first"),
        "non-enclosed frame must not name crossed functions:\n{stdout}"
    );
    assert!(
        !stdout.contains("complete"),
        "non-enclosed frame must not claim completeness:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_multi_section_renders_source_frame_per_block() {
    let dir = temp_repo("show_multi_section_source_frames");
    fs::write(
        dir.join("lib.rs"),
        "const VALUE: i32 = 1;\n\nfn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs", "--section", "2-2,4-6,7-9", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "show --section failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches("> Source frame:").count(),
        3,
        "expected one source frame per rendered block:\n{stdout}"
    );
    assert!(
        stdout.contains("> Source frame: requested 2; displayed 2; outside any function span."),
        "expected gap frame:\n{stdout}"
    );
    assert!(
        stdout.contains("> Source frame: requested 4-6; displayed 4-6; spans 1 structural function; not enclosed."),
        "expected non-enclosed frame:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "> Source frame: requested 7-9; displayed 7-9; within fn second 7-9; complete."
        ),
        "expected enclosed frame:\n{stdout}"
    );
    for word in [
        "calls",
        "returns",
        "depends",
        "runtime",
        "owns",
        "implements",
        "invokes",
        "because",
    ] {
        assert!(
            !stdout.to_ascii_lowercase().contains(word),
            "source frames must not contain behavior word {word}:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_routes_same_file_range_shorthand_to_section_reader() {
    let dir = temp_repo("show_same_file_shorthand");
    fs::write(
        dir.join("lib.rs"),
        "const VALUE: i32 = 1;\n\nfn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n}\n",
    )
    .unwrap();

    let inline = srcwalk()
        .args(["show", "lib.rs:2-2,4-6,7-9", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let explicit = srcwalk()
        .args(["show", "lib.rs", "--section", "2-2,4-6,7-9", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        inline.status.success(),
        "same-file shorthand should route to --section:\n{}",
        String::from_utf8_lossy(&inline.stderr)
    );
    assert!(
        explicit.status.success(),
        "explicit --section failed:\n{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    let inline_stdout = String::from_utf8_lossy(&inline.stdout);
    let explicit_stdout = String::from_utf8_lossy(&explicit.stdout);
    assert_eq!(inline_stdout, explicit_stdout);
    assert_eq!(
        inline_stdout.matches("> Source frame:").count(),
        3,
        "expected framed multi-section output:\n{inline_stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&inline.stderr).contains("same-file comma ranges"),
        "clean shorthand must not hit the old rejection"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_same_file_range_shorthand_accepts_paths_with_spaces() {
    let dir = temp_repo("show_same_file_shorthand_space");
    fs::write(
        dir.join("space file.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\n",
    )
    .unwrap();

    let inline = srcwalk()
        .args(["show", "space file.rs:1,3", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let explicit = srcwalk()
        .args(["show", "space file.rs", "--section", "1,3", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        inline.status.success(),
        "space-path shorthand should route to --section:\n{}",
        String::from_utf8_lossy(&inline.stderr)
    );
    assert!(
        explicit.status.success(),
        "explicit space-path --section failed:\n{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&inline.stdout),
        String::from_utf8_lossy(&explicit.stdout)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_ambiguous_same_file_range_shorthand_still_errors() {
    let dir = temp_repo("show_ambiguous_same_file_shorthand");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\n",
    )
    .unwrap();
    fs::write(dir.join("other.rs"), "fn other() {}\n").unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:1,3,other.rs:1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "ambiguous same-file shorthand should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambiguous same-file comma range shorthand"),
        "expected ambiguous shorthand guidance:\n{stderr}"
    );
    assert!(
        stderr.contains("srcwalk show lib.rs --section 1,3"),
        "expected --section guidance:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn context_same_file_range_shorthand_remains_out_of_scope() {
    let dir = temp_repo("context_same_file_shorthand");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["context", "lib.rs:1,3", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "context same-file shorthand should remain rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "multi-target context requires comma-separated exact path:symbol or path:range targets"
        ),
        "expected context comma rejection:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_reads_strict_comma_separated_locations() {
    let dir = temp_repo("show_multi");
    fs::write(
        dir.join("lib.rs"),
        "fn first() {}\nfn second() {}\nfn third() {}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:1,lib.rs:3", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "multi-show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Show: 2 locations"), "{stdout}");
    assert!(stdout.contains("## Target: lib.rs:1"), "{stdout}");
    assert!(stdout.contains("## Target: lib.rs:3"), "{stdout}");
    assert!(stdout.contains("►    1 │ fn first() {}"), "{stdout}");
    assert!(stdout.contains("►    3 │ fn third() {}"), "{stdout}");
    assert!(stdout.contains("\n---\n"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn show_preserves_multi_file_comma_locations() {
    let dir = temp_repo("show_multi_file_comma");
    fs::write(dir.join("lib.rs"), "fn first() {}\n").unwrap();
    fs::write(dir.join("other.rs"), "fn other() {}\n").unwrap();

    let output = srcwalk()
        .args(["show", "lib.rs:1,other.rs:1", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "multi-file comma show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Show: 2 locations"), "{stdout}");
    assert!(stdout.contains("## Target: lib.rs:1"), "{stdout}");
    assert!(stdout.contains("## Target: other.rs:1"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batched_show_respects_one_global_budget() {
    let dir = temp_repo("show_multi_budget");
    let body = (1..=120)
        .map(|line| format!("fallback evidence line {line:03} with repeated payload"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("a.txt"), &body).unwrap();
    fs::write(dir.join("b.txt"), body).unwrap();

    for budget in [80usize, 500] {
        let output = srcwalk()
            .args(["show", "a.txt,b.txt", "--scope"])
            .arg(&dir)
            .args(["--budget", &budget.to_string()])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "budgeted multi-show failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("# Show: 2 locations"), "{stdout}");
        assert!(
            stdout.contains("## Target: a.txt") && stdout.contains("## Target: b.txt"),
            "budget {budget} starved one target:\n{stdout}"
        );
        assert!(
            stdout.len().div_ceil(4) <= budget,
            "multi-show exceeded budget {budget}: ~{} tokens\n{stdout}",
            stdout.len().div_ceil(4)
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batched_show_reuses_unused_target_slack_before_truncating() {
    let dir = temp_repo("show_multi_slack");
    fs::write(dir.join("small.rs"), "fn small() {}\n").unwrap();
    let large = (1..=60)
        .map(|line| format!("let value_{line:03} = {line};"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("large.rs"), large).unwrap();

    let output = srcwalk()
        .args(["show", "small.rs:1,large.rs:1-60", "--scope"])
        .arg(&dir)
        .args(["--budget", "500"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "budgeted multi-show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Target: small.rs:1"), "{stdout}");
    assert!(stdout.contains("## Target: large.rs:1-60"), "{stdout}");
    assert!(
        stdout.contains("60  let value_060 = 60;"),
        "large exact range should use slack left by small target before truncating:\n{stdout}"
    );
    assert!(
        !stdout.contains("truncated to fit --budget"),
        "packet fit the global budget and should not be hard-truncated:\n{stdout}"
    );
    assert!(
        stdout.len().div_ceil(4) <= 500,
        "multi-show exceeded budget: ~{} tokens\n{stdout}",
        stdout.len().div_ceil(4)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batched_show_tight_budget_preserves_each_target_footer() {
    let dir = temp_repo("show_multi_footer_budget");
    let body = (1..=120)
        .map(|line| format!("fallback evidence line {line:03} with repeated payload"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(dir.join("a.txt"), &body).unwrap();
    fs::write(dir.join("b.txt"), body).unwrap();

    let output = srcwalk()
        .args(["show", "a.txt,b.txt", "--scope"])
        .arg(&dir)
        .args(["--budget", "500"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "budgeted multi-show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Target: a.txt"), "{stdout}");
    assert!(stdout.contains("## Target: b.txt"), "{stdout}");
    assert!(
        stdout.matches("> Next:").count() >= 4,
        "each target should retain its two read-drilldown footer hints:\n{stdout}"
    );
    assert!(
        stdout.len().div_ceil(4) <= 500,
        "multi-show exceeded budget: ~{} tokens\n{stdout}",
        stdout.len().div_ceil(4)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batched_show_redistributes_slack_when_batch_is_over_budget() {
    let dir = temp_repo("show_multi_over_budget_slack");
    fs::write(dir.join("small.rs"), "fn small() {}\n").unwrap();
    let large = format!(
        "fn large() {{\n{}\n}}\n",
        (1..=100)
            .map(|line| format!("    let value_{line:03} = {line};"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    fs::write(dir.join("large.rs"), large).unwrap();

    let output = srcwalk()
        .args(["show", "small.rs:1,large.rs:1-102", "--scope"])
        .arg(&dir)
        .args(["--budget", "800"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "budgeted multi-show failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Target: small.rs:1"), "{stdout}");
    assert!(stdout.contains("## Target: large.rs:1-102"), "{stdout}");
    assert!(
        stdout.contains("value_050"),
        "large target should receive slack from the small target instead of equal-split outline starvation:\n{stdout}"
    );
    assert!(
        stdout.len().div_ceil(4) <= 800,
        "multi-show exceeded budget: ~{} tokens\n{stdout}",
        stdout.len().div_ceil(4)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_as_text_forces_content_search_for_path_like_query() {
    let dir = temp_repo("discover_text_path_like");
    fs::write(dir.join("notes.txt"), "docs/missing.md is mentioned here\n").unwrap();

    let output = srcwalk()
        .args(["discover", "docs/missing.md", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --as text failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("notes.txt:1"), "{stdout}");
    assert!(
        stdout.contains("docs/missing.md is mentioned here"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_accepts_file_and_glob_scope_for_search_modes() {
    let dir = temp_repo("discover_scope_specs");
    fs::create_dir_all(dir.join("src/nested")).unwrap();
    fs::write(dir.join("src/one.rs"), "fn target() {}\nfn helper() {}\n").unwrap();
    fs::write(dir.join("src/two.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("src/nested/three.rs"), "fn target() {}\n").unwrap();

    let file_output = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--scope", "src/one.rs"])
        .output()
        .unwrap();
    assert!(
        file_output.status.success(),
        "file-scope symbol search failed:\n{}",
        String::from_utf8_lossy(&file_output.stderr)
    );
    let file_stdout = String::from_utf8_lossy(&file_output.stdout);
    assert!(file_stdout.contains("src/one.rs:1-1"), "{file_stdout}");
    assert!(
        !file_stdout.contains("src/two.rs"),
        "file scope should not scan siblings:\n{file_stdout}"
    );

    let glob_output = srcwalk()
        .current_dir(&dir)
        .args(["discover", "target", "--as", "text", "--scope", "src/*.rs"])
        .output()
        .unwrap();
    assert!(
        glob_output.status.success(),
        "glob-scope text search failed:\n{}",
        String::from_utf8_lossy(&glob_output.stderr)
    );
    let glob_stdout = String::from_utf8_lossy(&glob_output.stdout);
    assert!(glob_stdout.contains("src/one.rs:1"), "{glob_stdout}");
    assert!(glob_stdout.contains("src/two.rs:1"), "{glob_stdout}");
    assert!(
        !glob_stdout.contains("nested/three.rs"),
        "src/*.rs should not include nested files:\n{glob_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_all_reports_same_file_cooccurrence_caveat() {
    let dir = temp_repo("discover_match_all");
    fs::write(dir.join("one.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
    fs::write(dir.join("two.rs"), "fn alpha() {}\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "all",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --match all failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Co-occurrence:"), "{stdout}");
    assert!(stdout.contains("same-file co-occurrence only"), "{stdout}");
    assert!(stdout.contains("one.rs"), "{stdout}");
    assert!(
        !stdout.contains("two.rs"),
        "files without all terms must be excluded:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_exclude_filters_discovery_evidence_by_file_pattern() {
    let dir = temp_repo("discover_exclude");
    fs::write(dir.join("keep.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("skip_test.rs"), "fn target() {}\n").unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--exclude", "*test.rs", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --exclude failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.rs"), "{stdout}");
    assert!(!stdout.contains("skip_test.rs"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_as_file_supports_exclude_without_legacy_glob_filter() {
    let dir = temp_repo("discover_file_exclude");
    fs::write(dir.join("keep.rs"), "fn keep() {}\n").unwrap();
    fs::write(dir.join("skip_test.rs"), "fn skip() {}\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "*.rs",
            "--as",
            "file",
            "--exclude",
            "*test.rs",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover --as file --exclude failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep.rs"), "{stdout}");
    assert!(!stdout.contains("skip_test.rs"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_all_rejects_file_and_access_interpretations() {
    for as_kind in ["file", "access"] {
        let output = srcwalk()
            .args(["discover", "alpha,beta", "--match", "all", "--as", as_kind])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "--match all should reject --as {as_kind}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("symbol/text co-occurrence"),
            "expected co-occurrence error, got:\n{stderr}"
        );
    }
}

#[test]
fn discover_infers_file_mode_for_path_like_glob() {
    let dir = temp_repo("discover_infer_file_glob");
    fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.join("notes.txt"), "main.rs is mentioned here\n").unwrap();

    let output = srcwalk()
        .args(["discover", "*.rs", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover inferred file glob failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Files:"), "{stdout}");
    assert!(stdout.contains("main.rs"), "{stdout}");
    assert!(!stdout.contains("notes.txt:1"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_comma_terms_are_literal_or() {
    let dir = temp_repo("discover_text_or");
    fs::write(dir.join("one.txt"), "alpha only\n").unwrap();
    fs::write(dir.join("two.txt"), "beta only\n").unwrap();
    fs::write(dir.join("literal.txt"), "alpha,beta exact phrase\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Text OR:"), "{stdout}");
    assert!(stdout.contains("## alpha"), "{stdout}");
    assert!(stdout.contains("## beta"), "{stdout}");
    assert!(stdout.contains("one.txt:1"), "{stdout}");
    assert!(stdout.contains("two.txt:1"), "{stdout}");
    assert!(stdout.contains("literal OR text evidence only"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_go_owners_render_mechanical_call_evidence() {
    let dir = temp_repo("discover_text_or_go_owner_edges");
    fs::write(
        dir.join("feature.go"),
        "package feature\ntype DB struct{}\nfunc (d *DB) Apply() { /* alpha */ }\nfunc (d *DB) Set() { /* beta */ d.Apply() }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[owner DB.Apply@3-3]"), "{stdout}");
    assert!(stdout.contains("[owner DB.Set@4-4]"), "{stdout}");
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("DB.Set calls Apply@feature.go:4"),
        "{stdout}"
    );
    assert!(stdout.contains("candidate DB.Apply@:3-3"), "{stdout}");
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_go_owner_render_caps_edge_bullets_at_ten() {
    let dir = temp_repo("discover_text_or_go_owner_cap");
    // 12 candidate methods on the same receiver; Run calls each once so the
    // builder emits 12 edges but the renderer must cap the bullet list at 10.
    let mut src = String::from("package p\ntype DB struct{}");
    for i in 0..12 {
        src.push_str(&format!("\nfunc (d *DB) M{i}() {{ /* body */ }}"));
    }
    src.push_str("\nfunc (d *DB) Run() {\n");
    for i in 0..12 {
        src.push_str(&format!("    d.M{i}()\n"));
    }
    src.push('}');
    fs::write(dir.join("cap.go"), src).unwrap();

    let output = srcwalk()
        .args([
            "discover", "body,Run", "--match", "any", "--as", "text", "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    let bullets = stdout
        .lines()
        .filter(|l| l.starts_with("- [") && l.contains(" calls "))
        .count();
    assert_eq!(bullets, 10, "{stdout}");
    assert!(!stdout.contains("M11"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_go_owner_only_prints_exact_zero_edge_line_once() {
    let dir = temp_repo("discover_text_or_go_owner_zero_edge");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches(OWNER_LINK_ZERO_EDGE).count(), 1, "{stdout}");
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_single_go_owner_keeps_caveat_without_zero_edge_line() {
    let dir = temp_repo("discover_text_or_go_single_owner");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha beta */ }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[owner First@2-2]"), "{stdout}");
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_owner_caveat_survives_budget_truncation() {
    let dir = temp_repo("discover_text_or_go_owner_budget");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha beta */ }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--budget",
            "30",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("... truncated"), "{stdout}");
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_python_attributes_owners_without_go_call_appendix() {
    // US-067 phase 1: Python files now carry owner attribution. The owner
    // rollup is emitted, the Go-only mechanical-call appendix is NOT (no Go
    // call-analysis attempt), and the non-Go honesty caveat applies.
    let dir = temp_repo("discover_text_or_python_owner");
    fs::write(
        dir.join("app.py"),
        "def apply():\n    pass\ndef set():\n    apply()\ndef flush():\n    set()\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply,set,flush",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Python owner rollup is rendered.
    assert!(stdout.contains("owners (#N=Nth query term"), "{stdout}");
    assert!(stdout.contains("apply:1-2[#1]"), "{stdout}");
    assert!(stdout.contains("set:3-4[#1,#2]"), "{stdout}");
    // No Go call appendix is emitted for a non-Go-only query.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("[recv=same package-qualified receiver type"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // Non-Go honesty caveat present, and it must not imply call analysis ran.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_javascript_attributes_owners_without_go_call_appendix() {
    // US-067 phase 3: JavaScript files now carry owner attribution. The owner
    // rollup is emitted, the Go-only mechanical-call appendix is NOT (no Go
    // call-analysis attempt), and the non-Go honesty caveat applies.
    let dir = temp_repo("discover_text_or_javascript_owner");
    fs::write(
        dir.join("app.js"),
        "function apply() {\n    return 1;\n}\nfunction set() {\n    apply();\n}\nfunction flush() {\n    set();\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply,set,flush",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // JavaScript owner rollup is rendered with dot-less top-level names.
    assert!(stdout.contains("owners (#N=Nth query term"), "{stdout}");
    assert!(stdout.contains("apply:1-3[#1]"), "{stdout}");
    assert!(stdout.contains("set:4-6[#1,#2]"), "{stdout}");
    assert!(stdout.contains("flush:7-9[#2,#3]"), "{stdout}");
    // No Go call appendix is emitted for a non-Go-only query.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // Non-Go honesty caveat present, and it must not imply call analysis ran.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_jsx_routes_through_javascript_grammar_with_callback_barrier() {
    // A real `.jsx` file must route through the JavaScript grammar (JSX
    // elements parse), the named component gets an exact owner, and the inline
    // JSX callback (an anonymous arrow) must NOT fall through as a bogus owner.
    // The non-Go honesty caveat holds and no Go call appendix is emitted.
    let dir = temp_repo("discover_jsx_owner_barrier");
    fs::write(
        dir.join("app.jsx"),
        "function App() {\n  return <button onClick={() => { util(); }}>Click</button>;\n}\nfunction util() {\n  return 1;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover", "App,util", "--match", "any", "--as", "text", "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Named component routes through JS grammar with an exact owner.
    assert!(stdout.contains("app.jsx:1 [owner App@1-3]"), "{stdout}");
    assert!(stdout.contains("app.jsx:4 [owner util@4-6]"), "{stdout}");
    // The inline JSX callback line is an anonymous barrier: it does NOT carry
    // its own `[owner ...]` and does not fall through to a bogus name.
    assert!(!stdout.contains("app.jsx:2 [owner"), "{stdout}");
    // Non-Go honesty caveat present; no Go mechanical-call appendix.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_typescript_attributes_owners_without_go_call_appendix() {
    // A real `.ts` file routes through the TypeScript grammar: namespace
    // nesting yields dot-joined owners, exact ranges render, and the non-Go
    // honesty caveat holds with no Go call appendix.
    let dir = temp_repo("discover_typescript_owner");
    fs::write(
        dir.join("app.ts"),
        "namespace A.B {\n    export function foo() {}\n    export class C {\n        bar() {}\n    }\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover", "foo,bar", "--match", "any", "--as", "text", "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app.ts:2 [owner A.B.foo@2-2]"), "{stdout}");
    assert!(
        stdout.contains("app.ts:4 [owner A.B.C.bar@4-4]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_tsx_routes_through_tsx_grammar_with_callback_barrier() {
    // A real `.tsx` file routes through the TSX grammar: the named component is
    // an exact owner, the inline JSX callback is an anonymous barrier (no bogus
    // owner), and the non-Go caveat/no-Go-appendix holds.
    let dir = temp_repo("discover_tsx_owner_barrier");
    fs::write(
        dir.join("app.tsx"),
        "function App() {\n  return <button onClick={() => { util(); }}>Go</button>;\n}\nfunction util() {\n  return 1;\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover", "App,util", "--match", "any", "--as", "text", "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app.tsx:1 [owner App@1-3]"), "{stdout}");
    assert!(stdout.contains("app.tsx:4 [owner util@4-6]"), "{stdout}");
    assert!(!stdout.contains("app.tsx:2 [owner"), "{stdout}");
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_mts_cts_route_owners_through_existing_lang_mapping() {
    // `.mts`/`.cts` share the TypeScript Lang tier via the existing extension
    // mapping (no extension hacks): both carry owner attribution.
    let dir = temp_repo("discover_mts_cts_owner_routing");
    fs::write(
        dir.join("mod.mts"),
        "namespace N {\n    export function foo() {}\n}\n",
    )
    .unwrap();
    fs::write(dir.join("main.cts"), "class Svc {\n    handle() {}\n}\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "foo,handle",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mod.mts:2 [owner N.foo@2-2]"), "{stdout}");
    assert!(
        stdout.contains("main.cts:2 [owner Svc.handle@2-2]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_malformed_go_with_python_owners_suppresses_go_call_appendix() {
    // Regression for the gate bug: a malformed Go input must not leave
    // `go_call_analysis_attempted` true, because `build_owner_link_evidence`
    // only inserts successfully-parsed Go files into its analysis set. Python
    // owner attribution is retained, but the Go-only mechanical-call appendix
    // (zero-edge sentence + call caveat) must NOT render.
    let dir = temp_repo("discover_malformed_go_suppresses_go_appendix");
    fs::write(dir.join("app.py"), "def apply():\n    pass\n").unwrap();
    // Deterministically malformed Go: unclosed parameter list.
    fs::write(dir.join("broken.go"), "package p\nfunc main( {\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply,main",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Python owner evidence is retained.
    assert!(stdout.contains("[owner apply@1-2]"), "{stdout}");
    // The malformed Go hit is present but carries NO owner attribution.
    assert!(stdout.contains("broken.go:2"), "{stdout}");
    assert!(!stdout.contains("broken.go:2 [owner"), "{stdout}");
    // No Go zero-edge sentence, no Go call caveat, no Mechanical Go header.
    assert!(
        !stdout.contains("No direct name-level call evidence"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    // Non-Go honesty caveat is retained.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
}

#[test]
fn discover_text_or_rust_attributes_owners_without_go_call_appendix() {
    // US-067 phase 2: Rust files now carry owner attribution. The owner rollup
    // is emitted, the Go-only mechanical-call appendix is NOT (no Go call-
    // analysis attempt), and the non-Go honesty caveat applies.
    let dir = temp_repo("discover_text_or_rust_owner");
    fs::write(
        dir.join("app.rs"),
        "fn apply() {\n}\nfn set() {\n    apply();\n}\nfn flush() {\n    set();\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply,set,flush",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Rust owner rollup is rendered with exact owner ranges.
    assert!(stdout.contains("owners (#N=Nth query term"), "{stdout}");
    assert!(stdout.contains("apply:1-2[#1]"), "{stdout}");
    assert!(stdout.contains("set:3-5[#1,#2]"), "{stdout}");
    assert!(stdout.contains("flush:6-8[#2,#3]"), "{stdout}");
    // No Go call appendix is emitted for a non-Go-only query.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("[recv=same package-qualified receiver type"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // Non-Go honesty caveat present, and it must not imply call analysis ran.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_mixed_go_and_rust_attributes_both_and_go_appendix() {
    // Phase 2 capability honesty: a mixed Go + Rust query retains Rust owner
    // attribution AND the Go-only mechanical-call appendix (Go call analysis
    // ran), plus the non-Go honesty caveat for the Rust evidence.
    let dir = temp_repo("discover_mixed_go_rust_owner");
    fs::write(
        dir.join("app.rs"),
        "fn apply() {\n}\nfn set() {\n    apply();\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("db.go"),
        "package db\ntype DB struct{}\nfunc (d *DB) Apply() { /* apply body */ }\nfunc (d *DB) Set() { /* set body */ d.Apply() }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply,set",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Rust owner evidence is retained.
    assert!(stdout.contains("[owner apply@1-2]"), "{stdout}");
    // Go mechanical-call appendix is present (Go call analysis ran).
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // Non-Go honesty caveat present for the Rust owner evidence.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_go_compact_rollup_has_owner_line_by_default() {
    // Default-on owner attribution: a compact Go Text OR (>=3 terms) yields the
    // `owners (#N=...` rollup line with NO opt-in flag.
    let dir = temp_repo("discover_text_or_go_owner_default");
    fs::write(
        dir.join("db.go"),
        "package db\ntype DB struct{}\nfunc (d *DB) Apply() { /* apply body */ }\nfunc (d *DB) Set() { /* set body */ d.Apply() }\nfunc (d *DB) Flush() { /* flush body */ d.Set(); d.Apply() }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "apply body,set body,flush body",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("owners (#N=Nth query term; *K=hits)"),
        "{stdout}"
    );
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("[recv] DB.Set calls Apply@db.go:4; candidate DB.Apply@:3-3"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_large_output_rolls_up_by_file() {
    let dir = temp_repo("discover_text_or_file_rollup");
    fs::write(
        dir.join("feature.go"),
        "alpha here\nbeta here\ngamma here\nalpha beta gamma\n",
    )
    .unwrap();
    fs::write(
        dir.join("feature_test.go"),
        "alpha test\nbeta test\ngamma test\n",
    )
    .unwrap();
    fs::write(dir.join("other.go"), "alpha only\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma,missing",
            "--match",
            "any",
            "--as",
            "text",
            "--limit",
            "2",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files_idx = stdout.find("## Files ranked by term coverage").unwrap();
    let terms_idx = stdout.find("## Terms").unwrap();
    assert!(
        files_idx < terms_idx,
        "file rollup must lead term summary:\n{stdout}"
    );
    assert!(stdout.contains("feature.go — 3 terms"), "{stdout}");
    assert!(stdout.contains("terms: alpha("), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk show feature.go --section 1-4 -C 10"),
        "{stdout}"
    );
    assert!(
        stdout.contains("missing — 0/0 matches, 0 files"),
        "{stdout}"
    );
    assert!(stdout.contains("omitted by per-term limit"), "{stdout}");
    assert!(
        !stdout.contains("## alpha —"),
        "compact mode should not dump term-first hit blocks:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_next_read_uses_bounded_hit_windows() {
    let dir = temp_repo("discover_text_or_bounded_windows");
    let mut lines = vec!["filler".to_string(); 1_900];
    lines[65] = "rpc StopContainer has a timeout contract".to_string();
    lines[70] = "container is forcibly killed after the timeout".to_string();
    lines[1177] = "message StopContainerRequest {".to_string();
    lines[1182] = "int64 timeout = 2;".to_string();
    lines[1376] = "int64 unrelated_timeout = 1;".to_string();
    lines[1850] = "timeout for another request".to_string();
    fs::write(dir.join("api.proto"), lines.join("\n")).unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "StopContainerRequest,timeout,forcibly killed",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("windows: :66-71 terms="), "{stdout}");
    assert!(stdout.contains(":1178-1183 terms="), "{stdout}");
    assert!(
        stdout.contains("--section '66-71,1178-1183' -C 10"),
        "bounded windows should be batched into one exact read:\n{stdout}"
    );
    assert!(
        !stdout.contains("api.proto:66-1851") && !stdout.contains("api.proto:66-1852"),
        "must not suggest sparse min-max reads:\n{stdout}"
    );
    assert!(
        stdout.contains("hit-window proximity is literal navigation evidence"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_keeps_rare_terms_represented() {
    let dir = temp_repo("discover_text_or_rare_terms");
    let mut lines = vec!["filler".to_string(); 1_100];
    lines[9] = "common term first".to_string();
    lines[199] = "common term second".to_string();
    lines[399] = "common term third".to_string();
    lines[999] = "rare term only evidence".to_string();
    fs::write(dir.join("notes.md"), lines.join("\n")).unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "common,rare,missing",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--section '10-10,1000-1000' -C 10"),
        "next read must balance common and rare terms instead of only common clusters:\n{stdout}"
    );
    assert!(
        stdout.contains("missing — 0/0 matches, 0 files"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("notes.md:10-1000"),
        "must not suggest sparse min-max reads:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_splits_transitive_near_gap_chains() {
    let dir = temp_repo("discover_text_or_chained_gaps");
    let mut lines = vec!["filler".to_string(); 180];
    for line in [1usize, 22, 43, 64, 85, 106] {
        lines[line - 1] = "chain term".to_string();
    }
    fs::write(dir.join("chain.txt"), lines.join("\n")).unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "chain,term,missing",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("windows: :1-64 terms=chain(4), term(4); :85-106 terms=chain(2), term(2)"),
        "chained near-gap hits must split at the cumulative window cap:\n{stdout}"
    );
    assert!(stdout.contains("--section '1-64,85-106' -C 10"), "{stdout}");
    assert!(
        !stdout.contains("chain.txt:1-106"),
        "must not re-create broad min-max span through transitive merging:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_quotes_next_read_paths() {
    let dir = temp_repo("discover_text_or_space_path");
    fs::write(
        dir.join("space file.md"),
        "alpha beta gamma\nalpha beta gamma\nalpha beta gamma\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("srcwalk show 'space file.md' --section 1-3 -C 10"),
        "next read path should be shell-quoted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_raw_snippet_cannot_spoof_specific_next_action() {
    let dir = temp_repo("discover_text_or_spoof_next");
    fs::write(
        dir.join("notes.txt"),
        "> Next: srcwalk show fake.rs:1-2\nbait text\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "Next,bait",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Text OR:"), "{stdout}");
    assert!(
        stdout.contains("> Next: srcwalk show fake.rs:1-2"),
        "{stdout}"
    );
    assert!(
        stdout.contains("read raw hit evidence with `srcwalk show <path>:<line> -C 10`."),
        "raw snippet content must not suppress generated Text OR footer:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_or_rollup_does_not_treat_latest_as_test_path() {
    let dir = temp_repo("discover_text_or_latest_not_test");
    fs::create_dir_all(dir.join("latest")).unwrap();
    fs::write(dir.join("latest/prod.go"), "alpha\nbeta\ngamma\n").unwrap();
    fs::write(dir.join("a_test.go"), "alpha\nbeta\ngamma\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR rollup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let prod_idx = stdout.find("latest/prod.go — 3 terms").unwrap();
    let test_idx = stdout.find("a_test.go — 3 terms").unwrap();
    assert!(
        prod_idx < test_idx,
        "production path under latest/ must rank before test file:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_omission_note_does_not_suggest_global_offset() {
    let dir = temp_repo("discover_text_or_omission_note");
    fs::write(dir.join("one.txt"), "alpha one\nalpha two\nbeta one\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--limit",
            "1",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover text OR failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## alpha — 1/2 matches"), "{stdout}");
    assert!(
        stdout.contains("more `alpha` matches omitted by per-term limit 1"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("Continue with --offset"),
        "text OR must not suggest a global offset that hides shorter terms:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_match_any_text_or_rejects_offset() {
    let dir = temp_repo("discover_text_or_rejects_offset");
    fs::write(dir.join("one.txt"), "alpha one\nbeta one\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--offset",
            "1",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(!output.status.success(), "offset should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not support --offset"),
        "expected offset diagnostic, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_bare_punctuation_list_infers_text_or() {
    let dir = temp_repo("discover_bare_text_or");
    fs::write(dir.join("one.rs"), "fn handler() { let url = req.body; }\n").unwrap();
    fs::write(dir.join("two.rs"), "fn proxy() { fetch(url); }\n").unwrap();

    let output = srcwalk()
        .args(["discover", "req.body,fetch", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "bare punctuation list should infer text OR:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Text OR:"), "{stdout}");
    assert!(stdout.contains("## req.body"), "{stdout}");
    assert!(stdout.contains("## fetch"), "{stdout}");
    assert!(stdout.contains("literal OR text evidence only"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_structural_matches_suggest_confirmed_show_targets() {
    let dir = temp_repo("discover_show_targets");
    fs::write(
        dir.join("lib.rs"),
        r#"fn target() -> i32 {
    helper()
}

fn helper() -> i32 { 1 }

fn caller() -> i32 {
    target()
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover target failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show lib.rs:target"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show lib.rs:caller"),
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("> Next: srcwalk show lib.rs:target").count(),
        1,
        "confirmed structural next action should be deduplicated:\n{stdout}"
    );
    assert!(
        !stdout.contains("read the confirmed structural target above"),
        "confirmed-target block owns the next action; no generic guidance may follow:\n{stdout}"
    );

    assert!(
        !stdout.contains("use --expand to inline definition source"),
        "confirmed structural target should suppress definition expand guidance:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_over_cap_structural_target_omits_action_and_heading() {
    let dir = temp_repo("discover_over_cap_show_target");
    let mut content = String::from("fn target() {\n");
    for line in 2..201 {
        content.push_str(&format!("    let v{line} = {line};\n"));
    }
    content.push_str("}\n");
    fs::write(dir.join("lib.rs"), content).unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover target failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("> Next: srcwalk show lib.rs:1-201"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Caveat: confirmed structural target lib.rs:1-201 spans 201 lines, over the 200-line next-action bound."),
        "{stdout}"
    );
    // A structural target exists (just over the cap), so the generic guidance
    // is suppressed too; the caveat block owns the message.
    assert!(
        !stdout.contains("read the confirmed structural target above"),
        "structural-target query must suppress generic guidance:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_multiple_structural_matches_suggest_batched_show_target() {
    let dir = temp_repo("discover_batched_show_targets");
    fs::create_dir_all(dir.join("a")).unwrap();
    fs::create_dir_all(dir.join("b")).unwrap();
    fs::write(dir.join("a/lib.rs"), "fn target() -> i32 { 1 }\n").unwrap();
    fs::write(dir.join("b/lib.rs"), "fn target() -> i32 { 2 }\n").unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .args(["--limit", "5"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover target failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show a/lib.rs:target"),
        "expected symbol show next action:\n{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show b/lib.rs:target"),
        "expected symbol show next action:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk show a/lib.rs:target")
            .count(),
        1,
        "symbol show next action should be deduplicated:\n{stdout}"
    );
    assert!(
        !stdout.contains("srcwalk context a/lib.rs:1-1"),
        "discover should not route confirmed source reads through context:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_structural_show_targets_quote_spaces_and_comma_paths() {
    let dir = temp_repo("discover_show_target_path_safety");
    fs::write(dir.join("a file.rs"), "fn target() -> i32 { 1 }\n").unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("> Next: srcwalk show 'a file.rs:target'"),
        "space-bearing target must be shell-quoted:\n{stdout}"
    );

    fs::remove_file(dir.join("a file.rs")).unwrap();
    fs::write(dir.join("a,file.rs"), "fn target() -> i32 { 1 }\n").unwrap();

    let output = srcwalk()
        .args(["discover", "target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("> Next: srcwalk show 'a,file.rs' --section target"),
        "comma-bearing path must use --section with a symbol selector:\n{stdout}"
    );
    assert!(
        !stdout.contains("srcwalk show a,file.rs:1-1"),
        "comma-bearing path must not be emitted as an ambiguous inline target:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_typescript_function_definition_suggests_confirmed_show_target() {
    let dir = temp_repo("discover_ts_show_target");
    fs::write(
        dir.join("server.ts"),
        r#"import { Server } from "@modelcontextprotocol/sdk/server/index.js";

function createGatewayServer(context: unknown): Server {
    const server = new Server({ name: "gateway", version: "1" });
    server.setRequestHandler("list", async () => ({ tools: [] }));
    server.setRequestHandler("call", async (request) => request);
    return server;
}

export function startServer() {
    return createGatewayServer({});
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "createGatewayServer", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover createGatewayServer failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show server.ts:createGatewayServer"),
        "{stdout}"
    );
    assert!(
        stdout.contains("> Next: srcwalk show server.ts:startServer"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("use --expand to inline definition source"),
        "confirmed TypeScript structural target should suppress definition expand guidance:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_unsupported_language_function_does_not_suggest_context_target() {
    let dir = temp_repo("discover_unsupported_context_target");
    fs::write(
        dir.join("App.swift"),
        r#"func didUpdatePermissionStatus(_ message: String) {
    print(message)
}

func callStatus() {
    didUpdatePermissionStatus("ok")
}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "didUpdatePermissionStatus", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "discover didUpdatePermissionStatus failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[fn] didUpdatePermissionStatus"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("## Confirmed structural targets"),
        "unsupported context language must not suggest structural targets:\n{stdout}"
    );
    assert!(
        !stdout.contains("srcwalk context"),
        "unsupported context language must not suggest any context command:\n{stdout}"
    );

    assert!(
        stdout.contains("use --expand to inline definition source"),
        "unsupported context language should keep definition expand guidance:\n{stdout}"
    );

    fs::write(
        dir.join("OrderReturn.php"),
        r#"<?php
class OrderReturn {
    public static function checkEnoughProduct($id) {
        return $id;
    }
}
"#,
    )
    .unwrap();

    let php_output = srcwalk()
        .args(["discover", "checkEnoughProduct", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        php_output.status.success(),
        "discover checkEnoughProduct failed:\n{}",
        String::from_utf8_lossy(&php_output.stderr)
    );
    let php_stdout = String::from_utf8_lossy(&php_output.stdout);
    assert!(
        php_stdout.contains("[fn] checkEnoughProduct"),
        "{php_stdout}"
    );
    assert!(
        !php_stdout.contains("## Confirmed structural targets"),
        "PHP is structural but context-unsupported, so no confirmed structural targets:\n{php_stdout}"
    );
    assert!(
        !php_stdout.contains("srcwalk context"),
        "PHP context is unsupported, so fallback must not suggest context:\n{php_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_text_and_document_hits_do_not_guess_context_targets() {
    let dir = temp_repo("discover_no_context_guess");
    fs::write(dir.join("lib.rs"), "fn handler() { let token = 1; }\n").unwrap();
    fs::write(
        dir.join("README.md"),
        "# Getting Started\n\nUse token here.\n",
    )
    .unwrap();

    let text_output = srcwalk()
        .args(["discover", "token", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(text_output.status.success());
    let text_stdout = String::from_utf8_lossy(&text_output.stdout);
    assert!(
        !text_stdout.contains("## Confirmed structural targets"),
        "text evidence must not guess structural targets:\n{text_stdout}"
    );
    assert!(
        text_stdout.contains("read exact hit evidence with `srcwalk show <path>:<line> -C 10`"),
        "text discover footer should prefer exact reads without context guesses:\n{text_stdout}"
    );

    let doc_output = srcwalk()
        .args(["discover", "Getting Started", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(doc_output.status.success());
    let doc_stdout = String::from_utf8_lossy(&doc_output.stdout);
    assert!(
        doc_stdout.contains("[section] Getting Started"),
        "{doc_stdout}"
    );
    assert!(
        !doc_stdout.contains("## Confirmed structural targets"),
        "document evidence must not suggest structural targets:\n{doc_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_default_text_comma_stays_literal_and_hints_when_empty() {
    let dir = temp_repo("discover_text_literal_comma_hint");
    fs::write(dir.join("one.txt"), "alpha only\nbeta only\n").unwrap();

    let output = srcwalk()
        .args(["discover", "alpha,beta", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "literal comma text search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0 matches"), "{stdout}");
    assert!(
        stdout.contains("treated as one literal text query"),
        "{stdout}"
    );
    assert!(stdout.contains("--match any --as text"), "{stdout}");
    assert!(stdout.contains("--match all --as text"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn slash_delimited_text_query_is_literal_not_regex() {
    let dir = temp_repo("discover_regex_removed");
    fs::write(dir.join("one.txt"), "alpha only\nbeta only\n").unwrap();

    let output = srcwalk()
        .args(["discover", "/alpha|beta/", "--as", "text", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "slash literal text search failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Search: \"/alpha|beta/\""), "{stdout}");
    assert!(stdout.contains("0 matches"), "{stdout}");
    assert!(!stdout.contains("one.txt:1"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_discovery_caps_broad_default_without_starving_rare_symbols() {
    let dir = temp_repo("multi_symbol_fair_cap");
    fs::write(
        dir.join("main.go"),
        r#"package p
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func rareOne() {}
func rareTwo() {}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "Delete,rareOne,rareTwo",
            "--as",
            "symbol",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "multi-symbol discover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Search: \"rareOne\""), "{stdout}");
    assert!(stdout.contains("# Search: \"rareTwo\""), "{stdout}");
    assert!(stdout.contains("[fn] rareOne"), "{stdout}");
    assert!(stdout.contains("[fn] rareTwo"), "{stdout}");
    assert!(stdout.contains("# Search: \"Delete\""), "{stdout}");
    assert!(stdout.contains("3 matches (3 definitions)"), "{stdout}");
    assert!(
        stdout.contains("8 more matches available. Continue with --offset 3 --limit 3."),
        "broad symbol should expose deterministic continuation:\n{stdout}"
    );

    let next_page = srcwalk()
        .args([
            "discover",
            "Delete,rareOne,rareTwo",
            "--as",
            "symbol",
            "--offset",
            "3",
            "--limit",
            "3",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        next_page.status.success(),
        "multi-symbol continuation failed:\n{}",
        String::from_utf8_lossy(&next_page.stderr)
    );
    let next_stdout = String::from_utf8_lossy(&next_page.stdout);
    assert!(
        next_stdout.contains("# Search: \"Delete\""),
        "{next_stdout}"
    );
    assert!(
        next_stdout.contains("3 matches (3 definitions)"),
        "{next_stdout}"
    );
    assert!(
        next_stdout.contains("5 more matches available. Continue with --offset 6 --limit 3."),
        "continuation page must advance its next offset:\n{next_stdout}"
    );

    let final_page = srcwalk()
        .args([
            "discover",
            "Delete,rareOne,rareTwo",
            "--as",
            "symbol",
            "--offset",
            "9",
            "--limit",
            "3",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(final_page.status.success());
    let final_stdout = String::from_utf8_lossy(&final_page.stdout);
    assert!(
        final_stdout.contains("2 matches (2 definitions)"),
        "{final_stdout}"
    );
    assert!(
        !final_stdout.contains("more matches available"),
        "final page must not emit a false continuation:\n{final_stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_discovery_respects_explicit_limit() {
    let dir = temp_repo("multi_symbol_explicit_limit");
    fs::write(
        dir.join("main.go"),
        r#"package p
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func Delete() {}
func rareOne() {}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "Delete,rareOne",
            "--as",
            "symbol",
            "--limit",
            "5",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "multi-symbol discover with explicit limit failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Search: \"rareOne\""), "{stdout}");
    assert!(stdout.contains("# Search: \"Delete\""), "{stdout}");
    assert!(stdout.contains("5 matches (5 definitions)"), "{stdout}");
    assert!(
        !stdout.contains("2 more matches available. Continue with --offset 3 --limit 3."),
        "explicit limit should override default per-symbol cap:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_discovery_keeps_small_batches_complete() {
    let dir = temp_repo("multi_symbol_small_complete");
    fs::write(
        dir.join("main.go"),
        r#"package p
func Alpha() {}
func Alpha() {}
func Alpha() {}
func Alpha() {}
func Beta() {}
func Beta() {}
func Beta() {}
func Beta() {}
"#,
    )
    .unwrap();

    let output = srcwalk()
        .args(["discover", "Alpha,Beta", "--as", "symbol", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "small multi-symbol discover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches("4 matches (4 definitions)").count(),
        2,
        "small batch should keep every match for both symbols:\n{stdout}"
    );
    assert!(
        !stdout.contains("more matches available"),
        "small batch should not be compacted:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_symbol_exact_target_is_consumed_by_show_and_trace() {
    let dir = temp_repo("path_symbol_exact_target");
    fs::write(
        dir.join("lib.rs"),
        "fn target() {\n    helper();\n}\nfn helper() {}\n",
    )
    .unwrap();

    // `trace callees` roots on the exact definition named by the path.
    let trace = srcwalk()
        .args(["trace", "callees", "lib.rs:target", "--detailed", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let trace_stderr = String::from_utf8_lossy(&trace.stderr);
    assert!(
        trace.status.success(),
        "exact path:symbol root should be consumed, stderr:\n{trace_stderr}"
    );
    let trace_stdout = String::from_utf8_lossy(&trace.stdout);
    assert!(trace_stdout.contains("helper"), "{trace_stdout}");
    assert!(
        !trace_stderr.contains("`path:symbol` targets are accepted by `context`, not `trace`."),
        "path:symbol must no longer be rejected by trace:\n{trace_stderr}"
    );

    // `show` reads the exact owning body, i.e. `show <path> --section <symbol>`.
    let show = srcwalk()
        .args(["show", "lib.rs:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    let show_stderr = String::from_utf8_lossy(&show.stderr);
    assert!(
        show.status.success(),
        "exact path:symbol should read the owning body, stderr:\n{show_stderr}"
    );
    let show_stdout = String::from_utf8_lossy(&show.stdout);
    assert!(show_stdout.contains("fn target()"), "{show_stdout}");
    assert!(show_stdout.contains("helper();"), "{show_stdout}");
    assert!(
        !show_stdout.contains("fn helper() {}"),
        "show must read only the owning range, not the whole file:\n{show_stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_symbol_states_never_broaden_to_a_bare_search() {
    let dir = temp_repo("path_symbol_states");
    fs::write(dir.join("lib.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("plain.txt"), "target text\n").unwrap();
    // Two same-name definitions in ONE file: ambiguity is per named file.
    fs::write(
        dir.join("dup.rs"),
        "fn dup() {}\nfn other() {}\nfn dup() {}\n",
    )
    .unwrap();

    // Missing named file.
    for cmd in [
        vec!["show"],
        vec!["context"],
        vec!["trace", "callers"],
        vec!["trace", "callees"],
    ] {
        let out = srcwalk()
            .args(&cmd)
            .args(["missing.rs:target", "--scope"])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{cmd:?} should abstain on missing path"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("needs an existing named file"),
            "{cmd:?}:\n{stderr}"
        );
    }

    // Ambiguous within the named file: bounded candidates, no silent first-pick.
    let ambiguous = srcwalk()
        .args(["show", "dup.rs:dup", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
    let stderr = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(stderr.contains("Candidates:"), "{stderr}");
    assert!(stderr.contains("dup.rs:1-1"), "{stderr}");
    assert!(stderr.contains("dup.rs:3-3"), "{stderr}");

    // Existing file with no structural outline: honest abstention, not a bare read.
    let unresolvable = srcwalk()
        .args(["show", "plain.txt:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!unresolvable.status.success());
    let stderr = String::from_utf8_lossy(&unresolvable.stderr);
    assert!(stderr.contains("was not resolved"), "{stderr}");
    assert!(!stderr.contains("did you mean:"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn raw_colon_selector_reports_one_shared_intent_across_all_four_commands() {
    let dir = temp_repo("path_symbol_colon_intent");
    fs::write(dir.join("lib.rs"), "fn target() {}\n").unwrap();

    let mut intents = Vec::new();
    for cmd in [
        vec!["show"],
        vec!["context"],
        vec!["trace", "callers"],
        vec!["trace", "callees"],
    ] {
        let out = srcwalk()
            .args(&cmd)
            .args(["lib.rs:A::target", "--scope"])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{cmd:?} should reject a raw `::` selector"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("`path:symbol` takes a plain selector")
                && stderr.contains("contains `::`"),
            "{cmd:?}:\n{stderr}"
        );
        intents.push(
            stderr
                .split("takes a plain selector")
                .nth(1)
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert!(
        intents.windows(2).all(|w| w[0] == w[1]),
        "all four commands must surface the SAME colon intent: {intents:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_path_symbol_zero_result_reports_resolved_definition_only() {
    let dir = temp_repo("discover_path_symbol_zero");
    fs::write(dir.join("lib.rs"), "fn target() {}\n").unwrap();

    let resolved = srcwalk()
        .args(["discover", "lib.rs:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(resolved.status.success());
    let stdout = String::from_utf8_lossy(&resolved.stdout);
    assert!(stdout.contains("0 of 0 files"), "{stdout}");
    assert!(
        stdout.contains("`target` is defined at lib.rs:1-1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("`path:symbol` is `context` grammar, not `discover` grammar."),
        "{stdout}"
    );

    let unresolved = srcwalk()
        .args(["discover", "lib.rs:nothing", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(unresolved.status.success());
    let unresolved_stdout = String::from_utf8_lossy(&unresolved.stdout);
    assert!(
        !unresolved_stdout.contains("> Caveat:"),
        "{unresolved_stdout}"
    );

    let excluded = srcwalk()
        .args([
            "discover",
            "lib.rs:target",
            "--as",
            "file",
            "--exclude",
            "*",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(excluded.status.success());
    let excluded_stdout = String::from_utf8_lossy(&excluded.stdout);
    assert!(!excluded_stdout.contains("> Caveat:"), "{excluded_stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn trace_callers_path_symbol_success_path_remains_without_new_hint() {
    let dir = temp_repo("callers_path_symbol_unchanged");
    fs::write(dir.join("lib.rs"), "fn target() {}\n").unwrap();

    let output = srcwalk()
        .args(["trace", "callers", "lib.rs:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("srcwalk discover lib.rs:target"),
        "{stdout}"
    );
    assert!(output.stderr.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

/// US-067 cross-language orchestration fixture: one temp scope with a Go
/// owner pair + valid Go->Go edge, named owners in Python/Rust/JS/TS/TSX, one
/// malformed supported file, and one unsupported `.txt`.
fn write_mixed_owner_scope(dir: &Path) {
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();
    fs::write(dir.join("app.py"), "def load():\n    return \"alpha\"\n").unwrap();
    fs::write(
        dir.join("lib.rs"),
        "fn load() {\n    let _ = \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.js"),
        "function greet() {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.ts"),
        "function greet(): string {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("App.tsx"),
        "function App() {\n  return <div>alpha</div>;\n}\n",
    )
    .unwrap();
    // Malformed supported-language file: has a raw alpha hit but must abstain.
    fs::write(dir.join("broken.py"), "def broken(\n    return \"alpha\"\n").unwrap();
    // Unsupported raw file: preserved by search, absent from owner evidence.
    fs::write(dir.join("notes.txt"), "line with alpha here\n").unwrap();
}

/// US-067 orchestration + determinism (tasks 1+2): one mixed Go/Python/Rust/JS/
/// TS/TSX scope with a malformed supported file and an unsupported `.txt`. The
/// Go mechanical-call appendix renders Go-only, every clean file carries exact
/// per-file owner evidence, the malformed + unsupported raw hits stay visible
/// WITHOUT an owner, the non-Go honesty caveat holds, and no non-Go zero-edge/
/// call claim is made. The exact command runs twice with byte-identical stdout
/// AND stderr.
#[test]
fn discover_text_or_mixed_owner_orchestration_and_determinism() {
    let dir = temp_repo("discover_mixed_owner_orchestration");
    write_mixed_owner_scope(&dir);

    let run = || {
        srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(&dir)
            .output()
            .unwrap()
    };

    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // Determinism: run twice, byte-compare stdout AND stderr.
    let second = run();
    assert_eq!(
        first.stdout, second.stdout,
        "stdout bytes changed between runs"
    );
    assert_eq!(
        first.stderr, second.stderr,
        "stderr bytes changed between runs"
    );

    let stdout = String::from_utf8_lossy(&first.stdout);

    // Header: 2-term Text OR, 9 matches across 8 files (detail path, <=30 hits).
    assert!(stdout.contains("— 2 terms, 9 matches, 8 files"), "{stdout}");

    // Every supported clean file carries exact per-file owner evidence.
    for tag in [
        "feature.go:2 [owner First@2-2]",
        "feature.go:3 [owner Second@3-3]",
        "app.py:2 [owner load@1-2]",
        "lib.rs:2 [owner load@1-3]",
        "app.js:2 [owner greet@1-3]",
        "app.ts:2 [owner greet@1-3]",
        "App.tsx:2 [owner App@1-3]",
    ] {
        assert!(stdout.contains(tag), "missing {tag}:\n{stdout}");
    }

    // Malformed supported file + unsupported .txt: raw hit rows remain visible
    // but carry NO owner tag.
    assert!(
        stdout.contains("broken.py:2 — return \"alpha\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("notes.txt:1 — line with alpha here"),
        "{stdout}"
    );
    assert!(!stdout.contains("broken.py:2 [owner"), "{stdout}");
    assert!(!stdout.contains("notes.txt:1 [owner"), "{stdout}");

    // Go mechanical-call appendix exists and the single rendered edge is Go-only.
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("- [bare] Second calls First@feature.go:3; candidate First@:2-2",),
        "{stdout}"
    );
    let go_edge_rows = stdout
        .lines()
        .filter(|l| l.starts_with("- [") && l.contains(" calls "))
        .count();
    assert_eq!(go_edge_rows, 1, "{stdout}");

    // No non-Go zero-edge/call claim: not the zero-edge sentence, and no non-Go
    // edge candidate appears.
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    assert!(!stdout.contains("candidate app."), "{stdout}");
    assert!(!stdout.contains("candidate lib."), "{stdout}");

    // Go mechanical caveat + non-Go honesty caveat both present.
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-067 Go isolation (task 5): adding non-matching non-Go files to a Go-only
/// scope leaves the Go query's stdout AND stderr byte-identical.
#[test]
fn discover_text_or_go_only_bytes_unchanged_by_non_matching_non_go_files() {
    let dir = temp_repo("discover_go_isolation");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();

    let run = || {
        srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(&dir)
            .output()
            .unwrap()
    };

    let before = run();
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );

    // Add non-matching non-Go files (no alpha/beta anywhere) to the SAME dir so
    // the scope path is unchanged.
    fs::write(dir.join("app.py"), "def load():\n    return 1\n").unwrap();
    fs::write(dir.join("lib.rs"), "fn load() {\n    let _ = 1;\n}\n").unwrap();
    fs::write(dir.join("app.js"), "function greet() {\n    return 1;\n}\n").unwrap();
    fs::write(
        dir.join("App.tsx"),
        "function App() {\n  return <div>plain</div>;\n}\n",
    )
    .unwrap();
    fs::write(dir.join("notes.txt"), "just a plain text note\n").unwrap();

    let after = run();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    assert_eq!(
        before.stdout, after.stdout,
        "Go stdout changed when non-Go files added"
    );
    assert_eq!(
        before.stderr, after.stderr,
        "Go stderr changed when non-Go files added"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// Wave 2A full mixed-language scope: Go + TSX/TS/JS + Python + Rust + Java +
/// Kotlin + C# + PHP, each with a named owner over a matching hit. Only Go has
/// a name-level call edge (`Second -> First`).
fn write_wave2a_full_mixed_scope(dir: &Path) {
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();
    fs::write(dir.join("app.py"), "def load():\n    return \"alpha\"\n").unwrap();
    fs::write(
        dir.join("lib.rs"),
        "fn load() {\n    let _ = \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.js"),
        "function greet() {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.ts"),
        "function greet(): string {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("App.tsx"),
        "function App() {\n  return <div>alpha</div>;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("Service.java"),
        "class Service {\n    void handle() {\n        int x = 1; // alpha\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("App.kt"),
        "class App {\n    fun load() {\n        val s = \"alpha\"\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("Program.cs"),
        "class Service {\n    void Load() {\n        // alpha\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("page.php"),
        "<?php\nfunction load() {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
}

/// US-068 integration task 1: one Text OR query spans Go + TSX/TS/JS + Python +
/// Rust + Java + Kotlin + C# + PHP. Every supported language carries exact owner
/// evidence; the only mechanical-call edge rendered is the Go `Second -> First`;
/// no non-Go edge or zero-edge sentence appears.
#[test]
fn discover_text_or_wave2a_full_mixed_owner_orchestration() {
    let dir = temp_repo("discover_wave2a_full_mixed_owner");
    write_wave2a_full_mixed_scope(&dir);

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Header: 2 terms, 11 matches across 10 files (detail path, <=30 hits).
    assert!(
        stdout.contains("— 2 terms, 11 matches, 10 files"),
        "{stdout}"
    );

    // Every supported language carries exact per-file owner evidence.
    for tag in [
        "feature.go:2 [owner First@2-2]",
        "feature.go:3 [owner Second@3-3]",
        "app.py:2 [owner load@1-2]",
        "lib.rs:2 [owner load@1-3]",
        "app.js:2 [owner greet@1-3]",
        "app.ts:2 [owner greet@1-3]",
        "App.tsx:2 [owner App@1-3]",
        "Service.java:3 [owner Service.handle@2-4]",
        "App.kt:3 [owner App.load@2-4]",
        "Program.cs:3 [owner Service.Load@2-4]",
        "page.php:3 [owner load@2-4]",
    ] {
        assert!(stdout.contains(tag), "missing {tag}:\n{stdout}");
    }

    // Go mechanical-call appendix exists and the single rendered edge is Go-only.
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("- [bare] Second calls First@feature.go:3; candidate First@:2-2"),
        "{stdout}"
    );
    let go_edge_rows = stdout
        .lines()
        .filter(|l| l.starts_with("- [") && l.contains(" calls "))
        .count();
    assert_eq!(go_edge_rows, 1, "{stdout}");

    // No non-Go edge/zero-edge claim.
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    assert!(!stdout.contains("candidate app."), "{stdout}");
    assert!(!stdout.contains("candidate Service."), "{stdout}");
    assert!(!stdout.contains("candidate App."), "{stdout}");

    // Both honesty caveats present.
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-068 integration task 2: same mixed scope run under several file-creation
/// orders must yield byte-identical stdout AND stderr (owner rollups, term
/// indices, ranges, caveats, and the Go appendix).
#[test]
fn discover_text_or_wave2a_determinism_across_file_creation_orders() {
    let dir = temp_repo("discover_wave2a_determinism");
    write_wave2a_full_mixed_scope(&dir);

    let run = |d: &Path| {
        srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(d)
            .output()
            .unwrap()
    };
    let first = run(&dir);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // Recreate the same files in reverse lexical order in the same directory.
    // This changes file-creation order without changing the rendered scope path.
    let mut files = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (path, _) in &files {
        fs::remove_file(path).unwrap();
    }
    for (path, bytes) in files.iter().rev() {
        fs::write(path, bytes).unwrap();
    }

    let second = run(&dir);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        first.stdout, second.stdout,
        "stdout differs across creation orders"
    );
    assert_eq!(
        first.stderr, second.stderr,
        "stderr differs across creation orders"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-068 integration task 3: one malformed file per Wave 2A language. Raw hits
/// remain visible; the error-overlapping region abstains; a valid region in the
/// same partially-parsed file stays eligible; no outer owner leakage occurs.
#[test]
fn discover_text_or_wave2a_malformed_files_abstain_without_outer_leakage() {
    let dir = temp_repo("discover_wave2a_malformed");
    // Java: valid method + malformed method (unclosed param list) in one class.
    fs::write(
        dir.join("Bad.java"),
        "class Service {\n    void good() {\n        // alpha\n    }\n    void bad( {\n        // beta\n    }\n}\n",
    )
    .unwrap();
    // Kotlin: malformed statement inside a function body leaves the valid region
    // eligible while the error-overlapping region abstains.
    fs::write(
        dir.join("Bad.kt"),
        "class App {\n    fun good() {\n        val a = \"alpha\"\n    }\n    fun bad() {\n        val b = \"beta\"\n        if ( {\n            val c = \"gamma\"\n        }\n    }\n}\n",
    )
    .unwrap();
    // C#: valid method + malformed method in one class.
    fs::write(
        dir.join("Bad.cs"),
        "class Service {\n    void Good() {\n        // alpha\n    }\n    void Bad( {\n        // beta\n    }\n}\n",
    )
    .unwrap();
    // PHP: valid function + malformed function in one file.
    fs::write(
        dir.join("Bad.php"),
        "<?php\nfunction good() {\n    return \"alpha\";\n}\nfunction bad( {\n    return \"beta\";\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Valid regions in partially-parsed files stay eligible.
    for tag in [
        "Bad.java:3 [owner Service.good@2-4]",
        "Bad.kt:3 [owner App.good@2-4]",
        "Bad.cs:3 [owner Service.Good@2-4]",
        "Bad.php:3 [owner good@2-4]",
    ] {
        assert!(stdout.contains(tag), "missing {tag}:\n{stdout}");
    }

    // Malformed regions keep raw hits but carry NO owner (abstain, no leakage).
    for file in ["Bad.java:6", "Bad.kt:6", "Bad.cs:6", "Bad.php:6"] {
        assert!(
            stdout.contains(&format!("{file} —")),
            "raw hit missing {file}:\n{stdout}"
        );
        assert!(
            !stdout.contains(&format!("{file} [owner")),
            "malformed region leaked owner at {file}:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// US-068 integration task 4: nonmatching Wave 2A files must not change the byte
/// output of Go-only or US-067-language-only queries. Go rollups/edges stay
/// byte-identical after adding non-matching Java/Kotlin/C#/PHP files.
#[test]
fn discover_text_or_wave2a_nonmatching_files_do_not_change_go_and_us067_bytes() {
    let run = |dir: &Path, query: &str| {
        srcwalk()
            .args([
                "discover", query, "--match", "any", "--as", "text", "--scope",
            ])
            .arg(dir)
            .output()
            .unwrap()
    };

    // Go-only baseline: adding nonmatching Wave 2A files cannot alter bytes.
    let go_dir = temp_repo("discover_wave2a_go_isolation");
    fs::write(
        go_dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();
    let go_before = run(&go_dir, "alpha,beta");
    assert!(
        go_before.status.success(),
        "{}",
        String::from_utf8_lossy(&go_before.stderr)
    );

    for (name, source) in [
        ("NonMatch.java", "class N { void m() { int x = 1; } }\n"),
        ("NonMatch.kt", "class N { fun m() { val x = 1 } }\n"),
        ("NonMatch.cs", "class N { void M() { int x = 1; } }\n"),
        ("NonMatch.php", "<?php\nfunction m() { return 1; }\n"),
    ] {
        fs::write(go_dir.join(name), source).unwrap();
    }
    let go_after = run(&go_dir, "alpha,beta");
    assert!(
        go_after.status.success(),
        "{}",
        String::from_utf8_lossy(&go_after.stderr)
    );
    assert_eq!(go_before.stdout, go_after.stdout, "Go-only stdout changed");
    assert_eq!(go_before.stderr, go_after.stderr, "Go-only stderr changed");

    // US-067-only baseline: adding the same nonmatching Wave 2A files cannot
    // alter the earlier Python/Rust/JS/TS/TSX output either.
    let us067_dir = temp_repo("discover_wave2a_us067_isolation");
    fs::write(
        us067_dir.join("app.py"),
        "def load():\n    return \"alpha\"\n",
    )
    .unwrap();
    fs::write(
        us067_dir.join("lib.rs"),
        "fn load() {\n    let _ = \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        us067_dir.join("app.js"),
        "function greet() {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        us067_dir.join("app.ts"),
        "function greet(): string {\n    return \"alpha\";\n}\n",
    )
    .unwrap();
    fs::write(
        us067_dir.join("App.tsx"),
        "function App() {\n  return <div>alpha</div>;\n}\n",
    )
    .unwrap();
    let us067_before = run(&us067_dir, "alpha");
    assert!(
        us067_before.status.success(),
        "{}",
        String::from_utf8_lossy(&us067_before.stderr)
    );
    for (name, source) in [
        ("NonMatch.java", "class N { void m() { int x = 1; } }\n"),
        ("NonMatch.kt", "class N { fun m() { val x = 1 } }\n"),
        ("NonMatch.cs", "class N { void M() { int x = 1; } }\n"),
        ("NonMatch.php", "<?php\nfunction m() { return 1; }\n"),
    ] {
        fs::write(us067_dir.join(name), source).unwrap();
    }
    let us067_after = run(&us067_dir, "alpha");
    assert!(
        us067_after.status.success(),
        "{}",
        String::from_utf8_lossy(&us067_after.stderr)
    );
    assert_eq!(
        us067_before.stdout, us067_after.stdout,
        "US-067 stdout changed"
    );
    assert_eq!(
        us067_before.stderr, us067_after.stderr,
        "US-067 stderr changed"
    );

    let _ = fs::remove_dir_all(&go_dir);
    let _ = fs::remove_dir_all(&us067_dir);
}

/// US-068 integration task 5: compact rollup shape (>=3 terms) renders owner
/// evidence for every language with no non-Go edge/zero-edge sentence.
#[test]
fn discover_text_or_wave2a_compact_rollup_attributes_all_languages() {
    let dir = temp_repo("discover_wave2a_compact");
    write_wave2a_full_mixed_scope(&dir);
    // Compact rendering intentionally ranks at most eight files. Keep one
    // representative JS file here; the inline mixed test covers TS and TSX.
    fs::remove_file(dir.join("app.ts")).unwrap();
    fs::remove_file(dir.join("App.tsx")).unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,gamma",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .args(["--budget", "20000", "--limit", "20"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Compact rollup owner line header present.
    assert!(
        stdout.contains("owners (#N=Nth query term; *K=hits)"),
        "{stdout}"
    );
    // Compact owner anchors for every language.
    for (file, owner) in [
        ("feature.go —", "First:2-2[#1]"),
        ("app.py —", "load:1-2[#1]"),
        ("lib.rs —", "load:1-3[#1]"),
        ("app.js —", "greet:1-3[#1]"),
        ("Service.java —", "Service.handle:2-4[#1]"),
        ("App.kt —", "App.load:2-4[#1]"),
        ("Program.cs —", "Service.Load:2-4[#1]"),
        ("page.php —", "load:2-4[#1]"),
    ] {
        assert!(stdout.contains(file), "missing {file}:\n{stdout}");
        assert!(stdout.contains(owner), "missing {owner}:\n{stdout}");
    }
    // Go mechanical appendix still present and Go-only edge.
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("Second calls First@feature.go:3"),
        "{stdout}"
    );
    // No non-Go zero-edge sentence.
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    // Non-Go honesty caveat retained.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

// US-069: low-signal term advisory for literal text Search and Text OR.
const LOW_SIGNAL_ADVISORY: &str = "> Note: 450 matches across 150 of 1,000 eligible files for `frobnicate`; if this spread is not intentional, consider `overview`, a narrower term or scope, or a structural route.";

/// Build a deterministic repo: `a/` and `b/` each hold 500 eligible source files.
/// 150 of the 1,000 files (75 per dir) carry 3 `frobnicate` matches each
/// (450 matches total), comfortably above the 400/150/1.5% advisory boundary.
fn build_low_signal_repo(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    for sub in ["a", "b"] {
        let sub = dir.join(sub);
        fs::create_dir_all(&sub).unwrap();
        for i in 0..75 {
            fs::write(
                sub.join(format!("m{i:03}.rs")),
                "fn a() { frobnicate }\nfn b() { frobnicate }\nfn c() { frobnicate }\n",
            )
            .unwrap();
        }
        for i in 75..500 {
            fs::write(sub.join(format!("g{i:03}.rs")), "fn x() {}\n").unwrap();
        }
    }
    dir
}

#[test]
fn discover_low_signal_route_coverage_and_guard() {
    let dir = build_low_signal_repo("low_signal_route_guard");

    // Explicit single-term text Search emits the exact shared advisory.
    let single = srcwalk()
        .args(["discover", "frobnicate", "--as", "text", "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(single.status.success());
    let single_out = String::from_utf8_lossy(&single.stdout);
    assert!(single_out.contains(LOW_SIGNAL_ADVISORY), "{single_out}");

    // Text OR emits the same line for the triggering term.
    let or = srcwalk()
        .args([
            "discover",
            "frobnicate,unrelated",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(or.status.success());
    let or_out = String::from_utf8_lossy(&or.stdout);
    assert!(or_out.contains(LOW_SIGNAL_ADVISORY), "{or_out}");

    // Multi-scope inferred/mixed routes abstain: no explicit literal-text route
    // exists, so the advisory must never appear even for a high-spread term.
    let multi = srcwalk()
        .args(["discover", "frobnicate", "--scope"])
        .arg(dir.join("a"))
        .arg("--scope")
        .arg(dir.join("b"))
        .arg("--no-budget")
        .output()
        .unwrap();
    assert!(multi.status.success());
    let multi_out = String::from_utf8_lossy(&multi.stdout);
    assert!(!multi_out.contains("eligible files"), "{multi_out}");

    // Below-threshold term emits no advisory.
    fs::write(dir.join("a/one.rs"), "fn tiny() { frobnicate }\n").unwrap();
    let below = srcwalk()
        .args(["discover", "tiny", "--as", "text", "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    let below_out = String::from_utf8_lossy(&below.stdout);
    assert!(!below_out.contains("eligible files"), "{below_out}");

    // File, access, and co-occurrence routes never emit the advisory.
    for (args, scope) in [
        (vec!["discover", "*.rs", "--as", "file"], &dir),
        (vec!["discover", "frobnicate", "--as", "access"], &dir),
        (
            vec!["discover", "frobnicate,frobnicate", "--match", "all"],
            &dir,
        ),
    ] {
        let output = srcwalk()
            .args(&args)
            .arg("--scope")
            .arg(scope)
            .arg("--no-budget")
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(!out.contains("eligible files"), "{args:?}\n{out}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_low_signal_budget_survives_and_replays() {
    let dir = build_low_signal_repo("low_signal_budget_replay");

    // Enough output plus a tiny budget forces body truncation; the advisory
    // must survive once and stay before the final `> Next:`.
    let first = srcwalk()
        .args(["discover", "frobnicate", "--as", "text", "--scope"])
        .arg(&dir)
        .arg("--budget")
        .arg("80")
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_out = String::from_utf8_lossy(&first.stdout);
    assert_eq!(
        first_out.matches(LOW_SIGNAL_ADVISORY).count(),
        1,
        "advisory must appear exactly once under budget:\n{first_out}"
    );
    let note_idx = first_out.find(LOW_SIGNAL_ADVISORY).unwrap();
    let next_idx = first_out.rfind("> Next:").unwrap();
    assert!(note_idx < next_idx, "advisory must precede final `> Next:`");

    // Identical command replays byte-identically.
    let second = srcwalk()
        .args(["discover", "frobnicate", "--as", "text", "--scope"])
        .arg(&dir)
        .arg("--budget")
        .arg("80")
        .output()
        .unwrap();
    assert_eq!(first_out, String::from_utf8_lossy(&second.stdout));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_symbol_exact_root_resolves_qualified_language_forms() {
    let dir = temp_repo("path_symbol_qualified_langs");
    // Every file pairs a qualified definition with a same-named sibling that must
    // never leak into the exact root: a C# namespaced method, a Go receiver method
    // shadowed by a top-level func, and a Rust impl method shadowed by a free fn.
    fs::write(
        dir.join("Svc.cs"),
        "namespace N {\n  class Svc {\n    public void Run() { Helper(); }\n    public void Helper() { Leaf(); }\n    public void Leaf() {}\n  }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("batch.go"),
        "package main\n\ntype Batch struct{}\n\nfunc (b *Batch) Set() {\n\tb.flush()\n}\n\nfunc (b *Batch) flush() {}\n\nfunc Set() {\n\tunrelatedTopLevel()\n}\n\nfunc unrelatedTopLevel() {}\n",
    )
    .unwrap();
    fs::write(
        dir.join("svc.rs"),
        "struct Svc;\nimpl Svc {\n    fn run(&self) {\n        self.helper();\n    }\n    fn helper(&self) {}\n}\nfn run() {\n    bare_only();\n}\nfn bare_only() {}\n",
    )
    .unwrap();

    for (target, frame, want, unwanted) in [
        (
            "Svc.cs:Svc.Run",
            "requested 3; displayed 3",
            "Helper",
            "Leaf",
        ),
        (
            "batch.go:Batch.Set",
            "requested 5-7; displayed 5-7",
            "flush",
            "unrelatedTopLevel",
        ),
        (
            "svc.rs:Svc.run",
            "requested 3-5; displayed 3-5",
            "helper",
            "bare_only",
        ),
    ] {
        // The qualified selector must pin the exact range, not the sibling body.
        let shown = srcwalk()
            .args(["show", target, "--scope"])
            .arg(&dir)
            .arg("--no-budget")
            .output()
            .unwrap();
        let shown_err = String::from_utf8_lossy(&shown.stderr);
        assert!(shown.status.success(), "{target} should show:\n{shown_err}");
        let shown_out = String::from_utf8_lossy(&shown.stdout);
        assert!(
            shown_out.contains(frame),
            "{target} must resolve the exact body frame `{frame}`:\n{shown_out}"
        );

        // ... and the callee view must be built from that body alone.
        let out = srcwalk()
            .args(["trace", "callees", target, "--scope"])
            .arg(&dir)
            .arg("--no-budget")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{target} should resolve:\n{stderr}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(want), "{target} callees:\n{stdout}");
        assert!(
            !stdout.contains(unwanted),
            "{target} leaked a sibling body's callee:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn path_symbol_exact_root_still_accepts_bare_selectors() {
    let dir = temp_repo("path_symbol_adjacent_langs");
    fs::write(
        dir.join("Svc.cs"),
        "namespace N {\n  class Svc {\n    public void Run() { Helper(); }\n    public void Helper() {}\n  }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("svc.go"),
        "package main\n\nfunc Run() {\n\tHelper()\n}\n\nfunc Helper() {}\n",
    )
    .unwrap();
    fs::write(
        dir.join("svc.rs"),
        "impl Svc {\n    fn run(&self) {\n        self.helper();\n    }\n    fn helper(&self) {}\n}\n",
    )
    .unwrap();

    for (target, want) in [
        ("Svc.cs:Run", "Helper"),
        ("svc.go:Run", "Helper"),
        ("svc.rs:run", "helper"),
    ] {
        let out = srcwalk()
            .args(["trace", "callees", target, "--scope"])
            .arg(&dir)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{target} should resolve:\n{stderr}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(want), "{target} callees:\n{stdout}");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callers_exact_root_outside_scope_is_readable_but_search_stays_scoped() {
    let dir = temp_repo("path_symbol_outside_scope");
    let inside = dir.join("inside");
    fs::create_dir_all(&inside).unwrap();
    // Root definition lives OUTSIDE the --scope directory.
    fs::write(dir.join("root.rs"), "fn target() {}\n").unwrap();
    fs::write(inside.join("use.rs"), "fn caller() { target(); }\n").unwrap();

    let out = srcwalk()
        .args(["trace", "callers"])
        .arg(dir.join("root.rs:target"))
        .args(["--scope"])
        .arg(&inside)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "an exact root outside --scope must still be readable:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("caller"),
        "scoped call site missing:\n{stdout}"
    );
    assert!(
        stdout.contains("lies outside --scope") && stdout.contains("searched inside --scope only"),
        "outside-scope root must be labeled:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callees_exact_root_does_not_require_global_uniqueness() {
    let dir = temp_repo("path_symbol_same_name_defs");
    fs::write(
        dir.join("a.rs"),
        "fn target() {\n    from_a();\n}\nfn from_a() {}\n",
    )
    .unwrap();
    fs::write(
        dir.join("b.rs"),
        "fn target() {\n    from_b();\n}\nfn from_b() {}\n",
    )
    .unwrap();

    for (target, want, unwanted) in [
        ("a.rs:target", "from_a", "from_b"),
        ("b.rs:target", "from_b", "from_a"),
    ] {
        let out = srcwalk()
            .args(["trace", "callees", target, "--scope"])
            .arg(&dir)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "a same-name definition elsewhere must not block {target}:\n{stderr}"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(want), "{target}:\n{stdout}");
        assert!(
            !stdout.contains(unwanted),
            "{target} leaked the other def:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callers_depth_two_marks_only_the_root_as_exact() {
    let dir = temp_repo("path_symbol_depth_two");
    fs::write(
        dir.join("lib.rs"),
        "fn target() {}\nfn mid() { target(); }\nfn top() { mid(); }\n",
    )
    .unwrap();

    let out = srcwalk()
        .args([
            "trace",
            "callers",
            "lib.rs:target",
            "--depth",
            "2",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("# BFS callers of \"lib.rs:target\""),
        "canonical root must be preserved in the BFS header:\n{stdout}"
    );
    assert!(
        stdout.contains("only the root is exact") && stdout.contains("later hop expands by name"),
        "depth>=2 must caveat that later hops are by-name:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callers_exact_count_by_keeps_canonical_root_and_by_name_caveat() {
    let dir = temp_repo("path_symbol_count_by_caveat");
    fs::write(
        dir.join("lib.rs"),
        "fn target() {}\nfn mid() { target(); }\nfn other() { target(); }\n",
    )
    .unwrap();

    // Counts are a grouped view of the SAME by-name search, so the canonical root
    // and the exact-root caveat must survive --count-by, --filter, and every page.
    for extra in [
        vec!["--count-by", "caller"],
        vec!["--count-by", "caller", "--filter", "args:0"],
        vec![
            "--count-by",
            "caller",
            "--filter",
            "args:0",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
    ] {
        let out = srcwalk()
            .args(["trace", "callers", "lib.rs:target"])
            .args(&extra)
            .arg("--scope")
            .arg(&dir)
            .arg("--no-budget")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{extra:?}:\n{stderr}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("[symbol] lib.rs:target"),
            "{extra:?} must echo the canonical root:\n{stdout}"
        );
        assert!(
            stdout.contains("the path identifies the requested definition of `target`")
                && stdout.contains("call sites are still matched by name"),
            "{extra:?} must keep the exact-root by-name caveat:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callees_exact_depth_two_marks_only_the_root_as_exact() {
    let dir = temp_repo("path_symbol_callee_depth_two");
    fs::write(
        dir.join("lib.rs"),
        "fn leaf() {}\nfn mid() { leaf(); }\nfn top() { mid(); }\n",
    )
    .unwrap();

    let out = srcwalk()
        .args(["trace", "callees", "lib.rs:top", "--depth", "2", "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("leaf"),
        "depth 2 should expand a transitive hop:\n{stdout}"
    );
    assert!(
        stdout.contains("only the root is exact") && stdout.contains("later hop resolves by name"),
        "depth>=2 callees must caveat that later hops are by-name:\n{stdout}"
    );

    // Depth 1 renders no later hop, so it must not claim one.
    let direct = srcwalk()
        .args(["trace", "callees", "lib.rs:top", "--scope"])
        .arg(&dir)
        .arg("--no-budget")
        .output()
        .unwrap();
    let direct_out = String::from_utf8_lossy(&direct.stdout);
    assert!(
        !direct_out.contains("only the root is exact"),
        "direct callees have no later hop to caveat:\n{direct_out}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn callees_exact_root_outside_scope_is_labeled_on_every_output_shape() {
    let dir = temp_repo("path_symbol_callee_outside_scope");
    let inside = dir.join("inside");
    fs::create_dir_all(&inside).unwrap();
    // Both roots live OUTSIDE --scope; only the boundary label is shared.
    fs::write(dir.join("root.rs"), "fn seed() { inside_helper(); }\n").unwrap();
    fs::write(dir.join("quiet.rs"), "fn quiet() { let _x = 1; }\n").unwrap();
    fs::write(inside.join("use.rs"), "fn inside_helper() {}\n").unwrap();

    // Transitive, detailed, and no-call returns are separate exits; each must
    // still say the root sits outside --scope.
    for (file, symbol, extra) in [
        ("root.rs", "seed", vec!["--depth", "2"]),
        ("root.rs", "seed", vec!["--detailed"]),
        ("quiet.rs", "quiet", vec![]),
    ] {
        let out = srcwalk()
            .args(["trace", "callees"])
            .arg(dir.join(format!("{file}:{symbol}")))
            .args(&extra)
            .arg("--scope")
            .arg(&inside)
            .arg("--no-budget")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "an exact root outside --scope must still be readable ({file} {extra:?}):\n{stderr}"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("lies outside --scope")
                && stdout.contains("traversal stay inside --scope only"),
            "{file} {extra:?} must label the scope boundary:\n{stdout}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ US-071 Step 4

/// Multi-symbol Discover emits the same confirmed structural targets the
/// single-symbol route would, once per per-query section, without touching the
/// numeric fallback for ambiguous names or deduplicating across sections.
#[test]
fn multi_symbol_discover_emits_canonical_targets_per_section() {
    let dir = temp_repo("multi_symbol_emit");
    fs::create_dir_all(dir.join("src")).unwrap();
    // One-unique `run` (canonical), plus two same-name `helper` bodies (ambiguous).
    fs::write(
        dir.join("src/alpha.rs"),
        "pub struct Alpha;\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/helper.rs"),
        "fn helper() -> i32 {\n    let x = 1;\n    let y = 2;\n    x + y\n}\nfn other() -> i32 { 2 }\nfn helper() -> i32 {\n    let a = 3;\n    let b = 4;\n    a + b\n}\n",
    )
    .unwrap();

    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "run,helper",
            "--as",
            "symbol",
            "--scope",
            "src",
            "--expand=0",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Canonical target in the `run` section, and it addresses the unique method.
    assert!(
        stdout.contains("> Next: srcwalk show src/alpha.rs:Alpha.run"),
        "multi-symbol run section must emit the canonical target:\n{stdout}"
    );

    // The ambiguous `helper` section never advertises a canonical `helper`
    // selector. With the body rendered inline it abstains ("already shown"
    // caveat); without inline rendering it would fall back to a numeric range.
    // Either way it must never claim a canonical symbol.
    assert!(
        !stdout.contains("srcwalk show src/helper.rs:helper"),
        "ambiguous bare name must not be advertised canonical:\n{stdout}"
    );
    assert!(
        stdout.contains("already shown in full above") || stdout.contains("numeric fallback"),
        "ambiguous multi-symbol term must abstain or fall back honestly:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A canonical target resolved by two different query terms is kept once in each
/// section — sections stay independently auditable against their own query.
#[test]
fn multi_symbol_duplicate_selector_kept_once_per_section() {
    let dir = temp_repo("multi_symbol_duplicate");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/a.rs"),
        "pub struct Alpha;\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
    )
    .unwrap();

    // `run` and `Alpha` are different terms, but only `run` is a function name;
    // to force a real cross-section duplicate, query the same bare name twice.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "run,run", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    let occurrences = stdout.matches("srcwalk show src/a.rs:Alpha.run").count();
    assert_eq!(
        occurrences, 2,
        "a canonical selector must appear once per section, not be globally deduped:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

// -------------------------------------------------- US-071 Step 4 dispatch (amendment)

/// Helper: run `discover` in a fresh repo with two uniquely-named Rust methods,
/// returning stdout so the caller can assert routing (multi-symbol section vs glob).
fn dotted_multi_symbol_repo(name: &str) -> PathBuf {
    let dir = temp_repo(name);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/alpha.rs"),
        "pub struct Alpha;\nimpl Alpha {\n    pub fn run(&self) {}\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/beta.rs"),
        "pub struct Beta;\nimpl Beta {\n    pub fn stop(&self) {}\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn dotted_multi_symbol_dispatches_before_auto_classify() {
    let dir = dotted_multi_symbol_repo("dotted_routes");
    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "Alpha.run,Beta.stop",
            "--as",
            "symbol",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // A dotted part must not turn the whole query into a `**/...` file glob.
    assert!(
        !stdout.contains("# Files:") && !stdout.contains("**/Alpha.run"),
        "dotted multi-symbol must not be a file glob:\n{stdout}"
    );
    // Each term is its own section.
    assert!(stdout.contains("# Search: \"Alpha.run\""), "{stdout}");
    assert!(stdout.contains("# Search: \"Beta.stop\""), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bare_multi_symbol_still_dispatches() {
    let dir = dotted_multi_symbol_repo("bare_still");
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "run,stop", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Search: \"run\""), "{stdout}");
    assert!(stdout.contains("# Search: \"stop\""), "{stdout}");
    assert!(
        stdout.contains("srcwalk show src/alpha.rs:Alpha.run"),
        "{stdout}"
    );
    assert!(
        stdout.contains("srcwalk show src/beta.rs:Beta.stop"),
        "{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn comma_filename_shapes_are_multi_symbol_in_auto_mode() {
    let dir = dotted_multi_symbol_repo("comma_filename");
    // `a,b.txt` and `foo.rs,bar.rs` become symbols `a`/`b.txt` per the pinned
    // amendment decision; they route multi-symbol (ID sections), never a glob.
    for query in ["a,b.txt", "foo.rs,bar.rs"] {
        let out = srcwalk()
            .current_dir(&dir)
            .args(["discover", query, "--as", "symbol", "--scope", "src"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("# Files:") && !stdout.contains("**/"),
            "`{query}` must be multi-symbol, not a glob:\n{stdout}"
        );
        assert!(
            stdout.contains("# Search:"),
            "`{query}` must emit sections:\n{stdout}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_whitespace_and_degenerate_shapes() {
    let dir = dotted_multi_symbol_repo("ws_degenerate");
    // `foo, bar` trims and dispatches two symbols.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "run, stop", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Search: \"run\""), "{stdout}");
    assert!(stdout.contains("# Search: \"stop\""), "{stdout}");

    // `,a,b` drops the empty leading slot.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", ",run,stop", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("# Search: \"run\""), "{stdout}");
    assert!(stdout.contains("# Search: \"stop\""), "{stdout}");

    // `foo,` drops the trailing slot, leaves one part -> None -> old flow
    // (single term, not multi-symbol sections).
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "run,", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Search: \"run\"\n"),
        "`run,` must fall back to the old single-term flow, not multi-symbol:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_over_five_keeps_explicit_error() {
    let dir = dotted_multi_symbol_repo("over_five");
    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "a,b,c,d,e,f",
            "--as",
            "symbol",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "6 symbols must error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("multi-symbol search supports 2-5 symbols"),
        "{stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_symbol_does_not_hijack_glob_regex_path_or_section() {
    let dir = dotted_multi_symbol_repo("no_hijack");
    // Real glob: `*?{[` must keep the glob/file route, never multi-symbol sections.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "*.{rs,ts}", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Search: \"*."),
        "glob must not become multi-symbol:\n{stdout}"
    );

    // Regex delimiters (leading `/`) must not be split into symbols.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "/foo,bar/", "--as", "symbol", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Search: \"/foo\""),
        "regex must not be hijacked to multi-symbol:\n{stdout}"
    );

    // Path separator must keep the path/file route.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "src/alpha.rs,beta.rs", "--scope", "."])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Search: \"src\""),
        "path-like query must not be hijacked:\n{stdout}"
    );

    // Section colon must keep the path/section route, not multi-symbol.
    let out = srcwalk()
        .current_dir(&dir)
        .args([
            "discover",
            "alpha.rs:1,2",
            "--as",
            "symbol",
            "--scope",
            "src",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("# Search: \"alpha.rs:1\""),
        "section query must not be hijacked to multi-symbol:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn explicit_as_text_and_file_are_not_intercepted() {
    let dir = dotted_multi_symbol_repo("explicit_as");
    // `--as text` treats the whole comma string as ONE literal text query.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "run,stop", "--as", "text", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("# Search: \"run,stop\""),
        "--as text must not split the comma into symbols:\n{stdout}"
    );
    assert!(
        !stdout.contains("# Search: \"run\"\n# Search: \"stop\""),
        "--as text must stay one literal query:\n{stdout}"
    );

    // `--as file` keeps the file route.
    let out = srcwalk()
        .current_dir(&dir)
        .args(["discover", "a.rs,b.rs", "--as", "file", "--scope", "src"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("# Files:") && !stdout.contains("# Search:"),
        "--as file must stay the file route, not multi-symbol:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072: C and C++ files enter owner-only dispatch. Inline Text OR rows carry
/// exact `[owner NAME@start-end]` evidence, the C++ owner renders `::`
/// qualification without being advertised as a canonical selector, the non-Go
/// honesty caveat is present, and the Go-only call appendix never appears for a
/// C/C++-only query.
#[test]
fn discover_text_or_c_cpp_attributes_owners_without_go_call_appendix() {
    let dir = temp_repo("discover_text_or_c_cpp_owner");
    fs::write(
        dir.join("util.c"),
        "int add(int a, int b) {\n    return a + b;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("svc.cpp"),
        "namespace ns {\nclass Foo {\npublic:\n    void bar() {\n        log();\n    }\n};\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "return a + b,log()",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Exact per-file owner evidence: C plain name, C++ `::` qualified name.
    assert!(stdout.contains("util.c:2 [owner add@1-3]"), "{stdout}");
    assert!(
        stdout.contains("svc.cpp:5 [owner ns::Foo::bar@4-6]"),
        "{stdout}"
    );
    // No Go call appendix is emitted for a C/C++-only query.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("[recv=same package-qualified receiver type"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // C++ `::` display is never promoted to a copyable canonical selector.
    assert!(
        !stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    // Non-Go honesty caveat present, and it must not imply call analysis ran.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(
        stdout.contains("no call analysis was run for non-Go languages"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072: compact rollup renders owners for BOTH C and C++ in one query, and
/// the exact command replays byte-identically (compact ordering + ranges).
#[test]
fn discover_text_or_c_cpp_compact_rollup_replays_byte_identically() {
    let dir = temp_repo("discover_c_cpp_compact_replay");
    fs::write(
        dir.join("util.c"),
        "int add(int a, int b) {\n    return a + b;\n}\nint sub(int a, int b) {\n    return a - b;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("svc.cpp"),
        "namespace ns {\nclass Foo {\npublic:\n    void bar() {\n        log();\n    }\n    void run() {\n        bar();\n    }\n};\n}\n",
    )
    .unwrap();

    let run = |d: &Path| {
        srcwalk()
            .args([
                "discover",
                "return a + b,return a - b,log(),bar()",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(d)
            .output()
            .unwrap()
    };
    let first = run(&dir);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8_lossy(&first.stdout);
    // Compact rollup line carries both languages with `::` C++ qualification.
    assert!(
        stdout.contains("owners (#N=Nth query term; *K=hits)"),
        "{stdout}"
    );
    assert!(stdout.contains("add:1-3[#1]"), "{stdout}");
    assert!(stdout.contains("sub:4-6[#2]"), "{stdout}");
    assert!(stdout.contains("ns::Foo::bar:4-6[#3,#4]"), "{stdout}");
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    // Byte-identical replay.
    let second = run(&dir);
    assert_eq!(
        first.stdout, second.stdout,
        "stdout differs across identical replays"
    );
    assert_eq!(
        first.stderr, second.stderr,
        "stderr differs across identical replays"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 §7.2: every C++ lambda is an anonymous barrier and macro-generated
/// `TEST(Foo, Bar)` bodies abstain. In compact mode the enclosing named
/// function keeps the owner for its non-lambda hits while lambda-body and
/// TEST-body hits contribute no owner evidence at all (the TEST file's rollup
/// has no owners line).
#[test]
fn discover_text_or_cpp_lambda_and_macro_lines_abstain() {
    let dir = temp_repo("discover_cpp_lambda_macro_abstain");
    fs::write(
        dir.join("lam.cpp"),
        "void run() {\n    init();\n    auto l = [](int x) {\n        return x + 1;\n    };\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests.cpp"),
        "TEST(Foo, Bar) {\n    EXPECT_EQ(1, 1);\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "init(),return x + 1,EXPECT_EQ(1, 1)",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The enclosing function keeps the owner for hits outside the lambda; the
    // lambda body and TEST body contribute nothing.
    let owners_lines = stdout
        .lines()
        .filter(|l| l.contains("owners (#N=Nth query term"))
        .collect::<Vec<_>>();
    assert_eq!(owners_lines.len(), 1, "{stdout}");
    assert!(owners_lines[0].contains("run:1-6[#1]"), "{stdout}");
    assert!(!owners_lines[0].contains("TEST"), "{stdout}");
    // No fabricated macro owner appears anywhere.
    assert!(!stdout.contains("TEST@"), "{stdout}");
    assert!(!stdout.contains("[owner TEST"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 P1 regression (Biscuit review): a type-less namespace
/// "constructor", a macro/TEST body, and an abstained (malformed) function
/// identity never gain owner evidence, and an abstained function never leaks
/// a nested function as an owner.
#[test]
fn discover_text_or_cpp_abstained_identities_never_leak_nested_owner() {
    let dir = temp_repo("discover_cpp_p1_regression");
    fs::write(
        dir.join("regress.cpp"),
        "namespace Faux {\n\
         Faux() {\n\
             hit_one();\n\
         }\n\
         }\n\
         TEST(Foo, Bar) {\n\
             void leaked() {\n\
                 hit_two();\n\
             }\n\
         }\n\
         void Foo::operator int() {\n\
             void leaked2() {\n\
                 hit_three();\n\
             }\n\
         }\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "hit_one(),hit_two(),hit_three()",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // All three hits are found, but zero owner evidence: the namespace
    // "constructor", the TEST body, and the malformed `operator int` all
    // abstain, and their nested functions never leak an owner.
    assert!(
        stdout.contains("hit_one() — 1/1 matches, 1 file"),
        "{stdout}"
    );
    assert!(
        stdout.contains("hit_two() — 1/1 matches, 1 file"),
        "{stdout}"
    );
    assert!(
        stdout.contains("hit_three() — 1/1 matches, 1 file"),
        "{stdout}"
    );
    assert!(!stdout.contains("[owner"), "{stdout}");
    assert!(!stdout.contains("owners (#N"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// US-072: a mixed Go + C/C++ scope keeps the mechanical-call appendix
/// Go-only. C/C++ owners are attributed, but they never enter call-edge
/// analysis and no non-Go zero-edge/call claim is made.
#[test]
fn discover_text_or_go_plus_c_cpp_mixed_keeps_edges_go_only() {
    let dir = temp_repo("discover_go_c_cpp_mixed");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.c"),
        "int load(void) {\n    return 1; /* alpha */\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.cpp"),
        "namespace svc {\nclass App {\npublic:\n    void handle() { /* beta */ }\n};\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // All three languages carry exact per-file owner evidence.
    assert!(
        stdout.contains("feature.go:2 [owner First@2-2]"),
        "{stdout}"
    );
    assert!(stdout.contains("app.c:2 [owner load@1-3]"), "{stdout}");
    assert!(
        stdout.contains("app.cpp:4 [owner svc::App::handle@4-4]"),
        "{stdout}"
    );
    // The Go mechanical-call appendix exists and the single rendered edge is
    // Go-only.
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("- [bare] Second calls First@feature.go:3; candidate First@:2-2"),
        "{stdout}"
    );
    let go_edge_rows = stdout
        .lines()
        .filter(|l| l.starts_with("- [") && l.contains(" calls "))
        .count();
    assert_eq!(go_edge_rows, 1, "{stdout}");
    // No non-Go zero-edge or call claim.
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    assert!(!stdout.contains("candidate load@"), "{stdout}");
    assert!(!stdout.contains("candidate svc::"), "{stdout}");
    // Both honesty caveats present.
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072: a C++ call operator (`operator()`) carries exact owner evidence, and
/// the non-Go isolation invariant holds (no call edge, no zero-edge line for a
/// C++-only hit set).
#[test]
fn discover_text_or_cpp_call_operator_owner_without_call_edges() {
    let dir = temp_repo("discover_cpp_call_operator");
    fs::write(
        dir.join("functor.cpp"),
        "namespace svc {\nstruct Adder {\n    int operator()(int x) {\n        return x; /* alpha */\n    }\n};\n}\nint svc::Adder::operator()(int x, int y) {\n    return x + y; /* beta */\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Inline member and out-of-line definitions both name the call operator.
    assert!(
        stdout.contains("functor.cpp:4 [owner svc::Adder::operator()@3-5]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("functor.cpp:9 [owner svc::Adder::operator()@8-10]"),
        "{stdout}"
    );
    // Non-Go isolation: no mechanical call appendix, no edge, no zero-edge line.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(!stdout.contains(" calls "), "{stdout}");
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// US-072: adding nonmatching C/C++ files to the scope does not change any
/// pre-existing stdout/stderr bytes (no owner evidence, no caveat, no rollup
/// line appears for files with zero hits).
#[test]
fn discover_text_or_c_cpp_nonmatching_files_do_not_change_output() {
    let dir = temp_repo("discover_c_cpp_nonmatching");
    fs::write(dir.join("app.c"), "int alpha(void) {\n    return 1;\n}\n").unwrap();
    fs::write(dir.join("app.cpp"), "namespace n { void beta() { } }\n").unwrap();

    let run = |d: &Path| {
        srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(d)
            .output()
            .unwrap()
    };
    let baseline = run(&dir);
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    // Add nonmatching C/C++ files (no query-term text).
    fs::write(
        dir.join("unrelated.c"),
        "static int unused_helper(void) {\n    return 42;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("unrelated.cpp"),
        "namespace other { class Widget { public: void draw() {} }; }\n",
    )
    .unwrap();

    let after = run(&dir);
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    assert_eq!(
        baseline.stdout, after.stdout,
        "stdout changed after adding nonmatching C/C++ files"
    );
    assert_eq!(
        baseline.stderr, after.stderr,
        "stderr changed after adding nonmatching C/C++ files"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2: Ruby enters owner-only dispatch. Inline evidence renders
/// exact `#` (instance) and `.` (singleton) display with full `::` container
/// paths; no Go mechanical-call appendix, no canonical-selector promotion, and
/// the non-Go honesty caveat is present.
#[test]
fn discover_text_or_ruby_attributes_owners_without_go_call_appendix() {
    let dir = temp_repo("discover_text_or_ruby_owner");
    fs::write(
        dir.join("app.rb"),
        "module Billing\n\
         \x20 class Invoice\n\
         \x20\x20 def paid?\n\
         \x20\x20\x20 compute_total\n\
         \x20\x20 end\n\
         \x20\x20 def self.find(id)\n\
         \x20\x20\x20 query\n\
         \x20\x20 end\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "compute_total,query",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Exact per-file owner evidence: `#` for instance, `.` for singleton.
    assert!(
        stdout.contains("app.rb:4 [owner Billing::Invoice#paid?@3-5]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("app.rb:7 [owner Billing::Invoice.find@6-8]"),
        "{stdout}"
    );
    // No Go call appendix is emitted for a Ruby-only query.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        !stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    // Ruby display punctuation is never promoted to a copyable canonical
    // selector.
    assert!(
        !stdout.contains("## Confirmed structural targets"),
        "{stdout}"
    );
    // Non-Go honesty caveat present, with no call-analysis claim.
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );
    assert!(
        stdout.contains("no call analysis was run for non-Go languages"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2: compact rollup renders Ruby owners for instance/singleton
/// methods, and the exact command replays byte-identically.
#[test]
fn discover_text_or_ruby_compact_rollup_replays_byte_identically() {
    let dir = temp_repo("discover_ruby_compact_replay");
    fs::write(
        dir.join("app.rb"),
        "class A\n\
         \x20 def run\n\
         \x20\x20 work\n\
         \x20 end\n\
         \x20 def self.find\n\
         \x20\x20 search\n\
         \x20 end\n\
         \x20 def stop\n\
         \x20\x20 halt\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();

    let run = |d: &Path| {
        srcwalk()
            .args([
                "discover",
                "work,search,halt",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(d)
            .output()
            .unwrap()
    };
    let first = run(&dir);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        stdout.contains("owners (#N=Nth query term; *K=hits)"),
        "{stdout}"
    );
    assert!(stdout.contains("A#run:2-4[#1]"), "{stdout}");
    assert!(stdout.contains("A.find:5-7[#2]"), "{stdout}");
    assert!(stdout.contains("A#stop:8-10[#3]"), "{stdout}");
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let second = run(&dir);
    assert_eq!(
        first.stdout, second.stdout,
        "stdout differs across identical replays"
    );
    assert_eq!(
        first.stderr, second.stderr,
        "stderr differs across identical replays"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2: ordinary blocks/procs/lambdas are transparent (hits inside
/// them inherit the enclosing method), while dynamic metaprogramming hazards
/// and their attached bodies emit zero owner evidence — including nested
/// recovered definitions.
#[test]
fn discover_text_or_ruby_blocks_transparent_metaprogramming_barriers() {
    let dir = temp_repo("discover_ruby_block_hazard");
    fs::write(
        dir.join("service.rb"),
        "def process\n\
         \x20 items.each do |item|\n\
         \x20\x20 handle(item)\n\
         \x20 end\n\
         \x20 define_method(:dyn) { }\n\
         end\n\
         class_eval do\n\
         \x20 def leaked\n\
         \x20\x20 secret\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "handle(item),define_method(:dyn),secret",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The transparent block inherits the enclosing method owner: the compact
    // rollup attributes term #1 (handle(item), line 3 inside the block) to
    // `process`, while the define_method hazard line and the class_eval body
    // contribute no owner evidence.
    assert!(stdout.contains("process:1-6[#1]"), "{stdout}");
    assert!(stdout.contains("owners (#N"), "{stdout}");
    assert!(!stdout.contains("service.rb:5 [owner"), "{stdout}");
    assert!(!stdout.contains("[#2"), "{stdout}");
    assert!(!stdout.contains("[#3"), "{stdout}");
    // `leaked` is never rendered as an owner (the class_eval block and its
    // nested recovered definition never regain a name).
    assert!(!stdout.contains("leaked"), "{stdout}");
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2 re-review: qualified and root-qualified container names never
/// fabricate a component through the CLI. `module A; class A::B` merges on the
/// suffix witness (`A::B`, never `A::A::B`), a witness-free `module C;
/// class A::B` barriers (no `C::A::B`, no shortened owner), and `class
/// ::Rooted` is absolute inside a module (`Rooted`, never `M::Rooted`).
#[test]
fn discover_text_or_ruby_qualified_containers_never_fabricate_components() {
    let dir = temp_repo("discover_ruby_qualified_containers");
    fs::write(
        dir.join("merged.rb"),
        "module A\n\
         \x20 class A::B\n\
         \x20\x20 def hit_method\n\
         \x20\x20\x20 duplicate_container_hit\n\
         \x20\x20 end\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();
    fs::write(
        dir.join("barriered.rb"),
        "module C\n\
         \x20 class A::B\n\
         \x20\x20 def unprovable_method\n\
         \x20\x20\x20 unprovable_container_hit\n\
         \x20\x20 end\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();
    fs::write(
        dir.join("rooted.rb"),
        "module M\n\
         \x20 class ::Rooted\n\
         \x20\x20 def rooted_method\n\
         \x20\x20\x20 rooted_container_hit\n\
         \x20\x20 end\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "duplicate_container_hit,unprovable_container_hit,rooted_container_hit",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Suffix-witness merge: the duplicated component appears exactly once.
    assert!(stdout.contains("A::B#hit_method:3-5"), "{stdout}");
    assert!(!stdout.contains("A::A::B"), "{stdout}");
    // Root-qualified is absolute: no lexical prefix.
    assert!(stdout.contains("Rooted#rooted_method:3-5"), "{stdout}");
    assert!(!stdout.contains("M::Rooted"), "{stdout}");
    // Witness-free qualified container: no guessed path, no shortened owner,
    // and no owner evidence of any kind for that hit.
    assert!(!stdout.contains("C::A::B"), "{stdout}");
    assert!(!stdout.contains("unprovable_method"), "{stdout}");
    assert!(!stdout.contains("barriered.rb:4 [owner"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2: a mixed Go + Ruby + C/C++ scope keeps the mechanical-call
/// appendix Go-only. Ruby owners are attributed but never enter call-edge
/// analysis, and no non-Go zero-edge/call claim is made.
#[test]
fn discover_text_or_go_plus_ruby_c_cpp_mixed_keeps_edges_go_only() {
    let dir = temp_repo("discover_go_ruby_c_cpp_mixed");
    fs::write(
        dir.join("feature.go"),
        "package feature\nfunc First() { /* alpha */ }\nfunc Second() { /* beta */ First() }\n",
    )
    .unwrap();
    fs::write(
        dir.join("service.rb"),
        "class Service\n\
         \x20 def load\n\
         \x20\x20 alpha_hit\n\
         \x20 end\n\
         \x20 def handle\n\
         \x20\x20 beta_hit\n\
         \x20 end\n\
         end\n",
    )
    .unwrap();
    fs::write(
        dir.join("app.c"),
        "int load_c(void) {\n    return 1; /* alpha */\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // All three languages carry exact per-file owner evidence.
    assert!(
        stdout.contains("feature.go:2 [owner First@2-2]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("service.rb:3 [owner Service#load@2-4]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("service.rb:6 [owner Service#handle@5-7]"),
        "{stdout}"
    );
    assert!(stdout.contains("app.c:2 [owner load_c@1-3]"), "{stdout}");
    // The Go mechanical-call appendix exists and the single rendered edge is
    // Go-only.
    assert!(stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(
        stdout.contains("- [bare] Second calls First@feature.go:3; candidate First@:2-2"),
        "{stdout}"
    );
    let go_edge_rows = stdout
        .lines()
        .filter(|l| l.starts_with("- [") && l.contains(" calls "))
        .count();
    assert_eq!(go_edge_rows, 1, "{stdout}");
    // No non-Go zero-edge or call claim; Ruby/C never enter candidate sets.
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");
    assert!(!stdout.contains("candidate Service#"), "{stdout}");
    assert!(!stdout.contains("candidate load_c@"), "{stdout}");
    // Both honesty caveats present.
    assert!(
        stdout.contains("structural owner and mechanically filtered"),
        "{stdout}"
    );
    assert!(
        stdout.contains("structural lexical ownership candidates"),
        "{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// US-072 Wave 2: adding nonmatching Ruby files to the scope does not change
/// any pre-existing stdout/stderr bytes.
#[test]
fn discover_text_or_ruby_nonmatching_files_do_not_change_output() {
    let dir = temp_repo("discover_ruby_nonmatching");
    fs::write(dir.join("app.rb"), "def alpha\n  hit\nend\n").unwrap();

    let run = |d: &Path| {
        srcwalk()
            .args([
                "discover", "alpha", "--match", "any", "--as", "text", "--scope",
            ])
            .arg(d)
            .output()
            .unwrap()
    };
    let baseline = run(&dir);
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    // Add nonmatching Ruby files (no query-term text).
    fs::write(
        dir.join("unrelated.rb"),
        "class Unrelated\n  def helper\n    work\n  end\nend\n",
    )
    .unwrap();
    fs::write(
        dir.join("another.rb"),
        "module Other\n  def run; end\nend\n",
    )
    .unwrap();

    let after = run(&dir);
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    assert_eq!(
        baseline.stdout, after.stdout,
        "stdout changed after adding nonmatching Ruby files"
    );
    assert_eq!(
        baseline.stderr, after.stderr,
        "stderr changed after adding nonmatching Ruby files"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ---- US-073: owner abstention transparency ----

/// The three owner states are distinguishable in one detailed Text OR run:
/// a named owner keeps its exact tag, an analyzed-but-abstained hit is
/// summarized once per file, and an unsupported language stays silent.
#[test]
fn text_or_distinguishes_named_abstained_and_unsupported_owner_states() {
    let dir = temp_repo("owner_abstain_three_states");
    fs::write(
        dir.join("a.py"),
        "X = 1  # alpha\ndef f():\n    return 2  # alpha\n",
    )
    .unwrap();
    fs::write(dir.join("notes.txt"), "alpha here\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Named: existing inline owner tag, unchanged.
    assert!(stdout.contains("a.py:3 [owner f@2-3]"), "{stdout}");
    // Abstained: one bounded file line naming the parser-known reason.
    assert!(stdout.contains("\n## Owner abstentions\n"), "{stdout}");
    assert!(stdout.contains("a.py — top-level ×1"), "{stdout}");
    // Unsupported: no owner tag and no abstention line for the text file.
    assert!(stdout.contains("notes.txt:1 — alpha here"), "{stdout}");
    assert!(!stdout.contains("notes.txt —"), "{stdout}");
    assert!(!stdout.contains("notes.txt ["), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// Compact mode: a fully abstained file states `owners: none`, a mixed file
/// keeps its exact owners line and adds only the abstention line, and the
/// abstention line sits before windows.
#[test]
fn text_or_compact_renders_all_abstained_and_mixed_owner_shapes() {
    let dir = temp_repo("owner_abstain_compact");
    fs::write(
        dir.join("mixed.py"),
        "TOP = 1  # alpha\ndef f():\n    return 2  # beta\n",
    )
    .unwrap();
    fs::write(dir.join("top.py"), "A = 1  # alpha\nB = 2  # beta\n").unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta,= 1",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Mixed file: exact owners line preserved, abstention line added after it.
    assert!(
        stdout.contains(
            "  owners (#N=Nth query term; *K=hits): f:2-3[#2]\n  owner abstentions: top-level ×2"
        ),
        "{stdout}"
    );
    // Fully abstained file: `owners: none` shape.
    assert!(
        stdout.contains("  owners: none — abstained (top-level ×3)"),
        "{stdout}"
    );
    // Placement: the abstention line precedes windows for the mixed file.
    let abstain_at = stdout.find("owner abstentions:").unwrap();
    let windows_at = stdout[abstain_at..].find("windows:").unwrap();
    assert!(windows_at > 0, "{stdout}");
    // Compact mode adds no detailed section.
    assert!(!stdout.contains("## Owner abstentions"), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// Backward safety: a named-only result gains zero new lines, and identical
/// commands replay byte-identically (Unix and Windows).
#[test]
fn text_or_named_only_output_is_unchanged_and_replays_byte_identically() {
    let dir = temp_repo("owner_abstain_named_only");
    fs::write(
        dir.join("only.py"),
        "def f():\n    return 1  # alpha\n\ndef g():\n    return 2  # beta\n",
    )
    .unwrap();

    let run = || {
        let output = srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(output.status.success());
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    };
    let (stdout, stderr) = run();
    // Every shown hit is named, so no abstention line or section may appear.
    assert!(stdout.contains("only.py:2 [owner f@1-2]"), "{stdout}");
    assert!(stdout.contains("only.py:5 [owner g@4-5]"), "{stdout}");
    assert!(!stdout.contains("Owner abstentions"), "{stdout}");
    assert!(!stdout.contains("owner abstentions"), "{stdout}");
    assert!(!stdout.contains("owners: none"), "{stdout}");
    // Deterministic replay.
    let (stdout2, stderr2) = run();
    assert_eq!(stdout, stdout2);
    assert_eq!(stderr, stderr2);

    let _ = fs::remove_dir_all(&dir);
}

/// Go isolation: an abstained Go hit never creates a call appendix, an edge, or
/// a zero-edge claim, and a malformed Go file reports `parse-failed`.
#[test]
fn text_or_go_abstentions_do_not_widen_call_evidence() {
    let dir = temp_repo("owner_abstain_go_isolation");
    fs::write(
        dir.join("broken.go"),
        "package p\nfunc F( {\n    // alpha\n}\n",
    )
    .unwrap();

    let output = srcwalk()
        .args([
            "discover",
            "alpha,beta",
            "--match",
            "any",
            "--as",
            "text",
            "--scope",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("broken.go — parse-failed ×1"), "{stdout}");
    // No Go call evidence of any kind for an unparsed file.
    assert!(!stdout.contains("## Mechanical Go calls"), "{stdout}");
    assert!(!stdout.contains(" calls "), "{stdout}");
    assert!(!stdout.contains(OWNER_LINK_ZERO_EDGE), "{stdout}");

    let _ = fs::remove_dir_all(&dir);
}

/// Every routed owner language reaches the shared decision end-to-end: each
/// fixture's top-level hit is reported as `top-level` for its own file.
#[test]
fn text_or_owner_abstentions_cover_every_routed_language() {
    // The detailed section is intentionally bounded, so the 13 routed languages
    // are exercised in batches that fit under that cap.
    let batches: &[&[(&str, &str)]] = &[
        &[
            ("a.go", "package p\n\nvar X = 1 // alpha\n"),
            ("a.py", "X = 1  # alpha\n"),
            ("a.rs", "const X: u8 = 1; // alpha\n"),
            ("a.js", "const X = 1; // alpha\n"),
            ("a.ts", "const X: number = 1; // alpha\n"),
            ("a.tsx", "const X = 1; // alpha\n"),
            ("A.java", "class A {\n    int x = 1; // alpha\n}\n"),
        ],
        &[
            ("a.kt", "val x = 1 // alpha\n"),
            ("A.cs", "class A {\n    int x = 1; // alpha\n}\n"),
            ("a.php", "<?php\n$x = 1; // alpha\n"),
            ("a.c", "int x = 1; /* alpha */\n"),
            ("a.cpp", "int y = 1; /* alpha */\n"),
            ("a.rb", "X = 1 # alpha\n"),
        ],
    ];
    for (batch_index, cases) in batches.iter().enumerate() {
        let dir = temp_repo(&format!("owner_abstain_lang_matrix_{batch_index}"));
        for (name, source) in cases.iter() {
            fs::write(dir.join(name), source).unwrap();
        }

        let output = srcwalk()
            .args([
                "discover",
                "alpha,beta",
                "--match",
                "any",
                "--as",
                "text",
                "--scope",
            ])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("## Owner abstentions"), "{stdout}");
        for (name, _) in cases.iter() {
            assert!(
                stdout.contains(&format!("{name} — top-level ×1")),
                "{name} missing from abstentions:\n{stdout}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// The abstention section stays inside a tight budget through the existing
/// footer-preserving path.
#[test]
fn text_or_owner_abstentions_survive_a_tight_budget() {
    let dir = temp_repo("owner_abstain_budget");
    for i in 0..12 {
        fs::write(
            dir.join(format!("f{i:02}.py")),
            "X = 1  # alpha\nY = 2  # beta\n",
        )
        .unwrap();
    }

    for budget in ["60", "400"] {
        let output = srcwalk()
            .args(["discover", "alpha,beta", "--match", "any", "--as", "text"])
            .args(["--budget", budget])
            .arg("--scope")
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "budget {budget} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // The footer-preserving budget path keeps the header and the trailing
        // next-action footer at every budget.
        assert!(stdout.contains("# Text OR:"), "budget {budget}: {stdout}");
        assert!(
            stdout.contains("> Next: read raw hit evidence"),
            "budget {budget} lost its footer: {stdout}"
        );
        // Any surviving abstention row is complete (`PATH — reason ×N`), never a
        // dangling label. Only rows inside the section are checked; the header
        // line legitimately contains the same em-dash separator.
        if let Some(section) = stdout.split("## Owner abstentions").nth(1) {
            for line in section
                .lines()
                .take_while(|line| !line.starts_with("> Next"))
                .filter(|line| line.contains(" — "))
            {
                assert!(
                    line.contains(" ×"),
                    "budget {budget} emitted a partial abstention row `{line}`: {stdout}"
                );
            }
        }
    }

    let _ = fs::remove_dir_all(&dir);
}
