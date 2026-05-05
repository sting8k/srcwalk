use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process;

use clap::{ArgAction, Args, CommandFactory, Parser};
use clap_complete::Shell;

// mimalloc: faster than system allocator for parallel walker workloads
// where many small Strings/Vecs are allocated across rayon threads.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// srcwalk — Tree-sitter indexed lookups, smart code reading for AI agents.
/// Run `srcwalk guide` for the embedded, version-matched agent guide.
#[derive(Parser)]
#[command(name = "srcwalk", version, about, after_help = ROOT_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// File path, path:line, symbol name, or text to search.
    query: Option<String>,

    /// Directory to search within or resolve relative paths against.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    scope: Vec<PathBuf>,

    /// Focus line, line range, markdown heading, or symbol (e.g. "45", "45-89", "## Architecture").
    #[arg(long, hide = true)]
    section: Option<String>,

    /// Max tokens in response. Reduces detail to fit.
    /// Default: 5000 when piped (non-TTY). Unlimited for interactive TTY.
    #[arg(long)]
    budget: Option<u64>,

    /// Disable default budget cap (for piped/scripted usage).
    #[arg(long)]
    no_budget: bool,

    /// Show explicit raw first page (capped at 200 lines / 5k tokens).
    #[arg(long, hide = true)]
    full: bool,

    /// Treat QUERY as an exact file path. Fails fast instead of falling back to search/glob.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "expand", "glob"])]
    path_exact: bool,

    /// Machine-readable JSON output.
    #[arg(long, hide = true, conflicts_with = "map")]
    json: bool,

    /// Show source context for top N matches/callers (default: 2 when flag present).
    #[arg(long, hide = true, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    expand: Option<usize>,

    /// File pattern filter (e.g. "*.rs", "!*.test.ts", "*.{go,rs}").
    #[arg(long, hide = true)]
    glob: Option<String>,

    /// Find direct callers as compact facts; use --expand[=N] for source context.
    #[arg(long, hide = true, conflicts_with_all = ["callees", "deps", "map", "flow", "impact"])]
    callers: bool,

    /// Filter search results or call sites with field:value qualifiers (e.g. path:foo, kind:fn; callers also support args:3 receiver:mgr; flow/detailed callees support callee:NAME).
    #[arg(long, hide = true, value_name = "QUALIFIERS", conflicts_with = "map")]
    filter: Option<String>,

    /// Count direct caller call sites by field: args, caller, receiver, path, or file.
    #[arg(long, hide = true, requires = "callers", value_name = "FIELD")]
    count_by: Option<String>,

    /// Show what a symbol calls (forward call graph).
    #[arg(long, hide = true, conflicts_with_all = ["callers", "deps", "map", "flow", "impact"])]
    callees: bool,

    /// Show ordered call sites with args and assignment context.
    #[arg(long, hide = true, requires = "callees")]
    detailed: bool,

    /// Depth for --callers/--callees BFS or --map tree. Default map: 3; BFS capped at 5.
    #[arg(long, hide = true, value_name = "N")]
    depth: Option<usize>,

    /// Max callers to expand per BFS hop (hub guard). Default: 50.
    #[arg(long, hide = true, value_name = "K", requires = "callers")]
    max_frontier: Option<usize>,

    /// Max total edges across all BFS hops. Default: 500.
    #[arg(long, hide = true, value_name = "M", requires = "callers")]
    max_edges: Option<usize>,

    /// Comma-separated symbols to skip as BFS frontier (hub guard).
    /// Default: new,clone,from,into,to_string,drop,fmt,default.
    /// Pass empty string "" to disable.
    #[arg(long, hide = true, value_name = "CSV", requires = "callers")]
    skip_hubs: Option<String>,

    /// Analyze blast-radius dependencies of a file.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "map", "flow", "impact"])]
    deps: bool,

    /// Summarize a known symbol's ordered calls, local resolves, and direct callers.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "impact", "expand", "section", "full"])]
    flow: bool,

    /// Summarize definitions, name-matched callers, and receiver/file groups.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "flow", "expand", "section", "full"])]
    impact: bool,

    /// Generate a structural codebase map.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "flow", "impact", "expand", "section", "full"])]
    map: bool,

    /// Include symbol names in --map output.
    #[arg(long, hide = true, requires = "map")]
    symbols: bool,

    /// Max results. Default: unlimited (or 50 for interactive TTY).
    /// Applies to: symbol/content/regex/callers search and deps dependents.
    /// NOTE: multi-symbol ("A,B,C") applies the limit per-query, not total.
    #[arg(long, hide = true, value_name = "N")]
    limit: Option<usize>,

    /// Skip N results (for pagination). Use with --limit.
    #[arg(long, hide = true, value_name = "N", default_value = "0")]
    offset: usize,

    /// Print shell completions for the given shell.
    #[arg(long, value_name = "SHELL")]
    completions: Option<Shell>,
}

