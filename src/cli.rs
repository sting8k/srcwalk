use std::path::PathBuf;

use clap::{ArgAction, Args, Parser};
use clap_complete::Shell;
use srcwalk::ArtifactMode;

/// srcwalk — Tree-sitter indexed lookups, smart code reading for AI agents.
/// Run `srcwalk guide` for the embedded, version-matched agent guide.
#[derive(Parser)]
#[command(name = "srcwalk", about, after_help = ROOT_HELP)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    /// File path, path:line, symbol name, or text to search.
    pub(crate) query: Option<String>,

    /// Directory to search within or resolve relative paths against.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    pub(crate) scope: Vec<PathBuf>,

    /// Focus line, line range, markdown heading, or symbol (e.g. "45", "45-89", "## Architecture").
    #[arg(long, hide = true)]
    pub(crate) section: Option<String>,

    /// Max tokens in response. Reduces detail to fit.
    /// Default: 5000 unless --no-budget is set.
    #[arg(long)]
    pub(crate) budget: Option<u64>,

    /// Disable default budget cap.
    #[arg(long)]
    pub(crate) no_budget: bool,

    /// Show explicit raw first page (capped at 200 lines / 5k tokens).
    #[arg(long, hide = true)]
    pub(crate) full: bool,

    /// Treat QUERY as an exact file path. Fails fast instead of falling back to search/glob.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "expand", "glob"])]
    pub(crate) path_exact: bool,

    /// Machine-readable JSON output.
    #[arg(long, hide = true, conflicts_with = "map")]
    pub(crate) json: bool,

    /// Include JS/TS artifacts as artifact-level evidence; direct file reads supported.
    #[arg(long)]
    pub(crate) artifact: bool,

    /// Show source context for top N matches/callers (default: 2 when flag present).
    #[arg(long, hide = true, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    pub(crate) expand: Option<usize>,

    /// File pattern filter (e.g. "*.rs", "!*.test.ts", "*.{go,rs}").
    #[arg(long, hide = true)]
    pub(crate) glob: Option<String>,

    /// Find direct callers as compact facts; use --expand[=N] for source context.
    #[arg(long, hide = true, conflicts_with_all = ["callees", "deps", "map", "flow", "impact"])]
    pub(crate) callers: bool,

    /// Filter search results or call sites with field:value qualifiers (e.g. path:foo, kind:fn; callers also support args:3 receiver:mgr; flow/detailed callees support callee:NAME).
    #[arg(long, hide = true, value_name = "QUALIFIERS", conflicts_with = "map")]
    pub(crate) filter: Option<String>,

    /// Count direct caller call sites by field: args, caller, receiver, path, or file.
    #[arg(long, hide = true, requires = "callers", value_name = "FIELD")]
    pub(crate) count_by: Option<String>,

    /// Show what a symbol calls (forward call graph).
    #[arg(long, hide = true, conflicts_with_all = ["callers", "deps", "map", "flow", "impact"])]
    pub(crate) callees: bool,

    /// Show ordered call sites with args and assignment context.
    #[arg(long, hide = true, requires = "callees")]
    pub(crate) detailed: bool,

    /// Depth for --callers/--callees BFS or --map tree. Default map: auto; BFS capped at 5.
    #[arg(long, hide = true, value_name = "N")]
    pub(crate) depth: Option<usize>,

    /// Max callers to expand per BFS hop (hub guard). Default: 50.
    #[arg(long, hide = true, value_name = "K", requires = "callers")]
    pub(crate) max_frontier: Option<usize>,

    /// Max total edges across all BFS hops. Default: 500.
    #[arg(long, hide = true, value_name = "M", requires = "callers")]
    pub(crate) max_edges: Option<usize>,

    /// Comma-separated symbols to skip as BFS frontier (hub guard).
    /// Default: new,clone,from,into,to_string,drop,fmt,default.
    /// Pass empty string "" to disable.
    #[arg(long, hide = true, value_name = "CSV", requires = "callers")]
    pub(crate) skip_hubs: Option<String>,

    /// Analyze blast-radius dependencies of a file.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "map", "flow", "impact"])]
    pub(crate) deps: bool,

    /// Summarize a known symbol's ordered calls, local resolves, and direct callers.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "impact", "expand", "section", "full"])]
    pub(crate) flow: bool,

    /// Summarize definitions, name-matched callers, and receiver/file groups.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "map", "flow", "expand", "section", "full"])]
    pub(crate) impact: bool,

    /// Generate a source map with local dependency groups.
    #[arg(long, hide = true, conflicts_with_all = ["callers", "callees", "deps", "flow", "impact", "expand", "section", "full"])]
    pub(crate) map: bool,

    /// Include symbol names in --map output.
    #[arg(long, hide = true, requires = "map")]
    pub(crate) symbols: bool,

    /// Max results. Default: unlimited.
    /// Applies to: symbol/content/regex/callers search and deps dependents.
    /// NOTE: multi-symbol ("A,B,C") applies the limit per-query, not total.
    #[arg(long, hide = true, value_name = "N")]
    pub(crate) limit: Option<usize>,

    /// Skip N results (for pagination). Use with --limit.
    #[arg(long, hide = true, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,

    /// Print shell completions for the given shell.
    #[arg(long, value_name = "SHELL")]
    pub(crate) completions: Option<Shell>,
}

