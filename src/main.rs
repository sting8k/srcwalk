use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{CommandFactory, Parser};
use clap_complete::Shell;

// mimalloc: faster than system allocator for parallel walker workloads
// where many small Strings/Vecs are allocated across rayon threads.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// srcwalk — Tree-sitter indexed lookups, smart code reading for AI agents.
/// One tool replaces `read_file`, grep, glob, `ast_grep`, and find.
#[derive(Parser)]
#[command(name = "srcwalk", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// File path, symbol name, glob pattern, or text to search.
    query: Option<String>,

    /// Directory to search within or resolve relative paths against.
    #[arg(long, default_value = ".")]
    scope: PathBuf,

    /// Line range or markdown heading (e.g. "45-89" or "## Architecture"). Bypasses smart view.
    #[arg(long)]
    section: Option<String>,

    /// Max tokens in response. Reduces detail to fit.
    /// Default: 5000 when piped (non-TTY). Unlimited for interactive TTY.
    #[arg(long)]
    budget: Option<u64>,

    /// Disable default budget cap (for piped/scripted usage).
    #[arg(long)]
    no_budget: bool,

    /// Force full output (override smart view).
    #[arg(long)]
    full: bool,

    /// Treat QUERY as an exact file path. Fails fast instead of falling back to search/glob.
    #[arg(long, conflicts_with_all = ["callers", "callees", "deps", "map", "expand", "glob"])]
    path_exact: bool,

    /// Machine-readable JSON output.
    #[arg(long)]
    json: bool,

    /// Expand top N search matches with inline source (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    expand: Option<usize>,

    /// File pattern filter (e.g. "*.rs", "!*.test.ts", "*.{go,rs}").
    #[arg(long)]
    glob: Option<String>,

    /// Find all callers of a symbol.
    #[arg(long, conflicts_with_all = ["callees", "deps", "map"])]
    callers: bool,

    /// Show what a symbol calls (forward call graph).
    #[arg(long, conflicts_with_all = ["callers", "deps", "map"])]
    callees: bool,

    /// Show ordered call sites with args and assignment context.
    #[arg(long, requires = "callees")]
    detailed: bool,

    /// BFS depth for --callers/--callees. 1 = direct (default). Capped at 5.
    #[arg(long, value_name = "N")]
    depth: Option<usize>,

    /// Max callers to expand per BFS hop (hub guard). Default: 50.
    #[arg(long, value_name = "K", requires = "callers")]
    max_frontier: Option<usize>,

    /// Max total edges across all BFS hops. Default: 500.
    #[arg(long, value_name = "M", requires = "callers")]
    max_edges: Option<usize>,

    /// Comma-separated symbols to skip as BFS frontier (hub guard).
    /// Default: new,clone,from,into,to_string,drop,fmt,default.
    /// Pass empty string "" to disable.
    #[arg(long, value_name = "CSV", requires = "callers")]
    skip_hubs: Option<String>,

    /// Analyze blast-radius dependencies of a file.
    #[arg(long, conflicts_with_all = ["callers", "callees", "map"])]
    deps: bool,

    /// Generate a structural codebase map.
    #[arg(long, conflicts_with_all = ["callers", "callees", "deps", "expand", "section", "full"])]
    map: bool,

    /// Max results. Default: unlimited (or 50 for interactive TTY).
    /// Applies to: symbol/content/regex/callers search.
    /// NOTE: multi-symbol ("A,B,C") applies the limit per-query, not total.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,

    /// Skip N results (for pagination). Use with --limit.
    #[arg(long, value_name = "N", default_value = "0")]
    offset: usize,

    /// Print shell completions for the given shell.
    #[arg(long, value_name = "SHELL")]
    completions: Option<Shell>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Show the project fingerprint (languages, scale, structural overview).
    Overview,
}

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

