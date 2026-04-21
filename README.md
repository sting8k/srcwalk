# another-tilth

A personal fork of [tilth](https://github.com/jahala/tilth) — smart code reading for LLM agents — tuned for heavier real-world workflows. Same core idea (small files come back whole, large files get an outline), with extra polish around pagination, output economy, and search ergonomics.

Upstream cuts cost-per-correct-answer by ~40% on benchmark runs. This fork keeps that and adds the bits I personally hit friction with every day.

## Features

Inherits everything from upstream tilth (tree-sitter outlines, structural search, callers/diff, MCP server, edit mode), plus:

- **Stable pagination** on every list result — `--limit` / `--offset` for glob, symbol, callers, callees, deps. No silent caps; soft warning at 100k matches.
- **Directory token rollups** in `--map` so you see scale before you read.
- **Content previews** in glob results — filename, token estimate, and a one-line summary.
- **Progressive read** for oversized `--full` — header + first 200 numbered lines + outline + continuation hint.
- **Line numbers** in full mode by default.
- **Smart view in pipe mode** — `tilth file.rs | …` returns the outline, not raw bytes (use `--full` for raw).
- **`--section <symbol>`** to jump straight to a symbol body, alongside line ranges and headings.
- **Fuzzy heading suggestions** when `--section "## Foo"` misses.
- **Indirect-call hint** when `--callers` returns 0 — explains trait objects, interfaces, reflection, callbacks per language family.
- **Type/constructor refs** counted as callers (`new Foo()`, `Foo {}`).
- **Outline omission indicator** when the view is capped.
- **Faster engine** — mmap walkers, Aho-Corasick multi-symbol search, parse cache, mimalloc, minified-file skip.
- **MCP output cap** at 25k chars with explicit truncation note.

See [PR #64](https://github.com/jahala/tilth/pull/64) for the full rationale and before/after examples.

## Installation

```sh
cargo install --git https://github.com/sting8k/tilth --branch another-tilth
```

For upstream tilth instead:

```sh
cargo install tilth
```

## Usage

```bash
$ tilth src/auth.ts
# src/auth.ts (258 lines, ~3.4k tokens) [outline]

[1-12]   imports: express(2), jsonwebtoken, @/config
[14-22]  interface AuthConfig
[24-42]  fn validateToken(token: string): Claims | null
[44-89]  export fn handleAuth(req, res, next)
[91-258] export class AuthManager
  [99-130]  fn authenticate(credentials)
  [132-180] fn authorize(user, resource)
```

Small files print whole. Large files outline. Drill in by line range, heading, or symbol name:

```bash
tilth src/auth.ts --section 44-89
tilth docs/guide.md --section "## Installation"
tilth src/auth.ts --section AuthManager
```

## Smart read

Output adapts to size and channel:

| Input | Behaviour |
|-------|-----------|
| 0 bytes | `[empty]` |
| Binary | `[skipped]` with mime type |
| Generated (lockfiles, `.min.js`) | `[generated]` |
| < ~6000 tokens | Full content, line-numbered |
| > ~6000 tokens | Structural outline |
| `--full` over cap | Progressive: header + first 200 lines + outline + continuation hint |
| Pipe mode | Same smart view as TTY (use `--full` for raw bytes) |

On a heading miss, the closest matches are suggested:

```
invalid query "## Get Started": heading not found. Closest matches:
  ## 🚀 Quick Start
  ## Contributors
  ## Contributing
```

## Search

Tree-sitter finds where symbols are **defined**, not just where strings appear:

```
$ tilth handleAuth --scope src/
# Search: "handleAuth" in src/ — 6 matches (2 definitions, 4 usages)

## src/auth.ts:44-89 [definition]
→ [44-89]  export fn handleAuth(req, res, next)

  44 │ export function handleAuth(req, res, next) {
  45 │   const token = req.headers.authorization?.split(' ')[1];
  ...

── calls ──
  validateToken    src/auth.ts:24-42
  refreshSession   src/auth.ts:91-120

## src/routes/api.ts:34 [usage]
→ [34]   router.use('/api/protected/*', handleAuth);
```

Multi-symbol search in one call (Aho-Corasick under the hood):

```bash
tilth "ServeHTTP, HandlersChain, Next" --scope .
```

Callers query (structural, not text):

```bash
tilth isTrustedProxy --kind callers --scope .
```

When callers returns 0, you get a per-language hint about indirect dispatch (trait objects, interfaces, reflection, callbacks) instead of "not found".

## Map

```bash
$ tilth --map --scope .
.pi-lens/  (~175.9k tokens)        ← skip, too large to read
.github/   (~1.0k tokens)          ← safe to read in full
src/       (~14.9k tokens)
  read/    (~10.2k tokens)
    outline/  (~3.7k tokens)
```

Directory rollups show cumulative tokens of descendants. Auto k/M formatting.

## Glob

```bash
$ tilth "*.rs" --scope src/
src/budget.rs  (~774 tokens · Apply token budget to output paths)
src/cache.rs   (~580 tokens · Tree-sitter parse cache with LRU eviction)
src/lib.rs     (~210 tokens · pub mod budget; pub mod cache;)

3 of 41 files (offset 0). Next page: --offset 3 --limit 3.
```

Every match includes a token estimate and a one-line preview. Pagination is stable across runs (deterministic sort).

## Structural diff

```bash
$ tilth diff HEAD~1
# Diff: HEAD~1 — 3 files, 2 modified, 1 added (~350 tokens)

## src/auth.rs (3 symbols)
  [~:sig]  fn handleAuth(req) → (req, ctx)    L42
  [~]      fn validate_session                 L88
  [+]      fn refresh_token                    L120
```

Function-level change detection. Use `--scope` to narrow, `--log` for history, conflict detection on by default.

## MCP server

```bash
tilth --mcp
```

Exposes `tilth_read`, `tilth_search`, and (with `--edit`) `tilth_edit`. Session dedup: previously expanded definitions show `[shown earlier]` on subsequent searches. Output is hard-capped at 25k chars with an explicit truncation warning so context never silently blows up.

## Edit mode

Install with `--edit` to enable hashline output and `tilth_edit`:

```
42:a3f|  let x = compute();
43:f1b|  return x;
```

Edits use these hashes as anchors. If the file changed since the last read, hashes won't match and the edit is rejected with current content shown.

```json
{
  "path": "src/auth.ts",
  "edits": [
    { "start": "42:a3f", "content": "  let x = recompute();" },
    { "start": "44:b2c", "end": "46:e1d", "content": "" }
  ]
}
```

Inspired by [The Harness Problem](https://blog.can.ac/2026/02/12/the-harness-problem/).

## Speed

CLI times on x86_64 Mac, 26–1060 file codebases. ~17ms process startup amortised in MCP mode.

| Operation | ~30 files | ~1000 files |
|-----------|-----------|-------------|
| File read + type detect | ~18ms | ~18ms |
| Code outline (400 lines) | ~18ms | ~18ms |
| Symbol search | ~27ms | — |
| Content search | ~26ms | — |
| Glob | ~24ms | — |
| Map | ~21ms | ~240ms |

Search uses early termination via bloom-filter pruning + length-sorted memchr — time is roughly constant regardless of codebase size.

## Related

- [jahala/tilth](https://github.com/jahala/tilth) — upstream
- [ripgrep](https://github.com/BurntSushi/ripgrep) — content search internals (`grep-regex`, `grep-searcher`)
- [tree-sitter](https://tree-sitter.github.io/) — AST parsing for 14 languages
- [The Harness Problem](https://blog.can.ac/2026/02/12/the-harness-problem/) — inspired edit mode

## Name

**tilth** — the state of soil that's been prepared for planting. Your codebase is the soil; tilth gives it structure so you can find where to dig. **another-tilth** is just another take on it.

## License

MIT
