# srcwalk — agent evidence contract

Default to srcwalk first for code-structure work. It is the contract for finding exact code evidence, next reads, and bounded claims before shell search.

Keep `--scope` narrow. Use raw `rg`, `read`, `fd`, or shell filesystem tools only for final text/regex confirmation, filesystem metadata, generated-output cleanup, or when srcwalk lacks structural support. If you bypass srcwalk for a code claim, say why.

## Contract

1. Start from intent, not files: orient with `overview`, find candidates with `discover`, then pick one exact target.
2. Follow srcwalk evidence: run `context`, `show`, `trace`, `deps`, `assess`, `review`, or `compare` from paths, ranges, symbols, and `> Next:` commands.
3. Cite bounded evidence: base conclusions on srcwalk path:line/range output and preserve its `source`, `kind`, `confidence`, and `caveat` limits.
4. Do not overclaim: text/file hits are literal evidence; structural hits are navigation evidence; neither proves runtime behavior, security, correctness, aliases, types, or dynamic dispatch unless explicitly supported.
5. Verify after edits: use `srcwalk review --staged` or the relevant srcwalk route before tests; use `rg` only for final raw text or regex confirmation.

Do not infer definitions, usages, callers, dependencies, or code paths from shell path lists or broad grep alone.

## Before grep/rg

Stop if you are about to do this for code navigation:

- `rg "functionName"` -> use `srcwalk discover 'functionName' --scope <dir>`.
- `rg "functionName\("` -> use `srcwalk trace callers functionName --scope <dir>`.
- `rg "^import|^use"` -> use `srcwalk deps <file>`.
- `srcwalk show <file>` without discovery -> use `srcwalk discover <query> --scope <dir>` first, unless you already know the exact file evidence you need.

Why: grep gives raw text matches that often require extra filtering. srcwalk gives scoped candidates, typed evidence, and exact next commands. Use `rg` after srcwalk when you need raw regex confirmation.

## Default workflow

Use the smallest subset of this flow that proves the task. For broad, unfamiliar, or risky code work, start here:

```text
request / bug / feature question
  -> srcwalk overview --scope <dir>
  -> srcwalk discover <query> --scope <dir>
  -> pick one plausible target from discovery output
  -> srcwalk context <symbol-or-file:line> --scope <dir>
  -> srcwalk show <path>:<line-or-range>
  -> srcwalk trace callers <symbol> --scope <dir>
  -> srcwalk trace callees <symbol> --detailed --scope <dir>
  -> srcwalk deps <file>
  -> srcwalk assess <symbol> --scope <dir>
  -> edit
  -> srcwalk review --staged
  -> run relevant tests
  -> rg for final raw text or regex confirmation only
```

## Interpret evidence labels

When output includes `source`, `kind`, `confidence`, or `caveat`, treat them as trust bounds.

- structural syntax/source: navigation evidence, not runtime proof.
- text/comment/file: literal evidence, not semantic relation proof.
- document: navigation structure, not rendered or runtime behavior.
- artifact: artifact-level or byte-span evidence unless labeled source-level.

## Routes

### Orient and choose a target

Do not start broad code navigation with shell `tree`, shell `find`, repeated `ls`, or repo-wide `rg`.

```bash
srcwalk overview --scope <dir>
srcwalk overview --scope <dir> --symbols
srcwalk discover <query> --scope <dir>
srcwalk discover '<glob>' --as file --scope <dir>
srcwalk discover 'foo,bar,baz' --match any --as text --scope <dir>
srcwalk discover <field> --as access --scope <dir>
srcwalk context <symbol> --scope <dir>
```

Use auto overview depth first; explicit `--depth N` is strict. `[relations]` are static local dependency groups, not runtime calls. `[outbound deps]` imports targets outside `--scope`.
`overview --symbols` may show inline `kind name@line-range` anchors when budget allows; if output is too large it falls back to fewer anchors or compact symbol names.

`discover` only searches inside `--scope`; narrow scopes can hide definitions. After a first pass, use `--expand=3`, `--filter kind:fn`, or `--exclude 'tests/**'` only when the output is too broad.

Intent inference: path-like globs infer file discovery; punctuation/path comma lists infer literal Text OR; symbol globs stay symbol search. Add `--as symbol|file|text` when ambiguous.

For multiple literal text terms, use comma OR: `srcwalk discover 'foo,bar,baz' --match any --as text --scope <dir>`. Do not run separate grep commands first.

Text discovery is literal evidence. `--match any --as text` is comma literal OR; `--match all` is same-file co-occurrence, not semantic relation proof.

