# Delta for Symbol Index

No prior `openspec/specs/symbol-index/spec.md` exists yet (Phase 1 shipped ahead of formal specs). These MODIFIED blocks become the symbol-index baseline at archive time; they supersede the Phase 1 placeholder behavior described in `(Previously: ...)` notes below.

## MODIFIED Requirements

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

### Requirement: Stable Symbol Key Excludes Position

`symbol_key` MUST be the BLAKE3 hash of the tuple `(language, relative_path, symbol_kind, qualified_name, signature_fingerprint)` and MUST NOT include `start_byte` or any other line/byte position field (§16.3).

(Previously: Phase 1's `symbol_key` embedded `start_byte`, so any edit that shifted a symbol's byte offset changed its identity even when nothing else about it changed.)

#### Scenario: Reindex after unrelated edit

- GIVEN a file is reindexed after an earlier line in the same file gained a blank line (shifting later symbols' `start_byte` but not their name, kind, or signature)
- WHEN reindexing completes
- THEN unaffected symbols keep the same `symbol_key`, so relationship edges targeting them remain valid

#### Scenario: Rename changes identity

- GIVEN a symbol is renamed
- WHEN reindexed
- THEN its `symbol_key` changes (qualified_name changed), consistent with the accepted MVP limitation that renames are treated as a new identity

### Requirement: Schema Version Reflects Migration 0002

`doctor` and `index_runs` MUST report `SCHEMA_VERSION = 2` once migration 0002 has applied, and the `relationships`/`unresolved_references` tables MUST exist alongside the unmodified 0001 tables.

(Previously: `SCHEMA_VERSION` was 1, with only `projects`, `files`, `symbols`, `index_runs`, and `diagnostics`.)

#### Scenario: Doctor reports post-migration state

- GIVEN a project indexed after migration 0002 applied
- WHEN `codekurve doctor` runs
- THEN it reports schema version 2 and confirms FTS5 support unaffected
