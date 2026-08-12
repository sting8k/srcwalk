# Changelog

All notable changes to srcwalk are documented here.

## [1.6.0] - 2026-08-12

### Added
- Default-on Go structural owner attribution and bounded mechanical call evidence for `discover --match any --as text`: compact file rollups include a narrowest-owner rollup (`owners (#N=Nth query term; *K=hits)`) and a `## Mechanical Go calls` appendix with capped (≤10), deterministically ordered, same-file `@:`-elided direct-call edges.

### Changed
- Go owner evidence is now emitted by default (no opt-in flag). It is navigation evidence only, not runtime order, dynamic dispatch, or an inferred chain; zero-edge owners abstain with an explicit caveat. Owner ranges are candidate exact reads, not relation or binding proof.

### Docs
- `skills/srcwalk/GUIDE.md` documents the owner rollup, the `[recv]`/`[local]`/`[bare]` edge labels, the `calls NAME` = call-expression-name honesty note, `@:` same-file elision, and the mechanical-filter/dynamic-dispatch caveat.

Known behavior: on very large monorepos, initial scope-narrowing may add extra navigation turns (srcwalk-general behavior, unrelated to owner evidence; observed in one eval3 run; tokens remained neutral-to-better).

## [1.5.0] - 2026-08-10

### Added
- Regex-dialect and path-fragment query handling in `discover`: regex-escape queries (`parseGitUrl\(`, `models\.json`) are detected and routed as symbol/text lookups, `.*`/`.+` patterns run bounded same-line co-occurrence, and unresolvable path fragments (`packages/ai`) return bounded path-fragment rows instead of dead-ending (US-059).
- Positional-read recovery: when `show path:line` cannot locate the target, the packet points back to `discover` instead of ending at a dead read (US-059b).
- Scope-miss fallback: file-target queries that return zero in-scope but exist outside scope now report the outside-scope matches with a `--scope .` retry hint (bounded to 5, file-target route only; does not widen symbol/text search) (US-062).
- Receiver/container-qualified symbol queries: dotted forms like `Batch.Set`, `Manager.ApplyConfig`, `Config.load` resolve to the correct method definition (Go receivers incl. pointer/value/generic; container-language outlines by parent), with a qualify hint when a bare name has multiple definitions. Definition-only; usages remain name-based (US-064).
- Version provenance in `--version`: embeds short git SHA, dirty flag, and UTC build date via `build.rs`; builds without `.git` report `(unknown)` and do not fail (US-061).

### Changed
- Next-action offer precision guardrails: large caller lists collapse to top 3 + `N more`, total offered range is capped, and wide ranges anchor at the correct structural-target seam to keep worst-case packets out of context (US-060, US-060b).
- In-packet offer dedupe: offers whose target range is fully contained in already-rendered packet content are dropped (wired for `discover`; trace/context deferred pending rendered-line tracking) (US-063).

## [1.4.0] - 2026-08-09

### Added
- Ruby structural navigation: outline, `deps` require/require_relative resolution, Flow Map with honest abstention on unsupported constructs.
- JS/TS logical import scanning with `tsconfig.json`/`jsconfig.json` paths-alias resolution; alias-verified local deps are labeled `via tsconfig paths`.
- Unresolved local-looking JS/TS imports reported as a distinct bounded evidence class with line anchors.
- Alias-aware callee resolution and read-layer related-file suggestions for JS/TS.
- JVM (Java/Kotlin/Scala) standard-library imports omitted from external deps with dot-boundary safety (`javaparser.*` stays visible).
- Go grouped `import (...)` block parsing and `go.mod` module-directive awareness so dotless module-local imports stay visible.
- Bounded Python, PHP, and C/C++ include resolution in `deps`: unique local candidates resolve, stdlib is external or omitted, and ambiguous or missing targets stay honestly unresolved.
- Overview `[relations]` are alias-aware for JS/TS via shared tsconfig-paths classification, and `[outbound deps]` list JS/TS targets outside `--scope` (relative and alias forms).
- `.mjs/.cjs/.mts/.cts` structural support with runtime-to-source extension swaps.
- Private-member caller/callee evidence and export-wrapper definition dedup (`context <symbol>` no longer reports duplicate candidates for one definition).
- Deterministic reverse-dependency evidence across worker counts; determinism test suites for deps and overview relations.
- Route-aware CLI hint when `--scope` receives space-separated directories.
- Cross-platform npm installer guard tests and an npm-package README.
- Flow context callers preserved for line-range targets via resolved symbols.

