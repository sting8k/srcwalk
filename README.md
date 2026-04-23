# another-srcwalk

A personal fork of [srcwalk](https://github.com/jahala/srcwalk) — code-intelligence CLI for AI agents — tuned for heavier real-world workflows.

Same core idea as upstream (small files come back whole, large files get a structural outline), with extra polish around: **multi-hop caller graphs (BFS)**, output economy under token budgets, search ergonomics when the agent guesses the symbol name wrong, and a CLI-only surface that drops protocol cruft (no MCP, no edit mode, no diff subcommand).

> Your agent will **love** reaching for this — cheap outlines first, drill on demand, never blow the token budget. Drop the skill in once and watch it quietly prefer `srcwalk` over `cat` and `grep`. 😼

## Why this fork

Driven by real agent sessions across 8 cross-language codebases (Rust, TypeScript, Python, Go, PHP, Java, C#) plus a side-by-side audit experiment.

**Same agent, same prompt, same conclusion — a breadth-first code review on a large codebase, with vs without srcwalk:**

| | srcwalk-on | bash-only |
|---|---|---|
| Tool-result bytes consumed | **198 KB** | 230 KB (−14%) |
| Avg tool-result size | **3.2 KB** | 4.3 KB (−26%) |
| Files surveyed in adjacent surface the agent found on its own | **8** | 0 |
| Findings reported | 1 | 1 |

Same answer, less context burned, and the srcwalk-on run swept a whole adjacent surface that bash-only missed entirely. Headline numbers are modest; the real win is **fewer false-negatives on breadth-first work** because outlines are cheap enough that the agent looks around.

What that pressure-tested into the fork:

- A real **multi-hop caller graph** (BFS), not just one-hop callers — agents stop looping `srcwalk caller-of-caller-of-…` manually.
- Cascade behaviour for `--full` over `--budget` so big files degrade gracefully (`outline → signatures`) with explicit labels — no silent truncation.
- Search that **routes around dead ends**: cross-naming-convention + typo-tolerant Did-you-mean, bare-filename `--section` auto-pick (gitignore + depth-ranked), filename suggestions on concept-search misses.
- Map / glob that respect `.gitignore` so token totals reflect what you'd actually have to read, not what's on disk.
- A leaner CLI surface — MCP server, edit mode, hashline-edit, `srcwalk diff`, `srcwalk install`, `--files`, content-search dispatch all removed (overlap with `rg`/`fd`/native edit tools).
- Performance pass: mmap walkers, Aho-Corasick + length-sorted memchr, tree-sitter parse cache, rayon-parallel multi-symbol search, mimalloc, minified-file skip.
- Elixir language support.

Most of this lives in [PR #64](https://github.com/jahala/srcwalk/pull/64) upstream (rationale + before/after).

## Headline feature: multi-hop caller BFS

Trace callers transitively (up to 5 hops) in one call. Hub guard, deterministic edge cap, per-edge call-site source, and a cross-package collision warning — designed so the agent can trust deep hops or know exactly when not to.

```
$ srcwalk NewClient --callers --depth 3 --json
{
  "edges": [
    { "hop": 1, "from": "newDefaultClient", "from_file": "client/factory.go", "from_line": 42,
      "to": "NewClient", "call_text": "return NewClient(opts)" },
    { "hop": 2, "from": "Bootstrap",         "from_file": "cmd/server/main.go",  "from_line": 88,
      "to": "newDefaultClient", "call_text": "c, err := newDefaultClient(cfg)" },
    { "hop": 3, "from": "main",              "from_file": "cmd/server/main.go",  "from_line": 17,
      "to": "Bootstrap",        "call_text": "if err := Bootstrap(ctx); err != nil {" }
  ],
  "stats": { "edges_per_hop": [4, 7, 3], "suspicious_hops": [] },
  "elided": { "auto_hubs_promoted": ["Error"], "edges_truncated": 0 },
  "depth_reached": 3,
  "elapsed_ms": 41
}
```

`call_text` is the actual call-site line, so overloaded names disambiguate themselves. `suspicious_hops` flags cross-package name collisions. `auto_hubs_promoted` tells you which symbols got fan-out-capped so the graph stays readable.

Skill prompt teaches agents how to read this; flags / syntax live in [`skills/srcwalk/SKILL.md`](./skills/srcwalk/SKILL.md).

## Install

**Binary** — pick one:

```sh
# Cargo (latest from this branch)
cargo install --git https://github.com/sting8k/srcwalk --branch another-srcwalk --locked srcwalk

# Upstream srcwalk (no fork extras)
cargo install srcwalk
```

<details>
<summary>Pre-built binaries (macOS / Linux / Windows)</summary>

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

Windows: download `srcwalk-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/sting8k/srcwalk/releases/latest) and unzip.

Verify build provenance: `gh attestation verify srcwalk-<target>.tar.gz --owner sting8k`. Pin to a tagged release with `--tag v0.8.1` instead of `--branch …`.

</details>

**Agent skill** — teaches your coding agent the command vocabulary, when to fall back to `rg`/`cat`/`fd`, how to read Did-you-mean / cascade labels / BFS edge JSON. Ships at [`skills/srcwalk/SKILL.md`](./skills/srcwalk/SKILL.md).

```sh
npx skills add sting8k/srcwalk
```

Works with Claude Code, Cursor, codex, droid — any agent that follows the `<skill-name>/SKILL.md` convention.

<details>
<summary>Manual skill install</summary>

```sh
mkdir -p ~/.<your-agent>/skills/srcwalk && \
curl -L https://raw.githubusercontent.com/sting8k/srcwalk/another-srcwalk/skills/srcwalk/SKILL.md \
  -o ~/.<your-agent>/skills/srcwalk/SKILL.md
```

Common paths: `~/.claude/skills/`, `~/.pi/agent/skills/`, `~/.cursor/skills/`. For agents that use a single rules file (Cursor rules, Windsurf), paste the body of `SKILL.md` (without the YAML frontmatter) into your rules / custom-instructions file.

</details>

## Example outputs

A few snapshots — see the skill for the full command vocabulary.

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
<summary><b><code>--full</code> over <code>--budget</code> cascade — explicit label, no silent truncation</b></summary>

```
# src/Uno.UI.Runtime.Skia.Win32/Accessibility/Win32Accessibility.cs
  (687 lines, ~5.1k tokens) [outline (full requested, over budget)]
...
```
</details>

<details>
<summary><b>Symbol search — definitions first, with resolved callees</b></summary>

```
$ srcwalk handleAuth --scope src/
# Search: "handleAuth" in src/ — 6 matches (2 definitions, 4 usages)

## src/auth.ts:44-89 [definition]
→ [44-89]  export fn handleAuth(req, res, next)

  44 │ export function handleAuth(req, res, next) {
  45 │   const token = req.headers.authorization?.split(' ')[1];
  ...

── calls ──
  validateToken    src/auth.ts:24-42
  refreshSession   src/auth.ts:91-120
```
</details>

<details>
<summary><b>0-hit search → cross-convention + typo Did-you-mean (lev ≤ 2)</b></summary>

```
$ srcwalk searchSymbol --scope src/
no matches for "searchSymbol" in src/
> Did you mean: search_symbol (src/lib.rs:186)

$ srcwalk readByt --scope src/
no matches for "readByt" in src/
> Did you mean: readByte, readBytes, readInt
```
</details>

<details>
<summary><b>Bare filename + <code>--section</code> — auto-picks the primary copy</b></summary>

```
$ srcwalk lib.rs --section symbol_search
Resolved 'lib.rs' → src/lib.rs
  (skipped 9 non-primary copies [benchmark/fixtures/repos/ripgrep/crates/cli/src/lib.rs,
   benchmark/fixtures/repos/ripgrep/crates/globset/src/lib.rs, +7 more]).
   Pass full path to override.
...
```

Gitignore-aware, depth-ranked. Pass the full path to override.
</details>

<details>
<summary><b>Token-aware map — respects <code>.gitignore</code></b></summary>

```
$ srcwalk --map --scope .
# .gitignore + git excludes applied
.pi-lens/  (~175.9k tokens)        ← skip, too large to read
.github/   (~1.0k tokens)          ← safe to read in full
src/       (~14.9k tokens)
  read/    (~10.2k tokens)
    outline/  (~3.7k tokens)
```

Token totals reflect what you'd actually read.
</details>

<details>
<summary><b>Glob — token estimate + one-line preview</b></summary>

```
$ srcwalk "*.rs" --scope src/
src/budget.rs  (~774 tokens · Apply token budget to output paths)
src/cache.rs   (~580 tokens · Tree-sitter parse cache with LRU eviction)
src/lib.rs     (~210 tokens · pub mod budget; pub mod cache;)

3 of 41 files (offset 0). Next page: --offset 3 --limit 3.
```

Stable pagination across runs (deterministic sort).
</details>

## Speed

CLI times on x86_64 Mac, 26–1060 file codebases.

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

- [jahala/srcwalk](https://github.com/jahala/srcwalk) — upstream
- [ripgrep](https://github.com/BurntSushi/ripgrep) — content search internals (`grep-regex`, `grep-searcher`)
- [tree-sitter](https://tree-sitter.github.io/) — AST parsing for 14 languages
- [The Harness Problem](https://blog.can.ac/2026/02/12/the-harness-problem/) — inspired earlier edit-mode work (since removed in this fork)

## Name

**srcwalk** — the state of soil that's been prepared for planting. Your codebase is the soil; srcwalk gives it structure so you can find where to dig. **another-srcwalk** is just another take on it.

## License

MIT
