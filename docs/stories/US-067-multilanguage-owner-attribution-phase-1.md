# US-067: Multilanguage Owner Attribution — Phase 1

## Status

ready for implementation — audit conditions incorporated

## Lane

normal (accuracy-sensitive evidence feature)

## Context And Release Boundary

- Implementation owner: Toby after design audit approval.
- Review owner: Rocky; independent contract audit: Gizmo.
- Single working branch: `feat/owner-evidence-output-compaction`. Do not create a
  second branch.
- This story is part of one future release bundle: existing Go owner evidence,
  US-065 output compaction, and this phase-1 multilanguage owner attribution.
- The release version is intentionally undecided. Do not change Cargo/npm/lock
  versions, tag, publish, push, or ship without an explicit user instruction.
- Run the real Windows gate once after this story lands, for the complete bundle.

## Evidence-Based Scope Decision

Owner ranges and rollups carried the measured navigation value: owner-only
rollups were consumed in 10/95 eligible historical shows, while call edges were
useful mainly in chain-shaped tasks and require language-specific binding rules.
Therefore phase 1 adds **owner attribution only** for:

1. TypeScript/JavaScript: `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`;
2. Python;
3. Rust.

No call edges are inferred for these languages. A language may receive edges only
under a later evidence-backed story with its own binding and accuracy contract.

## User Outcome

For the existing Text OR flow, a hit inside a supported named callable shows the
same owner evidence shape already used by Go:

```text
  path/to/file.ts
  owners (#N=Nth query term; *K=hits): Service.handle:20-48[#1,#3*2]
```

In non-compact per-term detail mode, the existing inline shape is also reused:

```text
  path/to/file.py:31 [owner Service.handle@20-48] — matched text
```

No new command, flag, configuration, section, trigger, or output encoding is
introduced.

## Existing Design Seam

Current ownership is built in `src/evidence/owner_links.rs`:

1. `build_owner_link_evidence` groups visible Text OR hits by path.
2. Go files are parsed; `collect_file_owners` creates `OwnerAnchor`s.
3. `narrowest_owner` attributes a line to one unique narrowest callable range.
4. Go-only lexical analysis builds mechanically filtered call edges.
5. `src/commands/find.rs` renders inline tags, compact `owners (#N=...)`
   rollups, and the Go call appendix.

Phase 1 belongs at the owner-extraction boundary. It must not add language rules
inside `find.rs` or weaken the Go edge engine.

## Core Data Contract

### Owner regions, not only owner anchors

Each parsed supported file produces nested callable regions of two kinds:

```text
Named(owner anchor)
AnonymousBarrier(start_line, end_line)
```

For every visible hit:

1. Find all callable regions whose inclusive line range contains the hit.
2. Select the unique narrowest range by `(end_line - start_line)`.
3. If the narrowest region is `Named`, attach that owner.
4. If it is `AnonymousBarrier`, abstain.
5. If equally narrow regions tie, abstain.

An anonymous callable is a barrier, not merely an omitted owner node. Otherwise a
hit inside `items.map(x => ...)`, a Python lambda, or a Rust closure could fall
through and be falsely attributed to an enclosing named function.

A named callable nested inside an anonymous barrier remains eligible within its
own narrower range.

Region extraction must be exhaustive over **all callable node kinds** in each
version-pinned grammar. Every callable node is classified as either a supported
`Named` owner or an `AnonymousBarrier`; silently omitting a callable node is a
contract violation. Thus unsupported but callable constructs—object-literal
methods, getters/setters outside supported forms, IIFEs, prototype-assigned
functions, Python lambdas, Rust closures, and future callable kinds exposed by
the pinned grammar—cannot leak hits to an outer owner.

Each adapter maintains a version-pinned callable-kind manifest audited against
the grammar's bundled node-type metadata. Tests must prove every manifest kind
produces exactly one region classification, and fixtures must exercise every
kind. A grammar upgrade that adds or changes a callable kind must fail this
coverage check until the adapter classifies it deliberately.

### Anchor identity

Every named anchor contains:

- exact repository path;
- unqualified callable name;
- explicit display-qualified name;
- inclusive 1-indexed start/end lines from the callable AST node;
- source language.

