# Exploration: Fase 3 — Incremental y watcher

## Current State

`codekurve index` (`crates/codekurve/src/commands.rs::index`) is the only indexing entrypoint today and is unconditionally a full reindex: `discovery::discover` walks the whole root, every file is re-parsed with no hash comparison at all, and `repo::reindex` (`crates/codekurve-store/src/repo.rs:117`) runs one SQLite transaction that `DELETE`s every `relationships`/`unresolved_references`/`symbols_fts`/`symbols`/`files` row for the project and re-inserts everything. Its own doc comment says "the index is disposable (§5.5): the previous generation is wiped and replaced." There is no `watch` command, no debounce, no per-file update/delete path — `main.rs`'s command list is `version|init|index|search|symbol|doctor|references|callers|callees|implementations|trace|impact`.

**Hashing**: BLAKE3 is not implemented anywhere. `repo.rs` uses `std::collections::hash_map::DefaultHasher` for `hash_id`/`config_hash`/`file_id`/`symbol_id`, with an existing inline comment: "ponytail: DefaultHasher is a placeholder; BLAKE3 replaces it when hashing lands in Phase 3." `commands.rs::snippet()` also has a ponytail comment explicitly deferring hash-based staleness to Phase 3. The codebase already self-documents this phase boundary correctly.

**Schema** (`migrations.rs`, `SCHEMA_VERSION=2`): `files` has no `content_hash`/`modified_ns` columns (master-plan §24.2's target schema has both). `generation` is a flat integer always `1` on full reindex, not a real counter. No `index_runs` table exists for §22/§30.2 metrics ("pending changes", "last full index", etc.).

**Dependencies**: only `ignore`, `rusqlite` (bundled), `tree-sitter`/`tree-sitter-typescript`, `serde`/`thiserror`/`toml`, `serde_json` are in any `Cargo.toml`. Master-plan §12's `blake3`, `notify`, `rayon`, `crossbeam-channel`, `clap`, `tokio` are all absent. CLI parsing is hand-rolled (explicit ponytail comment: "no clap while the surface is this small").

**SQLite config** (`db.rs`) already matches §24.1 exactly (WAL, foreign_keys, synchronous=NORMAL, busy_timeout). This plus the single-transaction reindex already gives free Ctrl+C safety for full reindex today (killed process → uncommitted txn rolled back on reopen) — but there's no explicit SIGINT handling, no cancelled-run reporting (§32), and per-file incremental transactions will need their own atomicity story once the watcher lands.

## Affected Areas

- `crates/codekurve-store/src/repo.rs` — delete-all/insert-all `reindex`; needs a per-file incremental update/delete path (§23.4/23.5); `hash_id`/`config_hash`/`file_id`/`symbol_id` need BLAKE3.
- `crates/codekurve-store/src/migrations.rs` — needs migration 0003+: `content_hash`, `modified_ns`, real `index_generation`/`index_runs`, pending/stale status fields.
- `crates/codekurve/src/commands.rs::index` / `main.rs` / `cli.rs` — `index` needs to become hash-diff-and-reparse-only; no `watch` subcommand exists yet; hand-rolled arg parser has no daemon-style command support.
- `crates/codekurve-analysis/src/discovery.rs` — whole-root walk only, reusable for reconcile-on-start but no single-file variant.
- `Cargo.toml` (workspace + 3 crates) — needs `blake3`, `notify`, a debounce mechanism (crossbeam-channel or hand-rolled timer map), and a `clap` decision.
- `crates/codekurve-core/src/config.rs` — no `[watch]` section (debounce ms, default 750 per §23.2).
- `docs/DATA_MODEL.md` — currently marked "Phase 0 design target, not implemented"; Phase 3 is when it becomes literally true.

## Approaches

1. **Standalone `codekurve watch` on `notify` + debounce, new per-file incremental repo function** — Pros: matches scope directly. Cons: doesn't by itself fix `codekurve index` still being full-reindex, so two "what changed" implementations risk diverging. Effort: Medium-High.
2. **Debounced full reindex on any change** — Pros: trivial, no schema change. Cons: explicitly violates the stated exit criterion ("modifying a file does NOT trigger a full reindex") and the §33.2 perf budget. Disqualified, not a real option.
3. **Shared incremental engine used by both `codekurve index` (reconcile-on-start / manual) and `codekurve watch`** — matches §23.3's explicit requirement that reconcile-on-start, manual `codekurve index`, and the watcher all validate "by metadata + hash" through one path. Effort: Medium-High, most design-up-front, smallest total diff for full exit-criteria compliance.

## Recommendation

Approach 3. It's the correct scoping of approach 1 — building the watcher as a thin `notify`+debounce wrapper that calls the same per-file update function the reconciliation loop uses avoids building two divergent "detect what changed" implementations, and directly satisfies §23.3's wording.

## Risks

- Migration 0003 is schema surgery on tables already used by Phase 1/2 code; must stay purely additive per the existing migrations.rs convention.
- Switching `hash_id`/`file_id`/`symbol_id`/`config_hash` from `DefaultHasher` to BLAKE3 changes every persisted ID value — safe given the index is documented as disposable/rebuildable, but should be an explicit proposal-phase decision.
- Per-file incremental transactions need a genuinely different "previous index preserved on failure" invariant than today's single whole-project rollback (per-file atomicity vs. one giant rollback) — needs explicit design and tests.
- No async runtime or thread-bridging primitives exist yet; `notify`'s callback thread needs an explicit queue into the synchronous single-writer SQLite connection.
- §23.6 rename handling is explicitly MVP-deferred (delete+create) — confirm this reduced scope is accepted, not silently dropped.
- Open questions for `sdd-propose`: (a) is `watch` foreground-blocking or daemon-mode? (b) how is pending/stale status surfaced beyond `codekurve status`? (c) is debounce grouping per-path or one shared window across a burst? (d) hand-rolled debounce vs. `notify-debouncer-mini`.

## Ready for Proposal

Yes — scope is well-bounded by the master plan and cross-checked against the actual code; remaining decisions are the four open questions plus final crate/schema choices.
