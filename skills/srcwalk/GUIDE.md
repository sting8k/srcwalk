# srcwalk — agent routing policy

Use srcwalk before shell search for code navigation. Route by task. Keep `--scope` narrow. Use raw `rg` only for last-mile text confirmation.

## Routes

Do not start orientation with shell `tree`, shell `find`, repeated `ls`, or repo-wide `rg`.

```bash
srcwalk map --scope <dir>
```

Use `srcwalk find` for symbols, usages, text, and symbol/name globs.

```bash
srcwalk find <query> --scope <dir>
srcwalk find "A, B, C" --scope src --scope tests
srcwalk find '*Controller' --scope <dir>
srcwalk find 'displayAjax{Update,Refresh}*' --scope <dir> --filter kind:fn
```

Use `srcwalk files` for project file discovery by filename/glob. Do not use shell `find`/`fd` for codebase navigation. Do not use `srcwalk find` as filename search.

```bash
srcwalk files '<filename>' --scope <dir>
srcwalk files '**/*.<ext>' --scope <dir>
srcwalk files '*<name>*' --scope <dir>
```

Use shell `find`/`fd` only for filesystem metadata that srcwalk does not model: permissions, mtimes, empty dirs, symlinks, binary assets, generated outputs, or cleanup candidate lists.

```bash
# filesystem metadata / cleanup inventory
find <dir> -type f -mtime -1
find <dir> -empty
fd -HI -t f -x stat
```

Do not infer definitions, usages, callers, deps, or code paths from shell path lists. Do not convert identifiers into paths without evidence.

```bash
srcwalk find '<identifier>' --scope <dir>
srcwalk files '*<name>*' --scope <dir>
```

Use `callers` for upstream call sites. Do not grep `foo(`.

```bash
srcwalk callers <symbol> --scope <dir>
srcwalk callers <symbol> --depth 2 --scope <dir>
srcwalk callers <symbol> --count-by receiver --scope <dir>
```

Use `callees` for downstream calls.

```bash
srcwalk callees <symbol> --scope <dir>
srcwalk callees <symbol> --detailed --scope <dir>
srcwalk callees <symbol> --depth 2 --scope <dir>
```

Use `deps` for file-level imports, local symbol deps, and dependents. Run it before editing, moving, deleting, or explaining cross-file coupling for a file. Do not grep import/use/require.

```bash
srcwalk deps <file>
```

Use `flow` for a quick bidirectional slice around a symbol before deep-diving. It combines nearby upstream/downstream evidence; verify details with callers/callees/deps/path reads.

```bash
srcwalk flow <symbol> --scope <dir>
```

Use `impact` before changing, removing, renaming, or publicizing a symbol. It is a fast blast-radius scan; verify broad or risky results with callers/deps.

```bash
srcwalk impact <symbol> --scope <dir>
```

Use path reads only after srcwalk gives a path/line/range or you already know the target.

```bash
srcwalk <path>
srcwalk <path>:123-150
srcwalk <path> --section <symbol>
```


## Replace shell chains

One srcwalk command should replace many navigation commands.

```bash
# instead of shell tree/find/fd/ls/rg to understand a dir
srcwalk map --scope <dir>

# instead of shell find/fd for project filenames
srcwalk files '<glob>' --scope <dir>

# instead of head/cat/sed for preview
srcwalk <path>:1-50

# instead of rg Foo + open many files + guess definition/usages
srcwalk find Foo --scope <dir>

# instead of rg 'Foo(' + manual filtering
srcwalk callers Foo --scope <dir>

# instead of rg import/use/require
srcwalk deps <file>
```

## Map rules

Start with auto depth. Do not pass `--depth` first.

```bash
srcwalk map --scope <dir>
```

Explicit `--depth N` is strict.

`[relations]` are static local dependency groups, not runtime calls.

```txt
[relations] 27 groups
search deps:38
  -> (root) deps:30
```

`[outbound deps]` means the scope imports targets outside `--scope`.

```txt
[outbound deps] 8 groups (targets outside scope)
examples/custom-provider deps:8
  -> sdk/cliproxy deps:3
```

## Find rules

`srcwalk find` only searches inside `--scope`. A narrow scope can hide definitions.

`--filter kind:<label>` is exact. Common labels: `fn`, `class`, `mod`, `impl`, `base`, `usage`, `text`, `comment`. `kind:fn` matches function definitions; `kind:function` does not match. Zero matches can mean scope miss or wrong exact label.

```bash
srcwalk find TranslateRequest --scope internal/runtime/executor --filter kind:fn
# 0 matches can mean the definition is outside this scope.

srcwalk find TranslateRequest --scope .
```

Use `flow` first when you need a quick symbol slice, then use `callers`/`callees` for exact edges. Use `deps` when file coupling/imports/dependents matter. Use `impact` before edits that may break callers or public API.

## Artifact routes

Use `--artifact` for generated/bundled/minified JS/TS, vendor `dist` bundles, or when a JS/TS read says `minified artifact?`. Artifact mode gives byte-span evidence instead of useless line-1 ranges.

```bash
# JS/TS bundles
srcwalk map --artifact --scope <dir>
srcwalk find <query> --artifact --scope <dir>
srcwalk flow <symbol> --artifact --scope <dir>
srcwalk impact <symbol> --artifact --scope <dir>
srcwalk callers <symbol> --artifact --expand=1 --scope <dir>
srcwalk callees <symbol> --artifact --detailed --scope <dir>
srcwalk <path> --artifact
srcwalk <path> --artifact --section <symbol>
srcwalk <path> --artifact --section bytes:<start>-<end>

```

## Escalation

1. Orientation: `srcwalk map --scope <dir>`.
2. Symbol/text: `srcwalk find <query> --scope <dir>`.
3. Filenames: `srcwalk files '<glob>' --scope <dir>`.
4. Quick symbol slice: `srcwalk flow <symbol> --scope <dir>`.
5. Upstream exact edges: `srcwalk callers <symbol> --scope <dir>`.
6. Downstream exact edges: `srcwalk callees <symbol> --detailed --scope <dir>`.
7. File coupling/blast radius: `srcwalk deps <file>`.
8. Pre-edit symbol blast radius: `srcwalk impact <symbol> --scope <dir>`.
9. Evidence: `srcwalk <path>:<line|start-end>`, `srcwalk <path> --section <symbol>`, or artifact byte evidence with `srcwalk <path> --artifact --section bytes:<start>-<end>`.
10. Raw text confirmation: `rg`.

## Supported structural languages

Rust, TypeScript, TSX, JavaScript, Python, Go, Java, Scala, C, C++, Ruby, PHP, C#, Swift, Elixir, and Kotlin. Unsupported languages still work for reading files, but structural facts may be unavailable.
