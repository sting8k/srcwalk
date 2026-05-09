use std::{io, process};

// mimalloc: faster than system allocator for parallel walker workloads
// where many small Strings/Vecs are allocated across rayon threads.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod cli_run;
mod output;
mod version;

use clap::{CommandFactory, Parser};
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

fn main() {
    reset_sigpipe();
    configure_thread_pools();
    let cli = Cli::parse();

    // Shell completions
    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "srcwalk", &mut io::stdout());
        return;
    }

    match &cli.command {
        Some(Command::Guide) => {
            print!("{GUIDE}");
            return;
        }
        Some(Command::Version(cmd)) => {
            version::run_version(cmd.check);
            return;
        }
        Some(Command::Map(_)) if cli.budget.is_some() || cli.no_budget => {
            eprintln!(
                "error: map has a fixed 15k token cap; narrow --scope or lower --depth instead"
            );
            process::exit(2);
        }
        _ => {}
    }

    let config = match cli.command {
        Some(command) => RunConfig::from_command(command).expect("non-run command handled above"),
        None => RunConfig::from_legacy(cli),
    };
    cli_run::run(config);
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