const ROOT_HELP: &str = "\
Guide:\n  srcwalk guide               Full embedded, version-matched agent guide\n\nCommon:\n  srcwalk <path>              Read a file smartly\n  srcwalk <path>:<line>       Read around a line\n  srcwalk find <query>        Find definitions/usages/text\n  srcwalk files <glob>        Find files by glob\n  srcwalk callers <symbol>    Show who calls a symbol\n  srcwalk callees <symbol>    Show what a symbol calls\n  srcwalk deps <file>         Show imports and dependents\n  srcwalk map                 Show a structural repo map\n\nShortcuts:\n  srcwalk flow <symbol>       Compact caller/callee slice\n  srcwalk impact <symbol>     Heuristic blast-radius triage\n\nCompatibility:\n  Legacy flag syntax still works, e.g. `srcwalk Foo --callers`.";

const GUIDE: &str = include_str!("../skills/srcwalk/GUIDE.md");

#[derive(clap::Subcommand)]
enum Command {
    /// Find definitions, usages, text, or symbol-name glob matches.
    Find(FindCmd),
    /// Find files by glob pattern.
    Files(FilesCmd),
    /// Show who calls a symbol.
    Callers(CallersCmd),
    /// Show what a symbol calls.
    Callees(CalleesCmd),
    /// Compact caller/callee slice for a known symbol.
    Flow(FlowCmd),
    /// Heuristic blast-radius triage for a symbol.
    Impact(ImpactCmd),
    /// Analyze imports and dependents for a file.
    Deps(DepsCmd),
    /// Generate a structural codebase map.
    Map(MapCmd),
    /// Show the full embedded, version-matched agent guide.
    Guide,
}

