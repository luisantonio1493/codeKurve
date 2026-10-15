# codekurve — Audit: security, token efficiency, speed

## Context

codekurve is a local code-graph indexer (Rust, SQLite, tree-sitter) whose MCP server lets AI agents query a codebase. Its value depends on three things: answers must be **correct**, **cheap in tokens**, and **fast**. The untracked `docs/IMPROVEMENT_ROADMAP.md` (Sept 2026 audit) listed gaps; this audit re-verified each claim against HEAD `f4fa364` (v0.2.11) and found new ones. Only the analyzer-version reindex (commit 746fba8) has been fixed; every other roadmap claim is **still present**. All findings below carry `path:line` evidence; the five highest-impact ones were spot-checked by hand (install backup, `ignore.patterns`, unscoped symbol lookup, watcher ordering, unused `max_snippet_bytes` / no `prepare_cached`).

Checked and clean: all SQL is parameterized (no injection), no regex/glob from user input (no ReDoS), no `unsafe`, MCP reindex off by default, HTML export escapes output, one long-lived DB connection per MCP session, WAL + `synchronous=NORMAL` already set.

Severity: **H** high, **M** medium, **L** low.

---

## Phase 1 — Security (do first)

| ID | Sev | Finding | Evidence | Suggested fix |
|---|---|---|---|---|
| SEC-1 | H | `install` writes `<file>.bak` with `fs::write`, following symlinks. A cloned repo can ship `.mcp.json` + symlink `.mcp.json.bak -> ~/.claude/settings.json`; `codekurve install` then writes repo-controlled JSON (hooks) into the user's agent settings → code execution. Main config writes also follow symlinks and are non-atomic. | `crates/codekurve/src/install.rs:884-889`, writes at `:649,:717,:794,:843,:878`; project targets `:165-168` | `symlink_metadata` → refuse symlinks for target and `.bak`; write to temp file in same dir + `rename`; preserve original file mode (0600 for `~/.codex/config.toml`). |
| SEC-2 | H | `ignore.patterns` is never applied. Defaults `**/.env*`, `**/secrets.*`, `**/dist/**`, `**/*.min.js` do nothing unless gitignored → secrets can be indexed and served to agents. | declared `crates/codekurve-core/src/config.rs:79,167-181`; `discovery_options` `crates/codekurve/src/commands.rs:1154-1170` omits it; `crates/codekurve-analysis/src/discovery.rs:52-62` | Add `patterns` to `DiscoveryOptions`; build `ignore::overrides::OverrideBuilder` in `discover()`; invalid glob = hard error. Covers full, incremental and watch (all call `discover`). |
| SEC-3 | H | Source reads have no root containment. `root.join(relative_path)` then `fs::read`; symlinks followed. A post-index symlink (`src/a.ts -> ~/.ssh/id_rsa`) is served by `get_symbol`. With `follow_symlinks=true`, files outside root get indexed. | `crates/codekurve-mcp/src/tools.rs:281`, `crates/codekurve/src/query.rs:508`, `incremental.rs:321-324`, `commands.rs:1173`, `discovery.rs:61,104-110` | One helper `contained_path(root, rel) -> Result<PathBuf>`: reject absolute/`..`, canonicalize, require `starts_with(canonical_root)`, lstat-refuse symlinks. Use it at every read site. |
| SEC-4 | M | `find_symbol_by_id` not scoped by project; reachable from MCP `get_symbol` and BFS rows. Shared DB (`storage.database` absolute) leaks other projects. | `crates/codekurve-store/src/repo.rs:1233-1242`; `query.rs:250,479-480` (`_project_id` ignored) | Add `AND s.project_id = ?2`, thread `project_id` through. |
| SEC-5 | M | Unbounded output from MCP: search `limit` not clamped to `max_limit`; `max_snippet_bytes` never read; `ctx_lines` uncapped; whole file read with no size re-check. | `tools.rs:177-180`, `query.rs:434,478,508,531`, `config.rs:91` | Clamp `limit` to `max_limit`; cap `ctx_lines`; check `metadata().len()` before read; truncate snippet to `max_snippet_bytes` on a char boundary with `truncated:true`. |
| SEC-6 | M | Installers and `codekurve update` don't verify `SHA256SUMS` (already published by release.yml). `update` pipes `install.sh` from mutable `main`. No backup/smoke test of the new binary. | `install.sh:66-72`, `install.ps1:47-62`, `crates/codekurve/src/update.rs:25-28,88`, `.github/workflows/release.yml:117-119` | Download `SHA256SUMS`, verify (`sha256sum -c` / `Get-FileHash`), keep `codekurve.prev`, run `--version` smoke test, then swap. Fetch installer from the release tag. |
| SEC-7 | M | Stack overflow on deep ASTs: recursive walkers with no depth cap; a 2 MB generated file aborts `index`/`watch`/MCP reindex, every run. No parser timeout. | `languages/typescript.rs:169,184,222,287,306,388`, `languages/csharp.rs:179,564`, `frameworks/angular.rs:60-62`, `frameworks/dotnet.rs:232-234` | Iterative `TreeCursor` walk or depth cap (skip file + warning); `parser.set_timeout_micros`. |
| SEC-8 | M | Repo-committed `.codekurve/config.toml` can loosen security: `mcp.allow_reindex`, `index.follow_symlinks`, absolute `storage.database`. | `config.rs:97-101`, `crates/codekurve-mcp/src/lib.rs:34` | Treat these keys as user-scope only (or require an explicit CLI flag / print a warning when repo config sets them). |
| SEC-9 | M | CI supply chain: `release.yml` grants `contents: write` to all jobs; actions pinned by tag not SHA; `ci.yml` has no `permissions:`; `cargo install` unpinned; no dependabot; `cargo deny` not in release. | `release.yml:15-16,41-137`, `ci.yml:20-71` | `permissions: contents: read` default, `write` only on publish job; pin by SHA; `cargo install --locked --version`; add dependabot; run `cargo deny` on release. |
| SEC-10 | L | Poisoned mutex kills the MCP server: `session.lock().unwrap()` in every tool. | `tools.rs:146,176,228,271,358,399,435,470,502,531,565` | `lock().unwrap_or_else(PoisonError::into_inner)` or map to MCP error. |
| SEC-11 | L | Route search doesn't escape LIKE `%`/`_`. | `repo.rs:1183-1186` | Escape + `ESCAPE '\'`. |
| SEC-12 | L | DB (+`-wal`/`-shm`) and config created with default umask. | `crates/codekurve-store/src/db.rs:13`, `crates/codekurve-core/src/project.rs:32-33` | Create 0600 on Unix. |

