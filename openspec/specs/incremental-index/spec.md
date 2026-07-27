# Incremental Index Specification

## Purpose

Replace `codekurve index`'s always-full wipe-and-rebuild with change detection: hash and mtime tracking per file, a single shared engine that decides what changed, per-file create/update/delete applied atomically per batch, and freshness metadata (`pending_files`, `last_verified_at`) the tool can honestly report (proposal §Intent, §23.3-23.5).

## Requirements

### Requirement: Content Hash and Modification Time Tracked Per File

Migration 0003 MUST add `content_hash` (BLAKE3 digest) and `modified_ns` (filesystem mtime, nanosecond precision) columns to `files`, additively, without altering any 0001/0002 table.

#### Scenario: Fresh database migration

- GIVEN a database at schema version 2
- WHEN `codekurve index` runs
- THEN migration 0003 applies, `files` gains `content_hash` and `modified_ns`, and every existing 0001/0002 table is unchanged

#### Scenario: Column populated on insert

- GIVEN a new file discovered during indexing
- WHEN it is inserted into `files`
- THEN `content_hash` holds its BLAKE3 digest and `modified_ns` holds its filesystem mtime at read time

### Requirement: Shared Change Detection Engine

`codekurve index` and `codekurve watch` MUST use one shared change-detection engine, not two independent implementations. The engine MUST first compare `modified_ns` (and size) as a fast path, and confirm any apparent change by recomputing the BLAKE3 `content_hash` before treating the file as changed.

#### Scenario: Fast path short-circuits unchanged file

- GIVEN a file whose `modified_ns` on disk matches the stored value
- WHEN the change-detection engine evaluates it
- THEN the file is classified unchanged without recomputing its hash

#### Scenario: Mtime touch without content change is not a false positive

- GIVEN a file whose mtime changed (e.g. `touch`) but whose content is byte-identical
- WHEN the change-detection engine evaluates it
- THEN the hash confirms no content change and the file is classified unchanged, not queued for reparse

### Requirement: Unchanged Files Skip Reparsing

`codekurve index` MUST NOT reparse or re-resolve a file the change-detection engine classifies as unchanged. Editing exactly one file MUST NOT cause every other file in the project to be reparsed.

#### Scenario: Single-file edit does not trigger full reindex

- GIVEN a previously indexed project of 100 files
- WHEN one file's content changes and `codekurve index` runs
- THEN only that file (and files whose relationships depend on it, per the relationship-graph delta) are reparsed/re-resolved; the other 98 unaffected files are neither reparsed nor rewritten

### Requirement: Per-File Create and Update Applied Atomically Per Batch

A batch of detected file changes (creates and updates) MUST be applied as one SQLite transaction per batch: parse each changed file, replace its symbols, and update its `content_hash`/`modified_ns`, all within that single transaction.

#### Scenario: Batch of updates commits together

- GIVEN three files changed since the last index
- WHEN the batch is applied
- THEN all three files' symbols and metadata are updated in one transaction; no partial subset of the three is visible before commit

### Requirement: Per-File Delete Removes Symbols and Converts Inbound Edges to Unresolved

Deleting a tracked file MUST remove its `files` row and all symbols it owned, MUST remove any `relationships` rows where the deleted symbols are the source, and MUST convert `relationships` rows where a deleted symbol was the resolved target into `unresolved_references` rows (never silently dropped), within the same batch transaction as the delete.

#### Scenario: Deleted file's own symbols and outbound edges disappear

- GIVEN `src/a.ts` defines a function that calls a function in `src/b.ts`
- WHEN `src/a.ts` is deleted and the batch is applied
- THEN `src/a.ts`'s symbols and its outbound `calls` edge are removed

#### Scenario: Inbound edges to the deleted file become unresolved

- GIVEN `src/b.ts` calls a function exported from `src/a.ts`
- WHEN `src/a.ts` is deleted and the batch is applied
- THEN the previously `Resolved` `calls` edge from `src/b.ts` is removed and replaced by an `unresolved_references` row recording the unresolved target, not silently dropped

### Requirement: Batch Interruption Leaves the Index Consistent With Interrupted Files Still Pending

If the process is interrupted (e.g. Ctrl+C) while a batch transaction is in progress, the transaction MUST NOT be partially committed; on restart, files that were part of the interrupted batch MUST still be reported as pending by the change-detection engine.

#### Scenario: Ctrl+C mid-batch

- GIVEN a batch of 10 changed files is being applied
- WHEN the process is interrupted after 4 files are processed but before the transaction commits
- THEN the previous index state (before the batch started) is intact, and all 10 files are still classified as changed/pending on the next run

### Requirement: A Failed Batch Preserves the Previous Index

If applying a batch fails (parse error treated as batch failure, I/O error, constraint violation), the transaction MUST roll back in full; the index MUST remain exactly as it was before the batch was attempted, and the affected files MUST remain pending.

#### Scenario: Batch failure rolls back cleanly

- GIVEN a batch where one file's processing raises an unrecoverable error
- WHEN the batch fails
- THEN none of the batch's file changes are visible in the index, the previous index generation remains queryable, and the batch's files remain pending for the next run

### Requirement: Freshness Metadata Written Inside the Data Transaction

`pending_files` (the set/count of files known to need reprocessing) and `last_verified_at` (the timestamp change detection last ran) MUST be written in the same transaction as the batch's data changes, not in a separate transaction or after commit.

#### Scenario: Freshness metadata matches committed data

- GIVEN a batch that successfully commits
- WHEN the transaction completes
- THEN `pending_files` and `last_verified_at` reflect exactly that batch's outcome; there is no window where committed symbol data and freshness metadata disagree

### Requirement: Status Command Reports Pending Count and Last Verified Time

`codekurve status` MUST report the current count of pending files and the timestamp of the last successful freshness verification, read from stored metadata without performing a filesystem walk.

#### Scenario: Status after clean index

- GIVEN a project just fully indexed with no outstanding changes
- WHEN `codekurve status` runs
- THEN it reports a pending count of 0 and the `last_verified_at` timestamp of that index run

#### Scenario: Status with pending changes

- GIVEN 3 files changed on disk since the last index/watch run and not yet applied
- WHEN `codekurve status` runs
- THEN it reports a pending count of 3 and the `last_verified_at` timestamp of the last time change detection ran

### Requirement: Oversized Batch Falls Back to Full Reindex

If a detected batch of changes exceeds a configured size threshold, the engine MUST fall back to the existing full reindex path instead of applying an oversized per-file batch.

#### Scenario: Large batch triggers full reindex

- GIVEN a batch whose changed-file count exceeds the configured threshold
- WHEN the batch is evaluated
- THEN the engine performs a full reindex instead of a per-file incremental apply

### Requirement: Renames Are Not Detected

A file rename MUST be processed as a delete of the old path followed by a create of the new path; the engine MUST NOT attempt content-similarity rename correlation.

#### Scenario: Renamed file processed as delete+create

- GIVEN `src/old.ts` is renamed to `src/new.ts` with identical content
- WHEN the batch is applied
- THEN `src/old.ts`'s symbols are removed (with inbound edges converted to unresolved per the delete requirement) and `src/new.ts`'s symbols are created as new identities
