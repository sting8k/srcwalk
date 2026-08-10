use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    path::Path,
    process,
};

// mimalloc: faster than system allocator for parallel walker workloads
// where many small Strings/Vecs are allocated across rayon threads.
// Keep the system allocator on Windows: native Windows ARM64 crashed in
// walker/search commands with mimalloc as the global allocator.
#[cfg(not(windows))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod cli_run;
mod output;
mod version;

use clap::{error::ErrorKind, CommandFactory, Parser};
use cli::{Cli, Command, RunConfig, GUIDE};
/// Reset SIGPIPE to the OS default on Unix.
///
/// Rust's stdlib masks SIGPIPE to SIG_IGN at startup, which turns broken-pipe
/// into an `EPIPE` error that `println!` converts into a panic. For a CLI that
/// is routinely piped into `head`, `less`, or a truncating UI, that's the wrong
/// default: we want the process to exit silently like every other Unix tool.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: setting a signal disposition is a standard, thread-safe operation
    // before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

fn root_scope_was_overridden(cli: &Cli) -> bool {
    cli.scope.len() != 1
        || cli
            .scope
            .first()
            .is_some_and(|scope| scope != Path::new("."))
}

fn reject_root_options_before_subcommand(cli: &Cli) {
    if cli.command.is_none() {
        return;
    }

    let has_root_only_option = cli.query.is_some()
        || root_scope_was_overridden(cli)
        || cli.section.is_some()
        || cli.budget.is_some()
        || cli.no_budget
        || cli.full
        || cli.artifact;
    if !has_root_only_option {
        return;
    }

    eprintln!(
        "error: root-level options do not apply to subcommands; put options after the subcommand"
    );
    eprintln!(
        "hint: use `srcwalk discover QUERY --scope DIR`, not `srcwalk --scope DIR discover QUERY`"
    );
    process::exit(2);
}

fn looks_like_line_or_range(value: &str) -> bool {
    if value.parse::<u32>().is_ok_and(|line| line > 0) {
        return true;
    }
    value.split_once('-').is_some_and(|(start, end)| {
        let Ok(start) = start.parse::<u32>() else {
            return false;
        };
        let Ok(end) = end.parse::<u32>() else {
            return false;
        };
        start > 0 && end >= start
    })
}

fn line_target_path(value: &str) -> Option<&str> {
    let (path, section) = value.rsplit_once(':')?;
    (!path.is_empty() && looks_like_line_or_range(section)).then_some(path)
}

fn normalize_show_target_group(value: &OsStr) -> Option<Vec<String>> {
    let value = value.to_str()?;
    let mut normalized = Vec::new();
    let mut shorthand_path = None;

    for part in value.split(',').map(str::trim) {
        if part.is_empty() {
            return None;
        }
        if let Some(path) = line_target_path(part) {
            shorthand_path = Some(path);
            normalized.push(part.to_string());
        } else if Path::new(part).try_exists().unwrap_or(false) {
            shorthand_path = None;
            normalized.push(part.to_string());
        } else if looks_like_line_or_range(part) {
            normalized.push(format!("{}:{part}", shorthand_path?));
        } else {
            return None;
        }
    }

    Some(normalized)
}

/// Whether `discover` would accept repeated `--scope` (symbol/search mode).
/// Every other route takes exactly one scope; the rejection hint must teach
/// the correct shape per route without guessing past the argv evidence.
fn discover_accepts_repeated_scope(args: &[OsString]) -> bool {
    let explicit_symbol = args
        .windows(2)
        .any(|pair| pair[0] == OsStr::new("--as") && pair[1] == OsStr::new("symbol"))
        || args.iter().any(|arg| arg == OsStr::new("--as=symbol"));
    if explicit_symbol {
        return true;
    }
    // No `--as`: bare discover defaults to Search unless the query is a file
    // glob or comma-separated text OR, or `--match all` was passed.
    let has_as = args.windows(2).any(|pair| pair[0] == OsStr::new("--as"))
        || args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("--as="));
    if has_as {
        return false;
    }
    let match_all = args
        .windows(2)
        .any(|pair| pair[0] == OsStr::new("--match") && pair[1] == OsStr::new("all"))
        || args.iter().any(|arg| arg == OsStr::new("--match=all"));
    if match_all {
        return false;
    }
    if let Some(query) = args.get(2).and_then(|arg| arg.to_str()) {
        if query.contains(',') || query.contains('*') || query.contains('?') || query.contains('"')
        {
            return false;
        }
    }
    true
}

