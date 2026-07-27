# Delta for Relationship Graph

Phase 3 narrows re-resolution to the affected set for incremental batches instead of always re-resolving the whole project, moves atomicity from "one transaction per full index run" to "one transaction per batch" (full or incremental), and requires that deleting a symbol convert previously resolved inbound edges into unresolved references rather than leaving them dangling or silently dropped (proposal: "re-resolution narrowed to the affected set; per-batch atomicity").

## MODIFIED Requirements

### Requirement: Affected-Set Resolution for Incremental Batches

For an incremental batch, relationships MUST be re-resolved against the changed files' symbols plus every symbol whose existing relationships reference (or previously referenced) a changed or deleted symbol — the affected set — rather than the whole project's symbol table. A full reindex (whether run directly or as the oversized-batch fallback) MUST still resolve against the whole-project symbol table built after all files are parsed.

(Previously: relationships were always resolved against a whole-project symbol table built after all files are parsed, with no incremental/affected-set mode, because every index run was a full reindex.)

#### Scenario: Cross-file call resolution within a full reindex

- GIVEN `a.ts` calls a function exported from `b.ts`, both indexed in the same full reindex
- WHEN indexing completes
- THEN the `calls` edge resolves to `b.ts`'s symbol regardless of parse order

#### Scenario: Incremental batch re-resolves only affected dependents

- GIVEN `b.ts` exports a function called from `a.ts`, both previously indexed, and only `b.ts`'s export signature changes
- WHEN the incremental batch containing `b.ts` is applied
- THEN `a.ts`'s call edge into `b.ts` is re-resolved as part of the affected set, but unrelated files with no relationship to `b.ts`'s changed symbol are not re-resolved

### Requirement: Atomic Persistence Per Batch

One batch's relationships and unresolved references (whether the batch is a full reindex or an incremental per-file batch) MUST persist in a single atomic transaction; a failed or interrupted batch MUST leave no partial `relationships` or `unresolved_references` rows visible.

(Previously: atomicity was scoped to "one index run", which was always a full reindex; there was no smaller incremental-batch unit.)

#### Scenario: Atomic persistence on incremental batch failure

- GIVEN an incremental batch that fails mid-resolution
- WHEN the batch aborts
- THEN no partial `relationships` or `unresolved_references` rows from that batch are visible, and the previously committed graph state is unchanged

### Requirement: Unresolved Reference Handling Includes Delete-Induced Unresolution

A reference with zero resolution candidates, insufficient context to attempt resolution, **or whose previously resolved target symbol was deleted**, MUST be recorded in `unresolved_references` and never dropped silently (§18.3). A reference with one or more live candidates MUST NOT be recorded there.

(Previously: this requirement covered zero-candidate and insufficient-context references only; there was no delete path that could turn a resolved reference into an unresolved one, because deletion did not exist as an incremental operation.)

#### Scenario: Zero-candidate import

- GIVEN `import { Missing } from './nonexistent'`
- WHEN indexed
- THEN an `unresolved_references` row is created with `target_text = './nonexistent'`, `candidate_count = 0`, and a `reason`

#### Scenario: Deleting a symbol's file unresolves its inbound edges

- GIVEN `src/b.ts` has a `Resolved` `calls` edge into a function exported from `src/a.ts`
- WHEN `src/a.ts` is deleted and the batch is applied
- THEN the `Resolved` edge is removed and an `unresolved_references` row is created recording the now-missing target, not silently dropped and not left pointing at a nonexistent symbol row