#[derive(Args)]
struct FindCmd {
    /// Symbol name, symbol-name glob, or text to search.
    query: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Show source context for top N matches (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    expand: Option<usize>,
    /// File pattern filter (e.g. "*.rs", "!*.test.ts", "*.{go,rs}").
    #[arg(long)]
    glob: Option<String>,
    /// Filter search results with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    filter: Option<String>,
    /// Max results.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Skip N results.
    #[arg(long, value_name = "N", default_value = "0")]
    offset: usize,
}

#[derive(Args)]
struct FilesCmd {
    /// File glob pattern to list.
    pattern: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Max files.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Skip N files.
    #[arg(long, value_name = "N", default_value = "0")]
    offset: usize,
}

#[derive(Args)]
struct CallersCmd {
    /// Symbol whose callers should be found.
    symbol: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Show source context for top N callers (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    expand: Option<usize>,
    /// File pattern filter.
    #[arg(long)]
    glob: Option<String>,
    /// Filter call sites with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    filter: Option<String>,
    /// Count direct caller call sites by field: args, caller, receiver, path, or file.
    #[arg(long, value_name = "FIELD")]
    count_by: Option<String>,
    /// BFS depth for transitive callers, capped at 5.
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
    /// Max callers to expand per BFS hop.
    #[arg(long, value_name = "K")]
    max_frontier: Option<usize>,
    /// Max total edges across all BFS hops.
    #[arg(long, value_name = "M")]
    max_edges: Option<usize>,
    /// Comma-separated symbols to skip as BFS frontier.
    #[arg(long, value_name = "CSV")]
    skip_hubs: Option<String>,
    /// Max results.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Skip N results.
    #[arg(long, value_name = "N", default_value = "0")]
    offset: usize,
}

#[derive(Args)]
struct CalleesCmd {
    /// Symbol whose callees should be found.
    symbol: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Show ordered call sites with args and assignment context.
    #[arg(long)]
    detailed: bool,
    /// Depth for callee search.
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
    /// Filter detailed call sites with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS", requires = "detailed")]
    filter: Option<String>,
}

#[derive(Args)]
struct FlowCmd {
    /// Symbol to summarize.
    symbol: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Optional depth for the compact flow slice.
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
    /// Filter flow facts with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    filter: Option<String>,
}

#[derive(Args)]
struct ImpactCmd {
    /// Symbol to summarize with heuristic blast-radius triage.
    symbol: String,
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Args)]
struct DepsCmd {
    /// File to analyze.
    file: String,
    #[command(flatten)]
    common: CommonArgs,
    /// Max dependents.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Skip N dependents.
    #[arg(long, value_name = "N", default_value = "0")]
    offset: usize,
}

#[derive(Args)]
struct MapCmd {
    #[command(flatten)]
    common: MapCommonArgs,
    /// Map tree depth. Default: 3.
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
    /// File pattern filter.
    #[arg(long)]
    glob: Option<String>,
    /// Include symbol names in map output.
    #[arg(long)]
    symbols: bool,
}

#[derive(Args)]
struct CommonArgs {
    /// Directory to search within or resolve relative paths against.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    scope: Vec<PathBuf>,
    /// Max tokens in response. Reduces detail to fit.
    #[arg(long)]
    budget: Option<u64>,
    /// Disable default budget cap.
    #[arg(long)]
    no_budget: bool,
    /// Machine-readable JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct MapCommonArgs {
    /// Directory to map.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    scope: Vec<PathBuf>,
    /// Max tokens in response. Reduces detail to fit.
    #[arg(long)]
    budget: Option<u64>,
    /// Disable default budget cap.
    #[arg(long)]
    no_budget: bool,
}

#[derive(Clone, Copy)]
enum Mode {
    Search,
    PathExact,
    Map,
    Files,
    Flow,
    Impact,
    Callers,
    Callees,
    Deps,
}

struct RunConfig {
    mode: Mode,
    query: Option<String>,
    scopes: Vec<PathBuf>,
    allow_multi_scope: bool,
    section: Option<String>,
    budget: Option<u64>,
    no_budget: bool,
    full: bool,
    json: bool,
    expand: usize,
    glob: Option<String>,
    filter: Option<String>,
    count_by: Option<String>,
    detailed: bool,
    depth: Option<usize>,
    max_frontier: Option<usize>,
    max_edges: Option<usize>,
    skip_hubs: Option<String>,
    symbols: bool,
    limit: Option<usize>,
    offset: usize,
}

impl RunConfig {
    fn from_legacy(cli: Cli) -> Self {
        let mode = if cli.map {
            Mode::Map
        } else if cli.path_exact {
            Mode::PathExact
        } else if cli.flow {
            Mode::Flow
        } else if cli.impact {
            Mode::Impact
        } else if cli.callers {
            Mode::Callers
        } else if cli.callees {
            Mode::Callees
        } else if cli.deps {
            Mode::Deps
        } else {
            Mode::Search
        };
        Self {
            mode,
            query: cli.query,
            scopes: cli.scope,
            allow_multi_scope: false,
            section: cli.section,
            budget: cli.budget,
            no_budget: cli.no_budget,
            full: cli.full,
            json: cli.json,
            expand: cli.expand.unwrap_or(0),
            glob: cli.glob,
            filter: cli.filter,
            count_by: cli.count_by,
            detailed: cli.detailed,
            depth: cli.depth,
            max_frontier: cli.max_frontier,
            max_edges: cli.max_edges,
            skip_hubs: cli.skip_hubs,
            symbols: cli.symbols,
            limit: cli.limit,
            offset: cli.offset,
        }
    }

    fn from_command(command: Command) -> Option<Self> {
        match command {
            Command::Guide => None,
            Command::Find(cmd) => Some(
                Self::from_common(Mode::Search, cmd.query, cmd.common)
                    .with_find(cmd.expand, cmd.glob, cmd.filter, cmd.limit, cmd.offset),
            ),
            Command::Files(cmd) => Some(
                Self::from_common(Mode::Files, cmd.pattern, cmd.common)
                    .with_pagination(cmd.limit, cmd.offset),
            ),
            Command::Callers(cmd) => Some(Self::from_callers(cmd)),
            Command::Callees(cmd) => Some(Self::from_callees(cmd)),
            Command::Flow(cmd) => Some(Self::from_flow(cmd)),
            Command::Impact(cmd) => Some(Self::from_common(Mode::Impact, cmd.symbol, cmd.common)),
            Command::Deps(cmd) => Some(
                Self::from_common(Mode::Deps, cmd.file, cmd.common)
                    .with_pagination(cmd.limit, cmd.offset),
            ),
            Command::Map(cmd) => Some(Self::from_map(cmd)),
        }
    }