pub(crate) const ROOT_HELP: &str = "\
Start here:\n  srcwalk guide               Full embedded, version-matched agent guide for agents\n\nCommon:\n  srcwalk <path>              Read a file smartly\n  srcwalk <path>:<line>       Read around a line\n  srcwalk find <query>        Find definitions/usages/text\n  srcwalk files <glob>        Find files by glob\n  srcwalk callers <symbol>    Show who calls a symbol\n  srcwalk callees <symbol>    Show what a symbol calls\n  srcwalk deps <file>         Show imports and dependents\n  srcwalk map                 Show repo orientation and dependency groups\n  srcwalk version             Show version; add --check for latest\n\nShortcuts:\n  srcwalk flow <symbol>       Compact caller/callee slice\n  srcwalk impact <symbol>     Heuristic blast-radius triage\n\nCompatibility:\n  Legacy flag syntax still works, e.g. `srcwalk Foo --callers`.";

pub(crate) const GUIDE: &str = include_str!("../skills/srcwalk/GUIDE.md");

#[derive(clap::Subcommand)]
pub(crate) enum Command {
    /// Show the full embedded, version-matched agent guide. Must use!
    Guide,
    /// Show version, optionally checking the latest release.
    Version(VersionCmd),
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
    /// Generate a source map with local dependency groups.
    Map(MapCmd),
}

