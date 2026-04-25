---
name: srcwalk
compatible_srcwalk: ">=0.1.9"
description: "Code-intelligence CLI for tree-sitter-backed structural code reading. Use this whenever the user asks where a symbol is defined, who calls it, what a file imports, what a large file contains structurally, or wants a token-aware map of an unfamiliar codebase — even if they don't say 'srcwalk' or 'outline'. Prefer this over cat/grep/find for any code-structure question. For plain text search, reading small files whose path you already know, or listing paths to pipe, use ripgrep / cat / fd directly."
---

# Srcwalk — Code Intelligence CLI

srcwalk is a code-intelligence tool built on tree-sitter. It answers questions grep and cat can't: *where is this symbol defined*, *who calls it*, *what does this file depend on*, *what does this codebase look like structurally*.

**Use srcwalk for:** outlines of large files, symbol definitions, callers (single-hop or transitive BFS), callees/forward call flow from a known function, file dependencies, codebase maps, jumping to a symbol body, call-chain tracing, comparing sizes of partial/overloaded definitions with the same name.

**Don't use srcwalk for** plain text search, reading small files whose path you know, listing paths to pipe, or complex regex. Use `rg`, `cat`, `fd` directly — they're faster and you already know how to read their output.

**Binary:** `~/.cargo/bin/srcwalk` (in PATH).

```bash
srcwalk <args>
```

**Follow output hints first:** srcwalk prints contextual `> Tip:` footers for pagination, budget/cap truncation, section drill-in, callers/callees/deps, and graph traversal. Prefer those hints as next-step guidance before scanning this whole skill.

---

## Read a large file (outline + drill-in)

```bash
srcwalk <path>                         # outline if large, full if small
srcwalk <path>:123                     # focus exact hit line with context
srcwalk <path> --section 45-89         # exact line range
srcwalk <path> --section validateToken # jump to a symbol body by name
srcwalk <path> --full                  # force full output with line numbers
```

Large files return structural outlines; drill into rows with `srcwalk <path>:<line>` or `--section <range-or-symbol>`.

---

## Search for symbols (definitions + usages)

```bash
srcwalk <symbol> --scope <dir>         # definitions first, then usages
srcwalk "foo, bar, baz" --scope <dir>  # multi-symbol, one pass
```

Tree-sitter finds where symbols are **defined**, not just where strings appear. Each match shows the surrounding file structure so you know context without a second read.

Expanded definitions include a **callee footer** (`── calls ──`) listing resolved callees with file, line range, and signature — follow call chains without separate searches.

Every definition hit reports its **line range** (e.g. `[38-690]` vs `[9-16]`). Use this to:

- Pick the real implementation vs a generated stub in a partial/split class (C#, Kotlin) — the tiny range is usually the stub.
- Tell overloads apart at a glance without opening each file.
- Rank where to drill first when a symbol has many definitions.

Symbol search **definitions** use tree-sitter (precise). **Usages** are text-matched — fast across large codebases but can include comment/doc mentions. For real call sites, use `--callers`.

---

## Bare filename + `--section` auto-pick

`srcwalk config.go --section parseConfig` auto-picks the primary non-ignored, shallowest `config.go` when duplicates exist. If a monorepo has multiple legitimate matches or you need a vendored/generated copy, pass an explicit path: `srcwalk pkg/foo/config.go --section ...`.

---

## Callers — who calls this symbol

```bash
srcwalk <symbol> --callers --scope <dir>  # compact facts only
```

Structural (tree-sitter), not text-based. Default output is token-light: caller function, file:line, receiver, and argument count when available.

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

Use this when the question starts from a **known function/method body** and asks what it calls next: forward flow, ordered helper calls, setup pipelines, internal vs external calls, or transitive downstream impact. Do **not** use it for global text counts, file counts, or “who calls X?” — those are `rg`/`fd`/`--callers` jobs.

```bash
srcwalk <symbol> --callees --scope <dir>              # summary: resolved with sig + unresolved
srcwalk <symbol> --callees --detailed --scope <dir>   # ordered call sites with assignments & returns
srcwalk <symbol> --callees --depth N --scope <dir>    # transitive forward graph (up to 5 hops)
```

What does this function call? Default output groups resolved callees (file, line range, signature) and unresolved (stdlib/external) separately, then prints a `> Tip: use --detailed ...` footer.

`--detailed` shows **ordered call sites** as they appear in the function body — each line includes the call with assignment context (`result = foo(...)`) and return markers (`->ret`). Use this to understand control flow and data flow through the function.

Known function + “what/where/order of calls inside it” ⇒ use `srcwalk <symbol> --callees --detailed`.

If the symbol name is overloaded/common, first find the exact definition with `srcwalk <symbol> --scope <dir>`, then drill into the chosen file/range or narrow `--scope` before running `--callees`.

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

## Pick the command by question

Start narrow. Run the smallest command that can answer the question, then use `--expand[=N]` or footer tips only if compact output lacks needed context.

| Question | Example |
|---|---|
| What is this repo shaped like? | `srcwalk --map --scope .` |
| What is in this large file? | `srcwalk <file>` |
| Where is this symbol defined? | `srcwalk <symbol> --scope .` |
| Who directly calls this? | `srcwalk <symbol> --callers --scope .` |
| What does this function call? | `srcwalk <symbol> --callees --scope .` |
| Need ordered calls/data flow inside a function? | `srcwalk <symbol> --callees --detailed --scope .` |
| Need source around a hit? | add `--expand` or `srcwalk <path>:<line>` |
| What depends on this file? | `srcwalk <file> --deps` |
| Need transitive callers? | `srcwalk <symbol> --callers --depth 2 --scope .` |
| Need transitive downstream calls? | `srcwalk <symbol> --callees --depth 2 --scope .` |
| Need exact body/range? | `srcwalk <file> --section <range-or-symbol>` |

---

## Supported languages (tree-sitter)

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift. Unsupported languages still work for file reading — you just won't get structural outlines or definition detection.
