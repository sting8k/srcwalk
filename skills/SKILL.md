---
name: tilth
description: "Code-intelligence CLI — structural outlines, symbol definitions, callers (incl. multi-hop BFS), blast-radius deps, token-aware codebase maps. Use when the answer depends on code structure. For plain text search, reading small files, or listing paths, use ripgrep / cat / fd."
---

# Tilth — Code Intelligence CLI

tilth is a code-intelligence tool built on tree-sitter. It answers questions grep and cat can't: *where is this symbol defined*, *who calls it*, *what does this file depend on*, *what does this codebase look like structurally*.

**Use tilth for:** outlines of large files, symbol definitions, callers (single-hop or transitive BFS), file dependencies, codebase maps, jumping to a symbol body, call-chain tracing.

**Don't use tilth for** plain text search, reading small files whose path you know, listing paths to pipe, or complex regex. Use `rg`, `cat`, `fd` directly — they're faster and you already know how to read their output.

**Binary:** `~/.cargo/bin/tilth` (in PATH).

```bash
tilth <args>
```

---

## Read a large file (outline + drill-in)

```bash
tilth <path>                          # outline if large, full if small
tilth <path> --section 45-89          # exact line range
tilth <path> --section "## Foo"       # markdown heading
tilth <path> --section validateToken  # jump to a symbol's body by name
tilth <path> --full                   # force full output with line numbers
tilth <path> --budget 2000            # cap response to ~N tokens
```

**Behaviour table:**

| Input | Output |
|---|---|
| 0 bytes | `[empty]` |
| Binary | `[skipped]` with mime type |
| Generated (lockfiles, `.min.js`) | `[generated]` |
| < ~6000 tokens | Full content, line-numbered |
| > ~6000 tokens | Structural outline with line ranges |
| `--full` over cap | Progressive: header + first 200 numbered lines + outline + continuation hint |

On a heading miss, top-5 closest matches are suggested. Outlines are capped at a safe line count — when capped, drill in with `--section <symbol>` or a line range.

---

## Search for symbols (definitions + usages)

```bash
tilth <symbol> --scope <dir>                    # definitions first, then usages
tilth "foo, bar, baz" --scope <dir>             # multi-symbol, one pass
tilth <symbol> --scope <dir> --expand           # inline source for top 2
tilth <symbol> --scope <dir> --expand=5         # inline source for top 5
```

Tree-sitter finds where symbols are **defined**, not just where strings appear. Each match shows the surrounding file structure so you know context without a second read.

Expanded definitions include a **callee footer** (`── calls ──`) listing resolved callees with file, line range, and signature — follow call chains without separate searches.

---

## Callers — who calls this symbol

```bash
tilth <symbol> --callers --scope <dir>
```

Structural (tree-sitter), not text-based. Includes type/constructor references (`new Foo()`, `Foo {}`), not just function calls.

**When callers returns 0**, output includes a per-language hint about indirect dispatch (trait objects, interfaces, reflection, callbacks, duck typing). A symbol with zero direct callers is often still in use — check the hint before concluding it's dead code.

### Multi-hop callers (BFS)

```bash
tilth <symbol> --callers --depth <N> --scope <dir>
tilth <symbol> --callers --depth <N> --json
```

Trace callers transitively up to `N` hops (max 5). Use this instead of looping `--callers` manually.

- `--depth N` — 1 (default) up to 5.
- `--max-frontier K` — callers expanded per hop (default 50). Excess symbols auto-promoted to hubs, listed in `elided.auto_hubs_promoted`.
- `--max-edges M` — global edge cap (default 500). Truncation is deterministic.
- `--skip-hubs CSV` — explicit hub-skip list. Default is language-agnostic (`new,clone,from,into,to_string,drop,fmt,default`). `--skip-hubs ""` to disable.
- `--json` — machine-readable edge list.

**For agents reading `--json`:**

- Each `edges[]` entry has `hop, from, from_file, from_line, to, call_text`. Use `call_text` (the raw call-site line) to disambiguate overloaded callee names — you see `errors.New("timeout")` vs `pool.New(cfg)` directly, no extra lookup.
- Check `stats.suspicious_hops[]` before trusting deep hops. Entries there flag cross-package name collisions (e.g. `→ New` matching hundreds of unrelated `New` definitions). When flagged, qualify the target, drop that hop, or filter edges client-side using `call_text`.
- Check `elided` for truncation signals: `edges_cut_at_hop`, `frontier_cuts`, `auto_hubs_promoted`.

---

## Blast radius — file dependencies

```bash
tilth <file> --deps
```

Imports (what this file depends on) and dependents (what depends on it). Use before modifying a file to understand impact.

---

## Codebase map

```bash
tilth --map --scope <dir>
```

Structural skeleton. **Every directory is annotated with cumulative tokens of its descendants** (`src/ (~14.9k tokens)`, `.pi-lens/ (~175.9k tokens)`). See scale before choosing what to read. Auto k/M formatting.

---

## Pagination

`--limit N` and `--offset N` work on symbol search, callers, and deps. Ordering is stable across runs (deterministic sort), so retries return identical pages.

```bash
tilth <symbol> --scope . --limit 10              # first page
tilth <symbol> --scope . --limit 10 --offset 10  # second page
```

Output ends with `Next page: --offset N --limit M.` or `(end of results)`. No silent caps — at ≥100k matches you get a soft warning but the result set is still complete.

---

## Workflow patterns

### Understanding a new codebase
1. `tilth --map --scope .` — skeleton + directory token scale; skip huge subtrees
2. `tilth <key-file>` — outline the interesting files
3. `tilth <file> --section <range-or-symbol>` — drill into specific parts

### Finding where a symbol lives
1. `tilth <symbol> --scope .` — definitions first, usages after
2. Follow the `── calls ──` footer to trace call chains
3. If you need all call sites: `tilth <symbol> --callers --scope .`

### Tracing impact before a change
1. `tilth <file> --deps` — see dependents
2. For each dependent: `tilth <dep> --section <symbol>` to check actual usage

### Tracing transitive callers (who ultimately triggers this?)
1. Start shallow: `tilth <symbol> --callers --depth 2 --json`
2. Check `stats.suspicious_hops` — if present, qualify the target or filter by `call_text`
3. Read `call_text` on each edge to disambiguate overloaded callees (`errors.New` vs `pool.New`)
4. Check `elided` for truncation signals; raise `--max-edges` / `--max-frontier` only when justified

---

## Supported languages (tree-sitter)

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift.

Unsupported languages still work for file reading — you just won't get structural outlines or definition detection.
