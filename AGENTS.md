# srcwalk

Code-intelligence CLI for AI agents. It provides target-first file reads,
structural discovery, caller/callee navigation, dependency evidence, context and
review packets, and token-aware output.

## Product North Star

srcwalk exists to maximize accurate, evidence-based links between knowledge
components in a source or artifact tree, so agents can reduce manual navigation
steps.

Priority order:

1. Accuracy and semantic correctness.
2. Avoiding confusion, guessing, and false conclusions.
3. Reducing agent steps through structured navigation.
4. Token/output efficiency.

Do not trade correctness for prettier or shorter output. When semantic
confidence is low, label the output honestly rather than implying source-level
truth.

## Source Of Truth

Read in this order:

1. `README.md` for user-facing behavior and install/use examples.
2. `skills/srcwalk/GUIDE.md` for agent-facing command routing behavior embedded
   in the distributed skill/binary.
3. `skills/srcwalk/SKILL.md` for the small bootstrap skill contract.
4. The relevant source files and tests.

## Build And Test

Common commands:

```bash
cargo build --release
cargo test --locked
cargo clippy -- -D warnings
cargo fmt --check
cargo install --path .       # -> ~/.cargo/bin/srcwalk
```

Formatting workflow: after editing Rust source or tests, run `cargo fmt` before
`cargo fmt --check` or full verification. Use `cargo fmt --check` first only when
no Rust edits were made since the last format.

Windows is part of the required verification surface. When touching path
parsing/display, traversal, deps/callers output, artifact/file matching,
npm/release binary behavior, or other platform-sensitive code, verify the
affected behavior on Windows too.

## Implementation Guardrails

- No speculative features: every changed line must trace to a request.
- Keep output evidence-first. Do not imply stronger semantic confidence than the
  parser, artifact source, or command evidence supports.
- Keep `skills/srcwalk/GUIDE.md`, `skills/srcwalk/SKILL.md`, README examples,
  and CLI behavior aligned when command routing, examples, caveats, or
  agent-facing UX changes.
- Tests live in in-source `#[cfg(test)]` modules and integration tests in
  `tests/`.

## Release Metadata

For version bumps, keep these files in sync:

1. `Cargo.toml`
2. `npm/package.json`
3. `skills/srcwalk/GUIDE.md` when agent-facing behavior changes
4. `skills/srcwalk/SKILL.md` only when the bootstrap compatibility contract
   changes
5. `CHANGELOG.md`
6. `Cargo.lock` via `cargo update -p srcwalk`
