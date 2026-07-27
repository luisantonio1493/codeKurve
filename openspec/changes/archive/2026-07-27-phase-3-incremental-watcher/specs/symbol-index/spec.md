# Delta for Symbol Index

Phase 3 changes how `symbol_key` identity is computed (BLAKE3 replaces the unspecified/`DefaultHasher` hash) and adds the requirement that `codekurve index` skip files the shared change-detection engine classifies as unchanged (proposal: "`index` skips unchanged files; IDs move to BLAKE3").

## MODIFIED Requirements

### Requirement: Stable Symbol Key Excludes Position and Uses BLAKE3

`symbol_key` MUST be the BLAKE3 hash of the tuple `(language, relative_path, symbol_kind, qualified_name, signature_fingerprint)` and MUST NOT include `start_byte` or any other line/byte position field (§16.3).

(Previously: `symbol_key` was "based on content identity" over the same tuple, with the hashing algorithm unspecified; the implementation used `DefaultHasher`.)

#### Scenario: Reindex after unrelated edit

- GIVEN a file is reindexed after an earlier line in the same file gained a blank line (shifting later symbols' `start_byte` but not their name, kind, or signature)
- WHEN reindexing completes
- THEN unaffected symbols keep the same BLAKE3-derived `symbol_key`, so relationship edges targeting them remain valid

#### Scenario: Rename changes identity

- GIVEN a symbol is renamed
- WHEN reindexed
- THEN its `symbol_key` changes (qualified_name changed, so the BLAKE3 input tuple changed), consistent with the accepted MVP limitation that renames are treated as a new identity

#### Scenario: Schema bump forces one full rebuild

- GIVEN a project last indexed under the pre-BLAKE3 ID scheme
- WHEN `codekurve index` first runs after upgrading
- THEN all symbol/file/config IDs are recomputed with BLAKE3 in one full reindex (the index is disposable per §5.5; there is no incremental migration of old IDs)

## ADDED Requirements

### Requirement: Index Skips Files Classified Unchanged

`codekurve index` MUST consult the shared change-detection engine (defined by the `incremental-index` capability) before reparsing a file, and MUST NOT reparse or rewrite the symbols of a file classified unchanged.

#### Scenario: Single-file edit does not trigger a full reindex

- GIVEN a previously indexed project of 100 files
- WHEN one file's content changes and `codekurve index` runs
- THEN only that file's symbols are reparsed and rewritten; the other 99 files' symbols are untouched
