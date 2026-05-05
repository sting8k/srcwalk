---
name: srcwalk
compatible_srcwalk: ">=0.3.0"
description: "Srcwalk is the agent's code navigator: one tree-sitter CLI for repo maps, token-aware large-file reads, symbol search, callers/callees, deps, impact checks, and precise drill-ins. Use it before raw reads or grep for code-structure work. Run `srcwalk guide` first; it is the installed binary's source of truth."
---

# srcwalk — bootstrap entry

Default to srcwalk for code navigation, large-file reading, repo maps, symbols, callers/callees, deps, and impact checks. Use raw reads or broad grep first only for pure text/path matching.

Before non-trivial use, run:

```bash
srcwalk guide
```

Do not pipe, truncate, summarize, or sample `srcwalk guide`; later sections contain important routing rules and caveats.

Use root/command help only for flags:

```bash
srcwalk --help
srcwalk <command> --help
```
