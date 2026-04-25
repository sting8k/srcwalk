# Changelog

All notable changes to srcwalk are documented here.

## [Unreleased]

### Added

### Changed

### Fixed

## [0.2.0] - 2026-04-25

### Added
- General search filters: `--filter 'path:TEXT file:TEXT text:TEXT kind:fn'` now narrow normal symbol/content search results.
- Caller classification filters: `--filter 'args:N receiver:NAME caller:NAME path:TEXT text:TEXT'` narrow direct `--callers` rows.
- Caller aggregation: `--count-by args|caller|receiver|path|file` groups direct call sites into semantic `[group] field=value count=N` rows.

### Changed
- Caller outputs now show compact callsite facts (`recv=`, `args=`) and contextual tips only when useful.
- Caller `--count-by` output is paginated for large group sets and emits continuation hints.
- README and srcwalk skill examples now document callsite classification and general path filtering.

### Fixed
- `--count-by` with zero matches now returns the standard no-callers diagnostic instead of an empty grouping header.
- Caller-only filter qualifiers (`args:`, `receiver:`, `caller:`) now fail clearly when used outside `--callers`.

### Examples
```bash
srcwalk Depends --filter 'path:param_functions' --scope .
srcwalk decompileFunction --callers --count-by args --scope .
srcwalk decompileFunction --callers --filter 'args:2' --scope .
```

## [0.1.9] - 2026-04-24

### Changed
- Maintenance release before caller classification and general filtering work.
