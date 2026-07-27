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
        stdout.contains("> Next: srcwalk show 'lib.rs:1-3,lib.rs:7-9'"),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk show 'lib.rs:1-3,lib.rs:7-9'")
            .count(),
        1,
        "confirmed structural next action should be deduplicated:\n{stdout}"
    );
    assert!(
        stdout.contains("read the confirmed structural target above"),
        "discover footer should route structural candidates to show first:\n{stdout}"
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
    assert!(
        stdout.contains("read the confirmed structural target above"),
        "existing prose footer must remain unchanged:\n{stdout}"
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
        stdout.contains("> Next: srcwalk show 'a/lib.rs:1-1,b/lib.rs:1-1'"),
        "expected one batched show next action:\n{stdout}"
    );
    assert_eq!(
        stdout
            .matches("> Next: srcwalk show 'a/lib.rs:1-1,b/lib.rs:1-1'")
            .count(),
        1,
        "batched show next action should be deduplicated:\n{stdout}"
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
        stdout.contains("> Next: srcwalk show 'a file.rs:1-1'"),
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
        stdout.contains("> Next: srcwalk show 'a,file.rs' --section 1-1"),
        "comma-bearing path must use --section instead of ambiguous inline target:\n{stdout}"
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
        stdout.contains("> Next: srcwalk show 'server.ts:3-8,server.ts:10-12'"),
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
fn path_symbol_rejections_preserve_command_intent() {
    let dir = temp_repo("path_symbol_rejections");
    fs::write(
        dir.join("lib.rs"),
        "fn target() {\n    helper();\n}\nfn helper() {}\n",
    )
    .unwrap();

    let trace = srcwalk()
        .args(["trace", "callees", "lib.rs:target", "--detailed", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!trace.status.success());
    let trace_stderr = String::from_utf8_lossy(&trace.stderr);
    assert!(
        trace_stderr.contains("> Did you mean: target"),
        "{trace_stderr}"
    );
    assert!(
        trace_stderr.contains("`path:symbol` targets are accepted by `context`, not `trace`."),
        "{trace_stderr}"
    );
    assert!(
        trace_stderr.contains("srcwalk trace callees target --detailed --scope"),
        "{trace_stderr}"
    );

    let show = srcwalk()
        .args(["show", "lib.rs:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!show.status.success());
    let show_stderr = String::from_utf8_lossy(&show.stderr);
    assert!(
        show_stderr.contains("did you mean: lib.rs:1-3"),
        "{show_stderr}"
    );
    assert!(
        show_stderr.contains("`show` takes line ranges; `path:symbol` is `context` grammar."),
        "{show_stderr}"
    );
    assert!(!show_stderr.contains("trace callees"), "{show_stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ambiguous_path_symbol_trace_names_candidates_without_runnable_command() {
    let dir = temp_repo("path_symbol_trace_ambiguous");
    fs::write(dir.join("a.rs"), "fn target() {}\n").unwrap();
    fs::write(dir.join("b.rs"), "fn target() {}\n").unwrap();

    let output = srcwalk()
        .args(["trace", "callees", "a.rs:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Candidates:"), "{stderr}");
    assert!(stderr.contains("a.rs:1-1"), "{stderr}");
    assert!(stderr.contains("b.rs:1-1"), "{stderr}");
    assert!(!stderr.contains("For this symbol:"), "{stderr}");
    assert!(!stderr.contains("Did you mean: target"), "{stderr}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_path_symbol_show_explains_grammar_without_guessing_range() {
    let dir = temp_repo("path_symbol_unsupported");
    fs::write(dir.join("plain.txt"), "target text\n").unwrap();

    let output = srcwalk()
        .args(["show", "plain.txt:target", "--scope"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`show` takes line ranges; `path:symbol` is `context` grammar."),
        "{stderr}"
    );
    assert!(!stderr.contains("did you mean:"), "{stderr}");

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
