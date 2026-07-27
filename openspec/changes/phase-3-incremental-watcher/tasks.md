# Tasks: Phase 3 — Incremental Indexing and Watcher

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~1700–2000 |
| 400-line budget risk | High |
| Chained PRs recommended | Yes |
| Suggested split | PR1 → PR2 → PR3 → PR4 → PR5 → PR6 → PR7 |
| Delivery strategy | auto-forecast |
| Chain strategy | feature-branch-chain |

Decision needed before apply: No
Chained PRs recommended: Yes
Chain strategy: feature-branch-chain
400-line budget risk: High

### Suggested Work Units

| Unit | Goal | Likely PR | Focused test command | Runtime harness | Rollback boundary |
|------|------|-----------|----------------------|-----------------|-------------------|
| 1 | BLAKE3 hashing + migration 0003 schema (base=tracker) | PR1 | `cargo test -p codekurve-store migrations::` | `codekurve index` on fixture repo, inspect schema 3 | Revert repo.rs hash fns + migrations.rs 0003 |
| 2 | `symbol_key` 5-tuple + `signature_fingerprint` (base=PR1) | PR2 | `cargo test -p codekurve-store symbol_key` | `codekurve index` then `codekurve symbol <name>` unchanged output | Revert ir.rs/extract.rs/symbol.rs/repo.rs field + 2 test fixtures |
| 3 | Stable `rel`/`unr` row ids (base=PR2) | PR3 | `cargo test -p codekurve-store stable_id` | N/A — pure id-derivation unit scope | Revert `stable_id` helper + 2 persist fns |
| 4 | Affected-set resolution + `ProjectBaseline` (base=PR3) | PR4 | `cargo test -p codekurve-analysis resolve::` | `codekurve index` on 2-file cross-import fixture | Revert resolve_with + resolution_snapshot |
| 5 | `incremental.rs` detect/apply_batch engine (base=PR4) | PR5 | `cargo test -p codekurve incremental::` | Edit one file, run `codekurve index`, confirm only it reparses | Revert incremental.rs + index() rewiring |
| 6 | `codekurve watch` + debounce (base=PR5) | PR6 | `cargo test -p codekurve watch::` | `codekurve watch` in fixture repo, touch 3 files, confirm 1 batch | Revert watch.rs + CLI dispatch |
| 7 | `status`, stale warning, golden tests (base=PR6, merges to tracker) | PR7 | `cargo test -p codekurve --test incremental_golden` | `codekurve status` + `codekurve callers` after edit, check stderr warning | Revert status/warn_if_stale + golden test file |

## Phase 1: PR1 — BLAKE3 Foundation (req: incremental-index "Content Hash Tracked", symbol-index "Schema bump")

- [x] 1.1 Add `blake3 = "1"` to `crates/codekurve-store/Cargo.toml`.
- [x] 1.2 `repo.rs`: rewrite `hash_id`, `config_hash`, add `content_hash(bytes)`; drop `use std::hash::{Hash, Hasher}`.
- [x] 1.3 `migrations.rs`: add MIGRATION_0003 (`files.content_hash`/`modified_ns`, `index_state` table), `SCHEMA_VERSION = 3`, wipe DML.
- [x] 1.4 Extend `fresh_database_reaches_schema_version_2` → `_3`; assert `index_state` exists.
- [x] 1.5 Test: BLAKE3 id stability (same input → same id across calls).

## Phase 2: PR2 — symbol_key 5-Tuple + signature_fingerprint (req: symbol-index MODIFIED)

- [x] 2.1 `codekurve-analysis/src/ir.rs`: add `ExtractedSymbol.signature_fingerprint: String`.
- [x] 2.2 `extract.rs`: add `signature_fingerprint(node, source)` helper; wire into `push_named`.
- [x] 2.3 `codekurve-core/src/symbol.rs`: add `Symbol.signature_fingerprint: String`.
- [x] 2.4 Update `traverse.rs:373-386` and `repo.rs:592-604` test-helper `Symbol { .. }` literals with the new field.
- [x] 2.5 `repo.rs`: `symbol_key` takes 5 args, BLAKE3-hashed, `\x1f`-delimited.
- [x] 2.6 `commands.rs`: `build_file_inputs` copies field; `module_symbol` uses `String::new()`; `reindex` passes 5th arg.
- [x] 2.7 Test: reindex after signature edit changes `symbol_key`; blank-line edit does not (sibling of `symbol_key_excludes_start_byte`).

