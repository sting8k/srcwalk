---
name: tilth
description: "Smart code navigation using tilth CLI — reading, outlining, searching, and drilling into codebases. Use whenever you'd reach for cat/grep/find/ripgrep on a codebase, or want file structure, definitions, usages, callers, or blast-radius awareness in one call. Output is tuned for LLM-agent token economy: stable pagination, directory token rollups, content previews, progressive reads on oversized files."
---

# Tilth — Smart Code Reading CLI

tilth combines `ripgrep`, `tree-sitter`, and `cat` into one tool that understands code structure. It replaces the grep-read-grep-read-again cycle with structural outlines, definition search, callers, and blast-radius deps in a single invocation.

**Binary:** `~/.cargo/bin/tilth` (in PATH). Invoke via:

```bash
tilth <args>
```

## Core idea: smart view adapts to size

Every read/search output adapts to input size. Small files print whole, large files outline, oversized `--full` pages progressively. Pipe mode uses the same smart view as TTY (not raw bytes) — pass `--full` when you actually need raw content.

---

## Read a file

```bash
tilth <path>                          # smart view (outline if large, full if small)
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
| Pipe mode | Same smart view as TTY (use `--full` for raw bytes) |

**On a heading miss, top-5 closest matches are suggested** — no need to re-read the file to find the right heading.

**Outline is capped at a safe line count.** When capped, output ends with `> _outline capped at N lines — more symbols exist..._`. Drill in with `--section <symbol>` or a line range rather than trying to dump more.

---

## Search for symbols (definitions + usages)

```bash
tilth <symbol> --scope <dir>                    # definitions first, then usages
tilth "foo, bar, baz" --scope <dir>             # multi-symbol search, one pass
tilth <symbol> --scope <dir> --expand           # inline source for top 2 matches
tilth <symbol> --scope <dir> --expand=5         # inline source for top 5
```

Tree-sitter finds where symbols are **defined**, not just where strings appear. Each match shows the surrounding file structure so you know context without a second read.

Expanded definitions include a **callee footer** (`── calls ──`) listing resolved callees with file, line range, and signature — follow call chains without separate searches.

---

## Content search (text / regex)

```bash
tilth "TODO: fix" --scope <dir>
tilth "/def\s+my_func/" --scope <dir>           # regex
```

---

## Glob files

```bash
tilth "*.test.ts" --scope <dir>
```

Each result includes a **token estimate** and a **one-line content preview** (first non-trivial doc/code line). Use the token estimate to decide what's safe to read before you read it.

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

- `--depth N` — 1 (default, legacy behavior) up to 5.
- `--max-frontier K` — callers expanded per hop (default 50). Excess symbols auto-promoted to hubs, listed in `elided.auto_hubs_promoted`.
- `--max-edges M` — global edge cap (default 500). Truncation is deterministic.
- `--skip-hubs CSV` — explicit hub-skip list. Default is language-agnostic (`new,clone,from,into,to_string,drop,fmt,default`). `--skip-hubs ""` to disable.
- `--json` — machine-readable edge list.

**For agents reading `--json`:**

- Each `edges[]` entry has `hop, from, from_file, from_line, to, call_text`. Use `call_text` (the raw call-site line) to disambiguate overloaded callee names like `New` — you will see `errors.New("timeout")` vs `pool.New(cfg)` directly, no extra lookup.
- Check `stats.suspicious_hops[]` before trusting deep hops. An entry there means that hop is likely polluted by cross-package name collision (e.g. `→ New` matching hundreds of unrelated `New` definitions). When flagged, either qualify the target, drop that hop, or filter edges client-side using `call_text`.
- Check `elided` — it tells you if edges were cut (`edges_cut_at_hop`), frontier was capped (`frontier_cuts`), or hubs were auto-promoted (`auto_hubs_promoted`).

---

## Blast radius — file dependencies

```bash
tilth <file> --deps
```

Shows imports (what this file depends on) and dependents (what depends on it). Use before modifying a file to understand impact.

---

## Codebase map

```bash
tilth --map --scope <dir>
```

Structural skeleton of the codebase. **Every directory is annotated with cumulative tokens of its descendants** (`src/ (~14.9k tokens)`, `.pi-lens/ (~175.9k tokens)`). See scale before choosing what to read. Auto k/M formatting.

---

## Pagination — every list result

`--limit N` and `--offset N` work on glob, symbol search, content search, callers, and deps. Ordering is stable across runs (deterministic sort), so retries return identical pages.

```bash
tilth "*.rs" --scope . --limit 10                # first page
tilth "*.rs" --scope . --limit 10 --offset 10    # second page
```

Output ends with either `Next page: --offset N --limit M.` or `(end of results)`. No silent caps — at ≥100k matches you get a soft warning but the result set is still complete. If you see the warning, narrow `--scope` or refine the pattern.

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
2. Check `stats.suspicious_hops` — if present, that hop has cross-package name collision; either qualify the target or filter edges by `call_text` pattern
3. Read `call_text` on each edge to disambiguate overloaded callees (`errors.New` vs `pool.New`)
4. Check `elided` for truncation signals; raise `--max-edges` / `--max-frontier` only when justified

### Reading a large file efficiently
1. `tilth <path>` — outline first
2. `tilth <path> --section <line-range-or-symbol>` — read the part you need
3. Only reach for `--full` when you genuinely need the whole file (it pages progressively above the cap)

### Paging through many results
1. Start without `--limit`: see total count in the footer
2. If too many, narrow `--scope` or the pattern
3. Otherwise paginate with `--limit N --offset M`

---

## Supported languages (tree-sitter)

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift.

Unsupported languages still work for file reading and content search — you just won't get structural outlines or definition detection.

---

## Why this beats grep/cat/read

- **One call instead of many:** outline + definitions + usages in a single invocation
- **Structure-aware:** tree-sitter finds definitions, not text matches
- **Token-efficient:** smart view, token estimates on every match, directory rollups, progressive read
- **Stable pagination:** retries return identical pages, no silent truncation
- **Call chain tracing:** callee footers on expanded definitions let you follow code flow
- **Honest misses:** fuzzy heading suggestions, indirect-call hints, omission indicators — you know when the view is incomplete and how to recover
