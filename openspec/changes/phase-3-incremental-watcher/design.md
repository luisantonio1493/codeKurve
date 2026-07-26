# Design: Phase 3 — Incremental Indexing and Watcher

## Technical Approach

One change-detection function, one apply function, both in the binary crate (`crates/codekurve/src/incremental.rs`) — the existing composition root that already glues `discovery` + `extract` + `resolve` + `repo`. `index` calls them once; `watch` calls them per debounced batch. No new crate, no second "what changed" implementation.

Detection reuses `discovery::discover` verbatim (exact ignore semantics, one walk per batch, walking is cheap — parsing is not). A watcher batch never re-parses the whole repo; it intersects the walk with the pending path set.

## Architecture Decisions

| Decision | Choice | Rejected | Rationale |
|---|---|---|---|
| Engine location | `codekurve/src/incremental.rs` (bin crate) | New `codekurve-incremental` crate; put it in `-store` | Needs fs + analysis + store; only the composition root may depend on all three |
| Per-path eligibility | Re-run `discovery::discover` once per batch, intersect with pending set | Per-path `Gitignore` matcher | A second matcher duplicates ignore semantics and misses nested `.gitignore`; a walk is ~ms |
| Pending truth | Authoritative signal is `content_hash`/`modified_ns` mismatch; `pending_files` is a cached counter | Pending-path table | Counter is display-only; the next sweep re-derives reality, so no table can drift |
| Debounce | Sliding quiet window + hard `max_batch_wait` cap | Pure sliding window | Pure sliding starves under a continuously-writing process |
| Baseline for re-resolution | Read stored symbols/exports back as a `ProjectBaseline`, re-parse only the affected set | Re-parse whole project; keep whole-project cache | Reading rows is far cheaper than tree-sitter; keeps `resolve()` a single code path |
| Relationship + unresolved ids | Derive from the row's own content + ordinal-in-group | Keep batch-vector index `i` | Today's `i` is the position in the whole-run vector; under batching the same row would get different ids per batch. Both `persist_relationships` (`repo.rs:238`) and `persist_unresolved` (`repo.rs:271`) have this bug |
| `signature_fingerprint` source | Whitespace-normalized declaration text of `type_parameters` + `parameters` + `return_type`, captured at extraction | A semantic type model; storing a parsed signature AST | The spec only needs a value that changes iff the signature changed; the parser already has the nodes at `push_named` |
| BLAKE3 rollout | `SCHEMA_VERSION = 3`; migration 0003 deletes all project data | Silent coexistence; version column on ids | Every id changes; deleting forces the existing full-reindex path exactly once |

## Migration 0003

Follows the numbered `if current < N { tx … INSERT schema_migrations … }` block in `migrations.rs`. Additive; `ADD COLUMN` is used (0002 avoided ALTER, but nullable `ADD COLUMN` is O(1) in SQLite and rebuilding `files` is strictly worse).

```sql
ALTER TABLE files ADD COLUMN content_hash TEXT;   -- BLAKE3 hex, NULL = never hashed
ALTER TABLE files ADD COLUMN modified_ns INTEGER; -- mtime, ns since epoch

CREATE TABLE index_state (
    project_id TEXT PRIMARY KEY,
    pending_files INTEGER NOT NULL DEFAULT 0,
    last_verified_at TEXT,
    updated_at TEXT NOT NULL
);

-- IDs move to BLAKE3 (from DefaultHasher) and symbol_key gains
-- signature_fingerprint: every stored id and key is invalid.
DELETE FROM relationships; DELETE FROM unresolved_references;
DELETE FROM symbols_fts;   DELETE FROM symbols;
DELETE FROM files;         DELETE FROM projects;
```

Post-migration the DB is empty, `project_id()` fails, queries exit 4 with "run `codekurve index`" — honest, one-time, no corruption. Extend `fresh_database_reaches_schema_version_2` to 3.

## BLAKE3 Substitution

Every id funnels through one private fn, so the hashing swap itself is three bodies in `repo.rs`:

```rust
fn hash_id(prefix: &str, input: &str) -> String {
    format!("{prefix}-{}", &blake3::hash(input.as_bytes()).to_hex()[..32])
}
pub fn config_hash(text: &str) -> String { blake3::hash(text.as_bytes()).to_hex().to_string() }
pub fn content_hash(bytes: &[u8]) -> String { blake3::hash(bytes).to_hex().to_string() }
```

`file_id` and the `prj`/`rel`/`unr` ids inherit it unchanged. Drop `use std::hash::{Hash, Hasher}`.

### `symbol_key` and `signature_fingerprint`