Qualified display identity must be separate from Go receiver/binding identity.
Do not overload `receiver_type` to encode Python, JS, or Rust containers. Existing
Go edge fields may remain Go-specific.

Ordering and equality are deterministic over path, qualified name, and range.
No owner may render placeholder names such as `<anonymous>` or `<impl>`.

### Parse failure and local error regions

If a supported file cannot be read, produces no tree, or its root node itself is
an `ERROR`, preserve all raw text hits and emit no owner evidence for that file.
For a partial tree with local `ERROR` or missing-node descendants:

1. Record every error/missing byte range and inclusive line range.
2. A callable whose byte range overlaps any error range is replaced by an
   `AnonymousBarrier` covering the full callable; it is never a `Named` owner.
3. An error range outside a callable is itself an abstention barrier, so a hit on
   an error-overlapping line cannot fall through to an enclosing owner.
4. Named regions with no error overlap remain eligible.

Because Text OR hit inputs are line-based, any hit line intersecting an error
range abstains even when the parser provides narrower columns. Never recover
owner names from signatures, indentation, regexes, or raw-line guessing.

## Language Contracts

### A. TypeScript And JavaScript Family

Use the grammar selected by the existing `Lang`/extension routing, including the
TSX grammar for `.tsx` and JavaScript grammar for JS/JSX/MJS/CJS.

#### Supported named owners

1. Named function and generator declarations: `load`.
2. Class methods with a static/simple/private identifier name:
   `Service.handle`, `Service.#reset`.
3. A direct variable declarator with a simple identifier whose initializer is an
   arrow or function expression: `load`.
4. A class field with a simple/private property name whose direct value is an
   arrow or function expression: `Service.handle`.
5. A function expression with an explicit name may use that explicit name when
   no supported binding/property owner exists.
6. Nested supported named callables append lexical containers with `.`:
   `outer.inner`, `Service.handle.inner`.
7. TypeScript namespace/internal-module and ECMAScript module-block lexical
   containers append with `.` (for example `Api.load`) but are not callable
   owners themselves. This prevents equal unqualified names in separate
   namespaces from conflating.

Transparent syntax wrappers such as `export`, `default`, TypeScript annotations,
and decorators do not become containers and do not change the callable node
range.

#### Required abstentions/barriers

- Inline anonymous arrows/functions used as call arguments, returns, array/object
  elements, JSX expressions, or assignment targets without a supported simple
  binding/property name.
- Computed method/property names whose stable source name is not a simple or
  private identifier.
- Overload signatures or declarations without a body.
- Any construct whose class/binding container cannot be read structurally.
- Object-literal methods/accessors, IIFEs, and prototype/member-assigned function
  expressions are barriers unless they independently satisfy a supported named
  binding form above.

Do not invent labels such as `callback@42`, `<arrow>`, property text, or a callee
name from the surrounding call.

### B. Python

#### Supported named owners

1. Synchronous and asynchronous `function_definition` nodes.
2. Class methods: `Service.handle`.
3. Nested classes and functions use their complete lexical hierarchy with `.`:
   `Outer.Inner.handle`, `outer.inner`, `Outer.method.inner`.

The anchor range starts at the underlying `def`/`async def` node and ends at its
body. A `decorated_definition` wrapper is transparent: decorator lines are not
inside the callable owner range, and a hit in a decorator expression is not
attributed to the decorated function.

#### Required abstentions/barriers

- Every `lambda` is an anonymous barrier.
- A malformed decorated definition or a callable with no structural name
  abstains.
- Do not infer class/function hierarchy from indentation or rendered signatures.

### C. Rust

#### Supported named owners

1. Top-level body-bearing `function_item`: `run`.
2. Lexically nested functions/modules use Rust `::`: `outer::inner`,
   `server::run`.
3. Inherent impl method: `Type::method`.
4. Trait default/body method: `Trait::method`.
5. Trait implementation method: `<Type as Trait>::method`.

For impl names, read the AST `type` and optional `trait` fields. Preserve their
trimmed source spelling. Collapse every non-empty run recognized by Rust
`str::split_whitespace` to exactly one ASCII space; preserve all other source
tokens and punctuation. Do not semantically resolve aliases or modules. Examples:

