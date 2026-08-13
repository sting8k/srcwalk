# srcwalk — agent evidence contract

Default to srcwalk first for code-structure work. Use this contract to find exact evidence, choose the next read, and keep claims bounded before shell search.

Keep `--scope` narrow. Use `rg`, `read`, `fd`, `find`, or shell filesystem tools only for final text/regex confirmation, filesystem metadata, generated-output cleanup, or when srcwalk lacks structural support. If you bypass srcwalk for a code claim, say why.

## Choose one route first

| If you know... | Run... | Use when... |
| --- | --- | --- |
| unknown area | `srcwalk overview --scope <dir>` | orient a tree before choosing targets |
| unknown target in known area | `srcwalk discover <query> --scope <dir>` | find candidate symbols/files/text/access hits |
| known body/citation | `srcwalk show <path>:<line-or-range>` | read/cite exact source lines without a relation packet |
| need rich local packet | `srcwalk context <target> --scope <dir>` | inspect a Flow Map, source excerpt, scoped occurrences, or call neighborhood |
| upstream relation | `srcwalk trace callers <symbol> --scope <dir>` | find who calls a symbol |
| downstream relation | `srcwalk trace callees <symbol> --detailed --scope <dir>` | inspect what a symbol calls |
| file coupling | `srcwalk deps <file>` | imports, links/assets, local deps, dependents |
| pre-edit risk | `srcwalk assess <symbol> --scope <dir>` | blast-radius triage before rename/remove/change |
| change set | `srcwalk review --staged` | review changed evidence before tests |
| two known targets | `srcwalk compare <target-a> <target-b> --scope <dir>` | compare structural evidence, not equivalence |

## Batch by evidence dependency

Turn the request's explicit evidence questions into a short coverage list. In each tool round, run independent discoveries or exact reads in parallel; batch exact `show` targets when safely representable. Serialize only when one result names the next target, and treat `> Next:` as a candidate rather than a required round. Before answering, cite each explicit facet or label it unresolved, including relevant branch conditions and caveats.

## Command-shape guardrails

- Multi-root symbol discovery may repeat the flag: `srcwalk discover 'foo,bar' --as symbol --scope src --scope tests`.
- Other routes and `discover --as text|file|access` accept one scope; use a common ancestor or run independent commands in the same model/tool round.
- Keep scope as small as the evidence question allows; narrow scopes can hide definitions.
- Symbol batches accept 2-5 comma-separated symbols: `srcwalk discover 'foo,bar,baz' --as symbol --scope src`. Split larger symbol sets. One dot qualifies a method by receiver/container in a single-symbol query (`'Batch.Set'`); dotted terms inside a batch are a known matching limitation, so run that dotted term separately when exact qualification is required.
- Text OR is separate: `srcwalk discover 'alloc,copy' --match any --as text --scope src` is literal text evidence, not a symbol batch.
- Do not infer definitions, usages, callers, deps, or code paths from shell path lists, broad grep, or converted identifier paths.

## Evidence trust bounds

When output includes `source`, `kind`, `confidence`, or `caveat`, keep those limits in your answer.

- Structural syntax/source is navigation evidence, not runtime behavior, security, correctness, alias, type, order, or dynamic-dispatch proof.
- Text/name/comment/file hits are literal evidence or navigation candidates, not binding-resolved references or relation proof.
- `discover --as access` is syntax only: no runtime order, type proof, alias proof, or call relation proof.
- `overview` `[relations]` are static local dependency groups, not runtime calls; `[outbound deps]` are imports outside `--scope`.
- Documents are navigation structure, links/assets, headings, elements, and code blocks; not rendered DOM, runtime behavior, or accessibility proof.
- Artifacts, generated, minified, bundled, and binary-like outputs are artifact-level or byte-span evidence unless labeled source-level.
- Unsupported languages still support exact reads; structural facts may be unavailable.

## Routes and examples

Use `srcwalk <command> --help` for flags. The examples below show routing, not every option.

### Orient or discover candidates

Do not start broad code navigation with shell `tree`, shell `find`, repeated `ls`, or repo-wide `rg`.

```bash
srcwalk overview --scope <dir>
srcwalk overview --scope <dir> --symbols
srcwalk discover <query> --scope <dir>
srcwalk discover '<glob>' --as file --scope <dir>
srcwalk discover <field> --as access --scope <dir>
srcwalk discover 'foo,bar,baz' --as symbol --scope <dir>     # 2-5 symbol batch
srcwalk discover 'foo,bar,baz' --match any --as text --scope <dir>  # literal text OR
```

