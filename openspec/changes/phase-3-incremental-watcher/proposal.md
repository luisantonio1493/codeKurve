# Proposal: Phase 3 — Incremental Indexing and Watcher

## Intent

`codekurve index` is always a full wipe-and-rebuild (`repo.rs::reindex`). Editing one file re-parses the whole repo, so the index is either expensive or stale, and nothing tells the user which. Phase 3 makes freshness cheap and honest: detect what actually changed, apply only that, and never claim freshness the tool cannot prove.

## Scope

### In Scope
- BLAKE3 content hashing; `files.content_hash` + `modified_ns` (migration 0003).
- Shared change-detection engine: mtime/size fast path, hash confirm, used by **both** `codekurve index` and `codekurve watch` (§23.3, one path only).
- Per-file create/update/delete applied in one transaction per batch (§23.4/23.5).
- `codekurve watch`: foreground-blocking, `notify` + debounce, reconcile on start.
- Freshness metadata (`pending_files`, `last_verified_at`) written **inside** the data transaction.
- `codekurve status` (§30.2) + one-line stderr stale warning on query commands.

### Out of Scope
- Rename detection — delete + create (§23.6 MVP deferral).
- Daemon/service mode, PID files (§ anti-goals: "no daemon before basic watcher").
- `rayon`, `tokio`, `clap`, staging-table run promotion (§22.3 alternative kept).

## Capabilities

### New Capabilities
- `incremental-index`: hashing, change detection, per-file apply/delete, freshness metadata, `status`.
- `file-watcher`: `watch` command, debounce, reconcile-on-start.

### Modified Capabilities
- `symbol-index`: `index` skips unchanged files; IDs move to BLAKE3.
- `relationship-graph`: re-resolution narrowed to the affected set; per-batch atomicity.
- `graph-queries`: stale warning on stderr; stdout/exit codes unchanged.

## Key Decisions

| Question | Decision | Rationale |
|---|---|---|
| watch mode | Foreground-blocking | Plan anti-goal forbids a daemon at this stage |
| stale surfacing | Stored counters, warn on stderr | No filesystem walk at query time; JSON stdout stays clean |
| debounce | Coalesce per path into one shared quiet window → one batch | §23.2 "agrupan por path" + "event storm se agrupa" |
| debouncer crate | Hand-rolled (`mpsc::recv_timeout` + set) | Not in §12; its rename correlation is out of scope |
| oversized batch | Above threshold, fall back to existing full reindex | Reuses code already present |
| BLAKE3 ID change | Schema bump forces one full rebuild | Index is disposable (§5.5) |

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `codekurve-store/src/repo.rs` | Modified | Per-file apply/delete; BLAKE3 IDs |
| `codekurve-store/src/migrations.rs` | Modified | Additive migration 0003 |
| `codekurve/src/commands.rs`, `main.rs` | Modified | `index` diffs; new `watch`, `status` |
| `codekurve-analysis/src/discovery.rs` | Modified | Single-path variant |
| `codekurve-core/src/config.rs` | Modified | `[index.watch]` |
| `Cargo.toml` | Modified | `blake3`, `notify` |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Incremental drift vs. full reindex | Med | Golden test: incremental result == full reindex result |
| Interrupted batch corrupts state | Low | Freshness written in the same transaction; interrupted files stay pending |
| Watcher misses OS events | Med | Reconcile on start + manual `index` |
| Cross-file resolution misses dependents | Med | Explicit dependent set (§23.4 step 7) |

## Rollback Plan

Revert the commits and delete `.codekurve/index.db`; `codekurve index` rebuilds under the prior schema. No external state.

## Dependencies

- `blake3`, `notify` (both listed in §12).

## Success Criteria

- [x] Modifying one file does not trigger a full reindex.
- [x] Deleting a file removes its symbols and relationships; inbound edges become unresolved.
- [x] A 50-file `git checkout` becomes one debounced batch.
- [x] Ctrl+C mid-batch leaves a consistent index with those files still pending.
- [x] A failed batch leaves the previous index intact.
- [x] `status` reports pending count and last verified time.
