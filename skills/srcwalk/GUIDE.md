# srcwalk — agent routing policy

Use srcwalk before shell search for code navigation. Keep `--scope` narrow.
Use raw `rg` only for final text confirmation.

## Default workflow

Use this command flow for broad, unfamiliar, or risky code tasks:

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

### Orient an unfamiliar area

Do not start orientation with shell `tree`, shell `find`, repeated `ls`, or repo-wide `rg`.

```bash
srcwalk overview --scope <dir>
```

Use auto depth first. Do not pass `--depth` first. Explicit `--depth N` is strict.

`[relations]` are static local dependency groups, not runtime calls.
`[outbound deps]` imports targets outside `--scope`.

Drill down with candidate intake, then context.

```bash
srcwalk discover <query> --scope <dir>
srcwalk discover '<glob>' --as file --scope <dir>
srcwalk discover <term> --as text --scope <dir>
srcwalk context <symbol> --scope <dir>
```

`discover` searches only inside `--scope`; narrow scopes can hide definitions.

`--filter kind:<label>` is exact: `fn`, `class`, `mod`, `impl`, `base`, `usage`, `text`, `comment`.

Intent inference:

- path-like globs such as `*.rs` or `src/**/*.ts` infer file discovery;
- punctuation/path comma lists such as `req.body,fetch` infer literal Text OR;
- symbol globs such as `*Controller` remain symbol search. Add `--as symbol` if ambiguous.

Text discovery: `--match any --as text` is comma literal OR; broad results roll up files first.
`--match all` is same-file co-occurrence, not semantic relation proof.

If discover prints `## Confirmed next context targets`, run one of those `context` commands.

Use `discover <field> --as access` for field/member write/reset/read groups.
It is syntax only, not runtime order, type proof, alias proof, or call relation proof.

### Understand one selected target

Use `context` for one known target. It emits Flow Map facts, call neighborhoods, and exact `> Next:` commands.
Run it before review or trace chains.

```bash
srcwalk context <file>:<symbol>
srcwalk context <file>:<line-or-range>
srcwalk context <symbol> --scope <dir>
```

Read exact evidence after srcwalk gives a path/line/range, or when you already know the target.
Do not pass a bare file to `context`; use `show` or root reads.

```bash
srcwalk show <path>:123 -C 20
srcwalk show 'a.rs:12,b.rs:40-55'
srcwalk show <path> --section <symbol>
srcwalk <path>
srcwalk <path>:123-150
```

### Inspect call direction

Use `trace callers` for upstream call sites. Do not grep `foo(`.

Use `--count-by receiver|caller|file|args|path` for grouped summaries.

```bash
srcwalk trace callers <symbol> --scope <dir>
srcwalk trace callers <symbol> --depth 2 --scope <dir>
srcwalk trace callers <symbol> --count-by receiver --scope <dir>
```

Use `trace callees` for downstream calls.

```bash
srcwalk trace callees <symbol> --scope <dir>
srcwalk trace callees <symbol> --detailed --scope <dir>
srcwalk trace callees <symbol> --depth 2 --scope <dir>
```

Drill down from trace with exact call-site reads or context on a caller/callee.

```bash
srcwalk show <path>:123 -C 20
srcwalk context <caller-or-callee> --scope <dir>
```

### Inspect file coupling

Use `deps` for imports, links/assets, local symbol deps, and dependents.
Run it before file moves, deletes, or coupling explanations. Do not grep import/use/require/link tags.

```bash
srcwalk deps <file>
```

Drill down from deps with exact import/link reads or context on a related source target.

```bash
srcwalk show <path>:123-150
srcwalk context <related-symbol> --scope <dir>
```

### Assess edit risk

Use `assess` before changing, removing, renaming, or publicizing a symbol.
It is fast blast-radius triage; verify risky results with trace callers/deps.

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
srcwalk review HEAD~1..HEAD --scope src
```

Drill down from review with context on a changed symbol or exact changed-range reads.

```bash
srcwalk context <changed-symbol> --scope <dir>
srcwalk show <path>:123-150
```

### Compare two known targets

Use `compare` for two known source targets.
It reports shared/only structural evidence, not equivalence, runtime, security, or correctness proof.

```bash
srcwalk compare <file>:<symbol-a> <file>:<symbol-b>
srcwalk compare <symbol-a> <symbol-b> --scope <dir>
srcwalk show <path>:123-150
```

### Confirm raw text or filesystem metadata

For raw regex and regex flags, use `rg`; srcwalk text discovery is literal evidence plus navigation context.

Use shell `find`/`fd` only for filesystem metadata:
permissions, mtimes, empty dirs, symlinks, binary assets, generated outputs, cleanup lists.

```bash
rg '<regex>' <dir>
find <dir> -type f -mtime -1
find <dir> -empty
fd -HI -t f -x stat
```

Do not infer definitions, usages, callers, deps, or code paths from shell path lists.
Do not convert identifiers into paths without evidence.

```bash
srcwalk discover '<identifier>' --scope <dir>
srcwalk discover '*<name>*' --as file --scope <dir>
```

## Artifact routes

Exact artifact reads/scopes may auto-enable artifact mode.
Use `--artifact` for broad generated, bundled, minified, or binary-like traversal.

Prefer exact footer commands. Artifact output is byte-span evidence only.

```bash
srcwalk <artifact-file> --artifact
srcwalk <artifact-file> --artifact --section bytes:<start>-<end>
srcwalk dist/app.min.js --artifact  # artifact-level outline for bundled/minified output
```

## Supported structural languages

Code/source structure: Rust, TypeScript/TSX, JavaScript, Python, Go, Java/Scala/Kotlin, C/C++.
Also Ruby, PHP, C#, Swift, Elixir, CSS/SCSS/Less.

Documents: HTML/HTM plus Markdown-style `.md`, `.mdx`, `.rst` fallback.
Covers sections, elements, code blocks, links, assets.

Treat document output as navigation evidence, not rendered or runtime proof.

Unsupported languages still work for exact reads; structural facts may be unavailable.
