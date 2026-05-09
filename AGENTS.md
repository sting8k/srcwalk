# srcwalk

Code-intelligence CLI built on tree-sitter. Outlines, symbol search, caller/callee graphs, deps, maps — structured, token-efficient output for AI agents.

## Product North Star

srcwalk exists to maximize accurate, evidence-based links between knowledge components in a source/artifact tree, so agents can reduce manual navigation steps.

Priority order:
1. Accuracy and semantic correctness.
2. Avoiding confusion, guessing, and false conclusions.
3. Reducing agent steps through structured navigation.
4. Token/output efficiency.

Do not trade correctness for prettier or shorter output. When semantic confidence is low, label the output honestly rather than implying source-level truth.

Before implementing artifact/minified/binary-tool language support, you must read `core/project/srcwalk-artifact-language-implementation-playbook-2026-05-08.md`.

## Project structure

```
src/
  main.rs              Tiny CLI entrypoint (parse CLI, completions, guide, thread pool).
  cli.rs               Clap CLI definitions, subcommands, RunConfig normalization.
  cli_run.rs           CLI dispatch/runtime routing to map/find/read/callers/callees/etc.
  version.rs           Version command, latest-version fetch/parse helpers.
  output.rs            stdout/json direct output helpers.
  lib.rs               Public API facade/wrappers; command implementations live in commands/.
  classify.rs          Query type detection (file path, glob, symbol, content).
  types.rs             Shared types (QueryType, Lang, OutlineEntry, etc.).
  error.rs             Error types with exit codes.
  format.rs            Output formatting helpers.
  budget.rs            Token budget enforcement.
  map.rs               Source-focused codebase map generation: structure + static local dependency relations with fixed hard cap/degrade.
  cache.rs             OutlineCache — DashMap of path → (mtime, outline).
  session.rs           Session state (expanded definition dedup).
  commands/
    find.rs            Core find/read/glob dispatch behind public lib wrappers.
    multi_scope.rs     Multi-scope find merge/header/query handling.
    path.rs            Exact path reads.
    callers.rs         Callers command service.
    callees.rs         Callees command service.
    flow.rs            Flow command service and helpers.
    impact.rs          Impact command service.
    deps.rs            Deps command service.
    context.rs         ArtifactMode, expanded context, artifact output helpers.
    section_disambiguation.rs Bare filename + --section glob disambiguation.
  lang/
    mod.rs             detect_file_type(), package_root().
    outline.rs         Tree-sitter outline extraction.
    treesitter.rs      DEFINITION_KINDS, extract_definition_name().
    detection.rs       Generated/binary file detection.
  read/
    mod.rs             Read facade and default smart file reading.
    full.rs            Full/raw rendering, caps, budget cascade.
    section.rs         Section/range/heading/symbol/multi-section reads.
    directory.rs       Directory listing.
    suggest.rs         Similar-file suggestions and edit distance.
    imports.rs         Import extraction for deps.
    outline/           Code, markdown, structured, tabular, fallback outlines.
  search/
    mod.rs             Search public wrappers/orchestration.
    artifact_snippet.rs Artifact result snippet compaction.
    filter.rs          General field filters for search results.
    display/           Search result formatting, expand budget, semantic rows, glob output.
    symbol.rs          Symbol search facade/wrappers.
    symbol/            Definitions, usages, batch, glob, suggestions, comments helpers.
    content.rs         Text/regex search.
    callers/           Single-hop + multi-hop BFS (up to 5 hops).
    callees.rs         Forward call graph extraction + resolution.
    deps.rs            File-level imports + dependents.
    rank.rs            Result ranking.
    facets.rs          Grouping (definitions, usages, implementations).
    siblings.rs        Sibling symbol surfacing.
    strip.rs           Noise stripping in expanded code.
    truncate.rs        Smart truncation for budget.
    glob.rs            File glob search.
    io.rs              Search I/O helpers.
    pagination.rs      Offset/limit pagination.
  index/
    symbol.rs          In-memory symbol index.
    bloom.rs           Bloom filter for fast pre-check.
npm/                   npm distribution wrapper (postinstall downloads binary).
skills/srcwalk/        Agent guide sources: GUIDE.md embeds into binary; SKILL.md bootstraps agents to `srcwalk guide`.
benchmark/             Evaluation harness (26 tasks, 4 repos).
```

