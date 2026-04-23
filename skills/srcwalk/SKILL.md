---
name: srcwalk
description: "Code-intelligence CLI for tree-sitter-backed structural code reading. Use this whenever the user asks where a symbol is defined, who calls it, what a file imports, what a large file contains structurally, or wants a token-aware map of an unfamiliar codebase — even if they don't say 'srcwalk' or 'outline'. Prefer this over cat/grep/find for any code-structure question. For plain text search, reading small files whose path you already know, or listing paths to pipe, use ripgrep / cat / fd directly."
---

# Srcwalk — Code Intelligence CLI

srcwalk is a code-intelligence tool built on tree-sitter. It answers questions grep and cat can't: *where is this symbol defined*, *who calls it*, *what does this file depend on*, *what does this codebase look like structurally*.

**Use srcwalk for:** outlines of large files, symbol definitions, callers (single-hop or transitive BFS), file dependencies, codebase maps, jumping to a symbol body, call-chain tracing, comparing sizes of partial/overloaded definitions with the same name.

**Don't use srcwalk for** plain text search, reading small files whose path you know, listing paths to pipe, or complex regex. Use `rg`, `cat`, `fd` directly — they're faster and you already know how to read their output.

**Binary:** `~/.cargo/bin/srcwalk` (in PATH).

```bash
srcwalk <args>
```

---

## Read a large file (outline + drill-in)

```bash
srcwalk <path>                          # outline if large, full if small
srcwalk <path> --section 45-89          # exact line range
srcwalk <path> --section "## Foo"       # markdown heading
srcwalk <path> --section validateToken  # jump to a symbol's body by name
srcwalk <path> --full                   # force full output with line numbers
srcwalk <path> --budget 2000            # cap response to ~N tokens
```

**Behaviour table:**

| Input | Output |
|---|---|
| 0 bytes | `[empty]` |
| Binary | `[skipped]` with mime type |
| Generated (lockfiles, `.min.js`) | `[generated]` |
| < ~6000 tokens | Full content, line-numbered |
| > ~6000 tokens | Structural outline with line ranges |
| `--full` over `--budget` | Cascades: outline first (label `outline (full requested, over budget)`), then signatures (`signatures (full requested, over budget)`) if outline still over. Not a bug — srcwalk degraded gracefully because the budget was tight. |
| Pipe mode | Same smart view as TTY (use `--full` for raw bytes) |

On a heading miss, top-5 closest matches are suggested. Outlines are capped at a safe line count — when capped, drill in with `--section <symbol>` or a line range.

---

## Search for symbols (definitions + usages)

```bash
srcwalk <symbol> --scope <dir>                    # definitions first, then usages
srcwalk "foo, bar, baz" --scope <dir>             # multi-symbol, one pass
srcwalk <symbol> --scope <dir> --expand           # inline source for top 2
srcwalk <symbol> --scope <dir> --expand=5         # inline source for top 5
```

Tree-sitter finds where symbols are **defined**, not just where strings appear. Each match shows the surrounding file structure so you know context without a second read.

Expanded definitions include a **callee footer** (`── calls ──`) listing resolved callees with file, line range, and signature — follow call chains without separate searches.

Every definition hit reports its **line range** (e.g. `[38-690]` vs `[9-16]`). Use this to:

- Pick the real implementation vs a generated stub in a partial/split class (C#, Kotlin) — the tiny range is usually the stub.
- Tell overloads apart at a glance without opening each file.
- Rank where to drill first when a symbol has many definitions.

---

## When something isn't found

srcwalk tries hard to convert misses into actionable suggestions. Trust the suggestion line before reformulating — it saves a round trip.

- **0-hit symbol search** → srcwalk suggests close matches across naming
  conventions (snake↔camel↔Pascal) and typo distance ≤ 2 (Levenshtein),
  filtered to source files (no markdown, no JSON, no lockfiles). The
  suggestion line `> Did you mean: <symbol> (<file>:<line>)` is reliable.
  Example: query `searchSymbol` → suggests `search_symbol`. Query `readByt`
  → suggests `readByte, readBytes, readInt`.

- **Concept / multi-word miss** → format is
  `no matches for "<query>" in <scope>`, followed by the same `> Did you
  mean:` line when applicable. Treat this as a normal "try this instead",
  not as an error to retry verbatim.

- **No suggestion at all** → the query is genuinely far from anything
  indexed. Reformulate (broader scope, partial name, related concept) or
  fall back to `rg` for a text-level scan.

---

## Bare filename + `--section` auto-pick

`srcwalk config.go --section parseConfig` works even if many `config.go`
files exist in scope. srcwalk picks the **primary** copy automatically:

- Files matched by `.gitignore` / `.ignore` / git excludes (test fixtures,
  vendor copies, generated mirrors) are dropped first.
- Among the rest, the file with the shallowest directory depth wins.
- The output sidebar lists what was skipped — usually safe to ignore, but
  scan it once if you suspect you wanted a vendored copy.

If the result still looks wrong (e.g. monorepo with multiple legitimate
`config.go`), pass an unambiguous path: `srcwalk pkg/foo/config.go --section ...`.

---

## Callers — who calls this symbol

```bash
srcwalk <symbol> --callers --scope <dir>
```

Structural (tree-sitter), not text-based. Includes type/constructor references (`new Foo()`, `Foo {}`), not just function calls.

**When callers returns 0**, output includes a per-language hint about indirect dispatch (trait objects, interfaces, reflection, callbacks, duck typing). A symbol with zero direct callers is often still in use — check the hint before concluding it's dead code.

### Multi-hop callers (BFS)

```bash
srcwalk <symbol> --callers --depth <N> --scope <dir>
srcwalk <symbol> --callers --depth <N> --json
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
srcwalk <file> --deps
```

Imports (what this file depends on) and dependents (what depends on it). Use before modifying a file to understand impact.

---

## Codebase map

```bash
srcwalk --map --scope <dir>
```

Structural skeleton. **Every directory is annotated with cumulative tokens of its descendants** (`src/ (~14.9k tokens)`, `.pi-lens/ (~175.9k tokens)`). See scale before choosing what to read. Auto k/M formatting.

`--map` respects `.gitignore`, `.ignore`, and git excludes — token totals
reflect what you would actually have to read, not the unfiltered tree on
disk. A header note calls out when ignores are active.

---

## Pagination

`--limit N` and `--offset N` work on symbol search, callers, and deps. Ordering is stable across runs (deterministic sort), so retries return identical pages.

```bash
srcwalk <symbol> --scope . --limit 10              # first page
srcwalk <symbol> --scope . --limit 10 --offset 10  # second page
```

Output ends with `Next page: --offset N --limit M.` or `(end of results)`. No silent caps — at ≥100k matches you get a soft warning but the result set is still complete.

---

## Workflow: understanding a new codebase

A common pattern that compounds these features:

1. `srcwalk --map --scope .` — skeleton + directory token scale; skip huge subtrees that the gitignore-aware totals confirm are noise (build outputs, vendored deps).
2. `srcwalk <key-file>` — outline the interesting files.
3. `srcwalk <symbol> --scope .` — find definitions; follow the `── calls ──` footer instead of re-searching.
4. `srcwalk <file> --section <range-or-symbol>` — drill into specific parts.
5. `srcwalk <symbol> --callers --depth 2 --json` when you need transitive call sites.

Other tasks (impact analysis, dead-code check, etc.) compose the same primitives.

---

## Supported languages (tree-sitter)

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift. Unsupported languages still work for file reading — you just won't get structural outlines or definition detection.