#[derive(Args)]
pub(crate) struct VersionCmd {
    /// Check GitHub for the latest release and print update commands if newer.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Args)]
pub(crate) struct FindCmd {
    /// Symbol name, symbol-name glob, or text to search.
    pub(crate) query: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Show source context for top N matches (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    pub(crate) expand: Option<usize>,
    /// File pattern filter (e.g. "*.rs", "!*.test.ts", "*.{go,rs}").
    #[arg(long)]
    pub(crate) glob: Option<String>,
    /// Filter search results with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    pub(crate) filter: Option<String>,
    /// Max results.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N results.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct FilesCmd {
    /// File glob pattern to list.
    pub(crate) pattern: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Max files.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N files.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct CallersCmd {
    /// Symbol whose callers should be found.
    pub(crate) symbol: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Show source context for top N callers (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    pub(crate) expand: Option<usize>,
    /// File pattern filter.
    #[arg(long)]
    pub(crate) glob: Option<String>,
    /// Filter call sites with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    pub(crate) filter: Option<String>,
    /// Count direct caller call sites by field: args, caller, receiver, path, or file.
    #[arg(long, value_name = "FIELD")]
    pub(crate) count_by: Option<String>,
    /// BFS depth for transitive callers, capped at 5.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// Max callers to expand per BFS hop.
    #[arg(long, value_name = "K")]
    pub(crate) max_frontier: Option<usize>,
    /// Max total edges across all BFS hops.
    #[arg(long, value_name = "M")]
    pub(crate) max_edges: Option<usize>,
    /// Comma-separated symbols to skip as BFS frontier.
    #[arg(long, value_name = "CSV")]
    pub(crate) skip_hubs: Option<String>,
    /// Max results.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N results.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct CalleesCmd {
    /// Symbol whose callees should be found.
    pub(crate) symbol: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Show ordered call sites with args and assignment context.
    #[arg(long)]
    pub(crate) detailed: bool,
    /// Depth for callee search.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// Filter detailed call sites with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS", requires = "detailed")]
    pub(crate) filter: Option<String>,
}

#[derive(Args)]
pub(crate) struct FlowCmd {
    /// Symbol to summarize.
    pub(crate) symbol: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Optional depth for the compact flow slice.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// Filter flow facts with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    pub(crate) filter: Option<String>,
}

#[derive(Args)]
pub(crate) struct ImpactCmd {
    /// Symbol to summarize with heuristic blast-radius triage.
    pub(crate) symbol: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
}

#[derive(Args)]
pub(crate) struct DepsCmd {
    /// File to analyze.
    pub(crate) file: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Max dependents.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N dependents.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct MapCmd {
    #[command(flatten)]
    pub(crate) common: MapCommonArgs,
    /// Map tree depth. Default: auto.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// File pattern filter.
    #[arg(long)]
    pub(crate) glob: Option<String>,
    /// Include symbol names in map output.
    #[arg(long)]
    pub(crate) symbols: bool,
}

#[derive(Args)]
pub(crate) struct CommonArgs {
    /// Directory to search within or resolve relative paths against.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    pub(crate) scope: Vec<PathBuf>,
    /// Max tokens in response. Reduces detail to fit.
    #[arg(long)]
    pub(crate) budget: Option<u64>,
    /// Disable default budget cap.
    #[arg(long)]
    pub(crate) no_budget: bool,
    /// Machine-readable JSON output.
    #[arg(long)]
    pub(crate) json: bool,
    /// Include JS/TS artifacts as artifact-level evidence; relation modes are direct-only.
    #[arg(long)]
    pub(crate) artifact: bool,
}

#[derive(Args)]
pub(crate) struct MapCommonArgs {
    /// Directory to map.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    pub(crate) scope: Vec<PathBuf>,
    /// Include JS/TS artifacts as artifact-level evidence.
    #[arg(long)]
    pub(crate) artifact: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Mode {
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

pub(crate) struct RunConfig {
    pub(crate) mode: Mode,
    pub(crate) query: Option<String>,
    pub(crate) scopes: Vec<PathBuf>,
    pub(crate) allow_multi_scope: bool,
    pub(crate) section: Option<String>,
    pub(crate) budget: Option<u64>,
    pub(crate) no_budget: bool,
    pub(crate) full: bool,
    pub(crate) json: bool,
    pub(crate) artifact: ArtifactMode,
    pub(crate) expand: usize,
    pub(crate) glob: Option<String>,
    pub(crate) filter: Option<String>,
    pub(crate) count_by: Option<String>,
    pub(crate) detailed: bool,
    pub(crate) depth: Option<usize>,
    pub(crate) max_frontier: Option<usize>,
    pub(crate) max_edges: Option<usize>,
    pub(crate) skip_hubs: Option<String>,
    pub(crate) symbols: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: usize,
}

impl RunConfig {
    pub(crate) fn from_legacy(cli: Cli) -> Self {
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
            artifact: ArtifactMode::from(cli.artifact),
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

    pub(crate) fn from_command(command: Command) -> Option<Self> {
        match command {
            Command::Guide | Command::Version(_) => None,
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
            artifact: ArtifactMode::from(common.artifact),
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
            budget: None,
            no_budget: false,
            full: false,
            json: false,
            artifact: ArtifactMode::from(cmd.common.artifact),
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