## Phase 2 — Index correctness (wrong data = wasted agent tokens)

| ID | Sev | Finding | Evidence | Suggested fix |
|---|---|---|---|---|
| IDX-1 | H | Discovery/read failures become deletions. Walk errors dropped by `.flatten()`; missing-from-`seen` ⇒ `Deleted`. Full fallback silently drops non-UTF-8 files then wipes/reinserts. Incremental path does the opposite: one non-UTF-8 file fails the whole batch forever. | `discovery.rs:65`, `incremental.rs:121-131,214-219,275`, `repo.rs:197-217` | Collect walk/read errors as a "degraded" set; never emit `Deleted` under a failed subtree; read bytes + lossy decode; expose degraded status. |
| IDX-2 | H | No analysis-input fingerprint: `config_hash` stored but never compared; `tsconfig.json` edits never trigger reindex; alias resolution order depends on `HashMap` iteration (not longest prefix); `force_full` with an empty snapshot misses offline deletions. | `commands.rs:57,62-67,83-99,472-501`, `resolve.rs:288-301`, `incremental.rs:81-85` | Persist hash(`ANALYZER_VERSION` + effective config + resolved aliases), compare on open; sort aliases by prefix length; `force_full` ⇒ always `apply_via_full_reindex`; watch `tsconfig.json`. |
| IDX-3 | H | Watcher registered after `reconcile` → edits during reconcile lost. | `crates/codekurve/src/watch.rs:23,29-37` | Register watcher first, buffer, reconcile, drain. |
| IDX-4 | H | Watcher drops `Err`/rescan/root events. | `watch.rs:30,171` | Map them to a full-sweep `detect(None)`. |
| IDX-5 | H | Failed watcher batches are logged then `pending.clear()`; next success calls `mark_verified` ⇒ status says "fresh" while stale. Fatal-ness decided by `e.contains("max_total_files")`. | `watch.rs:125-129,152,211-212`, `incremental.rs:385`, `repo.rs:506-516` | Requeue failed paths with bounded backoff; persisted dirty set `mark_verified` can't clear; typed `IndexError`. |
| IDX-6 | M | Read-then-stat race stores new mtime with old hash → change hidden forever by fast path. | `incremental.rs:108,214-220,275-278,324-337` | Stat before and after read; retry if different. |
| IDX-7 | M | FK failure: unreadable dependent keeps edges to deleted symbol ids → whole T2 tx fails (FKs have no `ON DELETE`). | `incremental.rs:324-336`, `repo.rs:474-476`, `migrations.rs:97-98` | Treat skipped dependent as delete-and-unresolve. |
| IDX-8 | M | Created/renamed modules not invalidated (namespace imports resolve to `"*"`, no target id). | `commands.rs:382-388`, `incremental.rs:291-315` | Store resolved target file per import edge; query dependents by target file. |
| IDX-9 | M | `.tsx` parsed with the TypeScript (non-TSX) grammar → JSX becomes ERROR nodes, symbols lost. | `crates/codekurve-core/src/language.rs:19`, `typescript.rs:84-86`, `frameworks/mod.rs:92` | Pick `LANGUAGE_TSX` by extension. |
| IDX-10 | M | Touch-only files never refresh `modified_ns` → re-read+re-hash forever after `git checkout`. | `incremental.rs:115-116` | Batch `UPDATE files SET modified_ns=?` in T2. |

