# srcwalk

Code-intelligence CLI built on tree-sitter. Outlines, symbol search, caller/callee graphs, deps, maps — structured, token-efficient output for AI agents.

## Project structure

```
src/
  main.rs              CLI entry (clap). Dispatches to map, callees, or single-query.
  lib.rs               Public API: classify → read/search/glob → formatted output.
  classify.rs          Query type detection (file path, glob, symbol, content).
  types.rs             Shared types (QueryType, Lang, OutlineEntry, etc.).
  error.rs             Error types with exit codes.
  format.rs            Output formatting helpers.
  budget.rs            Token budget enforcement.
  map.rs               Codebase map generation.
  overview.rs          Codebase fingerprinting.
  cache.rs             OutlineCache — DashMap of path → (mtime, outline).
  session.rs           Session state (expanded definition dedup).
  lang/
    mod.rs             detect_file_type(), package_root().
    outline.rs         Tree-sitter outline extraction.
    treesitter.rs      DEFINITION_KINDS, extract_definition_name().
    detection.rs       Generated/binary file detection.
  read/
    mod.rs             Smart file reading (full vs outline by token count).
    imports.rs         Import extraction for deps.
    outline/           Code, markdown, structured, tabular, fallback outlines.
  search/
    mod.rs             Search orchestration.
    symbol.rs          AST-based symbol search (definitions first).
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
skills/srcwalk/        Agent skill — full command reference.
benchmark/             Evaluation harness (26 tasks, 4 repos).
```

## Languages

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift, Elixir.

## Build & test

```bash
cargo build --release
cargo test
cargo clippy -- -D warnings
cargo fmt --check
cargo install --path .       # → ~/.cargo/bin/srcwalk
```

## Version bumps

Update all three, then tag:
1. `Cargo.toml` — `version = "X.Y.Z"`
2. `npm/package.json` — `"version": "X.Y.Z"`
3. `cargo update -p srcwalk` — refreshes `Cargo.lock`
4. Tag `vX.Y.Z` → CI builds binaries for 5 platforms + creates GitHub Release.

## Release flow

```bash
# 1. Validate
cargo test
cargo clippy -- -D warnings
cargo fmt --check

# 2. Bump version (all three)
# Cargo.toml, npm/package.json, then:
cargo update -p srcwalk

# 3. Commit & push, wait for CI green
git add -A && git commit -m "chore: bump vX.Y.Z"
git push srcwalk main
# Wait for CI ✅

# 4. Tag & release (triggers build + publish)
git tag vX.Y.Z && git push srcwalk vX.Y.Z
```

## Key conventions

- **Split-on-touch**: modifying a mega-file (>800 LOC) >50 LOC? Split the affected concern first.
- **No speculative features**: every line traces to a request.
- **Tests**: in-source `#[cfg(test)]` modules + integration tests in `tests/`.
