srcwalk — code-intelligence CLI. Tree-sitter outlines, symbol definitions, callers (single-hop + multi-hop BFS), blast-radius deps, token-aware codebase maps.

Binary: `~/.cargo/bin/srcwalk` (in PATH). Invoke as `srcwalk <args>`.

## When to use srcwalk

Use srcwalk when the answer depends on code **structure**:
- Where is this symbol defined / who calls it / what does it call?
- What does this file import and who depends on it?
- Outline of a large file; drill into a specific symbol or line range.
- Token-annotated map of a codebase.
- Transitive caller chains (up to 5 hops) with call-site disambiguation.

**Don't** use srcwalk for plain text search, reading small files you already know the path of, listing paths to pipe, or complex regex. Use `rg`, `cat`, `fd` directly — they're faster and the output is familiar.

## Core commands

### Read a file
```
srcwalk <path>                   # outline if large, full if small
srcwalk <path> --section 45-89   # line range
srcwalk <path> --section "## H"  # markdown heading
srcwalk <path> --section symbol  # jump to symbol body
srcwalk <path> --full            # force full with line numbers
srcwalk <path> --budget 2000     # cap response tokens
```
0 bytes → `[empty]`. Binary → `[skipped]`. Generated (lockfiles, `.min.js`) → `[generated]`. Heading miss → top-5 closest suggestions. Outline cap → drill with `--section`.

### Symbol search
```
srcwalk <symbol> --scope <dir>              # definitions first, then usages
srcwalk "foo, bar, baz" --scope <dir>       # multi-symbol, one pass
srcwalk <symbol> --scope <dir> --expand     # inline source for top 2
srcwalk <symbol> --scope <dir> --expand=5   # inline source for top 5
```
Tree-sitter finds **definitions**, not just string matches. Expanded hits include a `── calls ──` footer of resolved callees (file, line range, signature). Every definition hit reports its line range (e.g. `[38-690]` vs `[9-16]`) — use this to tell the real impl from a stub (partial classes), disambiguate overloads, and rank drill order.

### Callers
```
srcwalk <symbol> --callers --scope <dir>
```
Structural. Includes type/constructor references (`new Foo()`, `Foo {}`), not just call expressions. Zero callers → output includes a per-language indirect-dispatch hint (trait objects, interfaces, reflection, callbacks). Check the hint before concluding dead code.

#### Multi-hop callers (BFS)
```
srcwalk <symbol> --callers --depth N --scope <dir>
srcwalk <symbol> --callers --depth N --json
```
Trace up to 5 hops. Flags:
- `--depth N` — 1 (default) up to 5.
- `--max-frontier K` — callers expanded per hop (default 50). Over-cap symbols auto-promoted to hubs, listed in `elided.auto_hubs_promoted`.
- `--max-edges M` — global cap (default 500). Deterministic truncation.
- `--skip-hubs CSV` — explicit skip-list. Default (language-agnostic): `new,clone,from,into,to_string,drop,fmt,default`. `--skip-hubs ""` to disable.
- `--json` — machine-readable edge list.

In `--json`, each edge has `hop, from, from_file, from_line, to, call_text`. Use `call_text` (raw call-site line) to disambiguate overloaded callees. Check `stats.suspicious_hops[]` before trusting deep hops — it flags cross-package name collisions. Check `elided` for truncation signals.

### Deps (blast radius)
```
srcwalk <file> --deps
```
Imports and dependents. Use before modifying a file.

### Map
```
srcwalk --map --scope <dir>
```
Structural skeleton with cumulative per-directory tokens (`src/ (~14.9k tokens)`). See scale before picking what to read.

## Pagination

`--limit N` / `--offset N` on symbol search, callers, deps. Deterministic order across runs. Footer: `Next page: --offset N --limit M.` or `(end of results)`.

## Supported languages

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift. Unsupported languages still work for file reading — just no structural outlines or definition detection.