Use auto overview depth first; explicit `--depth N` is strict. Narrow `overview --symbols` shows inline `kind name@line-range` anchors when budget allows; broad auto overview may summarize areas/candidates and emit narrow-scope drilldowns.

Intent inference: path-like globs infer file discovery; punctuation/path comma lists infer literal Text OR; symbol globs stay symbol search. Add `--as symbol|file|text|access` when ambiguous. After a first pass, use `--expand=3`, `--filter kind:fn`, or `--exclude 'tests/**'` only when output is too broad. Regex-style queries are translated, not executed as regex: `foo\(` de-escapes to literal + symbol search, `a.*b` runs same-line ordered co-occurrence, `models\.json` behaves like `models.json`, and an unresolved path fragment like `packages/ai` lists matching relative paths (≤20). Each translation is labeled `interpreted as`; zero-match branches print a `> Try:` recovery line. Windows drive paths and `./`/`../` paths are never treated as regex. Text Search and Text OR may add a conditional `> Note:` when a term has at least 400 matches across at least 150 files and reaches at least 1.5% of eligible files; treat it as measured spread, then consider `overview`, a narrower term or scope, or a structural route when that spread was not intentional.

If `discover` prints `## Confirmed structural targets`, run the printed `> Next:` command verbatim. The target is a canonical `<path>:<symbol>` built from parser outline primitives and proven to resolve to that one body; a numeric `<path>:<start-end>` command is the safe fallback when ambiguity prevents a unique symbol selector. A numeric range shown beside a stable symbol is evidence metadata (a bounded preview), not the preferred body address. Use `srcwalk context <target>` only when you need a Flow Map, scoped occurrences, or call neighborhood; do not run `context` for each hop just to read source. Reuse that `<path>:<symbol>` string unchanged across `show`, `context`, `trace callers`, and `trace callees`, keeping any `--scope` printed with it; never rebuild it from a displayed qualified name, because a namespace/module display prefix (`System.Text.Json.GetTypeInfoInternal`) is not a selector and the emitted form uses the owning container (`JsonSerializerOptions.GetTypeInfoInternal`). The path identifies the requested definition; it does not upgrade caller evidence — `trace callers` stays a direct by-name search and may include same-name definitions elsewhere. Unresolved or ambiguous targets fail explicitly instead of falling back to a bare-name search.

Symbol discovery separates parser-backed definitions from text-matched name occurrences. Repeated same-name definitions receive an ambiguity caveat. Text discovery remains literal evidence; `--match all` is same-file co-occurrence, not semantic relation proof.

For Go `discover --as text --match any`, matched lines may carry a structural owner rollup and a bounded mechanical call appendix:
- `owners (#N=...; *K=hits)` lists each matched line's narrowest structural owner by query-term position; the range is a candidate exact read (`srcwalk show`), not relation or runtime binding proof.
- Edge labels: `[recv]`=same package-qualified receiver type, `[local]`=single-assignment constructor local, `[bare]`=same-package invocation.
- `calls NAME` names the call expression, not the candidate binding; the candidate is a mechanically filtered definition candidate, not a proven binding.
- Edges are mechanically filtered direct-call navigation (absence does not rule out dynamic dispatch, DI, callbacks, or protocol wiring); `@:` = candidate in the same call file, cross-file keeps the repo-relative path. Capped, deterministically ordered; zero-edge owners abstain with an explicit caveat.

### Understand one target or read exact evidence

Use `show` for known bodies and citations. Use `context` when the task needs a rich local packet such as Flow Map, scoped occurrences, or call neighborhood. Exact path/range contexts may include bounded `Source Evidence`; `show` `Source frame` lines orient exact numeric reads and numeric `--section` blocks, not relation proof.

```bash
srcwalk context <path>:<symbol>     # or <path>:<line-or-range>
srcwalk context <symbol> --scope <dir>
srcwalk show <path>:123 -C 10
srcwalk show 'a.rs:12,b.rs:40-55'
srcwalk show <file>:12,40-55        # same-file ranges route to --section
srcwalk show <path>:<symbol>        # canonical exact target, reusable across all four commands
srcwalk show <path> --section <symbol>
srcwalk show README.md --section '# Install'
```

