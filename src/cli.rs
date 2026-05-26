use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, ValueEnum};
use clap_complete::Shell;
use srcwalk::ArtifactMode;

/// srcwalk — Tree-sitter indexed lookups, smart code reading for AI agents.
/// Run `srcwalk guide` for the embedded, version-matched agent guide.
#[derive(Parser)]
#[command(name = "srcwalk", about, after_help = ROOT_HELP)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    /// Exact file path, path:line, or path:start-end to read. Use `discover` for search.
    pub(crate) query: Option<String>,

    /// Directory to resolve relative paths against.
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

    /// Include JS/TS artifacts as artifact-level evidence; exact artifact file reads may auto-enable this.
    #[arg(long)]
    pub(crate) artifact: bool,

    /// Print shell completions for the given shell.
    #[arg(long, value_name = "SHELL")]
    pub(crate) completions: Option<Shell>,
}

pub(crate) const ROOT_HELP: &str = "\
Start here:\n  srcwalk guide                         Full embedded, version-matched agent guide for agents\n\nCommon:\n  srcwalk overview                      Show repo orientation and dependency groups\n  srcwalk context <symbol-or-file:line> Understand one known target\n  srcwalk trace callers <symbol>        Show who calls a symbol\n  srcwalk trace callees <symbol>        Show what a symbol calls\n  srcwalk deps <file>                   Show imports and dependents\n  srcwalk assess <symbol>               Heuristic blast-radius triage\n  srcwalk review <range-or-staged>      Review a change set with Flow Map evidence\n  srcwalk compare <target-a> <target-b> Compare two known source targets structurally\n  srcwalk discover <query>              Find candidate symbols/usages/text\n  srcwalk discover <glob> --as file     Find files by glob\n  srcwalk show <path>:<line> -C 10      Read exact evidence with extra line context\n  srcwalk <path>                        Read a file smartly\n  srcwalk <path>:<line>                 Read around a line\n  srcwalk version                       Show version; add --check for latest";

pub(crate) const GUIDE: &str = include_str!("../skills/srcwalk/GUIDE.md");

#[derive(clap::Subcommand)]
pub(crate) enum Command {
    /// Show the full embedded, version-matched agent guide. Must use!
    Guide,
    /// Show repo orientation and local dependency groups.
    Overview(MapCmd),
    /// Understand one known target with Flow Map and neighborhood evidence.
    Context(ContextCmd),
    /// Traverse call graph relations.
    Trace(TraceCmd),
    /// Analyze imports and dependents for a file.
    Deps(DepsCmd),
    /// Heuristic blast-radius triage for a symbol.
    Assess(AssessCmd),
    /// Review a change set with bounded Flow Map evidence.
    Review(ReviewCmd),
    /// Compare two known source targets structurally.
    Compare(CompareCmd),
    /// Find candidate symbols, usages, text, field/member access, or files.
    Discover(DiscoverCmd),
    /// Read exact file, line, range, section, or comma-separated locations.
    Show(ShowCmd),
    /// Show version, optionally checking the latest release.
    Version(VersionCmd),
    /// Structural decision-flow compatibility primitive for review internals.
    #[command(hide = true)]
    DecisionFlow(DecisionFlowCmd),
    /// Diff evidence compatibility primitive for review internals.
    #[command(hide = true)]
    Diff(DiffCmd),
}

