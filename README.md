# srcwalk

[![Crates.io](https://img.shields.io/crates/v/srcwalk)](https://crates.io/crates/srcwalk)
[![npm](https://img.shields.io/npm/v/srcwalk)](https://www.npmjs.com/package/srcwalk)
[![Discord](https://img.shields.io/discord/1401062214831575060?label=discord)](https://discord.gg/p7gj6BPb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Agent's code navigator CLI** — target-first file reading, action-first code analysis, one binary, zero config.

> Tree-sitter outlines · symbol search · caller/callee graphs · deps · maps · token-aware footers

File reads default to structural views, not raw full-file dumps. Use target-first reads for files (`srcwalk <path>`, `<path>:<line>`, `--section`) and action-first commands for analysis (`find`, `callers`, `callees`, `deps`, `map`).

> Originally forked from [jahala/tilth](https://github.com/jahala/tilth), now developed independently.

## What it does

- **Read** — structural outline by default; `--section` and capped `--full` for explicit raw pages
- **Find** — tree-sitter definitions first, then usages, with optional inline source
- **Callers** — single-hop or multi-hop BFS (up to 5 hops), hub guard, collision warnings
- **Callees** — forward call graph, resolved + unresolved, with depth support
- **Deps** — blast-radius: imports and dependents of a file
- **Map** — token-annotated directory skeleton, respects `.gitignore`, `.ignore`, git excludes, and parent ignores

Structural support for Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift, Elixir, and Kotlin. Unsupported files still get smart text/outline reads.

## Install

```sh
# npm (recommended)
npm install -g srcwalk    # or: npx srcwalk

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

```sh
# Read a file (structural view by default; raw pages are explicit)
srcwalk src/auth.ts
srcwalk src/auth.ts:72                       # drill into exact hit line
srcwalk src/auth.ts --section handleAuth     # drill into symbol
srcwalk src/auth.ts --section 72             # focused line context
srcwalk src/auth.ts --section 44-89          # line range

# Find definitions/usages/text/name globs
srcwalk find handleAuth --scope src/                  # definitions + usages
srcwalk find "foo, bar" --scope src/ --scope tests/   # multi-symbol + multi-scope
srcwalk find '*Controller' --scope src/ --filter kind:class
srcwalk find handleAuth --scope src/ --expand         # inline source context
srcwalk files '*.ts' --scope src/                     # file globs live under files

# Callers (reverse call graph)
srcwalk callers handleAuth --scope src/
srcwalk callers decompileFunction --filter 'args:3' --scope src/
srcwalk callers handleAuth --count-by caller --scope src/

# Callees (forward call graph)
srcwalk callees handleAuth --scope src/
srcwalk callees handleAuth --detailed --filter 'callee:validateToken' --scope src/
srcwalk callees handleAuth --depth 2 --scope src/   # transitive

# Flow (compact slice: ordered calls + local resolves + callers)
srcwalk flow handleAuth --filter 'callee:validateToken' --scope src/

# Impact (heuristic blast-radius triage)
srcwalk impact validateToken --scope src/

# Deps (blast radius)
srcwalk deps src/auth.ts

# Map
srcwalk map --scope src/
```

Discovery commands respect ignore files; explicit file reads can still inspect ignored paths.

## Output examples

<details>
<summary><b>Outline of a large file</b></summary>

```
$ srcwalk src/auth.ts
# src/auth.ts (258 lines, ~3.4k tokens) [outline]

[1-12]       imports: express(2), jsonwebtoken, @/config
[14-22]      interface AuthConfig
[24-42]      fn validateToken
             function validateToken(token: string): Claims | null
[44-89]      fn handleAuth
             export function handleAuth(req, res, next)
[91-258]     class AuthManager
  [99-130]     fn authenticate
  [132-180]    fn authorize

> Next: drill into a symbol with --section <name> or a line range
```
</details>

<details>
<summary><b>Compact multi-section read</b></summary>

```
$ srcwalk src/auth.ts --section "handleAuth,120-140,authorize" --budget 900
# src/auth.ts (86 lines, ~1.1k tokens) [2 sections, compact (over limit)]

## section: handleAuth, 120-140 [44-140] (compact)

   44 │ export function handleAuth(req, res, next) {
   45 │   const token = req.headers.authorization?.split(' ')[1];
  ...
► 120 │   audit.log({ user, route: req.path });
  ... 82 lines omitted; narrow --section or raise --budget.

---

## section: authorize [132-180] (compact)

  132 │ authorize(user, resource) {
  133 │   return this.policy.can(user, resource);
  ... 46 lines omitted; narrow --section or raise --budget.

> Caveat: compacted ~1100/900 tokens; shown 2 sections.
> Next: narrow --section or raise --budget.
```
</details>

<details>
<summary><b>Find — multi-symbol and multi-scope</b></summary>

```
$ srcwalk find "handleAuth, validateToken" --scope src --scope tests --limit 2
# Search: "handleAuth" in 2 scopes — 2 matches (1 definitions, 1 usages)
Scopes on this page: src (1), tests (1)
  [fn] handleAuth src/auth.ts:44-89
  [usage] tests/auth.test.ts:18 handleAuth(req, res, next)

> Next: 3 more matches available. Continue with --offset 2 --limit 2.
> Next: drill into any hit with `srcwalk <path>:<line>`.

---
# Search: "validateToken" in 2 scopes — 2 matches (1 definitions, 1 usages)
Scopes on this page: src (1), tests (1)
  [fn] validateToken src/auth.ts:24-42
  [usage] tests/auth.test.ts:9 validateToken(token)
```
</details>

<details>
<summary><b>Multi-hop caller BFS</b></summary>

Trace callers transitively in one call:

```
$ srcwalk callers NewClient --depth 3 --json
{
  "edges": [
    { "hop": 1, "from": "newDefaultClient", "from_file": "client/factory.go",
      "to": "NewClient", "call_text": "return NewClient(opts)" },
    { "hop": 2, "from": "Bootstrap", "from_file": "cmd/server/main.go",
      "to": "newDefaultClient", "call_text": "c, err := newDefaultClient(cfg)" },
    { "hop": 3, "from": "main", "from_file": "cmd/server/main.go",
      "to": "Bootstrap", "call_text": "if err := Bootstrap(ctx); err != nil {" }
  ],
  "stats": { "edges_per_hop": [4, 7, 3], "suspicious_hops": [] },
  "elided": { "auto_hubs_promoted": ["Error"], "edges_truncated": 0 }
}
```

`call_text` disambiguates overloads. `suspicious_hops` flags cross-package name collisions. `auto_hubs_promoted` shows fan-out-capped symbols.

</details>

<details>
<summary><b>Did-you-mean — cross-convention + typo tolerance</b></summary>

```
$ srcwalk find read_file_with_budgt --scope src
# Search: "read_file_with_budgt" in src — 0 matches

(~14 tokens)

> Did you mean: read_file_with_budget (src/lib.rs:686)?
```
</details>

<details>
<summary><b>Token-aware map</b></summary>

```
$ srcwalk map --scope .
# Map: . (depth 3, sizes ~= tokens)
# Note: respects .gitignore, .ignore, and parent ignores; explicit file reads can still inspect ignored paths.

src/       ~180k
  search/  ~87k
  read/    ~26k

> Next: add --symbols, or narrow with --scope <dir>.
```
</details>

## Speed

| Operation | ~30 files | ~1000 files |
|-----------|-----------|-------------|
| File read + outline | ~18ms | ~18ms |
| Find definitions/usages | ~27ms | — |
| Map | ~21ms | ~240ms |

Bloom-filter pruning + length-sorted memchr + tree-sitter parse cache.

## Key features

- **Command-first analysis** — `find`, `callers`, `callees`, `flow`, `impact`, `deps`, `map`.
- **Target-first reading** — `srcwalk <path>`, `<path>:<line>`, and `--section <symbol|range>`.
- **Multi-hop caller BFS** — up to 5 hops, hub guard, collision detection.
- **Forward callees** — resolved/unresolved calls, detailed ordered call sites, and depth support.
- **Search ergonomics** — cross-naming-convention Did-you-mean, bare-filename auto-pick, typo tolerance.
- **Performance** — mmap walkers, Aho-Corasick, rayon-parallel search, mimalloc.

## License

MIT — originally forked from [jahala/tilth](https://github.com/jahala/tilth).
