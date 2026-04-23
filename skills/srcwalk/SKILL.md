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

**Follow output hints first:** srcwalk now prints contextual `> Tip:` footers (for `--section`, `--expand`, `--callers`, `--depth`, `--deps`, `--detailed`) in relevant outputs. Prefer those hints as next-step guidance before scanning this whole skill.

---

## Read a large file (outline + drill-in)

```bash
srcwalk <path>                          # outline if large, full if small
srcwalk <path> --section 45-89          # exact line range
srcwalk <path> --section "## Foo"       # markdown heading
srcwalk <path> --section validateToken  # jump to a symbol's body by name
srcwalk <path> --section "fn_a,fn_b"   # multiple symbols in one call
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

On large files, outlines are capped at a safe line count — follow the footer hint to drill in with `--section <symbol>` or a line range.

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

**Symbol search usages vs callers — pick the right tool:**

| Need | Command | Precision |
|------|---------|-----------|
| Where is X defined? | `srcwalk X --scope .` | AST-based — precise |
| Who calls X? | `srcwalk X --callers --scope .` | AST-based — only real call sites |
| All mentions of X | `srcwalk X --scope .` | Text-based — includes comments/docs |

Symbol search **definitions** use tree-sitter (precise). **Usages** are text-matched — fast across large codebases but include comment/doc mentions. The output separates code usages from comment mentions in faceted sections and prints a `--callers` tip when relevant.

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

## Callees — forward call graph

```bash
srcwalk <symbol> --callees --scope <dir>              # summary: resolved with sig + unresolved
srcwalk <symbol> --callees --detailed --scope <dir>   # ordered call sites with assignments & returns
srcwalk <symbol> --callees --depth N --scope <dir>    # transitive (up to 5 hops, cycle-safe)
```

What does this function call? Default output groups resolved callees (file, line range, signature) and unresolved (stdlib/external) separately, then prints a `> Tip: use --detailed ...` footer.

`--detailed` shows **ordered call sites** as they appear in the function body — each line includes the call with assignment context (`result = foo(...)`) and return markers (`->ret`). Use this to understand control flow and data flow through the function.

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