    fn from_common(mode: Mode, query: String, common: CommonArgs) -> Self {
        Self {
            mode,
            query: Some(query),
            scopes: common.scope,
            allow_multi_scope: matches!(mode, Mode::Search),
            section: None,
            budget: common.budget,
            no_budget: common.no_budget,
            full: false,
            json: common.json,
            expand: 0,
            glob: None,
            filter: None,
            count_by: None,
            detailed: false,
            depth: None,
            max_frontier: None,
            max_edges: None,
            skip_hubs: None,
            symbols: false,
            limit: None,
            offset: 0,
        }
    }

    fn from_map(cmd: MapCmd) -> Self {
        Self {
            mode: Mode::Map,
            query: None,
            scopes: cmd.common.scope,
            allow_multi_scope: false,
            section: None,
            budget: cmd.common.budget,
            no_budget: cmd.common.no_budget,
            full: false,
            json: false,
            expand: 0,
            glob: cmd.glob,
            filter: None,
            count_by: None,
            detailed: false,
            depth: cmd.depth,
            max_frontier: None,
            max_edges: None,
            skip_hubs: None,
            symbols: cmd.symbols,
            limit: None,
            offset: 0,
        }
    }

    fn with_find(
        mut self,
        expand: Option<usize>,
        glob: Option<String>,
        filter: Option<String>,
        limit: Option<usize>,
        offset: usize,
    ) -> Self {
        self.expand = expand.unwrap_or(0);
        self.glob = glob;
        self.filter = filter;
        self.limit = limit;
        self.offset = offset;
        self
    }

    fn from_callers(cmd: CallersCmd) -> Self {
        let mut config = Self::from_common(Mode::Callers, cmd.symbol, cmd.common);
        config.expand = cmd.expand.unwrap_or(0);
        config.glob = cmd.glob;
        config.filter = cmd.filter;
        config.count_by = cmd.count_by;
        config.depth = cmd.depth;
        config.max_frontier = cmd.max_frontier;
        config.max_edges = cmd.max_edges;
        config.skip_hubs = cmd.skip_hubs;
        config.limit = cmd.limit;
        config.offset = cmd.offset;
        config
    }

    fn from_callees(cmd: CalleesCmd) -> Self {
        let mut config = Self::from_common(Mode::Callees, cmd.symbol, cmd.common);
        config.detailed = cmd.detailed;
        config.depth = cmd.depth;
        config.filter = cmd.filter;
        config
    }

    fn from_flow(cmd: FlowCmd) -> Self {
        let mut config = Self::from_common(Mode::Flow, cmd.symbol, cmd.common);
        config.depth = cmd.depth;
        config.filter = cmd.filter;
        config
    }

    fn with_pagination(mut self, limit: Option<usize>, offset: usize) -> Self {
        self.limit = limit;
        self.offset = offset;
        self
    }
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

    if let Some(Command::Guide) = cli.command {
        print!("{GUIDE}");
        return;
    }

