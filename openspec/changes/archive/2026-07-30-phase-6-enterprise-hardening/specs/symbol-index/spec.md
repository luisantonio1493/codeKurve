# Delta Spec: symbol-index — Phase 6

## ADDED Requirements (Phase 6)

### Requirement: Discovery Enforces a Configurable Total-File Limit

`[index]` MUST accept a `max_total_files: u64` field (new, alongside the existing `max_file_size_bytes` and `follow_symlinks`), defaulting to a fixed built-in ceiling. When discovery finds more eligible files (post-ignore, post-language-filter) than `max_total_files`, `codekurve index`/`reindex` MUST hard-fail before writing any symbol/file rows for that run: it MUST NOT truncate the file set, MUST NOT produce a partial index, and MUST return a clear error naming the configured limit and the discovered count. A value of `0` MUST be treated as "unset/unlimited" (disables the check), matching the additive `#[serde(default)]` pattern already used by `[index.watch]` and `[mcp]` so existing config files without this field keep working unchanged.

#### Scenario: Discovered file count is under the limit

- GIVEN `max_total_files = 1000` and a project with 500 eligible files
- WHEN `codekurve index` runs
- THEN discovery proceeds normally and all 500 files are indexed

#### Scenario: Discovered file count exactly equals the limit

- GIVEN `max_total_files = 500` and a project with exactly 500 eligible files
- WHEN `codekurve index` runs
- THEN discovery proceeds normally (the limit is inclusive) and all 500 files are indexed

#### Scenario: Discovered file count exceeds the limit

- GIVEN `max_total_files = 500` and a project with 501 eligible files
- WHEN `codekurve index` runs
- THEN the run hard-fails before any symbol or file row is written, the error names both the configured limit (500) and the discovered count (501), and the previous index state (if any) is left untouched

#### Scenario: Limit disabled via zero

- GIVEN `max_total_files = 0`
- WHEN `codekurve index` runs against a project of any size
- THEN no total-file check is applied and discovery proceeds as it did before this requirement existed

#### Scenario: Existing config files parse unchanged

- GIVEN a config file written before this requirement existed, with no `max_total_files` key under `[index]`
- WHEN the config is loaded
- THEN it parses successfully and `max_total_files` takes the built-in default value
