# Changelog

All notable changes to srcwalk are documented here.

## [Unreleased]

### Added
- Action-first analysis subcommands: `find`, `callers`, `callees`, `flow`, `impact`, `deps`, and `map`. Legacy flag syntax remains supported.

### Changed
- CLI help and the srcwalk skill now present the mental model as target-first file reading plus action-first analysis commands.

### Fixed

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