/// Space-separated `--scope` values (`srcwalk discover foo --scope src tests`)
/// die at clap with a generic `unexpected argument 'tests'`. When every value
/// trailing a `--scope <existing-dir>` is itself an existing directory, treat
/// argv as a mis-typed multi-scope attempt and print route-aware corrections:
/// `discover` symbol/search repeats `--scope`; all other routes need one
/// command per search root. Ambiguous traps (non-directory values, no `--scope`
/// at all) stay silent and keep clap's original error.
fn scope_space_separated_hint(args: &[OsString]) -> Option<String> {
    // A positional query/target is required before flags; otherwise the error
    // is about a missing argument, not a stray scope value.
    if args.get(2)?.to_str()?.starts_with('-') {
        return None;
    }
    let mut pairs = Vec::new(); // (--scope flag idx, existing-dir value idx)
    let mut extras = Vec::new(); // bare existing-dir tokens trailing a pair
    let mut scope_values: Vec<&str> = Vec::new();
    let mut index = 2;
    while index < args.len() {
        let text = args.get(index)?.to_str()?;
        let value_start = if text == "--scope" {
            let value = args.get(index + 1)?.to_str()?;
            if !Path::new(value).is_dir() {
                index += 2;
                continue;
            }
            pairs.push((index, index + 1));
            scope_values.push(value);
            index + 2
        } else if let Some(value) = text.strip_prefix("--scope=") {
            if !Path::new(value).is_dir() {
                index += 1;
                continue;
            }
            pairs.push((index, index));
            scope_values.push(value);
            index + 1
        } else {
            index += 1;
            continue;
        };

        let mut extra = value_start;
        while let Some(candidate) = args.get(extra).and_then(|arg| arg.to_str()) {
            if candidate.starts_with('-') || !Path::new(candidate).is_dir() {
                break;
            }
            extras.push(extra);
            scope_values.push(candidate);
            extra += 1;
        }
        index = extra.max(index + 1);
    }
    if extras.is_empty() {
        return None;
    }

    // `discover` symbol/search repeats the flag (supported); every other
    // route needs one command per search root.
    if args.get(1)?.to_str()? == "discover" && discover_accepts_repeated_scope(args) {
        let mut corrected = Vec::new();
        for (index, arg) in args.iter().enumerate().skip(1) {
            if extras.contains(&index) {
                corrected.push("--scope".to_string());
            }
            corrected.push(srcwalk::format::shell_quote_arg(arg.to_str()?)?);
        }
        return Some(format!(
            "hint: `discover --as symbol` requires `--scope` before each search root:\n  srcwalk {}",
            corrected.join(" ")
        ));
    }

    // One-scope route: teach one command per search root in the same round.
    let mut base = Vec::new();
    for (index, arg) in args.iter().enumerate().skip(1) {
        if extras.contains(&index)
            || pairs
                .iter()
                .any(|&(flag, value)| index == flag || index == value)
        {
            continue;
        }
        base.push(srcwalk::format::shell_quote_arg(arg.to_str()?)?);
    }
    let label = route_label(args)?;
    let mut lines = vec![format!(
        "hint: `{label}` accepts one --scope; run one command per search root in the same round (or use a common ancestor --scope):"
    )];
    for value in scope_values {
        let mut cmd = base.clone();
        cmd.push("--scope".to_string());
        cmd.push(srcwalk::format::shell_quote_arg(value)?);
        lines.push(format!("  srcwalk {}", cmd.join(" ")));
    }
    Some(lines.join("\n"))
}

