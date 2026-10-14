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
T8.. remaining S2-S5 IDs — pending (one task per ID group)
Verifier (independent, Phase 1): no blockers; advisories A1 follow_symlinks still indexes outside files, A2 depth-capped file skipped silently, A4 stray .bak before symlink refusal, A5 cargo install not --version pinned

## Log
L1 (2026-10-14) "quiero que analises el proyecto y encuentres puntos de mejora y optimizacion. necesitamos que el codigo sea lo mas precisio y optimo ya que esto es una herramienta que lo necesita la ida es ayudar a agentes de ai a consumir menos tokens al momneto de hacer busqueda en un proyecto y esas busquedas sean rapidas. y tambien verificar cualquier vunelabilidad. en este plan debe estar el finding y una sugerencia de solucion"
L2 (2026-10-14) "si comienza a ejecutar y si es posible crea subagentes con haiku 5.5 para trabajos no tan complejos"
L3 (2026-10-14) SEC-8 decision, user answer: "Advertir y acotar la DB (Recomendado)"
L4 (2026-10-14) PERF-2 decision, user answer: "sí, hazlo así" (cap 8, rest recorded as unresolved ambiguous)