## Phase 3 — Token efficiency of MCP responses (core product value)

| ID | Sev | Finding | Evidence | Suggested fix |
|---|---|---|---|---|
| TOK-1 | H | `trace_path` returns every visited node (≤500) instead of the path; real `outcome.path` collapsed to a bool. | `tools.rs:410-421`, `traverse.rs:110`, `commands.rs:682` | Return ordered path nodes + `frontier_count`; `reached` only on request. |
| TOK-2 | H | BFS rows (`trace`/`impact`) have no name — only 2×36-char ids; agent must call `get_symbol` per node (more round-trips, more tokens). Root rows emit 3 nulls. | `query.rs:245-271` | Add `qualified_name`; `predecessor` as row index; omit nulls. |
| TOK-3 | H | `qualified_name` already starts with `path::`, yet rows also send `path` (search sends `name`+`qualified_name`+`path` = 3 copies). | `tools.rs:189-196,294-299`, `query.rs:333-337`; QN format `typescript.rs:796-799`, `csharp.rs:948-952` | Drop `path`/`name` when `qualified_name` present (or split once into `{path, qn}`). Document in `docs/MCP.md`. |
| TOK-4 | M | Fixed per-row fields not hoisted: `path` on `callees` (1e26071 missed it), `kind:"calls"` on `find_callers`, hardcoded `confidence:"high",provenance:"tree-sitter"` on search/get_symbol (~45 chars/row), repeated source fields in filtered `find_unresolved`. | `query.rs:316-319,345-356,407-418`, `repo.rs:866`, `tools.rs:202-203,302-303` | Extend the existing `relationship_anchor` hoist; hoist single-valued kind; drop constants; `reasons` legend. |
| TOK-5 | M | Nulls/empties always serialized (`reason:null`, `stale_reason:null`, `target_*:null` on routes, `warnings:[]`). | `query.rs:151-157,278-292,342`, `tools.rs:304-309` | `skip_serializing_if` / omit in `json!` builders. |
| TOK-6 | M | No truncation signal: `truncated_reason` dropped; trace/impact `total` = capped count; search always `truncated:false`. Agents can't tell if they should page. | `tools.rs:416-417,452-453`, `query.rs:437-442`, `traverse.rs:112` | Emit `truncated_reason`; search fetches `limit+1`. |
| TOK-7 | M | Tool listing ≈ 8.5k chars (~2.1k tokens) per session: `min_confidence` caveat (~520 chars) duplicated 6× on the wire; `find_unresolved` description ~700 chars; meanwhile `limit`/`offset`/`depth`/`ctx_lines` defaults undocumented. | `tools.rs:76-83,109-115,127-134,352` | One-line `min_confidence`; `find_unresolved` ≈200 chars; add "default 50, max 500" hints. Measure before/after with a snapshot test of `list_tools`. |
| TOK-8 | L | Envelope overhead and duplicate data: `schema_version` twice in `project_status`; `get_symbol` `total:1`; `project_overview` embeds 20 full route rows (same as `find_routes`). | `query.rs:151-160,614`, `tools.rs:154,313` | Trim envelope; overview returns route count only. |
| TOK-9 | L | `analyze_impact`/`trace_path` have no `limit`/`offset`. | `tools.rs` graph inputs | Add optional `limit`/`offset` (default unchanged). |

## Phase 4 — Query speed