    let config = match cli.command {
        Some(command) => RunConfig::from_command(command).expect("guide handled above"),
        None => RunConfig::from_legacy(cli),
    };
    run(config);
}

fn canonicalize_scopes_or_exit(scopes: Vec<PathBuf>) -> Vec<PathBuf> {
    scopes
        .into_iter()
        .map(|scope| {
            let meta = match std::fs::metadata(&scope) {
                Ok(meta) => meta,
                Err(e) => {
                    eprintln!("error: invalid scope: {} [{e}]", scope.display());
                    process::exit(2);
                }
            };
            if !meta.is_dir() {
                eprintln!(
                    "error: invalid scope: {} [not a directory]",
                    scope.display()
                );
                process::exit(2);
            }
            scope.canonicalize().unwrap_or(scope)
        })
        .collect()
}

fn run(config: RunConfig) {
    let is_tty = io::stdout().is_terminal();

    // Effective budget: explicit --budget wins, --no-budget disables,
    // otherwise default 5000 tokens for piped (non-TTY) output.
    let effective_budget = if config.no_budget {
        None
    } else if config.budget.is_some() {
        config.budget
    } else if !is_tty {
        Some(5_000)
    } else {
        None
    };

    let cache = srcwalk::cache::OutlineCache::new();
    let scopes = canonicalize_scopes_or_exit(config.scopes);
    if scopes.len() > 1 && !config.allow_multi_scope {
        eprintln!("error: repeated --scope is currently supported only by `srcwalk find`");
        process::exit(2);
    }
    let scope = scopes
        .first()
        .expect("at least one scope from clap default")
        .clone();

    if matches!(config.mode, Mode::Map) {
        let depth = config.depth.unwrap_or(3);
        match srcwalk::map::generate(
            &scope,
            depth,
            effective_budget,
            &cache,
            config.symbols,
            config.glob.as_deref(),
        ) {
            Ok(output) => emit_output(&output, is_tty),
            Err(e) => {
                eprintln!("{e}");
                process::exit(e.exit_code());
            }
        }
        return;
    }

    let query = if let Some(q) = config.query {
        q
    } else {
        eprintln!("usage: srcwalk <query> [--scope DIR] [--section N-M] [--budget N]");
        process::exit(3);
    };

    // TTY interactive mode: cap at 50 unless user set --limit or --full.
    // Piped / scripted → unlimited so grep/wc/etc. see everything.
    let effective_limit = config.limit.or({
        if is_tty && !config.full {
            Some(50)
        } else {
            None
        }
    });

    if matches!(config.mode, Mode::Files) {
        let result = srcwalk::run_files(
            &query,
            &scope,
            effective_budget,
            effective_limit,
            config.offset,
        );
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if matches!(config.mode, Mode::PathExact) {
        let result = srcwalk::run_path_exact(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            config.full,
            &cache,
        );
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if config.filter.is_some() && matches!(config.mode, Mode::Deps | Mode::Impact)
        || (config.filter.is_some() && matches!(config.mode, Mode::Callees) && !config.detailed)
    {
        eprintln!(
            "error: --filter applies to search results, direct callers, flow, and detailed callees"
        );
        process::exit(2);
    }

    if matches!(config.mode, Mode::Flow) {
        let result = srcwalk::run_flow(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.depth,
            config.filter.as_deref(),
        );
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if matches!(config.mode, Mode::Impact) {
        let result = srcwalk::run_impact(&query, &scope, effective_budget, &cache);
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if matches!(config.mode, Mode::Callers) {
        let bfs_json = config.json && matches!(config.depth, Some(d) if d >= 2);
        let result = srcwalk::run_callers(
            &query,
            &scope,
            config.expand,
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            &cache,
            config.depth,
            config.max_frontier,
            config.max_edges,
            config.skip_hubs.as_deref(),
            config.filter.as_deref(),
            config.count_by.as_deref(),
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
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if matches!(config.mode, Mode::Callees) {
        let result = srcwalk::run_callees(
            &query,
            &scope,
            effective_budget,
            &cache,
            config.depth,
            config.detailed,
            config.filter.as_deref(),
        );
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    if matches!(config.mode, Mode::Deps) {
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
        let result = srcwalk::run_deps(
            &path,
            &scope,
            effective_budget,
            &cache,
            config.limit,
            config.offset,
        );
        emit_result(result, &query, config.json, is_tty);
        return;
    }

    let result = if scopes.len() > 1 {
        srcwalk::run_multi_scope_find_filtered(
            &query,
            &scopes,
            effective_budget,
            config.expand,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    } else if config.expand > 0 {
        srcwalk::run_expanded_filtered(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            config.full,
            config.expand,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    } else if config.full {
        srcwalk::run_full_filtered(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    } else {
        srcwalk::run_filtered(
            &query,
            &scope,
            config.section.as_deref(),
            effective_budget,
            effective_limit,
            config.offset,
            config.glob.as_deref(),
            config.filter.as_deref(),
            &cache,
        )
    };

    emit_result(result, &query, config.json, is_tty);
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