Do not pass a bare file to `context`; use `show` or root reads. `context` accepts up to 3 comma-separated exact `path:symbol` or `path:range` targets and splits one global budget across them; repeat the file path for each `context` range. `show` comma-separated multi reads clamp each target to 10 context lines, while clean same-file inline ranges (`file:a,b`) route to the single-file `--section` reader. Supported exact structural definitions may include bounded same-file scoped name occurrences; they are not binding-, type-, or runtime-resolved references.

### Trace calls

Use `trace callers` for upstream call sites and `trace callees` for downstream calls. Do not grep `foo(` for relation claims. Long result lists collapse to the top entries plus a `+N more → <command>` pointer, and wide evidence ranges may appear as an anchor plus an `expand: <command>` line. These pointer lines are exact commands: run them verbatim when you need the remaining items or the full range. Nothing is dropped — collapsed items are always one command away. Drill down with exact call-site reads or `context` on a caller/callee.

```bash
srcwalk trace callers <symbol|path:symbol> --scope <dir>   # exact root; call sites still matched by name
srcwalk trace callees <symbol|path:symbol> --detailed --scope <dir>
srcwalk trace callees <symbol> --depth 2 --scope <dir>
```

### Inspect file coupling

Use `deps` for imports, links/assets, local symbol deps, and dependents. Run it before file moves, deletes, or coupling explanations. Do not grep import/use/require/link tags for dependency claims. For JS/TS/TSX, alias-derived edges marked `(via tsconfig paths)` are config-derived static evidence, not runtime proof, and `Uses (unresolved local-looking)` lists local-looking specifiers (`./`, `../`, `@/`, `~/`) that resolve to no existing file — known-missing local references, not external packages.

```bash
srcwalk deps <file>
srcwalk context <related-symbol> --scope <dir>
```

### Assess and review changes

`assess` is blast-radius triage; verify risky results with `trace callers` or `deps`. `review` composes changed evidence with bounded Flow Maps for changed function-like symbols.

```bash
srcwalk assess <symbol> --scope <dir>
srcwalk review
srcwalk review --staged
srcwalk review HEAD~1..HEAD --scope src
srcwalk context <changed-symbol> --scope <dir>
```

### Compare two known targets

Use `compare` for two known source targets. It reports shared/only structural evidence, not equivalence, runtime, security, or correctness proof.

```bash
srcwalk compare <file>:<symbol-a> <file>:<symbol-b>
srcwalk compare <symbol-a> <symbol-b> --scope <dir>
```

### Confirm raw text or filesystem metadata

Use `rg` for raw regex and regex flags; srcwalk translates common regex-style queries into literal/co-occurrence searches but never runs a regex engine. srcwalk text discovery is literal evidence plus navigation context. Use shell `find`/`fd` only for filesystem metadata: permissions, mtimes, empty dirs, symlinks, binary assets, generated outputs, cleanup lists. Do not infer definitions, usages, callers, deps, or code paths from shell path lists, or convert identifiers into paths without evidence.

```bash
rg '<regex>' <dir>
```

## Artifact and language support

Exact artifact reads/scopes may auto-enable artifact mode. Use `--artifact` for broad generated, bundled, minified, or binary-like traversal. Prefer exact footer commands. Artifact output is byte-span evidence only.

```bash
srcwalk <artifact-file> --artifact
srcwalk <artifact-file> --artifact --section bytes:<start>-<end>
```

Code/source structure varies by command: Rust, TypeScript/TSX (incl. `.mts`/`.cts`), JavaScript (incl. `.jsx`, `.mjs`, `.cjs`), Python, Go, Java/Scala/Kotlin, C/C++, Ruby, PHP, C#, Swift, Elixir, CSS/SCSS/Less. `context`/Flow Map support is narrower: trust confirmed structural targets emitted by srcwalk for exact `show` reads, and use `context` only when you need a rich local packet instead of guessing command support. Flow Maps hard-abstain with an exact caveat instead of emitting a partial graph on constructs the IR cannot represent; trust the caveat.

Documents: HTML/HTM plus Markdown-style `.md`, `.mdx`, `.rst` fallback. Covers sections, elements, code blocks, links, assets. Treat document output as navigation evidence, not rendered or runtime proof.
