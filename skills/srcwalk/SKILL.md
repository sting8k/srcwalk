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

**Follow output hints first:** srcwalk prints contextual `> Tip:` footers for pagination, budget/cap truncation, section drill-in, callers/callees/deps, and graph traversal. Prefer those hints as next-step guidance before scanning this whole skill.

---

## Read a large file (outline + drill-in)

```bash
srcwalk <path>                          # outline if large, full if small
srcwalk <path> --section 45-89          # exact line range
srcwalk <path> --section "## Foo"       # markdown heading
srcwalk <path> --section validateToken  # jump to a symbol's body by name
srcwalk <path> --section "fn_a,fn_b"   # multiple symbols in one call
srcwalk <path> --full                   # force full output with line numbers
srcwalk <path> --path-exact --full      # exact file only; fail instead of search fallback
srcwalk <path> --budget 2000            # cap response to ~N tokens
```

**Smart view:** small text files print full line-numbered content; large files print structural outlines with line ranges. Binary/generated files are skipped or labeled. If `--full` exceeds `--budget`, srcwalk degrades to outline/signatures instead of dumping over-budget content.

---

## Search for symbols (definitions + usages)

```bash
srcwalk <symbol> --scope <dir>                    # definitions first, then usages
srcwalk "foo, bar, baz" --scope <dir>             # multi-symbol, one pass
srcwalk <symbol> --scope <dir> --expand           # source context for top 2 matches
srcwalk <symbol> --scope <dir> --expand=5         # source context for top 5 matches
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

Symbol search **definitions** use tree-sitter (precise). **Usages** are text-matched — fast across large codebases but include comment/doc mentions. The output separates code usages from comment mentions in faceted sections.

---

## Bare filename + `--section` auto-pick

`srcwalk config.go --section parseConfig` auto-picks the primary non-ignored, shallowest `config.go` when duplicates exist. If a monorepo has multiple legitimate matches or you need a vendored/generated copy, pass an explicit path: `srcwalk pkg/foo/config.go --section ...`.

---

## Callers — who calls this symbol

```bash
srcwalk <symbol> --callers --scope <dir>             # compact facts only
srcwalk <symbol> --callers --scope <dir> --expand    # source context for top 2 callers
srcwalk <symbol> --callers --scope <dir> --expand=5  # source context for top 5 callers
```

Structural (tree-sitter), not text-based. Default output is token-light: caller function, file:line, receiver, and argument count when available. Use `--expand[=N]` only when you need source context around the call site.

Includes type/constructor references (`new Foo()`, `Foo {}`), not just function calls.

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

For `--json`, inspect `edges[]`, `stats.suspicious_hops[]`, and `elided` before trusting deep hops. Use `call_text` to disambiguate overloaded/common names when needed.

---

## Blast radius — file dependencies

```bash
srcwalk <file> --deps
```

Imports (what this file depends on) and dependents (what depends on it). Use before modifying a file to understand impact.

Dependents are paginated: default output shows the first 15 dependent files. Use the footer tip or pass `--limit N --offset M` to continue.

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
srcwalk --map --scope <dir>            # compact tree, no symbols
srcwalk --map --scope <dir> --symbols  # include symbol names
```

`--map` respects `.gitignore`, `.ignore`, and git excludes — token totals
reflect what you would actually have to read, not the unfiltered tree on
disk. A header note calls out when ignores are active.

Default `--map` is intentionally compact; use `--symbols` only when you need symbol names.

---

## Pagination

`--limit N` and `--offset N` work on symbol/content search, glob results, callers, and deps dependents. Ordering is stable across runs (deterministic sort), so retries return identical pages.

```bash
srcwalk <symbol> --scope . --limit 10              # first page
srcwalk <symbol> --scope . --limit 10 --offset 10  # second page
```

Paginated outputs end with `> Tip:` footer guidance, e.g. `Continue with --offset X --limit Y` or an end-of-results tip. No silent caps — at ≥100k matches you get a soft warning but the result set is still complete.

---

## Pick the command by question

Start narrow. Run the smallest command that can answer the question, then use `--expand[=N]` or footer tips only if compact output lacks needed context.

| Question | Example |
|---|---|
| What is this repo shaped like? | `srcwalk --map --scope .` |
| What is in this large file? | `srcwalk <file>` |
| Where is this symbol defined? | `srcwalk <symbol> --scope .` |
| Who directly calls this? | `srcwalk <symbol> --callers --scope .` |
| Need source around a hit? | add `--expand` or `--expand=N` |
| What depends on this file? | `srcwalk <file> --deps` |
| Need transitive callers? | `srcwalk <symbol> --callers --depth 2 --scope .` |
| Need exact body/range? | `srcwalk <file> --section <range-or-symbol>` |

---

## Supported languages (tree-sitter)

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift. Unsupported languages still work for file reading — you just won't get structural outlines or definition detection.