`symbol_key` is *not* a passenger here: today (`repo.rs:81`) it is a plain 4-part `format!` string, not a hash and with no signature component. The spec (symbol-index, MODIFIED "Stable Symbol Key Excludes Position and Uses BLAKE3") requires the BLAKE3 hash of a 5-tuple, so both the hashing and the fifth component are new work.

```rust
/// §16.3 + Phase 3: BLAKE3 over the 5-tuple, still excluding `start_byte`.
/// `\x1f` (unit separator) delimits components so a path containing `/`
/// cannot shift a boundary and forge another symbol's key.
pub fn symbol_key(
    language: &str,
    relative_path: &str,
    kind: &str,
    qualified_name: &str,
    signature_fingerprint: &str,
) -> String {
    let input = format!(
        "{language}\u{1f}{relative_path}\u{1f}{kind}\u{1f}{qualified_name}\u{1f}{signature_fingerprint}"
    );
    blake3::hash(input.as_bytes()).to_hex().to_string()
}
```

`symbol_id(file_id, symbol_key)` is unchanged — it now hashes the hashed key, which is fine and keeps one id shape.

**`signature_fingerprint` is the whitespace-normalized declaration text of the node's `type_parameters`, `parameters`, and `return_type` fields, joined by `\x1f`; empty string when the node has none** (class, interface, module stand-in). Computed in `extract.rs::push_named` — the single point where every `ExtractedSymbol` is created, and where the tree-sitter `Node` + source bytes are already in hand:

```rust
/// Empty for declarations without a call signature (class/interface).
fn signature_fingerprint(node: Node, source: &[u8]) -> String {
    ["type_parameters", "parameters", "return_type"]
        .iter()
        .filter_map(|f| node.child_by_field_name(f)?.utf8_text(source).ok())
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}
```

Raw normalized source text, not a semantic type model: `(id: string)` → `(id: number)` changes identity (the signature genuinely changed), a reformat or added newline does not — which is exactly the spec's "Reindex after unrelated edit" scenario. Overload/alias equivalence (`(x: Foo)` vs `(x: Bar)` where `type Bar = Foo`) is out of scope; a type-aware fingerprint needs the checker, not tree-sitter.

The value rides the existing extract → store path as one new field on each of the two symbol models, so no new plumbing:

| Carrier | Change |
|---|---|
| `codekurve-analysis/src/ir.rs` — `ExtractedSymbol` | `+ pub signature_fingerprint: String` |
| `codekurve-analysis/src/extract.rs` — `push_named` | Computes it via the helper above |
| `codekurve-core/src/symbol.rs` — `Symbol` | `+ pub signature_fingerprint: String` (store-facing model) |
| `codekurve/src/commands.rs` — `build_file_inputs` | Copies the field; `module_symbol` uses `String::new()`; passes it to the 5-arg `repo::symbol_key` |
| `codekurve-store/src/repo.rs` — `reindex` (`:176`) | Passes `symbol.signature_fingerprint` as the 5th argument |

ponytail: no `signature_fingerprint` column on `symbols`. Nothing queries it — it exists only as key input, and unchanged files are never re-keyed. Add the column when a CLI surface needs to display a signature.

## Stable Row Ids

`persist_relationships` (`repo.rs:238`) and `persist_unresolved` (`repo.rs:271`) both derive their id from the positional index `i` of a whole-run slice. Under per-batch application the same row lands at a different `i` in every batch, so ids churn, `unr`/`rel` rows can't be diffed, and edges targeting them are unstable. One shared helper fixes both:

```rust
/// Content-derived id; the ordinal only disambiguates rows whose entire
/// content tuple is identical (e.g. the same call twice on one line).
fn stable_id(prefix: &str, seen: &mut HashMap<String, u32>, parts: &[&str]) -> String {
    let base = parts.join("\u{1f}");
    let n = seen.entry(base.clone()).or_default();
    let id = hash_id(prefix, &format!("{base}\u{1f}{n}"));
    *n += 1;
    id
}
```

| Table | Tuple |
|---|---|
| `relationships` | `(project_id, source_symbol_id, kind, target_symbol_id ∥ target_external ∥ "", start_line, start_column)` |
| `unresolved_references` | `(project_id, source_file_id, source_symbol_id ∥ "", relationship_kind, target_text)` |

`unresolved_references` has no line/column columns, hence the shorter tuple; its rows are grouped by source file instead.

The ordinal is only deterministic because every row in a group is rewritten together: a group is always fully contained in one source file's rows (`source_symbol_id` implies its file; `source_file_id` is explicit), and `apply_batch` deletes then re-inserts *all* rows for each file in `B ∪ D` inside one transaction. No batch ever writes half a group. That invariant is what the golden "incremental == full reindex" test actually pins down.