#[derive(Args)]
pub(crate) struct VersionCmd {
    /// Check GitHub for the latest release and print update commands if newer.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum DiscoverAs {
    Symbol,
    File,
    Text,
    Access,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum MatchMode {
    #[default]
    Any,
    All,
}

#[derive(Args)]
pub(crate) struct DiscoverCmd {
    /// Symbol name, field/member name, symbol-name glob, text, or file glob to discover.
    pub(crate) query: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Interpret the query as symbol, file, text, or field/member access.
    #[arg(long = "as", value_enum)]
    pub(crate) as_kind: Option<DiscoverAs>,
    /// Query matching mode. Explicit `any` means comma-separated literal OR for text; `all` means same-file co-occurrence.
    #[arg(long = "match", value_enum)]
    pub(crate) match_mode: Option<MatchMode>,
    /// Show source context for top N matches (default: 2 when flag present).
    #[arg(long, num_args = 0..=1, default_missing_value = "2", require_equals = true)]
    pub(crate) expand: Option<usize>,
    /// File pattern filter (legacy compatibility; prefer narrowing --scope when possible).
    #[arg(long, hide = true)]
    pub(crate) glob: Option<String>,
    /// Exclude files matching this pattern from discovery evidence.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) exclude: Option<String>,
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
pub(crate) struct ShowCmd {
    /// File path, path:line, path:start-end, or comma-separated exact locations.
    pub(crate) target: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Focus line, line range, markdown heading, symbol, or comma-separated sections.
    #[arg(long)]
    pub(crate) section: Option<String>,
    /// Show explicit raw first page (capped at 200 lines / 5k tokens).
    #[arg(long)]
    pub(crate) full: bool,
    /// Lines of context before and after a line/range/section target; multi targets clamp to 10.
    #[arg(short = 'C', long = "context-lines", value_name = "N")]
    pub(crate) context_lines: Option<usize>,
}

#[derive(Args)]
pub(crate) struct TraceCmd {
    #[command(subcommand)]
    pub(crate) relation: TraceRelation,
}

#[derive(clap::Subcommand)]
pub(crate) enum TraceRelation {
    /// Show who calls a symbol.
    Callers(CallersCmd),
    /// Show what a symbol calls.
    Callees(CalleesCmd),
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
    /// File pattern filter (legacy compatibility; prefer narrowing --scope when possible).
    #[arg(long, hide = true)]
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
pub(crate) struct DiffCmd {
    /// Explicit git revision range. Use REV^..REV for a single commit.
    pub(crate) rev_range: Option<String>,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Show staged changes only.
    #[arg(long)]
    pub(crate) staged: bool,
    /// Max changed files to render.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N changed files.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct CompareCmd {
    /// First function/line target to compare.
    pub(crate) target_a: String,
    /// Second function/line target to compare.
    pub(crate) target_b: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
}

#[derive(Args)]
pub(crate) struct ReviewCmd {
    /// Function/line target, explicit revision range, or omitted for working tree changes.
    pub(crate) target: Option<String>,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Review staged changes only.
    #[arg(long)]
    pub(crate) staged: bool,
    /// Max changed files to render for change review.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    /// Skip N changed files for change review.
    #[arg(long, value_name = "N", default_value = "0")]
    pub(crate) offset: usize,
}

#[derive(Args)]
pub(crate) struct DecisionFlowCmd {
    /// Function symbol, file:symbol, file:line, or file:start-end target.
    pub(crate) target: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
}

#[derive(Args)]
pub(crate) struct ContextCmd {
    /// Symbol, file:symbol, file:line, or file:start-end target.
    pub(crate) symbol: String,
    #[command(flatten)]
    pub(crate) common: CommonArgs,
    /// Optional depth for the compact context slice.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// Filter context facts with field:value qualifiers.
    #[arg(long, value_name = "QUALIFIERS")]
    pub(crate) filter: Option<String>,
}

#[derive(Args)]
pub(crate) struct AssessCmd {
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
    /// Overview tree depth. Default: auto.
    #[arg(long, value_name = "N")]
    pub(crate) depth: Option<usize>,
    /// File pattern filter (legacy compatibility; prefer narrowing --scope when possible).
    #[arg(long, hide = true)]
    pub(crate) glob: Option<String>,
    /// Include symbol names in overview output.
    #[arg(long)]
    pub(crate) symbols: bool,
}

#[derive(Args)]
pub(crate) struct CommonArgs {
    /// Scope root for search and relative path resolution.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    pub(crate) scope: Vec<PathBuf>,
    /// Max tokens in response. Reduces detail to fit.
    #[arg(long)]
    pub(crate) budget: Option<u64>,
    /// Disable default budget cap.
    #[arg(long)]
    pub(crate) no_budget: bool,
    /// Include JS/TS artifacts as artifact-level evidence; exact artifact file scopes may auto-enable this; relation modes are direct-only.
    #[arg(long)]
    pub(crate) artifact: bool,
}

#[derive(Args)]
pub(crate) struct MapCommonArgs {
    /// Directory to summarize.
    #[arg(long, default_value = ".", action = ArgAction::Append)]
    pub(crate) scope: Vec<PathBuf>,
    /// Include JS/TS artifacts as artifact-level evidence.
    #[arg(long)]
    pub(crate) artifact: bool,
    /// Max tokens in response. Overview uses a fixed cap and rejects this flag.
    #[arg(long, hide = true)]
    pub(crate) budget: Option<u64>,
    /// Disable default budget cap. Overview uses a fixed cap and rejects this flag.
    #[arg(long, hide = true)]
    pub(crate) no_budget: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    Search,
    Text,
    MatchAll,
    Show,
    Overview,
    Files,
    Context,
    DecisionFlow,
    Diff,
    Review,
    Compare,
    Assess,
    Callers,
    Callees,
    Deps,
}

fn looks_like_text_or_discovery_query(query: &str) -> bool {
    let terms = query
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if !(2..=8).contains(&terms.len()) {
        return false;
    }

    terms.iter().any(|term| !looks_like_plain_symbol_term(term))
}

fn looks_like_plain_symbol_term(term: &str) -> bool {
    let mut chars = term.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '@')) {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn looks_like_file_discovery_query(query: &str) -> bool {
    let has_glob = query
        .bytes()
        .any(|b| matches!(b, b'*' | b'?' | b'[' | b'{'));
    if !has_glob {
        return false;
    }

    query.contains('/') || query.contains('\\') || PathBuf::from(query).extension().is_some()
}

pub(crate) struct RunConfig {
    pub(crate) mode: Mode,
    pub(crate) query: Option<String>,
    pub(crate) scopes: Vec<PathBuf>,
    pub(crate) allow_multi_scope: bool,
    pub(crate) section: Option<String>,
    pub(crate) context_lines: Option<usize>,
    pub(crate) discover_as: Option<DiscoverAs>,
    pub(crate) match_explicit: bool,
    pub(crate) inferred_text_or: bool,
    pub(crate) budget: Option<u64>,
    pub(crate) no_budget: bool,
    pub(crate) full: bool,
    pub(crate) artifact: ArtifactMode,
    pub(crate) expand: usize,
    pub(crate) access: bool,
    pub(crate) glob: Option<String>,
    pub(crate) exclude: Option<String>,
    pub(crate) filter: Option<String>,
    pub(crate) count_by: Option<String>,
    pub(crate) detailed: bool,
    pub(crate) diff_staged: bool,
    pub(crate) depth: Option<usize>,
    pub(crate) max_frontier: Option<usize>,
    pub(crate) max_edges: Option<usize>,
    pub(crate) skip_hubs: Option<String>,
    pub(crate) symbols: bool,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: usize,
    pub(crate) compare_target_b: Option<String>,
}

impl RunConfig {
    pub(crate) fn from_root(cli: Cli) -> Self {
        Self {
            mode: Mode::Show,
            query: cli.query,
            scopes: cli.scope,
            allow_multi_scope: false,
            section: cli.section,
            context_lines: None,
            discover_as: None,
            match_explicit: false,
            inferred_text_or: false,
            budget: cli.budget,
            no_budget: cli.no_budget,
            full: cli.full,
            artifact: ArtifactMode::from(cli.artifact),
            expand: 0,
            access: false,
            glob: None,
            exclude: None,
            filter: None,
            count_by: None,
            detailed: false,
            diff_staged: false,
            depth: None,
            max_frontier: None,
            max_edges: None,
            skip_hubs: None,
            symbols: false,
            limit: None,
            offset: 0,
            compare_target_b: None,
        }
    }

    pub(crate) fn from_command(command: Command) -> Option<Self> {
        match command {
            Command::Guide | Command::Version(_) => None,
            Command::Discover(cmd) => Some(Self::from_discover(cmd)),
            Command::Show(cmd) => Some(Self::from_show(cmd)),
            Command::Trace(cmd) => Some(Self::from_trace(cmd)),
            Command::Context(cmd) => Some(Self::from_context(cmd)),
            Command::DecisionFlow(cmd) => Some(Self::from_common(
                Mode::DecisionFlow,
                cmd.target,
                cmd.common,
            )),
            Command::Diff(cmd) => Some(Self::from_diff(cmd)),
            Command::Review(cmd) => Some(Self::from_review(cmd)),
            Command::Compare(cmd) => Some(Self::from_compare(cmd)),
            Command::Assess(cmd) => Some(Self::from_common(Mode::Assess, cmd.symbol, cmd.common)),
            Command::Deps(cmd) => Some(
                Self::from_common(Mode::Deps, cmd.file, cmd.common)
                    .with_pagination(cmd.limit, cmd.offset),
            ),
            Command::Overview(cmd) => Some(Self::from_map(cmd)),
        }
    }

    fn from_discover(cmd: DiscoverCmd) -> Self {
        let match_explicit = cmd.match_mode.is_some();
        let match_mode = cmd.match_mode.unwrap_or_default();
        let inferred_file = cmd.as_kind.is_none() && looks_like_file_discovery_query(&cmd.query);
        let inferred_text_or = cmd.as_kind.is_none()
            && cmd.match_mode.is_none()
            && looks_like_text_or_discovery_query(&cmd.query);
        let mode =
            match (match_mode, cmd.as_kind, inferred_file, inferred_text_or) {
                (MatchMode::All, _, _, _) => Mode::MatchAll,
                (MatchMode::Any, Some(DiscoverAs::File), _, _)
                | (MatchMode::Any, None, true, _) => Mode::Files,
                (MatchMode::Any, Some(DiscoverAs::Text), _, _)
                | (MatchMode::Any, None, _, true) => Mode::Text,
                (MatchMode::Any, _, _, _) => Mode::Search,
            };
        let mut config = Self::from_common(mode, cmd.query, cmd.common);
        config.expand = cmd.expand.unwrap_or(0);
        config.access = matches!(cmd.as_kind, Some(DiscoverAs::Access));
        config.glob = cmd.glob;
        config.exclude = cmd.exclude;
        config.filter = cmd.filter;
        config.limit = cmd.limit;
        config.offset = cmd.offset;
        config.allow_multi_scope = matches!(mode, Mode::Search);
        config.discover_as = if inferred_file {
            Some(DiscoverAs::File)
        } else if inferred_text_or {
            Some(DiscoverAs::Text)
        } else {
            cmd.as_kind
        };
        config.match_explicit = match_explicit;
        config.inferred_text_or = inferred_text_or;
        config
    }

    fn from_diff(cmd: DiffCmd) -> Self {
        let mut config = Self::from_common(
            Mode::Diff,
            cmd.rev_range.clone().unwrap_or_default(),
            cmd.common,
        );
        config.query = cmd.rev_range;
        config.diff_staged = cmd.staged;
        config.limit = cmd.limit;
        config.offset = cmd.offset;
        config
    }

    fn from_compare(cmd: CompareCmd) -> Self {
        let mut config = Self::from_common(Mode::Compare, cmd.target_a, cmd.common);
        config.compare_target_b = Some(cmd.target_b);
        config
    }

    fn from_review(cmd: ReviewCmd) -> Self {
        let mut config = Self::from_common(
            Mode::Review,
            cmd.target.clone().unwrap_or_default(),
            cmd.common,
        );
        config.query = cmd.target;
        config.diff_staged = cmd.staged;
        config.limit = cmd.limit;
        config.offset = cmd.offset;
        config
    }

    fn from_show(cmd: ShowCmd) -> Self {
        let mut config = Self::from_common(Mode::Show, cmd.target, cmd.common);
        config.section = cmd.section;
        config.full = cmd.full;
        config.context_lines = cmd.context_lines;
        config
    }

    fn from_trace(cmd: TraceCmd) -> Self {
        match cmd.relation {
            TraceRelation::Callers(cmd) => Self::from_callers(cmd),
            TraceRelation::Callees(cmd) => Self::from_callees(cmd),
        }
    }

    fn from_common(mode: Mode, query: String, common: CommonArgs) -> Self {
        Self {
            mode,
            query: Some(query),
            scopes: common.scope,
            allow_multi_scope: matches!(mode, Mode::Search),
            section: None,
            context_lines: None,
            discover_as: None,
            match_explicit: false,
            inferred_text_or: false,
            budget: common.budget,
            no_budget: common.no_budget,
            full: false,
            artifact: ArtifactMode::from(common.artifact),
            expand: 0,
            access: false,
            glob: None,
            exclude: None,
            filter: None,
            count_by: None,
            detailed: false,
            diff_staged: false,
            depth: None,
            max_frontier: None,
            max_edges: None,
            skip_hubs: None,
            symbols: false,
            limit: None,
            offset: 0,
            compare_target_b: None,
        }
    }

    fn from_map(cmd: MapCmd) -> Self {
        Self {
            mode: Mode::Overview,
            query: None,
            scopes: cmd.common.scope,
            allow_multi_scope: false,
            section: None,
            context_lines: None,
            discover_as: None,
            match_explicit: false,
            inferred_text_or: false,
            budget: cmd.common.budget,
            no_budget: cmd.common.no_budget,
            full: false,
            artifact: ArtifactMode::from(cmd.common.artifact),
            expand: 0,
            access: false,
            glob: cmd.glob,
            exclude: None,
            filter: None,
            count_by: None,
            detailed: false,
            diff_staged: false,
            depth: cmd.depth,
            max_frontier: None,
            max_edges: None,
            skip_hubs: None,
            symbols: cmd.symbols,
            limit: None,
            compare_target_b: None,
            offset: 0,
        }
    }

    fn from_callers(cmd: CallersCmd) -> Self {
        let mut config = Self::from_common(Mode::Callers, cmd.symbol, cmd.common);
        config.expand = cmd.expand.unwrap_or(0);
        config.glob = cmd.glob;
        config.exclude = None;
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

    fn from_context(cmd: ContextCmd) -> Self {
        let mut config = Self::from_common(Mode::Context, cmd.symbol, cmd.common);
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