```text
impl<T> Store<T>                 => Store<T>::get
impl<T> Cache for Store<T>       => <Store<T> as Cache>::get
```

The trait-qualified form is binding: it prevents a trait method from being
presented as the same owner as an inherent method with the same name.

#### Required abstentions/barriers

- Every closure expression is an anonymous barrier.
- Trait signatures and extern declarations without bodies are not owners.
- If an impl target or trait name cannot be extracted structurally, its methods
  abstain rather than fall back to an unqualified name.
- Macro-generated owners are not inferred from expanded or guessed source.

## Rendering And Trigger Parity

- Run attribution in the same existing Text OR flow and at the same point after
  filtering/pagination as Go.
- Existing compact selection remains unchanged: compact file rollup is selected
  by the current term/match thresholds (including three-or-more terms); otherwise
  existing per-term detail rows are used.
- Reuse `OwnerAnchor`, inline `[owner ...@START-END]`, and
  `owners (#N=Nth query term; *K=hits)` rendering.
- Preserve US-065 conditional file grouping and K=3 repeated-owner folding.
- Preserve path rendering, term indices, count multipliers, ordering, caps,
  budgets, and omission accounting.
- Artifact mode continues to abstain exactly as today.

### Capability-honest call appendix

Owner attribution support and edge-analysis support are separate capabilities.

- New-language owners never populate `OwnerCallEvidence`.
- `## Mechanical Go calls`, its zero-edge sentence, and its call-specific caveat
  are driven only by Go owner/edge analysis.
- A non-Go-only result must not say “No direct name-level call evidence...”;
  absence means unsupported analysis, not measured zero.
- A mixed-language query may render non-Go owner rollups plus a Go-only mechanical
  appendix, but that appendix must remain explicitly labeled Go.
- Existing Go-only output stays byte-identical.
- Non-Go owner output includes one concise honesty caveat stating that ranges are
  structural lexical ownership candidates, not runtime ownership or binding
  proof. It must not imply call analysis ran.

The evidence model should carry the capability/attempt distinction explicitly;
do not infer it from `edges.is_empty()`.

## Implementation Shape

Keep orchestration and Go edges in `owner_links.rs`, but do not add three large
parser implementations to that already-large file. Add focused owner extractors,
for example:

```text
src/evidence/owners/mod.rs
src/evidence/owners/js_ts.rs
src/evidence/owners/python.rs
src/evidence/owners/rust.rs
```

A small shared walker for `Named`/`AnonymousBarrier`, lexical container stacks,
and unique-narrowest selection is appropriate. Language-specific node kinds,
field extraction, and qualified naming stay in their adapter.

Do not refactor unrelated outline, caller, callee, qualified-symbol, or semantic
search logic. Existing outline entries may be reused only where they preserve the
contracts above; direct AST extraction is required for barriers, decorators, and
Rust impl identity.

## Correctness Invariants

1. Raw matches, ranking, pagination, and omission counts are identical with
   attribution enabled or abstaining.
2. Every attributed hit has exactly one structurally containing callable range.
3. The displayed qualified name and inclusive range match source exactly.
4. The narrowest callable region wins; ties and anonymous barriers abstain.
5. Parse errors and unsupported name forms produce no owner evidence, never a
   fallback guess.
6. No new-language edge or zero-edge claim is rendered.
7. Existing Go attribution, Go edges, output, and tests remain byte-identical.
8. Same source and visible hit slice produce deterministic byte-stable output.
9. TS/JS family extensions route through the correct existing grammar.
10. Scope-relative and Windows path behavior remains unchanged.

## Accuracy Suite

Create curated table-driven fixtures per language adapter. “0 wrong” cannot be
satisfied by abstaining everywhere.

### Per-language minimum

For each of JS/TS family, Python, and Rust:

- at least 60 positive hit assertions across supported owner forms;
- at least 20 explicit abstention assertions across anonymous barriers,
  unsupported names, body-less declarations, malformed source, and equal-range
  ambiguity where applicable;
- 100% of required supported-form positives attributed;
- 100% exact qualified names and inclusive ranges;
- 0 false attributions;
- deterministic output under repeated runs;
- 100% callable-kind manifest coverage against the pinned grammar node metadata;
  every callable kind classified as `Named` or `AnonymousBarrier`.