/// Human label for the failing subcommand, e.g. `trace callers` or
/// `discover --as text`, so the hint teaches the exact route's contract.
fn route_label(args: &[OsString]) -> Option<String> {
    let command = args.get(1)?.to_str()?;
    if command == "trace" {
        let relation = args.get(2)?.to_str()?;
        return Some(format!("trace {relation}"));
    }
    if command == "discover" {
        for pair in args.windows(2) {
            if pair[0] == OsStr::new("--as") {
                return Some(format!("discover --as {}", pair[1].to_str()?));
            }
        }
        for arg in args.iter().skip(2) {
            if let Some(kind) = arg.to_str()?.strip_prefix("--as=") {
                return Some(format!("discover --as {kind}"));
            }
        }
    }
    Some(command.to_string())
}

fn cli_rejection_hint(args: &[OsString]) -> Option<String> {
    let command = args.get(1)?.to_str()?;
    if command == "discover"
        || command == "trace"
        || command == "context"
        || command == "assess"
        || command == "overview"
        || command == "compare"
    {
        if let Some(hint) = scope_space_separated_hint(args) {
            return Some(hint);
        }
    }
    if command != "show" {
        return None;
    }
    let target_groups = args
        .get(2..)?
        .iter()
        .take_while(|arg| !arg.to_string_lossy().starts_with('-'))
        .collect::<Vec<_>>();
    if target_groups.len() < 2 {
        return None;
    }

    let mut targets = Vec::new();
    for group in &target_groups {
        targets.extend(normalize_show_target_group(group)?);
    }
    if targets.len() > cli_run::MAX_SHOW_TARGETS {
        return None;
    }

    let merged = srcwalk::format::shell_quote_arg(&targets.join(","))?;
    let trailing = args
        .get(2 + target_groups.len()..)?
        .iter()
        .map(|arg| srcwalk::format::shell_quote_arg(arg.to_str()?))
        .collect::<Option<Vec<_>>>()?;
    let trailing = if trailing.is_empty() {
        String::new()
    } else {
        format!(" {}", trailing.join(" "))
    };
    Some(format!(
        "hint: pass multiple read targets as one comma-separated argument:\n  srcwalk show {merged}{trailing}"
    ))
}

fn parse_cli_or_exit() -> Cli {
    let args = env::args_os().collect::<Vec<_>>();
    match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) if error.kind() == ErrorKind::UnknownArgument => {
            let Some(hint) = cli_rejection_hint(&args) else {
                error.exit();
            };
            let exit_code = error.exit_code();
            let _ = error.print();
            eprintln!("{hint}");
            process::exit(exit_code);
        }
        Err(error) => error.exit(),
    }
}

fn main() {
    reset_sigpipe();
    configure_thread_pools();
    let cli = parse_cli_or_exit();

    // `--version` / `-V` (clap's auto version is disabled so provenance shows).
    if cli.version {
        version::run_version(false);
        return;
    }

    // Shell completions
    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "srcwalk", &mut io::stdout());
        return;
    }

    if matches!(cli.command, Some(Command::Overview(_))) && (cli.budget.is_some() || cli.no_budget)
    {
        eprintln!(
            "error: overview has a fixed 15k token cap; narrow --scope or lower --depth instead"
        );
        process::exit(2);
    }

    reject_root_options_before_subcommand(&cli);

    match &cli.command {
        Some(Command::Guide) => {
            print!("{GUIDE}");
            return;
        }
        Some(Command::Version(cmd)) => {
            version::run_version(cmd.check);
            return;
        }
        _ => {}
    }

    let config = match cli.command {
        Some(command) => RunConfig::from_command(command).expect("non-run command handled above"),
        None => RunConfig::from_root(cli),
    };
    cli_run::run(config);
}

