# US-065 Output Compaction: Group Repeated Paths And Hoist Stable Metadata

## Status

implemented — review complete; real Windows verification pending

## Lane

normal (with stronger validation)

## Intake Record

- Input type: measured output-efficiency follow-up from the Go owner-evidence
  validation.
- Risk flags: existing output behavior and agent-visible public contracts; path
  rendering is platform-sensitive. No matching, ranking, evidence, or semantic
  relation changes.
- Implementation owner: Toby.
- Single working branch: `feat/owner-evidence-output-compaction`; continue on
  this branch. Do not create a second feature branch for US-065.
- Scope split: this story ships independently before multi-language owner
  attribution. Multi-language owners are explicitly out of scope here.

## Problem Statement

Several high-frequency navigation outputs repeat a copy-pasteable repository path
on every hit even when adjacent entries are from the same file. One discover
section spends about 25% of its bytes repeating paths. Definition output also
repeats an identical provenance line per entry even though Tests and Comments
already hoist it once per section.

Measured redundant surfaces on `benchmark/fixtures/repos/pebble`:

1. Text OR per-term details (`render_text_or_term_details`): every hit repeats the
   full path; 10 same-file hits print the path 10 times.
2. Single-term discover Tests and Comments facets: every hit repeats the path.
3. `trace callers`: every visible call site repeats the path.
4. Discover Definitions: identical `source · kind · confidence` provenance is
   printed after every definition (172 repetitions in one audit query).

Estimated opportunity on exploratory commands: 15–25% less stdout without
removing evidence.

## Existing Conventions To Reuse

Do not invent a new packet grammar. Reuse these shipped shapes:

- Discover name occurrences: `batch.go [4 name occurrences]` followed by
  indented `:LINE | text` rows.
- Text OR file-rollup: one file header with indented hits and terms.
- Deps: one directory/file header with indented children.
- Assess: callers grouped by file.

The new shape must look like these existing grouped packets.

## Feature Shape Gate

1. Product fit: yes — reduces context bloat in the commands agents call most.
2. Intent shape: default rendering improvement; no new command, flag, config, or
   environment variable.
3. Cognitive load: none — the existing file-group convention is reused.
4. Evidence source: unchanged. Every hit, line, owner anchor, caller attribute,
   and provenance tuple remains visible.
5. Output bounds: output must not grow for single-hit files; ordering, limits,
   pagination, collapse pointers, and budgets remain unchanged.
6. Platform scope: displayed paths stay copy-pasteable from the current working
   directory. Windows verification is mandatory.
7. Validation: unit, integration, byte-accounting, round-trip, and Windows tests.
8. Agent-facing UX: update GUIDE/README only if an affected example or stated
   packet shape exists there.

## Hard Invariants

1. **Never shorten or strip the scope prefix from a displayed path.** Grouping
   means printing the same current `rel_nonempty`/anchor path once as a file
   header, not changing the path value.
2. A printed group path must remain directly copy-pasteable into `srcwalk show`
   and printed `> Next:` commands from the command's working directory.
3. No hit, definition, call site, owner anchor, metadata tuple, receiver, arg
   count, expansion, caveat, omission note, or pointer may disappear.
4. Preserve the current evidence order. File groups follow first appearance;
   entries inside each group retain current relative order. Do not re-rank.
5. Apply current pagination, limits, top-K collapse, budgets, and omission
   accounting **before** presentation grouping. Grouping must not change which
   entries are visible or reachable.
6. A file with one visible entry keeps its current one-line shape. Group only
   files with at least two visible entries so compaction cannot create bloat.
7. Same input and worker count produce deterministic, byte-stable output.
8. Identity-equivalence acceptance is measured with `--no-budget` (or a budget
   large enough to render the complete compared slice). On budgeted paths,
   compaction must never lose a previously visible entry; it may retain extra
   detail that now fits inside the same budget.

## Product Contract

### A. Text OR Per-Term Details

Seam: `render_text_or_term_details` in `src/commands/find.rs`.