## Interfaces

```rust
pub enum FileChange { Created(DiscoveredFile), Modified(DiscoveredFile), Deleted { relative_path: String } }

/// `filter = None` → full sweep (`index`, watch reconcile-on-start).
/// `filter = Some(paths)` → restrict to a debounced batch. Same body either way.
pub fn detect(conn: &Connection, project_id: &str, root: &Path,
              opts: &DiscoveryOptions, filter: Option<&HashSet<String>>)
    -> Result<Vec<FileChange>, String>;

pub fn apply_batch(conn: &mut Connection, ctx: &IndexContext, changes: &[FileChange])
    -> Result<BatchOutcome, String>;
```

`detect` per candidate: `(size_bytes, modified_ns)` equal to stored → skip; otherwise read + `content_hash` → equal → skip (touch-only), else `Modified`. In DB but absent from the walk → `Deleted`.

`resolve::resolve` gains a baseline parameter: `resolve_with(&mut [FileAnalysis], &TsconfigAliases, &ProjectBaseline)`; `resolve()` becomes `resolve_with(.., &ProjectBaseline::EMPTY)` — one code path, full and incremental. `ProjectBaseline` is plain analysis-side data (`files`, `symbols{file,name,qualified_name,kind,has_parent}`, `exports`) that `SymbolTable::build` seeds itself from before folding in the fresh analyses. The store side is one new query set, `repo::resolution_snapshot(conn, project_id)`:

| Baseline part | Source |
|---|---|
| `files` | `SELECT relative_path FROM files` |
| `symbols` | `symbols` join; `has_parent` = a `contains` relationship targets it |
| `exports` | `relationships` where `kind='exports'`, joined to the target symbol |

`commands.rs` maps store rows → `ProjectBaseline` (store still never depends on analysis).

## Dependent Re-Resolution Scope (§23.4 step 7)

For changed file set `B`, the dependent set `D` is two indexed lookups — no walk, no full reparse:

| Trigger | Query | Index used |
|---|---|---|
| `B` removed/renamed a symbol | `relationships` WHERE `target_symbol_id IN (old symbol ids of B)` AND `source_file_id NOT IN B` | `idx_relationships_target_kind` |
| `B` added a symbol / a new file others import | `unresolved_references` WHERE `target_text IN (new names of B ∪ module specifiers reaching B)` | `idx_unresolved_project_target` |

Only `B ∪ D` is re-parsed and re-resolved; everything else contributes read-only baseline rows. Bounded by edge count touching `B`, not project size. If `|B ∪ D| > watch.full_reindex_threshold_pct` of tracked files, fall back to the existing `repo::reindex`. Deleting a file naturally demotes inbound edges to `unresolved_references`, because `D` re-resolves against a baseline where `B`'s symbols are gone.

## Data Flow — `watch`

```
notify thread ──mpsc──▶ debounce loop ──batch──▶ detect ──▶ parse+resolve ──▶ apply_batch
   (OS events)          (main thread)              (walk+hash)   (B ∪ D)        (one txn)
```

Debounce (main thread, no extra threads, no crates):

```
pending: HashSet<PathBuf>; first: Option<Instant>; last: Option<Instant>
deadline = min(last + debounce_ms, first + max_batch_wait_ms)
loop rx.recv_timeout(deadline - now):
    Ok(ev)      -> pending.extend(ev.paths); last = now; first.get_or_insert(now)
    Timeout     -> flush(pending.drain()); first = last = None
    Disconnected-> break
```

A 50-file `git checkout` fires 50 events milliseconds apart → each resets `last` → one flush → one batch → one transaction.

## Batch Atomicity (no signal handler)

1. **T1** (short txn): `index_state.pending_files = <count of detected changes>`. Commit.
2. Parse + resolve `B ∪ D`. **No DB writes.** CPU-only.
3. **T2** (one txn): delete then re-insert rows for `B ∪ D`, delete rows for removed files, update `files.content_hash`/`modified_ns`, **and in the same transaction** set `pending_files = 0` / `last_verified_at = now`. Commit.

Ctrl+C or an error anywhere in step 2 or before T2 commits → rollback → previous index intact, `content_hash` unchanged for those files, `pending_files` still nonzero. The next sweep re-detects exactly the unapplied files. Freshness can never claim more than the data transaction actually wrote.

## `codekurve status` and Stale Warning

```
project: codekurve
database: .codekurve/index.db (schema 3)
files: 412   symbols: 5830   relationships: 9214 (1204 unresolved)
last verified: 2026-07-26T12:30:42Z
pending files: 7
status: stale
```

