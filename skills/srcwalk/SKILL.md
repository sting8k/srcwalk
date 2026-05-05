---
name: srcwalk
compatible_srcwalk: ">=0.2.6"
description: "Code-intelligence CLI for tree-sitter-backed structural code reading. Use this whenever the user asks where a symbol is defined, who calls it, what a file imports, what a large file contains structurally, or wants a token-aware map of an unfamiliar codebase — even if they don't say 'srcwalk' or 'outline'. Prefer this for code-structure questions and token-aware file reading. For pure text grep or path listing, use ripgrep / fd directly."
---

# srcwalk — agent routing policy

Use srcwalk for structural code questions: repo maps, large-file outlines, symbol definitions/usages, callers, callees, file dependencies, and precise drill-in reads.

For plain text grep or path listing, prefer `rg`/`fd`. For known files, use srcwalk when structural outline, token-aware reading, sections, or line-focused drill-in will help; otherwise a normal file read is fine.

## Mental model

- **Target-first for reading:** `srcwalk <path>`, `srcwalk <path>:<line>`, `srcwalk <path> --section <symbol|range>`.
- **Action-first for analysis:** `srcwalk find|callers|callees|deps|map ...`.
- Start compact. Drill in when you need source evidence, exact context, or a narrower answer.
- Follow footers by prefix: `> Next:` suggests a useful next command, `> Note:` adds context/status, and `> Caveat:` marks limitations to verify before making claims.

## Choose the command by intent

| User intent | Use first |
|---|---|
| Understand repo shape / entry points | `srcwalk map --scope .` |
| Read or inspect a large known file | `srcwalk <path>` |
| Jump around a hit line | `srcwalk <path>:<line>` |
| Read exact body/range | `srcwalk <path> --section <symbol|start-end>` |
| Find definition/usages/text/glob | `srcwalk find <query> --scope <dir>` |
| Find several symbols in one pass | `srcwalk find "A, B, C" --scope <dir>` |
| Who directly calls this? | `srcwalk callers <symbol> --scope <dir>` |
| Who reaches this transitively? | `srcwalk callers <symbol> --depth 2 --scope <dir>` |
| What does this function call? | `srcwalk callees <symbol> --scope <dir>` |
| Ordered calls/data flow inside function | `srcwalk callees <symbol> --detailed --scope <dir>` |
| Transitive downstream calls | `srcwalk callees <symbol> --depth 2 --scope <dir>` |
| File imports and dependents | `srcwalk deps <file>` |
| Quick caller+callee orientation slice | `srcwalk flow <symbol> --scope <dir>` |
| Heuristic blast-radius triage | `srcwalk impact <symbol> --scope <dir>` |

Legacy flag syntax still works (`srcwalk Foo --callers`, `srcwalk --map`), but prefer action-first commands for analysis.

## Default workflows

### Explore unfamiliar code

```bash
srcwalk map --scope .
srcwalk map --scope src --depth 2
srcwalk find <likely_symbol> --scope src
srcwalk <path>:<line>
```

`map` respects `.gitignore`, `.ignore`, git excludes, and parent ignores. Direct explicit file reads can still inspect ignored files.

### Read a large file

```bash
srcwalk <path>
srcwalk <path>:123
srcwalk <path> --section 45-89
srcwalk <path> --section SomeFunction
```

Prefer outline/section reads before `--full` for large files. Use `--full` when raw text is useful; output is capped.

### Find and drill into symbols

```bash
srcwalk find <symbol> --scope <dir>
srcwalk find <symbol> --expand --scope <dir>
srcwalk find <symbol> --filter 'path:api kind:fn' --scope <dir>
```

Definition hits are tree-sitter based. Usage hits are text-matched and may include comments/docs. For actual call sites, switch to `srcwalk callers <symbol>`.

### Trace upstream callers