The JS/TS curated matrix must contain `.ts`, `.tsx`, `.js`, and `.jsx` fixtures,
not only one grammar. Include nested named callables inside and outside anonymous
barriers. Python must include decorated async methods and nested class/def
hierarchies. Rust must include inherent impl, trait default method, trait impl,
generics, modules, nested functions, and closures.

Add cross-language orchestration tests proving one Text OR query can attribute
supported files while preserving raw hits from malformed/unsupported files.

## Real-Repository Replay

Use the already-pinned benchmark repositories and SHAs:

| Language | Repository | SHA |
| --- | --- | --- |
| TypeScript | Effect | `9245bc59ebfa688e8c92dd691296ee69d0815e59` |
| JavaScript | Express | `1140301f6a0ed5a05bc1ef38d48294f75a49580c` |
| Python | FastAPI | `6fa573ce0bc16fe445f93db413d20146dd9ff35d` |
| Rust | ripgrep | `0a88cccd5188074de96f54a4b6b44a63971ac157` |

Predeclare at least one three-term Text OR query per repository before examining
candidate results. If the first query yields fewer than 50 attributed hit-owner
pairs, add another query and record why; do not cherry-pick only easy owners.

For each repository, audit attributed pairs in deterministic
`(path,line,qualified_name,start,end)` order. Start with the first 50, which must
span at least 10 distinct files. If they do not, continue in the same order until
10 files are represented and audit every additional pair traversed; never skip
pairs to manufacture spread.

- required result: every audited pair has exact range containment and
  qualified-name correctness (at least 50 pairs across at least 10 files);
- required false-attribution count: 0;
- record total raw hits, attributed hits, barrier abstentions observed, parse-error
  abstentions, and distinct owner forms;
- report attribution rate descriptively, not as a quality score.

Also replay the same commands twice and require byte-identical owner rollups.
No new model A/B is required: this gate proves attribution accuracy, while prior
experiments already established owner-value demand.

## Regression And Verification Matrix

| Layer | Required proof |
| --- | --- |
| Unit | Per-language 60 positive + 20 abstention assertions; callable-kind manifest 100%; exact names/ranges; barriers; local errors; deterministic ties |
| Integration | Existing trigger parity; compact and detail shapes; mixed languages; artifact abstention; no non-Go edge/zero-edge claim |
| Go regression | Existing Go exact outputs and complete Go owner/edge suite byte-identical |
| Real repos | Effect/Express/FastAPI/ripgrep deterministic audit; ≥50 pairs and ≥10 files each; 0 false attribution |
| Build | `cargo fmt`; `cargo fmt --check`; targeted tests; `cargo test --locked`; `cargo clippy -- -D warnings` |
| npm | `node npm/install.test.js`; `(cd npm && npm pack --dry-run)` if package metadata or release surface changes |
| Platform | One real Windows build/run after phase 1, covering Go plus TS/Python/Rust owner paths and pasted `show` anchors |

## Documentation

After behavior is verified:

- Update GUIDE language scope and state explicitly that call edges remain Go-only.
- Update README only if an existing language-support/output statement becomes
  incomplete; do not add speculative examples.
- Keep CHANGELOG under `Unreleased` and draft one bundled release note covering
  Go owners/edges, US-065 compaction, and phase-1 owner-only languages.
- Do not choose or write a release version until the user decides it.

## Out Of Scope

- Call edges for TypeScript/JavaScript, Python, or Rust.
- Dynamic dispatch, callback target, dependency-injection, protocol, trait-object,
  import, or binding resolution.
- New flags, config, intent classifier, trigger thresholds, caps, or ranking.
- C, C++, Java, C#, or any phase-2 language.
- Anonymous-owner synthetic names.
- Semantic module/package inference from paths.
- New model A/B runs.
- Version bump, tag, publish, push, or release.

## Done Bar

Implementation is complete only when the curated matrices and callable-kind
manifests pass with 0 false attributions, all four deterministic real-repo audits
cover at least 50 pairs across at least 10 files with every audited pair exact,
Go output is unchanged, docs describe owner-only language scope honestly, and
full local verification passes. The complete release bundle is only
release-eligible after the real Windows gate; actual versioning and shipping
still require an explicit
user command.