`--json` reuses the existing `print_envelope`. One helper, `fn warn_if_stale(conn, project_id)` in `commands.rs`, prints `warning: index is stale (7 pending file(s)); run \`codekurve index\`` to **stderr** only. Three call sites cover all eight query commands: inside `require_indexed_project` (the six graph commands), plus one line each in `search` and `symbol` after `project_id()`. stdout, JSON, and exit codes unchanged.

## File Changes

| File | Action | Description |
|---|---|---|
| `crates/codekurve/src/incremental.rs` | Create | `detect` + `apply_batch` — the shared engine |
| `crates/codekurve/src/watch.rs` | Create | `notify` setup + debounce loop, calls `incremental` |
| `crates/codekurve-store/src/migrations.rs` | Modify | Migration 0003, `SCHEMA_VERSION = 3` |
| `crates/codekurve-store/src/repo.rs` | Modify | BLAKE3, 5-tuple `symbol_key`, `content_hash`, per-file apply/delete, `resolution_snapshot`, `index_state`, stable `rel` + `unr` ids |
| `crates/codekurve-analysis/src/extract.rs` | Modify | `signature_fingerprint` helper; `push_named` populates it |
| `crates/codekurve-analysis/src/ir.rs` | Modify | `ExtractedSymbol.signature_fingerprint` |
| `crates/codekurve-core/src/symbol.rs` | Modify | `Symbol.signature_fingerprint` (store-facing model) |
| `crates/codekurve-analysis/src/resolve.rs` | Modify | `resolve_with(.., &ProjectBaseline)`; `resolve()` delegates |
| `crates/codekurve/src/commands.rs` | Modify | `index` via `detect`/`apply_batch`; `status`; `warn_if_stale`; carries `signature_fingerprint` into `symbol_key` |
| `crates/codekurve/src/main.rs`, `cli.rs` | Modify | `watch`/`status` dispatch, `--debounce-ms` |
| `crates/codekurve-core/src/config.rs` | Modify | `[index.watch]`: `debounce_ms=750`, `max_batch_wait_ms=5000`, `full_reindex_threshold_pct=25` |
| `Cargo.toml` (workspace, store, bin) | Modify | `blake3`, `notify` |

## Testing Strategy

| Layer | What | Approach |
|---|---|---|
| Unit | Debounce coalescing, `max_batch_wait` cap | Drive the loop with a synthetic `mpsc` sender, no filesystem |
| Unit | `detect` classification: touch-only, size-same/content-changed, delete | tempdir + in-memory DB |
| Unit | BLAKE3 id stability, migration 0003 on top of 0002 | Extend existing `migrations.rs` tests |
| Unit | `symbol_key` changes on signature edit, holds on blank-line edit | Extend `symbol_key_excludes_start_byte` (`repo.rs:680`) with a signature-change sibling |
| Unit | `rel`/`unr` ids identical whether written in one batch or per-file | Persist the same rows twice, in different slice orders, compare id sets |
| Integration | **Golden: incremental result == full reindex result** | Index, mutate, incremental-apply, full-reindex a copy, compare all four tables |
| Integration | Delete → inbound edges become `unresolved_references` | Fixture with a cross-file call |
| Integration | Failed batch (FK violation) leaves prior index + `pending_files > 0` | Mirrors `reindex_rolls_back_completely_on_relationship_error` |

## Threat Matrix

N/A — no routing, shell, subprocess, VCS/PR automation, executable-file classification, or process-integration boundary. `notify` uses in-process OS filesystem APIs; no command is composed or executed. The filesystem-boundary cases this design does own (paths outside root, symlink escape, unreadable/non-UTF-8 files, event floods) are handled by reusing `discovery::discover`'s existing rules and the batch-size fallback, and are covered in the testing table.

## Migration / Rollout

One-time forced rebuild: opening any pre-3 DB applies 0003 and empties it; the next `codekurve index` repopulates under BLAKE3 ids. Rollback = revert commits and delete `.codekurve/index.db`.

## Open Questions

- [ ] `full_reindex_threshold_pct = 25` is the proposal's flagged assumption — unvalidated against real repo sizes; tune after the first benchmark.
- [ ] `watch` on macOS FSEvents may report directory-level events; batch flush must tolerate a directory path in `pending` (walk-intersection handles it, but needs one explicit test).
- [ ] `signature_fingerprint` is empty for kinds whose node has no `parameters`/`return_type` field (class, interface, type alias, module stand-in). Two same-named declarations of the same kind in one file would then still collide on `symbol_key` — the same collision that exists today, not a regression, but revisit if TypeScript overload signatures start being extracted.