## Languages

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift, Elixir, and Kotlin. Unsupported languages still work for reading files, but structural facts may be unavailable.

## Build & test

```bash
cargo build --release
cargo test
cargo clippy -- -D warnings
cargo fmt --check
cargo install --path .       # → ~/.cargo/bin/srcwalk
```

Formatting workflow: after editing Rust source/tests, run `cargo fmt` before any `cargo fmt --check`/full verify command. Use `cargo fmt --check` first only when no Rust edits were made since the last format.

## Version bumps

Update all release metadata, then tag:
1. `Cargo.toml` — `version = "X.Y.Z"` and package name must be `srcwalk`.
2. `npm/package.json` — `"version": "X.Y.Z"` and package name must be `srcwalk`.
3. `skills/srcwalk/GUIDE.md` — full embedded agent guide printed by `srcwalk guide`; update it when command routing, workflows, examples, caveats, or agent-facing UX changes.
4. `skills/srcwalk/SKILL.md` — small bootstrap entry; update `compatible_srcwalk` only when the bootstrap contract changes (for example, first release requiring `srcwalk guide`). Do not duplicate the full guide here.
5. `CHANGELOG.md` — add a curated `## [X.Y.Z] - YYYY-MM-DD` section. GitHub Release body is extracted from this section.
6. `cargo update -p srcwalk` — refreshes `Cargo.lock`.
7. Tag `vX.Y.Z` → CI builds binaries, creates GitHub Release, publishes crates.io/npm.

## Release flow

```bash
# 1. Validate
git status --short
cargo fmt --check
cargo clippy -- -D warnings
cargo test

# 2. Bump version + changelog
# Cargo.toml, npm/package.json, GUIDE.md if agent-facing behavior changed,
# SKILL.md only if bootstrap compatibility changed, CHANGELOG.md, then:
cargo update -p srcwalk   # refreshes Cargo.lock
rg -n 'name = "srcwalk"|"name": "srcwalk"|version = "X.Y.Z"|"version": "X.Y.Z"|compatible_srcwalk: ">=X.Y.Z"|## \[X.Y.Z\]' \
  Cargo.toml npm/package.json Cargo.lock CHANGELOG.md skills/srcwalk/GUIDE.md skills/srcwalk/SKILL.md

# 3. Commit & push, wait for CI green
git add -A && git commit -m "chore: bump vX.Y.Z"
git push srcwalk main
gh run list --repo sting8k/srcwalk --branch main --limit 3
# Wait for CI ✅

# 4. Tag sanity: tag must not already exist and must point at current main
git fetch srcwalk --tags
git rev-parse -q --verify refs/tags/vX.Y.Z && echo "tag already exists; stop"
git tag vX.Y.Z main
git show vX.Y.Z:Cargo.toml | sed -n '1,20p'   # confirm name=srcwalk, version=X.Y.Z

# 5. Release (triggers build + publish)
git push srcwalk vX.Y.Z
gh run watch --repo sting8k/srcwalk $(gh run list --repo sting8k/srcwalk --workflow Release --limit 1 --json databaseId -q '.[0].databaseId') --exit-status

# 6. Post-release checks
gh release view vX.Y.Z --repo sting8k/srcwalk --json assets,body | jq
# Confirm release body came from CHANGELOG.md and assets are srcwalk-*.
```

## Key conventions

- **Split-on-touch**: modifying a mega-file (>800 LOC) >50 LOC? Split the affected concern first.
- **No speculative features**: every line traces to a request.
- **Tests**: in-source `#[cfg(test)]` modules + integration tests in `tests/`.