fn main() {
    reset_sigpipe();
    configure_thread_pools();
    let cli = Cli::parse();

    // Shell completions
    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "srcwalk", &mut io::stdout());
        return;
    }

    // Subcommands
    if let Some(cmd) = cli.command {
        match cmd {
            Command::Overview => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let output = srcwalk::overview::fingerprint(&cwd);
                if output.is_empty() {
                    eprintln!("No project fingerprint could be generated.");
                    process::exit(1);
                }
                println!("{output}");
            }
        }
        return;
    }

    let is_tty = io::stdout().is_terminal();

    // Effective budget: explicit --budget wins, --no-budget disables,
    // otherwise default 5000 tokens for piped (non-TTY) output.
    let effective_budget = if cli.no_budget {
        None
    } else if cli.budget.is_some() {
        cli.budget
    } else if !is_tty {
        Some(5_000)
    } else {
        None
    };

    // Map mode
    if cli.map {
        let cache = srcwalk::cache::OutlineCache::new();
        let scope = cli.scope.canonicalize().unwrap_or(cli.scope);
        let output = srcwalk::map::generate(&scope, 3, effective_budget, &cache);
        emit_output(&output, is_tty);
        return;
    }

    // CLI mode: single query
    let query = if let Some(q) = cli.query {
        q
    } else {
        eprintln!("usage: srcwalk <query> [--scope DIR] [--section N-M] [--budget N]");
        process::exit(3);
    };

    let cache = srcwalk::cache::OutlineCache::new();
    let scope = cli.scope.canonicalize().unwrap_or(cli.scope);

    // Smart view by default (outline for large files). Use --full to force raw content.
    let full = cli.full;
    let expand = cli.expand.unwrap_or(0);

    // TTY interactive mode: cap at 50 unless user set --limit or --full.
    // Piped / scripted → unlimited so grep/wc/etc. see everything.
    let effective_limit = cli.limit.or({
        if is_tty && !full {
            Some(50)
        } else {
            None
        }
    });

    // Exact file mode: read only this path; never fall back to search/glob.
    if cli.path_exact {
        let result = srcwalk::run_path_exact(
            &query,
            &scope,
            cli.section.as_deref(),
            effective_budget,
            full,
            &cache,
        );
        emit_result(result, &query, cli.json, is_tty);
        return;
    }

    // Callers mode
    if cli.callers {
        let bfs_json = cli.json && matches!(cli.depth, Some(d) if d >= 2);
        let result = srcwalk::run_callers(
            &query,
            &scope,
            expand,
            effective_budget,
            effective_limit,
            cli.offset,
            cli.glob.as_deref(),
            &cache,
            cli.depth,
            cli.max_frontier,
            cli.max_edges,
            cli.skip_hubs.as_deref(),
            bfs_json,
        );
        if bfs_json {
            // run_callers already returns pretty JSON; skip the generic wrapper.
            match result {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(e.exit_code());
                }
            }
            return;
        }
        emit_result(result, &query, cli.json, is_tty);
        return;
    }

    // Callees mode
    if cli.callees {
        let result = srcwalk::run_callees(
            &query,
            &scope,
            effective_budget,
            &cache,
            cli.depth,
            cli.detailed,
        );
        emit_result(result, &query, cli.json, is_tty);
        return;
    }

    // Deps mode
    if cli.deps {
        let path = if Path::new(&query).is_absolute() {
            PathBuf::from(&query)
        } else {
            let scope_path = scope.join(&query);
            if scope_path.exists() {
                scope_path
            } else {
                let cwd_path = std::env::current_dir().unwrap_or_default().join(&query);
                if cwd_path.exists() {
                    cwd_path
                } else {
                    scope_path // fall back, let analyze_deps report the error
                }
            }
        };
        let result = srcwalk::run_deps(&path, &scope, effective_budget, &cache);
        emit_result(result, &query, cli.json, is_tty);
        return;
    }

    let result = if expand > 0 {
        srcwalk::run_expanded(
            &query,
            &scope,
            cli.section.as_deref(),
            effective_budget,
            full,
            expand,
            effective_limit,
            cli.offset,
            cli.glob.as_deref(),
            &cache,
        )
    } else if full {
        srcwalk::run_full(
            &query,
            &scope,
            cli.section.as_deref(),
            effective_budget,
            effective_limit,
            cli.offset,
            cli.glob.as_deref(),
            &cache,
        )
    } else {
        srcwalk::run(
            &query,
            &scope,
            cli.section.as_deref(),
            effective_budget,
            effective_limit,
            cli.offset,
            cli.glob.as_deref(),
            &cache,
        )
    };

    emit_result(result, &query, cli.json, is_tty);
}

fn emit_result(
    result: Result<String, srcwalk::error::SrcwalkError>,
    query: &str,
    json: bool,
    is_tty: bool,
) {
    match result {
        Ok(output) => {
            if json {
                let json = serde_json::json!({
                    "query": query,
                    "output": output,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json)
                        .expect("serde_json::Value is always serializable")
                );
            } else {
                emit_output(&output, is_tty);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(e.exit_code());
        }
    }
}

/// Write output to stdout. When TTY and output is long, pipe through $PAGER.
fn emit_output(output: &str, is_tty: bool) {
    let line_count = output.lines().count();
    let term_height = terminal_height();

    if is_tty && line_count > term_height {
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".into());
        if let Ok(mut child) = process::Command::new(&pager)
            .arg("-R")
            .stdin(process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(output.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }

    print!("{output}");
    let _ = io::stdout().flush();
}

fn terminal_height() -> usize {
    // Try LINES env var first (set by some shells)
    if let Ok(lines) = std::env::var("LINES") {
        if let Ok(h) = lines.parse::<usize>() {
            return h;
        }
    }
    // Fallback
    24
}

/// Configure rayon global thread pool to limit CPU usage.
///
/// Defaults to min(cores / 2, 6). Override with `SRCWALK_THREADS` env var.
/// This matters for long-lived MCP sessions where back-to-back searches
/// can sustain high CPU (see #27).
fn configure_thread_pools() {
    let num_threads = std::env::var("SRCWALK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(2, 6))
        });

    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
        .ok();
}