| ID | Sev | Finding | Evidence | Suggested fix |
|---|---|---|---|---|
| QRY-1 | H | Every MCP tool call runs 4 `COUNT(*)` (one is a full scan on unindexed `confidence`) just to read `pending_files`; `project_status` pays twice. | `query.rs:124-131`, `repo.rs:549-573` | New `repo::pending_files(conn, pid)` = `SELECT pending_files FROM index_state`. |
| QRY-2 | H | Traversal loads **every** project edge into a `HashMap<String,Vec<Edge>>` (5 Strings/edge) on every trace/impact call. | `crates/codekurve-store/src/traverse.rs:38-72`, `query.rs:208,229` | Frontier-batched SQL (`WHERE source/target_symbol_id IN (frontier)`, chunked) per BFS level; caps applied before materialization. |
| QRY-3 | H | N+1 in `bfs_rows`: `find_symbol_by_id` (re-prepared) per reached node, ≤500. | `query.rs:245-271`, `repo.rs:1234` | One chunked `WHERE s.id IN (…)` lookup (also feeds TOK-2 names). |
| QRY-4 | H | `relationships`/`unresolved`/`routes` load all rows, then paginate and filter `min_confidence` in Rust. | `query.rs:182-186,390-394,453-457`, `commands.rs:976-984`, `repo.rs:926-949,1002-1010,1187-1198` | SQL `LIMIT/OFFSET` + `COUNT(*) OVER ()`; confidence filter in SQL. |
| QRY-5 | M | Search fallback `LOWER(name) LIKE '%term%'` full scan; FTS is phrase-only (no prefix); FTS indexes `kind`/`relative_path` (noise); `ORDER BY name` before `LIMIT`. | `repo.rs:1087,1119-1141`, `migrations.rs:67-73` | Prefix queries `"term"*` with column filter `{name qualified_name}:`; consider trigram tokenizer before LIKE. |
| QRY-6 | M | No `prepare_cached` anywhere; per-row `tx.execute` in indexing re-prepares. | `repo.rs:253,295,324,424,714,759` | `prepare_cached` + hoist out of loops; raise statement cache capacity. |
| QRY-7 | M | Missing indexes: `relationships(source_file_id)`, `unresolved_references(source_file_id)`, `unresolved_references(source_symbol_id)`; composite `(project_id, target_symbol_id, kind)` / `(project_id, source_symbol_id, kind)`; no `ANALYZE`/`PRAGMA optimize`. | `migrations.rs:118-121`, `repo.rs:363-370,934,984` | New versioned migration with these indexes; `PRAGMA optimize` after index. Confirm with `EXPLAIN QUERY PLAN`. |
| QRY-8 | M | FTS deletes by `UNINDEXED symbol_id` → full FTS scan per changed file. | `migrations.rs:67-73`, `repo.rs:206,380` | External-content FTS keyed by `rowid` (migration). |
| QRY-9 | M | Missing pragmas: `cache_size`, `mmap_size`, `temp_store`; MCP session not `query_only`. | `crates/codekurve-store/src/db.rs:28-33` | Add pragmas; `query_only` when reindex is disabled. |
| QRY-10 | L | Unbounded IN-lists can exceed SQLite's 32766 variables; O(n·m) `retain`. | `repo.rs:600,1343,1362,1380`, `incremental.rs:308-311` | Dedupe + chunk ≤500 (or `json_each(?)`); `HashSet` for `changed_files`. |
| QRY-11 | L | BFS clones ids/edges repeatedly; `tool_router()` rebuilt per call; a `NotIndexed` session never reopens. | `traverse.rs:197-209`, `crates/codekurve-mcp/src/server.rs:41,66`, `crates/codekurve-mcp/src/lib.rs:33` | Intern ids to `i64` rowid; cache router; retry open on `NotIndexed`. |

## Phase 5 — Indexing speed

