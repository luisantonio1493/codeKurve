# Symbol Index Specification

## Overview

The symbol index captures all symbols (functions, classes, interfaces, variables, etc.) from TypeScript/JavaScript source code, with real qualified names and stable identities across reindexing. Phase 2 updates Phase 1's placeholder qualified names and position-based key with real hierarchical scope and position-independent stability.

## MODIFIED Requirements (Phase 2)

### Requirement: Qualified Name Reflects Real Scope

Every indexed symbol's `qualified_name` MUST reflect its real hierarchical scope per §20.3 (`relative_path::Owner.member` for nested symbols, `relative_path::name` for top-level symbols).

(Previously: `qualified_name` was a Phase 1 placeholder equal to `name`, with no path or scope information.)

#### Scenario: Top-level function

- GIVEN `src/utils.ts` defines `export function formatDate() {}`
- WHEN indexed
- THEN the symbol's `qualified_name` is `src/utils.ts::formatDate`

#### Scenario: Nested class method

- GIVEN `src/services/member.service.ts` defines `class MemberService { getEligibility() {} }`
- WHEN indexed
- THEN the method's `qualified_name` is `src/services/member.service.ts::MemberService.getEligibility`

### Requirement: Stable Symbol Key Excludes Position and Uses BLAKE3

`symbol_key` MUST be the BLAKE3 hash of the tuple `(language, relative_path, symbol_kind, qualified_name, signature_fingerprint)` and MUST NOT include `start_byte` or any other line/byte position field (§16.3).

(Previously: Phase 2 said `symbol_key` was "based on content identity" over the same tuple, with the hashing algorithm unspecified; the implementation used `DefaultHasher`. Phase 3 replaces this with explicit BLAKE3 hashing.)

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

### Requirement: Schema Version Reflects Migration 0002

`doctor` and `index_runs` MUST report `SCHEMA_VERSION = 2` once migration 0002 has applied, and the `relationships`/`unresolved_references` tables MUST exist alongside the unmodified 0001 tables.

(Previously: `SCHEMA_VERSION` was 1, with only `projects`, `files`, `symbols`, `index_runs`, and `diagnostics`.)

#### Scenario: Doctor reports post-migration state

- GIVEN a project indexed after migration 0002 applied
- WHEN `codekurve doctor` runs
- THEN it reports schema version 2 and confirms FTS5 support unaffected

## ADDED Requirements (Phase 3)

### Requirement: Index Skips Files Classified Unchanged

`codekurve index` MUST consult the shared change-detection engine (defined by the `incremental-index` capability) before reparsing a file, and MUST NOT reparse or rewrite the symbols of a file classified unchanged.

#### Scenario: Single-file edit does not trigger a full reindex

- GIVEN a previously indexed project of 100 files
- WHEN one file's content changes and `codekurve index` runs
- THEN only that file's symbols are reparsed and rewritten; the other 99 files' symbols are untouched
