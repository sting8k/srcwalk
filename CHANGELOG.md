# Changelog

All notable changes to srcwalk are documented here.

## Unreleased

## [0.3.1] - 2026-05-07

### Changed
- Grouped `srcwalk files` human output by directory by default, making larger file glob result sets easier to scan without adding a new flag.

### Fixed
- Fixed JavaScript and TypeScript IIFE handling so named, async, generator, anonymous, arrow, and assigned IIFEs surface useful outline/search/caller/callee contexts.
- Fixed JavaScript and TypeScript assigned arrow functions so `find`, `callees`, and `callers` can use the assigned variable name as the callable context.

## [0.3.0] - 2026-05-05

### Added
- Added `srcwalk guide` to print the full embedded, version-matched agent guide from the binary, making the installed binary the source of truth for agent routing policy.
- Added `srcwalk version --check` to check the latest release and print update commands for npm, cargo, and Git installs.
- Added multi-symbol + multi-scope `find` support, so commands like `srcwalk find "A, B, C" --scope src --scope tests` now search each query across all scopes and render one section per query.
- Added compact multi-section fallback output when `--section A,B --budget N` exceeds the section budget; output now keeps section labels, useful code snippets, omitted-line metrics, and concise follow-up hints instead of returning only a caveat.
- Added C/C++ declarator-based function-name extraction for structural search and sections, including K&R-style definitions such as `rust_demangle_callback`.

### Changed
- Converted the installable `skills/srcwalk/SKILL.md` into a minimal bootstrap entry that points agents to `srcwalk guide`; `compatible_srcwalk` now requires a binary with the embedded guide contract.
- Updated root `--help` and README examples to surface `srcwalk guide`, action-first analysis, multi-scope/multi-symbol `find`, compact section reads, and current footer shapes.
- Clarified paginated multi-scope `find` output by labeling page-local scope counts as `Scopes on this page:` instead of implying totals.
- Shortened section-budget footers to keep enough metrics for agents without verbose prose.

### Fixed
- Fixed repeated `--scope` combined with comma-separated `find` queries; each query now correctly searches all scopes instead of silently losing multi-scope behavior.
- Fixed paginated search count summaries so definition/usage/comment counts reflect the rendered page while continuation hints still use total matches.
- Fixed multi-section over-budget reads returning no body for large selections; they now degrade to compact snippets with anchor lines for requested ranges inside merged symbol sections.
- Fixed C structural outlines and symbol searches that previously surfaced some function definitions as `<anonymous>`, enabling name-glob `find` and `--section <symbol>` on affected C files.

## [0.2.7] - 2026-05-05

### Added
- Added `srcwalk files '<glob>'` for ignore-aware, paginated file discovery.
- Added symbol-name glob search in `find` for patterns like `displayAjax*`, `*Controller`, and `run_{full,expanded}*`.
- Added comma-separated mixed `--section` reads and repeated `--scope` support for `find`.

### Changed
- Moved file-glob queries out of `find`; use `srcwalk files '<glob>'` instead.
- Made `--expand` budget-aware so inline source is capped separately from search hits, with compact omitted-hit metrics.
- Made explicit `--full --budget <N>` act as the raw-read cap while preserving default safety caps.
- Shortened agent-facing caveats for capped output, callers, impact, and path-like misses.

## [0.2.6] - 2026-05-04

### Added
- Action-first analysis subcommands: `find`, `callers`, `callees`, `flow`, `impact`, `deps`, and `map`. Legacy flag syntax remains supported.
- The srcwalk skill now includes example output shapes for `flow` and `impact` so agents understand their orientation/triage roles.

### Changed
- CLI help, README, and the srcwalk skill now present the mental model as target-first file reading plus action-first analysis commands.
- Footer hints now use semantic prefixes: `Next:` for suggested commands, `Note:` for context/status, and `Caveat:` for limitations.

### Fixed
- `--section <symbol>` no longer degrades to an outline solely because the section exceeds 200 lines; explicit sections now return source when within the effective token budget.
- Path-like queries with separators now fail fast with an `fd` candidate hint when the file does not exist, instead of falling back to search and implying nested paths are unsupported.

## [0.2.5] - 2026-04-26

### Added
- `--map` now honors `--glob` to focus structural maps by file pattern while preserving directory rollups.

### Changed
- `--map --depth N` now controls tree depth instead of always using depth 3.
- `--map` now orders directories first, largest first, then files largest first for more useful agent navigation scaffolds.
- The srcwalk skill map examples now mention `--depth` and `--glob`.

### Fixed
- `--map --filter` and `--map --json` now fail clearly instead of acting as silent no-ops.

## [0.2.4] - 2026-04-26

### Added
- `--filter 'kind:base'` for neutral C# base-list relationships such as `class X : Y`, without claiming whether `Y` is a class or interface.

### Fixed
- `--filter 'kind:impl'` now displays Rust trait impl blocks as `[impl] impl Trait for Type path:start-end` instead of mislabeling associated type children.
- Java and TypeScript `class X implements Interface` relationships are now detected as `kind:impl`.

## [0.2.3] - 2026-04-25

### Changed
- File reads now default to structural views instead of raw full-file output; raw bodies require explicit `--full` or `--section` and are capped at 200 lines / 5k tokens.
- README, CLI help, and srcwalk skill guidance now emphasize outline-first drill-in reads instead of early `--full` usage.

## [0.2.2] - 2026-04-25

### Added
- Lab `--flow` slices for compact function-level call exploration.
- `--impact` slices for name-matched direct caller impact, with receiver/file grouping and broad-symbol warnings.
- `--filter 'callee:NAME'` for `--flow` and `--callees --detailed` callsite slices.

### Changed
- `--flow` resolves prioritize local helpers and stay hard-capped for readable agent output.
- README and srcwalk skill examples now document flow and detailed callee filtering.

### Fixed
- Existing file paths with spaces now classify as paths without requiring `--path-exact`.
- Nested C# methods under namespace/class containers are detected as symbol definitions, enabling method-level `--flow`.

## [0.2.0] - 2026-04-25

### Added
- General search filters: `--filter 'path:TEXT file:TEXT text:TEXT kind:fn'` now narrow normal symbol/content search results.
- Caller classification filters: `--filter 'args:N receiver:NAME caller:NAME path:TEXT text:TEXT'` narrow direct `--callers` rows.
- Caller aggregation: `--count-by args|caller|receiver|path|file` groups direct call sites into semantic `[group] field=value count=N` rows.

### Changed
- Caller outputs now show compact callsite facts (`recv=`, `args=`) and contextual tips only when useful.
- Caller `--count-by` output is paginated for large group sets and emits continuation hints.
- README and srcwalk skill examples now document callsite classification and general path filtering.

### Fixed
- `--count-by` with zero matches now returns the standard no-callers diagnostic instead of an empty grouping header.
- Caller-only filter qualifiers (`args:`, `receiver:`, `caller:`) now fail clearly when used outside `--callers`.

### Examples
```bash
srcwalk Depends --filter 'path:param_functions' --scope .
srcwalk decompileFunction --callers --count-by args --scope .
srcwalk decompileFunction --callers --filter 'args:2' --scope .
```

## [0.1.9] - 2026-04-24

### Changed
- Maintenance release before caller classification and general filtering work.
