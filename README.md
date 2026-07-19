# srcwalk

[![Crates.io](https://img.shields.io/crates/v/srcwalk)](https://crates.io/crates/srcwalk)
[![npm](https://img.shields.io/npm/v/srcwalk)](https://www.npmjs.com/package/srcwalk)
[![Discord](https://img.shields.io/discord/1401062214831575060?label=discord)](https://discord.gg/p7gj6BPb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Code navigation for AI agents** — exact reads, structural discovery, evidence packets,
one binary, zero config.

> Tree-sitter outlines · symbol/search discovery · callers/callees · deps ·
  context/review/diff packets · overview

## What it does

- **Show** — read files, line ranges, symbols, headings, and capped raw pages.
- **Discover** — find definitions, text-matched name occurrences, files, text,
  comments, and field/member access evidence.
- **Trace** — inspect callers and callees with bounded depth, hub guards,
  unresolved-call labels, and bounded direct-call evidence.
- **Context** — build one-target packets with Flow Maps, bounded same-file scoped
  name occurrences when structurally reliable, local structural links, call
  neighborhoods, and exact next commands.
- **Review & diff** — turn Git changes into bounded evidence packets for changed
  files, symbols, and untracked files.
- **Compare & assess** — compare two known targets structurally, or scan blast
  radius before changing a symbol.
- **Deps & overview** — inspect imports, links/assets, dependents, and a
  token-aware project skeleton.

Structural support covers Rust, TypeScript/TSX, JavaScript, Python, Go,
Java/Scala/Kotlin, C/C++, Ruby, PHP, C#, Swift, Elixir, CSS, SCSS, and Less.
Document navigation covers HTML, Markdown-style files, and `.rst` fallback.
Unsupported files still get smart text/outline reads.

## Install

```sh
# npm (recommended)
npm install -g srcwalk    # or: npx srcwalk

# If npm appears stuck, show the postinstall binary download:
npm install -g srcwalk --foreground-scripts

# crates.io
cargo install srcwalk --locked

# From source
cargo install --git https://github.com/sting8k/srcwalk --locked
```

<details>
<summary>Pre-built binaries</summary>

```sh
# macOS Apple Silicon
curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-aarch64-apple-darwin.tar.gz | tar xz -C /usr/local/bin

# macOS Intel
curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-x86_64-apple-darwin.tar.gz | tar xz -C /usr/local/bin

# Linux x86_64 (static musl)
curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-x86_64-unknown-linux-musl.tar.gz | tar xz -C ~/.local/bin

# Linux aarch64 (static musl)
curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-aarch64-unknown-linux-musl.tar.gz | tar xz -C ~/.local/bin
```

</details>

**Agent skill** — install the srcwalk skill into your agent environment.

```sh
npx skills add sting8k/srcwalk
```

After installing the CLI, `srcwalk guide` prints the full embedded, version-matched agent guide. The installable skill entry is [`skills/srcwalk/SKILL.md`](./skills/srcwalk/SKILL.md); it bootstraps agents to that embedded guide in the installed binary.

## Release notes

See [`CHANGELOG.md`](./CHANGELOG.md) for curated release notes. Maintainers should update the matching changelog section before pushing a `vX.Y.Z` tag; the release workflow uses that section as the GitHub Release body.

## Quick examples

These representative commands show the main shapes. Use `srcwalk --help` for the
full command and flag reference; agent routing lives in `srcwalk guide`.

```sh
# Read and drill into source
srcwalk src/auth.ts
srcwalk src/auth.ts:72
srcwalk src/auth.ts --section handleAuth

# Find and follow code
srcwalk discover handleAuth --scope src/
srcwalk context src/auth.ts:handleAuth
srcwalk trace callers handleAuth --scope src/
srcwalk trace callees handleAuth --detailed --scope src/

# Review changes and orient in a project
srcwalk review --staged
srcwalk deps src/auth.ts
srcwalk overview --scope src/
```

Discovery commands respect ignore files; explicit file reads can still inspect ignored paths.

## Output examples

Examples below use this repository. Timings may vary between machines; snippets are abbreviated only where `...` is shown.

<details>
<summary><b>Outline of a file</b></summary>

```
$ srcwalk src/evidence/next_action.rs
# src/evidence/next_action.rs (200 lines, ~1.2k tokens) [outline]

[1-]   imports: std::collections::BTreeMap, std::fmt::Write as _, crate::evidence
[7-13]       struct NextAction
[16-21]      enum NextActionConfidence
[23-107]     mod impl NextAction
  [24-38]      fn new
             pub(crate) fn new(
  [40-54]      fn from_evidence
             pub(crate) fn from_evidence(
  [56-68]      fn metadata
             pub(crate) fn metadata(
  [70-76]      fn guidance
             pub(crate) fn guidance(
  [78-80]      fn command
             pub(crate) fn command(&self) -> &str
  [82-84]      fn reason
             pub(crate) fn reason(&self) -> &str
  [86-88]      fn rank
             pub(crate) const fn rank(&self) -> u16
  [90-92]      fn confidence
             pub(crate) const fn confidence(&self) -> NextActionConfidence
  [94-96]      fn source_anchor
             pub(crate) fn source_anchor(&self) -> Option<&Anchor>
  [98-106]     fn sort_key
             fn sort_key(&self) -> (u16, u8, u32, &str, &str)
[109-118]    mod impl NextActionConfidence
  [110-117]    fn sort_rank
             const fn sort_rank(self) -> u8
[120-129]    mod impl NextActionConfidence
  [121-128]    fn from
             fn from(source: EvidenceSource) -> Self
[131-141]    fn render_next_actions
           pub(crate) fn render_next_actions(actions: &[NextAction]) -> String
[143-159]    fn ordered_unique
           fn ordered_unique(actions: &[NextAction]) -> Vec<NextAction>
[162-199]    mod tests
  [163]        import use std::path::Path;
  [165]        import use super::*;
  [168-185]    fn render_orders_by_rank_then_dedupes_by_command
             fn render_orders_by_rank_then_dedupes_by_command()
  [188-198]    fn duplicate_commands_keep_best_rank
             fn duplicate_commands_keep_best_rank()

> Next: drill into a symbol with --section <name> or a line range
> Next: need raw file text? retry with --full, or use --section <range> for a smaller slice.
```
</details>

<details>
<summary><b>Compact multi-section read</b></summary>

```
$ srcwalk src/evidence/next_action.rs --section "NextAction,render_next_actions,ordered_unique" --budget 260
# src/evidence/next_action.rs (35 lines, ~272 tokens) [3 symbols, compact (over limit)]

## section: NextAction [7-13] (compact)

   7 │ pub(crate) struct NextAction {
   8 │     command: String,
   9 │     reason: String,
  ... 4 lines omitted; narrow --section or raise --budget.

---

## section: render_next_actions [131-141] (compact)

  131 │ pub(crate) fn render_next_actions(actions: &[NextAction]) -> String {
  132 │     let actions = ordered_unique(actions);
  133 │     let mut out = String::new();
  ... 8 lines omitted; narrow --section or raise --budget.

---

## section: ordered_unique [143-159] (compact)

  143 │ fn ordered_unique(actions: &[NextAction]) -> Vec<NextAction> {
  144 │     let mut by_command = BTreeMap::<String, NextAction>::new();
  145 │     for action in actions {
  ... 14 lines omitted; narrow --section or raise --budget.

> Caveat: compacted ~272/260 tokens; shown 3 symbols.
> Next: narrow --section or raise --budget.
```
</details>

<details>
<summary><b>Context packet — Flow Map + scoped occurrences + call neighborhood</b></summary>

```
$ srcwalk context src/evidence/next_action.rs:ordered_unique --budget 1400
# Context Packet: src/evidence/next_action.rs:ordered_unique
confidence: structural syntax
caveat: source-evidence navigation only; no runtime proof

## Target
- src/evidence/next_action.rs:143-159 ordered_unique

## Flow Map
shape: 1 entry, 0 decisions, 1 loop, 1 exit, 4 actions
N1 entry :143-159 entry
  definitions: actions parameter :143
  next -> N2 action :144 BTreeMap::<String, NextAction>::new()
N2 action :144 BTreeMap::<String, NextAction>::new()
  calls: BTreeMap::<String, NextAction>::new :144
  writes: by_command assignment_lhs :144
  next -> N3 loop :145-154 actions
N3 loop :145-154 actions
  reads: actions condition :145
  body -> N4 action :146-153 by_command .entry(action.command.clone()) .and_modify(|existing| { if action.sort_key() < exist…
  next -> N5 action :156 by_command.into_values().collect()
N4 action :146-153 by_command .entry(action.command.clone()) .and_modify(|existing| { if action.sort_key() < exist…
  calls: by_command .entry :146
  reads: action.clone call_arg :153
  loop_back -> N3 loop :145-154 actions
N5 action :156 by_command.into_values().collect()
  calls: by_command.into_values :156
  writes: actions assignment_lhs :156
  next -> N6 action :157 actions.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()))
N6 action :157 actions.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()))
  calls: actions.sort_by :157
  reads: left call_arg :157; right call_arg :157; left.sort_key().cmp call_arg :157; +1 more
  next -> N7 return :159 end
N7 return :159 end

## Exits
- :159 end

## Scoped name occurrences (1)
target: ordered_unique
scope: src/evidence/next_action.rs:1-200

- src/evidence/next_action.rs:132
  let actions = ordered_unique(actions);
  source: AST identifier
  confidence: same-file structural scope candidate

> Caveat: scoped occurrences are not binding-, type-, or runtime-resolved references.

## Call Neighborhood
### Callees (ordered)
- L144 by_command = BTreeMap::<String, NextAction>::new()
- L153 by_command.entry(action.command.clone()).and_modify(|existing| { if action.sort_key() < existing.sort_key() { *existing = action.clone(); } }).or_insert_with(arg1=|| action.clone())
- L156 actions = by_command.into_values().collect()
- L157 actions.sort_by(arg1=|left, right| left.sort_key().cmp(&right.sort_key()))

### Resolved local callees
  [fn] NextAction src/evidence/next_action.rs:7-13
  [fn] new src/evidence/next_action.rs:24-38  pub(crate) fn new(
  [fn] sort_key src/evidence/next_action.rs:98-106  fn sort_key(&self) -> (u16, u8, u32, &str, &str)


### Callers
- [fn] render_next_actions src/evidence/next_action.rs:132

> Caveat: static context packet is capped; verify exact edges with trace commands.

> Next: srcwalk show src/evidence/next_action.rs:143-159 -C 20
> Next: srcwalk trace callers ordered_unique
> Next: srcwalk trace callees ordered_unique --detailed
```
</details>

<details>
<summary><b>Review packet — changed evidence + next reads</b></summary>

For staged or revision-range reviews, srcwalk summarizes the changed files,
hunks, changed symbols, bounded Flow Maps, and exact follow-up reads.

```
$ srcwalk review --staged --budget 1200
# Review Packet: staged
confidence: structural syntax + diff metadata
caveat: source-evidence navigation only; no runtime proof
files: changed=1 shown=1
hunks: total=1 shown=1
symbols: total=0 shown=0

## changed evidence

### README.md
status: modified
hunks:
- :286-323 file-level

## changed symbols
- none function-like in selected diff evidence

## flow maps
- none rendered; no changed function-like symbols in selected files

## omitted
- files: 0
- flow maps: 0

> Next: srcwalk show README.md:286-323 -C 20

(~131 tokens)
```
</details>

<details>
<summary><b>Discover — multi-symbol and multi-scope</b></summary>

```
$ srcwalk discover "render_next_actions, Anchor" --scope src/evidence --scope src/commands --limit 2
# Search: "render_next_actions" in 2 scopes — 2 matches (1 definitions, 1 name occurrences)
Scopes on this page: src/evidence (2), src/commands (0)
  [fn] render_next_actions src/evidence/next_action.rs:131-141
  source: ast · kind: definition · confidence: structural syntax

## src/evidence/mod.rs:14 [name occurrence]
source: text · kind: name occurrence · confidence: text evidence
→ [14]   pub(crate) use next_action::{render_next_actions, NextAction};

## Confirmed next context targets
> Next: srcwalk context src/evidence/next_action.rs:131-141

(~133 tokens)

> Next: 25 more matches available. Continue with --offset 2 --limit 2.
> Next: choose a confirmed context target above, or read exact hit evidence with `srcwalk show <path>:<line> -C 10`.

---
# Search: "Anchor" in 2 scopes — 2 matches (1 definitions, 1 name occurrences)
Scopes on this page: src/evidence (2), src/commands (0)

### File overview: src/evidence/anchor.rs (106 lines)
[1-]   imports: std::path, crate::format
[6-9]        struct Anchor
[12-16]      enum AnchorRange
[18-65]      mod impl Anchor
  [19-24]      fn file
             pub(crate) fn file(path: &Path) -> Self
  [26-32]      fn line
             pub(crate) fn line(path: &Path, line: u32) -> Self
  [34-41]      fn lines
             pub(crate) fn lines(path: &Path, start: u32, end: u32) -> Self
  [43-48]      fn start_line
             pub(crate) const fn start_line(&self) -> u32
  [50-52]      fn display
             pub(crate) fn display(&self) -> String
  [54-56]      fn display_relative_to
             pub(crate) fn display_relative_to(&self, scope: &Path) -> String
  [58-64]      fn display_with_path
             fn display_with_path(&self, path: &str) -> String
[68-106]     mod tests
  [69]         import use super::*;
  [72-80]      fn line_anchor_uses_existing_display_path
             fn line_anchor_uses_existing_display_path()
  [83-93]      fn range_anchor_uses_existing_relative_display_path
             fn range_anchor_uses_existing_relative_display_path()
  [96-105]     fn file_anchor_uses_existing_relative_display_path
             fn file_anchor_uses_existing_relative_display_path()
  [struct] Anchor src/evidence/anchor.rs:6-9
  source: ast · kind: definition · confidence: structural syntax

## src/evidence/anchor.rs:18 [name occurrence]
source: text · kind: name occurrence · confidence: text evidence
→ [18]   impl Anchor {
  [6-9]        struct Anchor
  [12-16]      enum AnchorRange
→ [18-65]      mod impl Anchor
    [19-24]      fn file
               pub(crate) fn file(path: &Path) -> Self

(~449 tokens)

> Next: 64 more matches available. Continue with --offset 2 --limit 2.
> Next: read exact hit evidence with `srcwalk show <path>:<line> -C 10`.
```
</details>

<details>
<summary><b>Multi-hop caller BFS</b></summary>

Trace callers transitively in one call:

```
$ srcwalk trace callers sort_key --scope src --depth 2
# BFS callers of "sort_key" in src — depth=2/2, 5 edges, 13 ms

── hop 1 (4 edges) ──
  ordered_unique               src/evidence/next_action.rs:147  → if action.sort_key() < existing.sort_key() {
  ordered_unique               src/evidence/next_action.rs:147  → if action.sort_key() < existing.sort_key() {
  ordered_unique               src/evidence/next_action.rs:155  → actions.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
  ordered_unique               src/evidence/next_action.rs:155  → actions.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

── hop 2 (1 edge) ──
  render_next_actions          src/evidence/next_action.rs:130  → let actions = ordered_unique(actions);

Static by-name call graph only. May miss indirect dispatch, reflection, macros, and calls from files > 500KB or from languages without a tree-sitter call query.

(~225 tokens)
```

Call-site source text disambiguates overloads. Budget notes flag cross-package name collisions and fan-out-capped symbols when they occur.

</details>

<details>
<summary><b>Did-you-mean — cross-convention + typo tolerance</b></summary>

```
$ srcwalk discover next_actoin --scope src/evidence
# Search: "next_actoin" in src/evidence — 0 matches

(~14 tokens)

> Did you mean: NextAction (src/evidence/next_action.rs:7), next_action (src/evidence/mod.rs:4)?
```
</details>

<details>
<summary><b>Token-aware overview</b></summary>

```
$ srcwalk overview --scope src/evidence --depth 1
# Overview: src/evidence (depth 1, sizes ~= tokens)
# Note: respects .gitignore, .git/info/exclude, core.excludesFile, .ignore (+ parents); dotfiles included; built-in SKIP_DIRS still apply (target, node_modules, …). Use `srcwalk <path>` to inspect an ignored file directly.
atom.rs  ~1.3k
next_action.rs  ~1.3k
anchor.rs  ~714
confidence.rs  ~269
mod.rs  ~82

> Next: no cross-group relations shown. Use `srcwalk deps <file>` for file-level deps, or adjust --scope/--depth.
```
</details>

## Speed

| Operation | ~30 files | ~1000 files |
|-----------|-----------|-------------|
| File read + outline | ~18ms | ~18ms |
| Find definitions/name occurrences | ~27ms | — |
| Overview | ~21ms | ~240ms |

Bloom-filter pruning + length-sorted memchr + tree-sitter parse cache.

## Key features

- **Intent-first analysis** — `discover`, `review`, `context`, `trace callers`,
  `trace callees`, `assess`, `deps`, `overview`.
- **Target-first reading** — `srcwalk <path>`, `<path>:<line>`, and `--section <symbol|range>`.
- **Multi-hop caller BFS** — up to 5 hops, hub guard, collision detection.
- **Forward callees** — resolved/unresolved calls, detailed ordered call sites, bounded unique-target argument mappings, and depth support.
- **Search ergonomics** — cross-naming-convention Did-you-mean, bare-filename auto-pick, typo tolerance.
- **Performance** — mmap walkers, Aho-Corasick, rayon-parallel search, mimalloc.

## License

MIT — originally forked from [jahala/tilth](https://github.com/jahala/tilth).