## Phase 3: PR3 — Stable Relationship/Unresolved Ids (req: relationship-graph "Atomic Persistence")

- [x] 3.1 `repo.rs`: add `stable_id(prefix, seen, parts)` helper.
- [x] 3.2 Rewrite `persist_relationships`/`persist_unresolved` to use it with the design's tuples.
- [x] 3.3 Test: same rows persisted in one batch vs. per-file batches yield identical id sets.

## Phase 4: PR4 — Affected-Set Resolution (req: relationship-graph "Affected-Set Resolution")

- [x] 4.1 `resolve.rs`: add `ProjectBaseline` + `resolve_with(files, aliases, baseline)`; `resolve()` delegates with `EMPTY`.
- [x] 4.2 `SymbolTable::build` seeds `by_name`/`exports` from baseline before folding fresh analyses.
- [x] 4.3 `repo.rs`: add `resolution_snapshot(conn, project_id)` (files/symbols/exports queries per design table).
- [x] 4.4 `commands.rs`: map store rows → `ProjectBaseline`.
- [x] 4.5 Add dependent-set queries: target-symbol lookup (`idx_relationships_target_kind`) and unresolved-target lookup (`idx_unresolved_project_target`).
- [x] 4.6 Test: incremental batch re-resolves only the affected dependent set, not unrelated files.

## Phase 5: PR5 — Incremental Engine (req: incremental-index core, symbol-index "Skips Unchanged")

- [x] 5.1 Create `crates/codekurve/src/incremental.rs`: `FileChange` enum, `detect()` (mtime/size fast path, hash confirm).
- [x] 5.2 `repo.rs`: per-file create/update primitives + delete cascade (own symbols/outbound edges removed, inbound edges → `unresolved_references`) + `index_state` upsert.
- [x] 5.3 `apply_batch()`: T1 (`pending_files` count) → parse+resolve `B ∪ D` (no DB writes) → T2 (per-file delete+reinsert, `pending_files=0`, `last_verified_at`), all per design's Batch Atomicity section.
- [x] 5.4 Oversized-batch fallback: `|B ∪ D| > full_reindex_threshold_pct` → call existing `reindex`.
- [x] 5.5 `commands.rs`: rewire `index()` to call `detect`/`apply_batch` instead of always-full reindex.
- [x] 5.6 Test: `detect` classifies touch-only/changed/deleted correctly (tempdir + in-memory DB).
- [x] 5.7 Test: failed batch rolls back fully, `pending_files` stays nonzero (mirrors `reindex_rolls_back_completely_on_relationship_error`).

## Phase 6: PR6 — Watch Command (req: file-watcher, all requirements)

- [x] 6.1 Add `notify = "6"` to `crates/codekurve/Cargo.toml`.
- [x] 6.2 Create `crates/codekurve/src/watch.rs`: notify setup + hand-rolled debounce loop per design's pseudocode.
- [x] 6.3 `codekurve-core/src/config.rs`: add `[index.watch]` (`debounce_ms=750`, `max_batch_wait_ms=5000`, `full_reindex_threshold_pct=25`).
- [x] 6.4 `cli.rs`: add `--debounce-ms` flag; `main.rs`: dispatch `watch` (reconcile-on-start via full-sweep `detect`, then event loop).
- [x] 6.5 Test: synthetic `mpsc` sender proves burst coalesces into one batch; `max_batch_wait` cap fires under continuous events.

## Phase 7: PR7 — Status, Stale Warning, Golden Tests (req: incremental-index "Status", graph-queries)

- [x] 7.1 `commands.rs`: add `status()` command (schema version, counts, `pending_files`, `last_verified_at`; `--json` via `print_envelope`).
- [x] 7.2 Add `warn_if_stale(conn, project_id)`; call from `require_indexed_project` plus one line each in `search`/`symbol`.
- [x] 7.3 `main.rs`: dispatch `status`.
- [x] 7.4 Integration test: golden incremental-result == full-reindex-result after a mixed create/update/delete batch.
- [x] 7.5 Integration test: deleting a file's cross-file callee converts the inbound edge to `unresolved_references`.
- [x] 7.6 Test: directory-level watch event (macOS FSEvents) still resolves via walk-intersection.