### Changed
- Parse-cache admission serialized to close a concurrent cap-bypass race; ranking and parse memory bounded; comment tagging and map relation scans parallelized with stable output ordering.
- Ruby require/require_relative resolution is scope-bounded: targets outside the active `--scope` are no longer claimed as local edges.

### Fixed
- Import keyword detection accepts tabs/multiple spaces; JVM `import static` and alias forms normalize correctly.

### Security
- Hardened npm binary installation with HTTPS-only bounded redirects, request timeouts, streaming size limits, published SHA-256 verification, strict single-binary archive validation, staged extraction, and atomic final placement.
- Pinned third-party CI actions to reviewed commit SHAs and added Linux/Windows npm installer guard jobs across the declared Node.js 14 floor and the release Node.js 20 runtime.

## [1.3.0] - 2026-07-27

This release reduces the number of steps needed to move from discovery output to exact source evidence.

### Navigation

**Discovery stopped at raw locations.**
Discovery returned `path:line` hits without a concrete follow-up command, so agents had to manually construct `show` targets, often guessing ranges or opening too much context.
→ Discovery now emits bounded `show` targets when structural ranges are reliable.

**Exact reads lacked structural context.**
`show file:44-50` returned raw lines without saying whether the range covered a complete function, a partial function, or no enclosing function at all.
→ Exact numeric reads now include source frames for enclosing and partial functions, without making runtime claims.

**Multiple reads in one file required multiple commands.**
Reading two ranges in the same file required separate `show` calls.
→ Same-file ranges can now be read as `show file:a,b` through the section reader.

**Context was single-target only.**
Comparing nearby exact targets required repeated `context` calls.
→ `context` now accepts up to three exact targets with one shared budget.

**Large overviews buried the useful areas.**
Broad overviews listed too many same-depth directories and made agents hunt for representative entry points.
→ Overview now prioritizes representative areas and emits concrete dependency follow-ups.

### Output bounds

**One large target could hide the rest.**
Batched reads could let the largest target consume the whole response budget.
→ Batched reads now share one budget and redistribute unused space across targets.

**Next actions could be too broad.**
Suggestions such as `show file:1-500` forced agents to narrow again before getting useful evidence.
→ Next actions now omit overly broad ranges and rank remaining commands by evidence quality.

**The default budget clipped useful packets.**
The 5,000-token default truncated some evidence packets that were still useful.
→ The default budget is now 6,000 tokens.

### Command recovery

Several near-valid inputs previously failed with generic errors:

| Input | Previous behavior | New behavior |
| --- | --- | --- |
| `show file/a file/b` | Reported an unrecognized argument | Hints to use comma-separated targets |
| `discover foo --scope src --scope tests` | Rejected the repeated scopes without naming the valid form | Explains when repeated symbol scopes are supported |
| `context file:1-10,file:20-30` | Failed without explaining the unsupported shorthand | Explains that context targets need explicit paths |

Corrections are only shown when srcwalk can identify a specific valid replacement.

### Reliability

- Incomplete glob hints now fail cleanly instead of reaching an invalid internal state.
- Caller recovery is preserved when target grammar is accepted but no callers are found.
- Generated follow-up commands now quote paths containing spaces or apostrophes.
- Executable usage assertions are portable across supported platforms.

## [1.2.0] - 2026-07-19

### Added
- Added bounded same-file scoped name-occurrence candidates to exact `context` targets when the declaration target and scope are structurally reliable.