Current repeated shape:

```text
  path/to/file.go:10 [owner Foo@1-30] — text
  path/to/file.go:20 [owner Foo@1-30] — text
```

Required grouped shape for two or more hits in one file:

```text
  path/to/file.go [2 matches]
    :10 [owner Foo@1-30] — text
    :20 [owner Foo@1-30] — text
```

Grouping is **conditional**: it is applied only when the grouped form is strictly
smaller in bytes than the current repeated inline form. Short paths (e.g. a
2-hit `batch.go`) can tie or grow under a group header, so such groups stay
inline to honor the no-growth invariant. Render both candidates and pick the
smaller; on a tie keep the inline (ungrouped) form.

Single-hit files remain byte-identical to current output.

The `[N matches]` noun is intentionally facet-local rather than copied from
`[N name occurrences]`; the evidence kind is already established by the enclosing
term section.

#### Owner-tag compaction

Owner information is evidence and must not be dropped. Within a multi-hit file
group:

- A contiguous run of **three or more** hits with the exact same owner anchor
  (`qualified name`, start, end) prints the owner once as a subgroup header.
  K=3 is deliberate: K=2 can grow output after subgroup indentation and the
  `[2 hits]` label, violating the no-growth invariant:

  ```text
    [owner Foo@1-30] [3 hits]
      :10 — text
      :20 — text
      :24 — text
  ```

- Runs of one or two, a unique owner, mixed owner, or unattributed hit stay
  inline on their hit rows.
- Only contiguous equal-owner runs may be folded; do not reorder hits to create
  owner groups.
- Owner grouping is presentation-only. It must use the existing owner evidence
  already attached to each hit and must not invoke new analysis.

### B. Single-Term Tests And Comments

Seams: Tests and Comments facet loops in
`format_search_result_with_header` (`src/search/display/mod.rs`).

For each facet independently, group two or more visible hits from the same file:

```text
### Tests (3)
source: ...
  commit_test.go [2 matches]
    :100 — first snippet
    :130 — second snippet
  db_test.go:50 — single-hit snippet
```

- Keep the existing section-level provenance behavior.
- Tests and Comments do not share a group even when paths match.
- Single-hit rows remain in their current shape.
- As in Seam A, grouping is conditional: render both candidates and pick the
  strictly-smaller one; on a tie keep the inline form.

### C. Trace Callers

Seam: source-mode rendering in
`search_callers_expanded_with_artifact` (`src/search/callers/single.rs`). Artifact
caller grouping is already separate and is not redesigned.

After current filtering, ranking, offset, limit, and top-K collapse select the
visible call sites, group two or more visible entries from the same file:

```text
<- calls
  pkg/file.go [2 call sites]
    [fn] CallerA :40 args=1
    [fn] CallerB :72 prefix=x(type) args=2
```

- Grouping occurs only on the already-selected visible page. It must not turn
  `precision::K` from call sites into file groups.
- `+N more` and `--offset` pointers remain byte-identical and enumerate the same
  remaining call sites.
- `--expand` source windows remain attached to the correct child call site and
  retain exact source lines.
- Single call sites keep the current line shape.
- As in Seam A, grouping is conditional: render the grouped and ungrouped
  candidates and pick the strictly-smaller one; on a tie keep the inline form.
  `--expand` windows are included in both candidates, interleaved at the correct
  child call site, so the byte comparison (and the retained source windows) stay
  correct.

### D. Definition Provenance Hoist

Seams: compact Definitions facet in `format_compact_facet_matches` and definition
formatting/provenance helpers in `src/search/display`.

A provenance tuple is `(source, displayed kind, confidence)`. For a Definitions
section:

1. Select the most frequent tuple among visible definitions as the section
   default; ties resolve by first occurrence.
2. Print that default once immediately below `### Definitions (N)`.
3. Do not repeat it after entries using the section default.
4. An entry with a different tuple prints its own provenance directly beneath
   that entry.
