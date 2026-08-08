//! End-to-end proof of Ruby `require` / `require_relative` dependency edges.
//!
//! Builds an isolated Ruby repo fixture (Gemfile + lib/) and asserts the CLI
//! `deps` output: forward local edges via the parser-backed resolver, external
//! classification of unresolved bare `require`, reverse `Used by` with a
//! file-level label, dedupe/ordering, >8 requires under a sufficient budget,
//! and that comments/string mentions/receiver calls never create edges.

use std::fs;
use std::path::Path;
use std::process::Command;

fn srcwalk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_srcwalk"))
}

/// Create an isolated Ruby repo fixture and return its root path. Each call
/// gets a unique root so parallel tests never share a directory.
fn fixture() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("srcwalk-ruby-deps-{}-{unique}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::create_dir_all(root.join("app")).unwrap();
    fs::write(root.join("Gemfile"), "source :rubygems\n").unwrap();

    fs::write(
        root.join("lib/order.rb"),
        "class Order\n  def total\n    1\n  end\nend\n",
    )
    .unwrap();
    fs::write(root.join("lib/line_item.rb"), "class LineItem\nend\n").unwrap();
    fs::write(root.join("lib/local_gem.rb"), "module LocalGem\nend\n").unwrap();
    root
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn deps_output(root: &Path, target: &str) -> String {
    let out = srcwalk()
        .arg("deps")
        .arg(root.join(target))
        .arg("--scope")
        .arg(root)
        .output()
        .expect("run deps");
    assert!(
        out.status.success(),
        "deps failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

#[test]
fn forward_require_relative_and_bare_local_show_local_paths() {
    let root = fixture();
    write(
        &root,
        "lib/main.rb",
        "require_relative './order'\nrequire_relative './line_item'\nrequire 'local_gem'\n",
    );

    let stdout = deps_output(&root, "lib/main.rb");
    assert!(stdout.contains("order.rb"), "missing order.rb:\n{stdout}");
    assert!(
        stdout.contains("line_item.rb"),
        "missing line_item.rb:\n{stdout}"
    );
    assert!(
        stdout.contains("local_gem.rb"),
        "missing local_gem.rb:\n{stdout}"
    );
    assert!(
        stdout.contains("0 external"),
        "unexpected external:\n{stdout}"
    );
}

#[test]
fn unresolved_bare_require_is_external_but_require_relative_is_not() {
    let root = fixture();
    write(
        &root,
        "lib/main.rb",
        "require 'json'\nrequire_relative './missing'\nrequire \"#{dynamic}\"\nrequire_relative File.expand_path('x')\n",
    );

    let stdout = deps_output(&root, "lib/main.rb");
    assert!(
        stdout.contains("json"),
        "json should be external:\n{stdout}"
    );
    assert!(
        !stdout.contains("missing"),
        "unresolved require_relative must not be external:\n{stdout}"
    );
    // Dynamic/interpolated/expand forms never become local or external edges.
    assert!(
        stdout.contains("0 local"),
        "dynamic forms must not be local:\n{stdout}"
    );
    let external_lines = stdout.lines().filter(|l| l.trim() == "json").count();
    assert_eq!(
        external_lines, 1,
        "external should list json exactly once:\n{stdout}"
    );
}

#[test]
fn resolved_bare_require_is_not_duplicated_external() {
    let root = fixture();
    write(
        &root,
        "lib/main.rb",
        "require 'order'\nrequire_relative './order'\n",
    );

    let stdout = deps_output(&root, "lib/main.rb");
    // order.rb resolved locally; never listed in external.
    assert!(
        stdout.contains("order.rb"),
        "missing local order.rb:\n{stdout}"
    );
    assert!(
        !stdout.contains("## Uses (external)\norder"),
        "bare require must not leak external:\n{stdout}"
    );
    assert!(
        stdout.contains("0 external"),
        "unexpected external:\n{stdout}"
    );
}

#[test]
fn reverse_used_by_finds_require_relative_dependent_even_without_symbols() {
    let root = fixture();
    // Target has no definitions/calls at all.
    write(
        &root,
        "lib/config.rb",
        "# frozen_string_literal: true\n\nDEFAULTS = { a: 1 }\n",
    );
    write(
        &root,
        "app/loader.rb",
        "require_relative '../lib/config'\n\ndef boot\nend\n",
    );

    let stdout = deps_output(&root, "lib/config.rb");
    assert!(
        stdout.contains("loader.rb"),
        "missing reverse dependent:\n{stdout}"
    );
    // File-level label, not a fabricated method owner.
    assert!(
        stdout.contains("<file>"),
        "require edge must use file-level label:\n{stdout}"
    );
    assert!(
        stdout.contains("require_relative ../lib/config"),
        "label should preserve source text:\n{stdout}"
    );
}

#[test]
fn reverse_used_by_preserves_exact_line() {
    let root = fixture();
    write(&root, "lib/target.rb", "class Target\nend\n");
    write(
        &root,
        "app/use.rb",
        "x = 1\nrequire_relative '../lib/target'\ny = 2\n",
    );

    let stdout = deps_output(&root, "lib/target.rb");
    assert!(
        stdout.contains("use.rb:2"),
        "missing exact dependent line:\n{stdout}"
    );
}

#[test]
fn reverse_used_by_dedupes_same_label_keeping_earliest_line() {
    let root = fixture();
    write(&root, "lib/target.rb", "class Target\nend\n");
    // Same file requires the same source on lines 1 and 2; both resolve to
    // the target, so the reverse edge must render ONCE at line 1 (the
    // earliest), not twice with a repeated `<file>` label.
    write(
        &root,
        "app/dup.rb",
        "require_relative '../lib/target'\nrequire_relative '../lib/target'\n",
    );

    let stdout = deps_output(&root, "lib/target.rb");
    let label_count = stdout.matches("require_relative ../lib/target").count();
    assert_eq!(
        label_count, 1,
        "duplicate same-label reverse requires must collapse to one:\n{stdout}"
    );
    assert!(
        stdout.contains("dup.rb:1"),
        "kept label must point at the earliest line:\n{stdout}"
    );
}

#[test]
fn duplicates_dedupe_and_order_is_deterministic() {
    let root = fixture();
    write(
        &root,
        "lib/main.rb",
        "require_relative './order'\nrequire_relative './line_item'\nrequire_relative './order'\n",
    );

    let stdout = deps_output(&root, "lib/main.rb");
    let count_order = stdout.matches("order.rb").count();
    assert_eq!(count_order, 1, "order.rb duplicated:\n{stdout}");
    assert!(
        stdout.contains("line_item.rb"),
        "missing line_item.rb:\n{stdout}"
    );
    // Deterministic: line_item.rb before order.rb (sorted by full path, 'l' < 'o').
    let idx_order = stdout.find("order.rb").expect("order.rb present");
    let idx_line = stdout.find("line_item.rb").expect("line_item.rb present");
    assert!(
        idx_line < idx_order,
        "line_item should sort before order:\n{stdout}"
    );
}

#[test]
fn more_than_eight_requires_all_resolve() {
    let root = fixture();
    let mut content = String::new();
    for i in 0..10 {
        write(&root, &format!("lib/mod{i}.rb"), "module M\nend\n");
        content.push_str(&format!("require_relative './mod{i}'\n"));
    }
    write(&root, "lib/main.rb", &content);

    let stdout = deps_output(&root, "lib/main.rb");
    for i in 0..10 {
        assert!(
            stdout.contains(&format!("mod{i}.rb")),
            "missing mod{i}:\n{stdout}"
        );
    }
}

#[test]
fn comments_string_mentions_and_receiver_calls_do_not_create_edges() {
    let root = fixture();
    write(
        &root,
        "lib/main.rb",
        "# require_relative './order'\ns = \"require 'json'\"\nloader.require 'order'\n",
    );

    let stdout = deps_output(&root, "lib/main.rb");
    assert!(
        stdout.contains("0 local"),
        "comment/string/receiver must not be local:\n{stdout}"
    );
    assert!(
        stdout.contains("0 external"),
        "comment/string/receiver must not be external:\n{stdout}"
    );
}

#[test]
fn nearest_package_boundary_ambiguity_does_not_guess() {
    let root = fixture();
    // Two separate packages in one repo: pkg_a has lib/shared.rb, pkg_b also has
    // lib/shared.rb. A require from pkg_a must stay within pkg_a's nearest
    // package boundary and not guess across the monorepo.
    write(&root, "pkg_a/Gemfile", "source :rubygems\n");
    write(&root, "pkg_a/lib/shared.rb", "module AShared\nend\n");
    write(&root, "pkg_b/Gemfile", "source :rubygems\n");
    write(&root, "pkg_b/lib/shared.rb", "module BShared\nend\n");
    write(&root, "pkg_a/lib/main.rb", "require 'shared'\n");

    let stdout = deps_output(&root, "pkg_a/lib/main.rb");
    // Resolves only within pkg_a's boundary to pkg_a/lib/shared.rb.
    assert!(stdout.contains("pkg_a/"), "expected pkg_a path:\n{stdout}");
    assert!(
        !stdout.contains("pkg_b/"),
        "must not cross package boundary:\n{stdout}"
    );
}
