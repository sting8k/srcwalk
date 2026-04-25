# srcwalk

[![Crates.io](https://img.shields.io/crates/v/srcwalk)](https://crates.io/crates/srcwalk)
[![npm](https://img.shields.io/npm/v/srcwalk)](https://www.npmjs.com/package/srcwalk)
[![Discord](https://img.shields.io/discord/1401062214831575060?label=discord)](https://discord.gg/p7gj6BPb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Agent's code navigator CLI** — instantly outline, search, and trace call graphs across any language. One binary, one skill, zero config.

> Tree-sitter outlines · symbol search · caller/callee graphs · blast-radius deps · token-aware maps

Small files come back whole, large files get a structural outline. Your agent reaches for `srcwalk` instead of `cat` and `grep` — cheap outlines first, drill on demand, never blow the token budget.

> Originally forked from [jahala/tilth](https://github.com/jahala/tilth), now developed independently.

## What it does

- **Read** — outline if large, full if small, `--section` to drill into a symbol or line range
- **Symbol search** — tree-sitter definitions first, then usages, with resolved callees
- **Callers** — single-hop or multi-hop BFS (up to 5 hops), hub guard, collision warnings
- **Callees** — forward call graph, resolved + unresolved, with depth support
- **Deps** — blast-radius: imports and dependents of a file
- **Map** — token-annotated directory skeleton, respects `.gitignore`

15 languages: Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift, Elixir.

## Install

```sh
# crates.io (recommended)
cargo install srcwalk

# npm
npm install -g srcwalk    # or: npx srcwalk

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

**Agent skill** — teaches your coding agent the command vocabulary:

```sh
npx skills add sting8k/srcwalk

# or install the skill file directly
mkdir -p ~/.pi/agent/skills/srcwalk && \
  curl -fsSL https://raw.githubusercontent.com/sting8k/srcwalk/main/skills/srcwalk/SKILL.md \
  -o ~/.pi/agent/skills/srcwalk/SKILL.md
```

Full skill at [`skills/srcwalk/SKILL.md`](./skills/srcwalk/SKILL.md).

## Release notes

See [`CHANGELOG.md`](./CHANGELOG.md) for curated release notes. Maintainers should update the matching changelog section before pushing a `vX.Y.Z` tag; the release workflow uses that section as the GitHub Release body.

## Quick examples

```sh
# Read a file (outline if large, full if small)
srcwalk src/auth.ts
srcwalk src/auth.ts:72                       # drill into exact hit line
srcwalk src/auth.ts --section handleAuth     # drill into symbol
srcwalk src/auth.ts --section 72             # focused line context
srcwalk src/auth.ts --section 44-89          # line range

# Symbol search
srcwalk handleAuth --scope src/              # definitions + usages
srcwalk Depends --filter 'path:param_functions' --scope .
srcwalk "foo, bar" --scope src/              # multi-symbol
srcwalk handleAuth --scope src/ --expand     # inline source + callees

# Callers (reverse call graph)
srcwalk handleAuth --callers --scope src/
srcwalk decompileFunction --callers --filter 'args:3' --scope src/
srcwalk handleAuth --callers --count-by caller --scope src/

# Callees (forward call graph)
srcwalk handleAuth --callees --scope src/
srcwalk handleAuth --callees --detailed --filter 'callee:validateToken' --scope src/
srcwalk handleAuth --callees --depth 2 --scope src/   # transitive

# Deps (blast radius)
srcwalk src/auth.ts --deps

# Map
srcwalk --map --scope src/
```

## Output examples

<details>
<summary><b>Outline of a large file</b></summary>

```
$ srcwalk src/auth.ts
# src/auth.ts (258 lines, ~3.4k tokens) [outline]

[1-12]   imports: express(2), jsonwebtoken, @/config
[14-22]  interface AuthConfig
[24-42]  fn validateToken(token: string): Claims | null
[44-89]  export fn handleAuth(req, res, next)
[91-258] export class AuthManager
  [99-130]  fn authenticate(credentials)
  [132-180] fn authorize(user, resource)
```
</details>

<details>
<summary><b>Symbol search — definitions first, with callees</b></summary>

```
$ srcwalk handleAuth --scope src/ --expand
# Search: "handleAuth" in src/ — 6 matches (2 definitions, 4 usages)

## src/auth.ts:44-89 [definition]
  44 │ export function handleAuth(req, res, next) {
  45 │   const token = req.headers.authorization?.split(' ')[1];
  ...

── calls ──
  validateToken    src/auth.ts:24-42
  refreshSession   src/auth.ts:91-120
```
</details>

<details>
<summary><b>Multi-hop caller BFS</b></summary>

Trace callers transitively in one call:

```
$ srcwalk NewClient --callers --depth 3 --json
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
$ srcwalk searchSymbol --scope src/
no matches for "searchSymbol" in src/
> Did you mean: search_symbol (src/lib.rs:186)
```
</details>

<details>
<summary><b>Token-aware map</b></summary>

```
$ srcwalk --map --scope .
src/       (~14.9k tokens)
  read/    (~10.2k tokens)
    outline/  (~3.7k tokens)
  search/  (~8.1k tokens)
```
</details>

## Speed

| Operation | ~30 files | ~1000 files |
|-----------|-----------|-------------|
| File read + outline | ~18ms | ~18ms |
| Symbol search | ~27ms | — |
| Map | ~21ms | ~240ms |

Bloom-filter pruning + length-sorted memchr + tree-sitter parse cache.

## Key features

- **Multi-hop caller BFS** (up to 5 hops, hub guard, collision detection)
- **`--callees` flag** — forward call graph as standalone query
- **File-grouped usages** with function annotations
- **`--section` degradation** — auto-outline when section too large
- **Search ergonomics** — cross-naming-convention Did-you-mean, bare-filename auto-pick, typo tolerance
- **Budget cascade** — `--full` over `--budget` degrades gracefully with explicit labels
- **Leaner CLI** — removed MCP server, edit mode, diff subcommand, `--files` dispatch
- **Performance** — mmap walkers, Aho-Corasick, rayon-parallel search, mimalloc
- **Elixir support**

## License

MIT — originally forked from [jahala/tilth](https://github.com/jahala/tilth).