5. All-mixed and one-entry cases must remain honest; no tuple may be inferred or
   generalized beyond the entries that carry it.
6. The default is computed from the visible Definitions section instance for
   the current invocation/page. It is deterministic per invocation and is never
   cached or carried between `--offset` pages.

The same hoist is applied to the compact `Matches — same package` (`usages_local`)
facet. Its default is chosen from the **existing rendered provenance carriers**
— one `group[0]` per path group, or a singleton row — never weighted by the
number of child hits; ties resolve to the first-seen carrier. The default is
printed once immediately below the heading, and identical per-group provenance
is suppressed. Definitions remains entry-level (each visible definition is its
own carrier). Deviations print locally. The cross-package (`usages_cross`)
section is intentionally **not** hoisted — it keeps per-group provenance. This
contract is bounded to Definitions and usages_local in this story; do not refactor
every provenance surface speculatively.

## Design Seams

- `src/commands/find.rs::render_text_or_term_details`: stable group-by-path plus
  contiguous owner-run rendering.
- `src/search/display/mod.rs::format_search_result_with_header`: Tests/Comments
  file groups and Definitions section-default provenance.
- `src/search/display/mod.rs::format_compact_facet_matches` and
  `src/search/display/semantic.rs`: suppress per-entry provenance only when it
  exactly equals the explicit section default.
- `src/search/callers/single.rs::search_callers_expanded_with_artifact`: group the
  final visible source call-site slice, after all selection semantics.
- Use existing path functions (`rel_nonempty`, anchor display) unchanged.
- A small stable grouping helper is acceptable if it serves multiple seams
  without erasing their different entry contracts. Do not force a generic
  renderer abstraction merely to share a loop.

## Acceptance Criteria

### Correctness

- Every pre-change visible entry maps one-to-one to a post-change entry with the
  same path, line/range, text, owner, metadata, and caller fields.
- When the grouped candidate wins, a multi-hit file path appears exactly once
  within its section group and child rows use `:LINE` or `:START-END` anchors.
  When grouping is unprofitable or a tie, the file's exact inline rows are
  retained at their original positions in the visible slice; no row is moved or
  reordered.
- Single-hit files are byte-identical to current output.
- Owner runs fold only for three or more contiguous hits with the exact same
  owner; K=2 stays inline, and mixed/unattributed rows stay honest.
- Definitions with uniform provenance print it once. Mixed-provenance fixtures
  print the default once and each deviation locally. The same hoist applies to
  the usages_local section; usages_cross keeps per-group provenance.
- Single-hit files, usages_cross groups, and non-compact paths stay byte-identical
  to current output.
- Trace collapse/pagination exposes exactly the same call-site identities before
  and after compaction; printed pointer commands return the same remainder.
- Group paths and printed commands round-trip when pasted from repository root,
  nested scope, and Windows working directories.

### Efficiency

- Target: on the pinned Pebble audit command set, combined stdout bytes decrease
  by 15% with identical evidence identities and omission counts. 15% is a target,
  not a hard gate. Record the actual combined `--no-budget` reduction; hard
  escalation is required only below 10%. A result of 10–15% is accepted honestly
  with no scope chasing.
- The Text OR per-term detail section with 10 same-file hits prints the path once
  and is strictly smaller.
- No representative command grows in bytes solely because of grouping. Grouping
  is conditional (applied only when strictly smaller); on a tie the inline form
  is kept, so grouping never grows output.
- Report bytes and o200k tokens separately; do not equate stdout bytes with API
  billed tokens.

### Compatibility

- Non-grouped outputs, artifact callers, Text OR file-rollup mode, ranking,
  matching, budgets, caps, and exit codes remain unchanged.
- Existing exact-format integration tests are updated in the same PR; unrelated
  snapshots remain byte-identical.
- GUIDE/README examples are updated only where their exact output changed.

## Pinned Validation Corpus

Run from `benchmark/fixtures/repos/pebble` (or reproduce equivalent committed
fixtures in integration tests):

