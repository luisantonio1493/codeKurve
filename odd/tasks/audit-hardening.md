# audit-hardening
Objective: fix security, index-correctness, token-efficiency and speed findings from the audit. Plan: /Users/luissuarez/.claude/plans/zesty-cuddling-hamming.md (IDs SEC/IDX/TOK/QRY/PERF).
Branch: fix/audit-hardening. Delivery: ask-on-risk (chained PRs per phase). Test runner: cargo test --workspace.

## Specs
S1 Phase 1 security: SEC-1..SEC-12 as written in the plan file.
S2 Phase 2 index correctness: IDX-1..IDX-10.
S3 Phase 3 token efficiency: TOK-1..TOK-9 (bump schema_version + docs on shape change).
S4 Phase 4 query speed: QRY-1..QRY-11.
S5 Phase 5 indexing speed: PERF-1..PERF-8.
Assumption: each finding is reproduced before fixing; non-reproducing ones are skipped and logged.

## Tasks
T1 S1 SEC-2 ignore.patterns enforced in discovery — route: delegated (haiku) — DONE, RED/GREEN observed, commit: see git log 'enforce ignore.patterns'
T2 S1 SEC-1 install backup/symlink-safe atomic writes — route: inline — DONE, RED/GREEN observed, commit: see git log 'symlink-safe'
T3 S1 SEC-3+SEC-4+SEC-5 contained reads, project-scoped lookup, output limits — route: inline — DONE (tests written after the code; no RED observed), commit: 'contain source reads'
T4 S1 SEC-7 + SEC-12 DONE (haiku writers; commits 'cap AST depth', 'owner-only'); SEC-10 poisoned lock (test after code, no RED) + SEC-11 LIKE escape (RED observed) DONE in the same commit as T3
T5 S1 SEC-6 (installers only; update.rs left on main by design, now verified by the script), SEC-8 (user chose: warn + bound DB path), SEC-9 — DONE; SEC-6/SEC-9 by haiku writers, SEC-8 inline (RED observed); commits 'verify release checksums', 'bound storage.database', 'harden workflows'
T6 S4 QRY-1 pending_files instead of 4 COUNTs per MCP call — inline, RED observed (compile) — DONE
T7 S5 PERF-2 cap by-name candidates at 8 (user chose: unresolved 'ambiguous: N candidates') — inline, RED observed — DONE. Real C# project (350 files): relationships 197014 -> 52202, DB 106 MB -> 38 MB, debug index 13.4s -> 5.8s
T8 S2 IDX-9 .tsx grammar (haiku writer, RED observed) — DONE
T9 S2 IDX-1 walk/read failures never deletions; non-UTF-8 — inline, RED observed — DONE
T10 S2 IDX-3/4/5 watcher order, rescan, bounded retry — inline, RED (compile) observed; IDX-3 ordering has no automated test — DONE
T11 S2 IDX-10 touch refresh, IDX-2 fingerprint + alias order + force_full deletions (force_full test: RED not observed separately), IDX-6 stable read (test seam), IDX-7 inbound edges, IDX-8 module dependents — inline — DONE
T12 S3 TOK-1,2,4(callees path),5,6 + schema_version 2 envelope — inline; tests written with the change (contract tests updated; new no-null guard in envelope_of) — DONE. Measured on real C# project: trace_path no-path 133579 -> 147 chars; find_callees -27%; impact +42% (names added). TOK-3/TOK-4 (user: 'Quitar y documentar (Recomendado)'): rows drop path/name and constant confidence/provenance on symbol rows (search 20.8KB -> 13.6KB, -35%); TOK-7 tool listing 9015 -> 6934 chars (-23%). Still open: TOK-8 overview routes, TOK-9 limit on trace/impact
T13 S2 verifier follow-ups: B1 degraded status (Unverified), A1 incremental==clean (baseline bug: exported fallback), A2, A3 watch reload — inline, RED observed for each — DONE. Phase 2 verifier (independent): no blockers after B1 fix
T14 S4 QRY-1 pending_files, QRY-2 lazy frontier-batched BFS (bfs_db; equivalence test; +project_id keeps planner on per-symbol index), QRY-6 prepare_cached/exec helper + statement cache 64 — DONE. On 197k-edge index (release): impact 164->52 ms, trace 144->45 ms (floor ~35 ms process start). QRY-6 index-time effect NOT measured yet.
T16 S4 QRY-3 batched symbol lookup (repo::find_symbols_by_ids, chunked 500) for bfs_rows/path_rows — inline, RED (compile) observed — DONE, commit 156a2be
T17 S4 QRY-7 migration 0007 (5 indexes, SCHEMA_VERSION 7), db::optimize wired at end of apply_batch, load_adjacency ORDER BY rowid (new composite index changed BFS tie-break order; matches bfs_db) — delegated (haiku, effort high; reason: parallel unit) + inline fix, RED observed (7 migration tests + 2 EXPLAIN QUERY PLAN tests) — DONE, commit aa663dc. Workspace fmt/clippy/test green. Speed effect NOT yet measured.
T18 S4 QRY-4 SQL LIMIT/OFFSET + confidence filter in SQL + separate COUNT (skipped when the first page is short) for relationships/unresolved/routes; ORDER BY now ends in rowid (stable pages); commands::paginate removed — inline; tests written with the change (RED not observed separately) — DONE, commit aff8c79. Sorts for unresolved/routes still run over all matching rows (no index for the ORDER BY); the gain is not materializing rows outside the page. Speed NOT measured.
T19 S4 QRY-9 pragmas: cache_size -32768 + temp_store MEMORY; NO mmap_size on purpose (SIGBUS if another process truncates the file); MCP session set read-only (PRAGMA query_only) when allow_reindex is off — inline, RED observed (cache_size -2000; session test compile) — DONE, commit aeaa223. Effect NOT measured.
T20 S4 QRY-8 changed approach: no FTS migration (rowid keying is fragile: symbols has TEXT PK, rowids can change on VACUUM). Real index: FTS delete scans the table, 4.5 ms/file; batching one DELETE per 500 files: 50 files 72 ms -> 5.4 ms (sqlite3 on a copy of the real C# index). apply_incremental now uses delete_symbols_of_files — inline; behaviour-preserving, new test passes on old and new code (no RED possible) — DONE, commit see git log 'delete a batch's FTS rows'. delete_file (deleted paths) still deletes per file.
T21 S4 QRY-10 chunked IN lists (strings_in_chunks helper for symbol_ids_for_files, dependents_by_target_symbol, dependents_by_unresolved_target; HashSet for changed_files) — inline, RED observed ('too many SQL variables' at 40k) — DONE, commit 'chunk long IN lists'.
T22 S4 QRY-11 partial: MCP builds the tool router once; a NotIndexed session is reopened on each call until it becomes Indexed (RED observed in an MCP integration test) — inline — DONE, commit 'build the tool router once'. NOT done: BFS id interning (clones in traverse.rs) — skipped, no evidence it matters after QRY-2.
T23 S4 QRY-5 search: FTS prefix + column filter {name qualified_name} + exact-name-first ordering — inline, RED observed for kind-noise and ranking (prefix test already passed via the LIKE fallback, kept as guard) — DONE, commit 'prefix search'. Real index: FTS 0.03 ms vs LIKE fallback 3.3 ms (5.5k symbols); 'class' query matched 466 symbols via the kind column vs 2 by name. LIKE fallback kept for longer-guessed names (trigram not done).
T24 S4 Phase 4 independent verifier (haiku, effort high, scratch clones of b6d1680 and a02a3ae): PASS with advisories, NO blockers; cargo test --workspace 378 passed. Verified: paging equivalence (720 CLI combos), impact/trace JSON identical on 153 cases, v6->v7 migration keeps data, FTS==symbols count over 6 edit cycles + 60-file batch, MCP read-only/reindex gate, NotIndexed reopen, 700-file batch. Parent spot check: fresh index lists idx_relationships_source_file and no composites; schema 7. Advisories: A1 impact/trace rows at equal depth are ordered by rowid now (same row set; baseline order was planner-dependent — reproduced: text and JSON row order differ, set identical); A2 export HTML layout order; A3 search ties (fixed with qualified_name,id tie-break; not reproducible in a unit test); A4 QRY-5 recall loss for kind/path words is intended; A5 delete_file still per path; A7 COUNT separate, id interning skipped. QRY-1 not exercisable by baseline diff (ancestor of baseline).
T15 PENDING: S5 PERF-1 (parallel parsing), PERF-3..8; Phase 3 leftovers TOK-8/TOK-9; verifier advisories from Phase 1 (A1 follow_symlinks outside root, A2 depth-capped file silent skip, A4 stray .bak, A5 cargo install --version pin); Phase 4 needs an independent verifier run (not yet done)
Verifier (independent, Phase 1): no blockers; advisories A1 follow_symlinks still indexes outside files, A2 depth-capped file skipped silently, A4 stray .bak before symlink refusal, A5 cargo install not --version pinned

## Log
L1 (2026-10-14) "quiero que analises el proyecto y encuentres puntos de mejora y optimizacion. necesitamos que el codigo sea lo mas precisio y optimo ya que esto es una herramienta que lo necesita la ida es ayudar a agentes de ai a consumir menos tokens al momneto de hacer busqueda en un proyecto y esas busquedas sean rapidas. y tambien verificar cualquier vunelabilidad. en este plan debe estar el finding y una sugerencia de solucion"
L2 (2026-10-14) "si comienza a ejecutar y si es posible crea subagentes con haiku 5.5 para trabajos no tan complejos"
L3 (2026-10-14) SEC-8 decision, user answer: "Advertir y acotar la DB (Recomendado)"
L4 (2026-10-14) PERF-2 decision, user answer: "sí, hazlo así" (cap 8, rest recorded as unresolved ambiguous)
L5 (2026-10-14) TOK-3/4 decision, user answer: "Quitar y documentar (Recomendado)"
