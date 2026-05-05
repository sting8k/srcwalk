---
name: srcwalk
compatible_srcwalk: ">=0.2.8"
description: "Bootstrap entry for srcwalk: a tree-sitter code-intelligence CLI for token-aware code reading, repo maps, symbol search, callers/callees, dependencies, and precise drill-ins. Run `srcwalk guide` immediately and treat the embedded guide from the installed binary as the source of truth."
---

# srcwalk — bootstrap entry

srcwalk is a tree-sitter-backed code-intelligence CLI for agents. It helps you inspect large or unfamiliar codebases with token-aware file reads, structural outlines, repo maps, symbol definitions/usages, caller/callee graphs, dependency views, and precise line/section drill-ins.

Use it when the user asks where code lives, what calls what, how a file/module fits together, what a large file contains, or how to navigate an unfamiliar repository.

Before using srcwalk beyond a trivial command, run this immediately:

```bash
srcwalk guide
```

Follow the embedded guide it prints. It is version-matched with the installed binary and contains the full routing policy, workflows, examples, caveats, and current command behavior.

For root help and command-specific flags, use:

```bash
srcwalk --help
srcwalk <command> --help
```