Two explicit measurement modes are run on the pinned set.

**Mode A — `--no-budget` (compaction KPI)**: all four commands use `--no-budget`:

```bash
srcwalk discover 'strictWALTail,readWAL' --match any --as text --scope . --no-budget
srcwalk discover SyncWait --scope . --no-budget
srcwalk discover Set --as symbol --scope . --no-budget
srcwalk trace callers Set --scope . --no-budget
```

**Mode B — original/default budget**: same four commands WITHOUT `--no-budget`
(original flags). These runs prove (a) no previously visible evidence is lost and
(b) within the same budget the compact rendering retains at least as much
evidence, potentially more. Default-budget bytes are NOT the compaction KPI.

For both modes, record before/after:

- stdout bytes and o200k tokens;
- ordered evidence identity list `(section, path, line/range, kind, owner)`;
- path occurrence count per section;
- omission/pagination/pointer text;
- pasted `show`/pointer command success.

If a pinned query no longer exercises the audited surface after fixture changes,
replace it with a committed minimal fixture and document the replacement; do not
relax the acceptance rule.

## Validation Matrix

| Layer | Required proof |
| --- | --- |
| Unit | stable first-appearance grouping; single/multi boundary; profitable/unprofitable/tie grouping decisions; interleaved order preservation (ungrouped rows keep original slice positions; profitable group emits at first occurrence); owner-run K=2 inline / K=3 folded; uniform/mixed/tied provenance defaults; carrier-majority default for usages_local; page-local default |
| Integration | exact packets for all four surfaces; single-hit byte identity; no lost evidence identities; collapse/offset/expand caller cases |
| Measurement | pinned Pebble before/after byte + o200k report in both modes (A `--no-budget` KPI, B default-budget evidence-retention); record actual reduction; hard escalation only below 10% |
| Round-trip | grouped path copied into `show`; unchanged pointer commands execute and return expected remainder |
| Platform | real Windows build/run for grouped relative, nested, and cross-directory paths |
| Release | `cargo fmt`; targeted tests; `cargo test --locked`; `cargo clippy -- -D warnings`; docs/examples aligned |

## Windows Gate

This story cannot be declared release-ready from Unix-only tests. On a real
Windows runner:

1. Build srcwalk.
2. Run one grouped discover Text OR, one grouped Tests/Comments fixture, and one
   grouped trace callers fixture.
3. Paste each displayed group path into `srcwalk show`.
4. Run any printed continuation pointer.
5. Confirm no slash conversion, drive-prefix, quoting, or nested-scope regression.

Parser/string round-trip tests alone do not satisfy this gate.

## Documentation And Release Notes

- Release notes: “group repeated same-file hits (only when strictly smaller) and
  hoist stable definition and same-package-usage provenance; evidence, ordering,
  paths, and reachability unchanged.”
- No GUIDE concept is added: agents already understand file headers and `:LINE`
  child rows. Update GUIDE only if an existing exact format statement/example is
  invalidated.
- Update any README exact-output example touched by the implementation.

## Out Of Scope

- Removing, truncating, or re-ranking evidence.
- Stripping scope prefixes or shortening paths.
- Changing owner inference, owner edge inference, matching, pagination, budgets,
  or collapse constants.
- Artifact caller redesign.
- Grouping every renderer in the repository.
- Multi-language owners. Follow-up plan is owners-only (no edges) for TS/JS,
  Python, and Rust first; Java, C/C++, and C# second. That work requires a
  separate spec and must not block this story.
- Benchmark semantic scoring; this story is presentation-preserving and is gated
  by identity equivalence, byte reduction, and round-trip tests.

## Done Bar

US-065 is done only when all acceptance criteria pass, the actual pinned-corpus
reduction is recorded (15% target; hard escalation only below 10%; 10–15%
accepted honestly with no scope chasing), exact-format docs/tests are updated,
and real Windows verification is attached. Internal merge may precede the
Windows run only if the PR is explicitly marked not release-ready.