### Changed
- Split symbol discovery evidence into parser-backed definitions, text-matched name occurrences, and literal text matches, with caveats instead of implying binding-resolved references.

### Fixed
- Prevented Python attribute and subscript assignment targets from being treated as local bindings that hide valid outer-name occurrences.

## [1.1.0] - 2026-07-13

### Added
- Added bounded local structural links to context call neighborhoods and bounded unique direct-call evidence with positional argument-to-parameter mappings to context and `trace callees --detailed`. Evidence uses exact source anchors and abstains when targets or local predecessors are ambiguous.

### Changed
- Aligned CLI error and overflow wording with command behavior, including unresolved callee labels and discover mode errors.
- Raised the default response budget from 5,000 to 6,000 tokens; explicit `--budget`, `--no-budget`, raw-read, overview, and evidence-row caps are unchanged.

## [1.0.1] - 2026-05-26

### Changed
- Added semantic drilldown footers to directory reads and made `overview --symbols` emit budget-adaptive inline `kind name@line-range` anchors before falling back to compact symbol names.
- Extended `show -C/--context-lines` to line ranges, resolved sections, and comma-separated show/section targets; comma-separated multi reads clamp each target to 10 context lines.
- Updated discover next-step guidance to prefer confirmed `context` targets when structural candidates exist and raw `show <path>:<line> -C 10` reads for text hits.
- Reframed the embedded agent guide as an evidence contract with explicit `srcwalk`-before-`rg` routing and comma-separated literal OR discovery guidance.

## [1.0.0] - 2026-05-24

### Highlights
- Reworked srcwalk around intent-first commands for agent navigation: `discover`,
  `show`, `trace`, `context`, `assess`, `overview`, `deps`, `compare`, `diff`,
  and `review`.
- Added evidence packets with `source`, `kind`, `confidence`, `caveat`, and
  next-action guidance so agents can bound claims and continue from exact reads.
- Added change-review and diff evidence flows for Git worktrees.
- Added HTML/Markdown document navigation and CSS/SCSS/Less structural navigation.
- Improved bundled/minified artifact navigation with provenance labels and clearer
  caveats.

### Breaking changes
- Removed legacy action-first commands and root analysis flags. Use the
  intent-first commands in the migration table below.
- Renamed the repo-orientation command from `map` to `overview`.
- Root positional input is now exact path evidence. Use `srcwalk discover <query>`
  for search.
- Slash-delimited text queries are literal text. Use `rg` for raw regex search.

### Migration notes
| Before | Use now |
| --- | --- |
| `srcwalk find <query>` | `srcwalk discover <query>` |
| `srcwalk files <glob>` | `srcwalk discover <glob> --as file` |
| `srcwalk callers <target>` | `srcwalk trace callers <target>` |
| `srcwalk callees <target>` | `srcwalk trace callees <target>` |
| `srcwalk flow <target>` | `srcwalk context <target>` |
| `srcwalk impact <target>` | `srcwalk assess <target>` |
| `srcwalk map` | `srcwalk overview` |
| `srcwalk <query>` | `srcwalk discover <query>` |

### Added
- Added `review` for change-set evidence packets with Flow Map context, changed
  symbol summaries, and next-step commands.
- Added `diff` for structured Git diff evidence across ranges, staged changes,
  working-tree changes, and untracked files.
- Added `compare <target-a> <target-b>` for structural shared/only evidence
  between two known function-like targets, without equivalence or runtime claims.
- Added `discover --as access` for field/member access grouping across writes,
  resets, reads, and unknown text evidence.
- Added HTML and Markdown-style document navigation for sections, elements, code
  blocks, links, and assets.
- Added CSS, SCSS, and Less structural navigation for selectors, at-rules,
  variables/properties, mixins/functions, imports, and `url(...)` references.
- Added `show -C/--context-lines`, strict comma-separated multi-location `show`,
  `discover --match all`, `discover --exclude`, and explicit literal OR through
  `discover --match any --as text`.
- Added smarter routing for exact bundled/minified artifact reads, exact
  artifact-file scopes, and path-like file glob discovery.