/// Configure rayon global thread pool to limit CPU usage.
///
/// Defaults to min(cores / 2, 6). Override with `SRCWALK_THREADS` env var.
/// This matters for long-lived MCP sessions where back-to-back searches
/// can sustain high CPU (see #27).
fn configure_thread_pools() {
    let num_threads = match srcwalk::threading::configured_rayon_threads() {
        Ok(n) => n,
        Err(message) => {
            eprintln!("error: {message}");
            process::exit(2);
        }
    };

    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_target_shape_does_not_treat_windows_drive_path_as_line_target() {
        assert!(line_target_path(r"C:\a").is_none());
        assert_eq!(line_target_path(r"C:\a:12"), Some(r"C:\a"));
        assert_eq!(line_target_path("src/lib.rs:4-9"), Some("src/lib.rs"));
    }

    #[test]
    fn unrelated_unknown_argument_has_no_hint() {
        let args = [
            OsString::from("srcwalk"),
            OsString::from("discover"),
            OsString::from("query"),
            OsString::from("--json"),
        ];
        assert_eq!(cli_rejection_hint(&args), None);
    }

    #[test]
    fn discover_extra_existing_path_has_no_hint() {
        let args = [
            OsString::from("srcwalk"),
            OsString::from("discover"),
            OsString::from("NextAction"),
            OsString::from("Cargo.toml"),
        ];
        assert_eq!(cli_rejection_hint(&args), None);
    }

    #[test]
    fn discover_symbol_space_separated_scope_repeats_flag() {
        // cwd is the repo; docs/ and src/ are existing dirs.
        let args = [
            OsString::from("srcwalk"),
            OsString::from("discover"),
            OsString::from("foo"),
            OsString::from("--as"),
            OsString::from("symbol"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("src"),
        ];
        let hint = cli_rejection_hint(&args).expect("symbol route should hint");
        assert!(
            hint.contains("requires `--scope` before each search root")
                && hint.contains("--scope docs --scope src"),
            "{hint}"
        );
    }

    #[test]
    fn discover_text_space_separated_scope_splits_commands() {
        let args = [
            OsString::from("srcwalk"),
            OsString::from("discover"),
            OsString::from("foo"),
            OsString::from("--as"),
            OsString::from("text"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("src"),
        ];
        let hint = cli_rejection_hint(&args).expect("text route should hint");
        assert!(
            hint.contains("accepts one --scope")
                && hint.contains("--scope docs")
                && hint.contains("--scope src")
                && !hint.contains("--scope docs --scope src"),
            "{hint}"
        );
    }

    #[test]
    fn trace_space_separated_scope_lists_one_command_per_root() {
        let args = [
            OsString::from("srcwalk"),
            OsString::from("trace"),
            OsString::from("callers"),
            OsString::from("bar"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("src"),
        ];
        let hint = cli_rejection_hint(&args).expect("trace route should hint");
        assert!(
            hint.contains("trace callers` accepts one --scope")
                && hint.contains("--scope docs")
                && hint.contains("--scope src"),
            "{hint}"
        );
    }

    #[test]
    fn context_space_separated_scope_lists_one_command_per_root() {
        let args = [
            OsString::from("srcwalk"),
            OsString::from("context"),
            OsString::from("foo.rs:12"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("src"),
        ];
        let hint = cli_rejection_hint(&args).expect("context route should hint");
        assert!(
            hint.contains("context` accepts one --scope")
                && hint.contains("--scope docs")
                && hint.contains("--scope src"),
            "{hint}"
        );
    }

    #[test]
    fn non_scope_unknown_argument_stays_silent() {
        // `--json` is not a scope value; the argument after `--scope` is not a
        // directory, so no scope mismatch is detected and clap keeps its error.
        let args = [
            OsString::from("srcwalk"),
            OsString::from("trace"),
            OsString::from("callers"),
            OsString::from("bar"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("--json"),
        ];
        assert_eq!(cli_rejection_hint(&args), None);
    }

    #[test]
    fn repeated_scope_flag_stays_unhinted() {
        // `--scope docs --scope src` parses fine at clap; the domain error in
        // cli_run only fires later, so the argv-time rejection hint must stay
        // silent here (no UnknownArgument in practice).
        let args = [
            OsString::from("srcwalk"),
            OsString::from("trace"),
            OsString::from("callers"),
            OsString::from("bar"),
            OsString::from("--scope"),
            OsString::from("docs"),
            OsString::from("--scope"),
            OsString::from("src"),
        ];
        assert_eq!(cli_rejection_hint(&args), None);
    }
}