| ID | Sev | Finding | Evidence | Suggested fix |
|---|---|---|---|---|
| PERF-1 | H | Parsing is sequential (no rayon); discovery uses `build()` not `build_parallel()`. | `incremental.rs:213-228,274-286`, `discovery.rs:52-65` | `rayon::par_iter` over read+analyze, single-threaded DB write. |
| PERF-2 | H | Unbounded multi-candidate fan-out: a call to `get` creates one edge per same-named symbol in the project → edge explosion, slower queries, noisier answers. | `crates/codekurve-analysis/src/resolve.rs:515-531,612,692,773,839` | Cap candidates (e.g. >8 ⇒ unresolved `ambiguous`). Also dedupe the 5 near-identical dispatch blocks. |
| PERF-3 | M | Every watcher event triggers a full discovery walk + O(files×events) prefix filter; DB writes under `.codekurve/` and `.git` re-trigger it. | `incremental.rs:80,142-151`, `watch.rs:36`, `commands.rs:59` | Stat precise paths, walk only subtrees, ancestor set lookup, drop DB/ignored paths before queuing. |
| PERF-4 | M | Framework pass re-parses the file (broad markers ⇒ most C# files parsed twice); new `Parser` per file. | `frameworks/mod.rs:47-69,96`, `typescript.rs:90`, `csharp.rs:92` | Pass the existing `Tree` to `recognize`; `thread_local!` parser per grammar. |
| PERF-5 | M | `resolution_snapshot` loads every symbol (+ `EXISTS` per row) on every watcher batch. | `repo.rs:1296-1320` | Cache `SymbolTable` across batches, apply deltas. |
| PERF-6 | M | Pending refs × symbols scan per file. | `languages/mod.rs:104-108` | Precompute name → symbols map. |
| PERF-7 | L | Changed file read + hashed twice (detect and apply); row-by-row inbound deletes; per-pair qualified-name lookups; `String` clones in `HashMap` lookups. | `incremental.rs:112-115,275,282`, `repo.rs:423-424,631`, `commands.rs:363-388,427-428`, `resolve.rs:110-200,373` | Carry bytes+hash from detect; set-based deletes; batched lookup; `Arc<str>`/borrowed keys. |
| PERF-8 | L | No generated/minified detection beyond `ignore.patterns` (`.d.ts`, `.g.cs`). | `discovery.rs:66-68` | Solved mostly by SEC-2 defaults; optionally add `*.g.cs`, `*.d.ts` to default patterns. |

## Docs drift (fix alongside the related phase)

- `docs/MCP.md:3` still says MCP is "Not implemented yet"; `find_routes` missing from `docs/MCP.md:13-19` and `docs/AGENT_USAGE.md:266-269`.
- Only search and routes inputs use `deny_unknown_fields` (`tools.rs:41,50`); apply it to all tool inputs.
- Replace the untracked `docs/IMPROVEMENT_ROADMAP.md` with the IDs in this plan so it stays the single source of truth.

---

## Handoff notes for the implementing model

- Line numbers are from HEAD `f4fa364`; re-read each cited range before editing, since earlier fixes shift lines.
- Work one ID at a time in phase order; do not start Phase 3 response-shape changes before Phase 1 is merged.
- Each ID: write the RED test listed in Verification → fix → run focused test → `cargo test --workspace`.
- Never change an MCP response shape without bumping `schema_version` and updating `docs/MCP.md` + CHANGELOG in the same commit.
- If a finding turns out not to reproduce, record that in the feature document and skip it; don't force a fix.

## Delivery

- Branch `fix/audit-hardening` off `main`; one feature document `odd/tasks/audit-hardening.md` (+ Engram mirror) with one task per ID group; Conventional Commits per work unit, tests and docs in the same commit.
- Order: Phase 1 → 2 → 3+4 (they touch the same rows; TOK-2 and QRY-3 ship together) → 5. Expect chained PRs (>400 lines total); each phase is a natural PR slice.
- Compatibility: keep tool names and existing input fields. TOK-1/TOK-3/TOK-5 change response shapes for agents → treat as contract change: bump `schema_version` and document in `docs/MCP.md` and CHANGELOG.
- Schema changes (QRY-7, QRY-8, IDX-8) go through a new versioned, transactional migration in `crates/codekurve-store/src/migrations.rs` — never by erasing the index.
- Reuse: `relationship_anchor` (`query.rs:316`) for all hoisting; `commands::paginate` removed only after QRY-4 moves paging to SQL; `discover()` stays the single discovery entry point for SEC-2.

## Verification

- Per finding: one RED test through the public interface first (CLI or MCP tool), then fix. Key cases: symlinked `.bak` refused (SEC-1); `secrets.env` not indexed with default config (SEC-2); symlink to outside root not served (SEC-3); cross-project id returns not-found (SEC-4); `limit=u32::MAX` clamped and snippet truncated on a multi-byte char (SEC-5); deeply nested file indexed or skipped without crash (SEC-7); unreadable subtree keeps its rows (IDX-1); edit during reconcile picked up (IDX-3); `.tsx` JSX component extracted (IDX-9); `trace_path` returns only the path (TOK-1).
- Token budget: snapshot tests for `list_tools` size and for representative tool responses on `fixtures/`, recording chars before/after.
- Speed: run `benchmarks/` (agent-context runner from 6ccbfa0) before and after phases 4–5 on the 100/1k/10k-file fixtures; record index time, no-op reindex, trace/impact latency, peak RSS, DB size. `EXPLAIN QUERY PLAN` checks for QRY-4/QRY-7.
- Every phase: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`.