### Changed
- Upgraded `context` into a one-target understanding packet with Flow Map
  evidence, caller/callee neighborhood summaries, trust labels, and exact next
  reads.
- Made `deps <file>` show explicit outbound and inbound sections by default,
  including empty sections when no edges are found.
- Grouped compact discover facets by file to reduce repeated path tokens while
  preserving exact `:line-range` evidence.
- Updated `compare` follow-up footers to route one-target follow-up reads through
  `context`.
- Refreshed README examples and `srcwalk guide` routing text around the
  intent-first command model and evidence-label interpretation.

### Fixed
- Bounded default `discover --as access` output and added pagination guidance for
  large result sets.
- Counted untracked diff file lines without loading the whole file into memory.
- Hardened quoted Git diff path parsing for paths containing ` b/` inside names.
- Made review fallback routing resilient to wording changes in unsupported-target
  diagnostics.
- Fixed Windows CI builds for parser dependencies by using the Clang toolchain
  environment consistently.
- Improved path disambiguation when canonical and displayed temporary paths differ.

## [0.5.0] - 2026-05-11

### Added
- Added Windows x64 CI, release asset, and npm install/run support, with path display, path filters, and path reads covered by Windows-specific tests.

### Changed
- Improved JavaScript and TypeScript local dependency extraction for ESM specifiers, re-exports, and CommonJS `require` calls, so `map` and `deps` report more complete local relations.
- Documented Windows verification expectations for maintainers and tightened pull request workflow permissions.

### Fixed
- Hardened source discovery against symlink scope escapes and invalid `SRCWALK_THREADS` values.
- Reduced noisy reverse dependency evidence by capping candidate searches and avoiding bare child-member matches without owner context.

## [0.4.1] - 2026-05-10

### Added
- Improved artifact-mode JS/TS navigation with anchors, byte-range evidence, and section reads.
- Extended artifact routing across find, flow, impact, callers, and callees.

### Changed
- Improved grouped artifact output, compact match snippets, and long-line hit evidence around matched terms.
- Clarified artifact workflow guidance and added regression coverage for artifact navigation and path reads.

## [0.4.0] - 2026-05-09

### Added
- Added action-first command routing for `find`, `files`, `callers`, `callees`, `deps`, `flow`, `impact`, and path reads, backed by split command services and focused integration coverage.
- Added dependency-aware `map` output with local relation groups, outbound dependency previews for narrowed scopes, and shallow directory token rollups for deep source trees.
- Added JS/TS artifact-mode navigation for bundle anchors, artifact reads, artifact search snippets, and artifact caller/callee support within the supported language surface.
- Added path range read shortcuts such as `srcwalk <path>:start-end` for direct evidence reads without shell `sed`/`head` chains.

### Changed
- Split large CLI, read, search, display, ranking, symbol, and command modules into smaller focused modules while preserving existing command behavior.
- Streamlined `srcwalk guide`, README, and agent instructions around srcwalk-first navigation, `srcwalk files` for project filename discovery, narrow `--scope`, and generic unsupported-file fallbacks.
- Improved caller, callee, deps, find, and map output UX with more compact semantic rows, directory grouping, footer tips, and clearer scope/depth wording.

### Fixed
- Fixed unsupported `find` query syntax diagnostics so misuse now reports supported forms instead of falling through to misleading path-not-found errors.
- Fixed map depth handling so explicit `--depth` remains strict while token totals still roll up deep files into visible shallow directories.
- Fixed several search/read edge cases around multi-scope queries, artifact snippets, path-line routing, semantic context display, and pagination coverage.

## [0.3.2] - 2026-05-07

### Changed
- Removed the legacy `benchmark/` harness from the repository and ignored local benchmark workspaces now that the maintained retrieval benchmark lives outside the published tree.

### Fixed
- Improved `callers` ranking for common/overloaded method names by showing named contexts before top-level matches, demoting repeated callsites from the same caller context, and ranking explicit receivers ahead of self/no-receiver calls without filtering any candidates.

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