```bash
srcwalk callers <symbol> --scope <dir>
srcwalk callers <symbol> --filter 'args:3 receiver:mgr' --scope <dir>
srcwalk callers <symbol> --count-by receiver --scope <dir>
srcwalk callers <symbol> --depth 2 --scope <dir>
```

Use direct callers for concrete call-site evidence. Use `--depth` for transitive reachability, capped at 5 hops. For JSON BFS, inspect `edges[]`, `stats.suspicious_hops[]`, and `elided` when making claims from deep hops.

### Trace downstream callees

```bash
srcwalk callees <symbol> --scope <dir>
srcwalk callees <symbol> --detailed --scope <dir>
srcwalk callees <symbol> --detailed --filter 'callee:NAME' --scope <dir>
srcwalk callees <symbol> --depth 2 --scope <dir>
```

Use `callees` when the question starts from a known function and asks what it calls. Use `--detailed` for ordered call sites with assignment/return context and argument slots (`arg1=...`, `arg2=...`).

### Check file blast radius

```bash
srcwalk deps <file>
srcwalk deps <file> --limit 30 --offset 30
```

Use before editing a file to see imports and dependents.

## Shortcuts and caveats

- `srcwalk flow <symbol>`: compact orientation slice combining ordered calls with argument slots, selected local resolves, and direct callers. Good for quick understanding; not a full graph.

  Example shape:

  ```text
  $ srcwalk flow read_file_with_budget --scope src
  # Slice: read_file_with_budget — flow

  [symbol] read_file_with_budget read/mod.rs:208-257
  -> calls (ordered)
    [call] L217 ->ret read_file(arg1=path, arg2=section, arg3=full, arg4=cache)
    [call] L232 outline_out = render_outline_view(arg1=path, arg2=cache, arg3=ViewMode::OutlineCascade)

  -> resolves (selected local helpers)
    [fn] read_file read/mod.rs:69-188
    [fn] render_outline_view read/mod.rs:259-281

  <- callers
    [fn] run_path_exact lib.rs:242
    [fn] run_inner lib.rs:1145

  > Caveat: flow is capped for readability. Use `srcwalk callees <symbol> --detailed` for all ordered calls, or `srcwalk callers <symbol>` for upstream sites.
  ```

  Nested/fluent chains and callback bodies may be collapsed to avoid noise; drill into the exact section when inner calls matter.

- `srcwalk impact <symbol>`: heuristic name-matched blast-radius triage. Good for broad “what might be affected?” checks; not proof. Common names like `run`, `init`, `close` need follow-up with receiver/file groups or callers.

  Example shape:

  ```text
  $ srcwalk impact read_file_with_budget --scope src
  # Slice: read_file_with_budget — impact

  [symbol] read_file_with_budget
  = definitions
    [def] read/mod.rs:208-257 pub fn read_file_with_budget(

  <- name-matched calls from
    [fn] run_path_exact lib.rs:242 recv=read args=5
    [fn] run_inner lib.rs:1145 recv=read args=5

  ~ groups
    [group] receiver=<bare> count=4
    [group] receiver=read count=3
    [group] file=lib.rs count=3

  > Caveat: direct name-matched call sites only. Use `srcwalk callers <symbol> --depth 2` for transitive upstream impact.
  ```

- `srcwalk find <query>` can handle symbol names, text, regex-like queries, and globs through smart classification.
- Bare filename + `--section` may auto-pick the primary non-ignored shallow match. If duplicates matter, pass an explicit path.

## Escalation rules

1. If you need orientation, start with `map` or `find`.
2. If output gives a path/line, drill with `srcwalk <path>:<line>`.
3. If the question is “who calls/reaches this?”, use `callers`.
4. If the question is “what happens inside/after this function?”, use `callees --detailed`.
5. Use `flow`/`impact` as quick orientation/triage, then verify important claims with `callers`, `callees`, or exact file reads.
6. Prefer narrowing `--scope` over broad repo-wide repeated searches.

## Supported structural languages

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift. Unsupported languages still work for reading files, but structural facts may be unavailable.