If discover prints `## Confirmed next context targets`, those are structural candidates from the match context; run one that matches your intent. If it only prints raw hit drilldowns, use `srcwalk show <path>:<line> -C 10` first. `discover <field> --as access` is syntax only: no runtime order, type proof, alias proof, or call relation proof.

### Understand and read exact evidence

Use `context` for one known target before review or trace chains. Use `show` for exact source after srcwalk gives a path/line/range, or when you already know the target.

```bash
srcwalk context <file>:<symbol>
srcwalk context <file>:<line-or-range>
srcwalk context <symbol> --scope <dir>
srcwalk show <path>:123 -C 10
srcwalk show 'a.rs:12,b.rs:40-55'
srcwalk show <path> --section <symbol>
srcwalk show <path> --section '120-140,SomeSymbol' -C 10
srcwalk show README.md --section '# Install'
srcwalk <path>:123-150
```

Do not pass a bare file to `context`; use `show` or root reads. `-C` uses the requested context for one target; comma-separated multi reads clamp each target to 10 lines.

### Trace calls

Use `trace callers` for upstream call sites and `trace callees` for downstream calls. Do not grep `foo(`.

```bash
srcwalk trace callers <symbol> --scope <dir>
srcwalk trace callers <symbol> --scope <dir> --expand=3
srcwalk trace callers <symbol> --count-by receiver --scope <dir>
srcwalk trace callers <symbol> --depth 3 --max-frontier 20 --max-edges 100 --skip-hubs log,emit --scope <dir>
srcwalk trace callees <symbol> --scope <dir>
srcwalk trace callees <symbol> --detailed --scope <dir>
srcwalk trace callees <symbol> --detailed --filter receiver:client --scope <dir>
srcwalk trace callees <symbol> --depth 2 --scope <dir>
```

Drill down with exact call-site reads or `context` on a caller/callee.

### Inspect file coupling

Use `deps` for imports, links/assets, local symbol deps, and dependents. Run it before file moves, deletes, or coupling explanations.

```bash
srcwalk deps <file>
srcwalk show <path>:123-150
srcwalk context <related-symbol> --scope <dir>
```

Do not grep import/use/require/link tags for dependency claims.

### Assess edit risk

Use `assess` before changing, removing, renaming, or publicizing a symbol. It is blast-radius triage; verify risky results with trace callers/deps.

```bash
srcwalk assess <symbol> --scope <dir>
srcwalk trace callers <symbol> --scope <dir>
srcwalk deps <file>
```

### Review changed evidence

Use `review` for change sets. It composes changed evidence with bounded Flow Maps for changed function-like symbols.

```bash
srcwalk review
srcwalk review --staged
srcwalk review --staged --limit 5 --offset 5
srcwalk review HEAD~1..HEAD --scope src
srcwalk context <changed-symbol> --scope <dir>
srcwalk show <path>:123-150
```

### Compare two known targets

Use `compare` for two known source targets. It reports shared/only structural evidence, not equivalence, runtime, security, or correctness proof.

```bash
srcwalk compare <file>:<symbol-a> <file>:<symbol-b>
srcwalk compare <symbol-a> <symbol-b> --scope <dir>
```

### Confirm raw text or filesystem metadata

Use `rg` for raw regex and regex flags; srcwalk text discovery is literal evidence plus navigation context. Use shell `find`/`fd` only for filesystem metadata: permissions, mtimes, empty dirs, symlinks, binary assets, generated outputs, cleanup lists.

```bash
rg '<regex>' <dir>
find <dir> -type f -mtime -1
fd -HI -t f -x stat
```

Do not infer definitions, usages, callers, deps, or code paths from shell path lists. Do not convert identifiers into paths without evidence.

## Artifact routes

Exact artifact reads/scopes may auto-enable artifact mode. Use `--artifact` for broad generated, bundled, minified, or binary-like traversal. Prefer exact footer commands. Artifact output is byte-span evidence only.

```bash
srcwalk <artifact-file> --artifact
srcwalk <artifact-file> --artifact --section bytes:<start>-<end>
srcwalk dist/app.min.js --artifact  # artifact-level outline for bundled/minified output
```

## Supported structural languages

Code/source structure: Rust, TypeScript/TSX, JavaScript, Python, Go, Java/Scala/Kotlin, C/C++, Ruby, PHP, C#, Swift, Elixir, CSS/SCSS/Less.

Documents: HTML/HTM plus Markdown-style `.md`, `.mdx`, `.rst` fallback. Covers sections, elements, code blocks, links, assets. Treat document output as navigation evidence, not rendered or runtime proof.

Unsupported languages still work for exact reads; structural facts may be unavailable.
