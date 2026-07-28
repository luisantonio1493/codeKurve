# Tasks: Phase 4 — MCP Server

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~2800–3400 |
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
| 1 | Library target: module-declaration move only (base=tracker) | PR1 | `cargo test -p codekurve --test '*'` (existing suite, zero edits) | `codekurve search foo` on fixture repo, diff stdout vs. pre-move baseline | Revert `lib.rs` + `main.rs` mod-line changes |
| 2 | `query.rs` extraction: search/get_symbol/relationships/trace/impact (base=PR1) | PR2 | `cargo test -p codekurve --test '*'` + `cargo test -p codekurve query::` | Run all 6 graph-query CLI commands with/without `--json` on fixture repo, byte-diff stdout vs. pre-extraction baseline | Revert `query.rs` additions + `commands.rs` print/return split for these 6 commands |
| 3 | `query.rs` extraction: status/doctor/overview + `warnings()` + `StoredSymbol.id`/store queries (base=PR2) | PR3 | `cargo test -p codekurve-store repo::` + `cargo test -p codekurve --test '*'` | `codekurve status`/`doctor` unchanged stdout; `codekurve search` still shows unchanged text output despite added `id` field | Revert `find_symbol_by_id`/`language_breakdown`/`StoredSymbol.id` + status/doctor/overview split |
| 4 | `codekurve-mcp` skeleton: rmcp pin, stdio bootstrap, one tool, stdout discipline (base=PR3) | PR4 | `cargo test -p codekurve-mcp` | Spawn `codekurve mcp`, send `initialize`+`tools/list`+`codekurve_project_status`, assert stdout is JSON-RPC only; real client (Claude Code/Codex) handshake, recorded in PR description | Revert `crates/codekurve-mcp/` + workspace `Cargo.toml` deps |
| 5 | 8 remaining read tools wired + goldens (base=PR4) | PR5 | `cargo test -p codekurve-mcp` | Call each tool via spawned-process harness against fixture repo, compare golden JSON | Revert `tools.rs` additions for these 8 tools + golden fixtures |
| 6 | `project_overview`, `doctor`, gated `reindex` tools (base=PR5) | PR6 | `cargo test -p codekurve-mcp` | Toggle `[mcp] allow_reindex`, confirm tool list changes and gated call succeeds/fails | Revert 3 tool bodies + gating test |
| 7 | Docs: `AGENT_USAGE.md`, README client install (base=PR6, merges to tracker) | PR7 | N/A — doc-only | Manual: follow README steps to connect Claude Code to `codekurve mcp` | Revert `docs/AGENT_USAGE.md` + README section |

## Phase 1: PR1 — Library Move (req: design "Query layer location", CLI regression risk)

- [x] 1.1 Create `crates/codekurve/src/lib.rs`: `pub mod commands; pub mod incremental; pub mod watch;` (module declarations only, no code moved).
- [x] 1.2 `main.rs`: drop `mod commands;`/`mod incremental;`/`mod watch;` lines, `use codekurve::{commands, incremental, watch};` (or equivalent), keep `mod cli;` local to the binary.
- [x] 1.3 Add `[lib]` target to `crates/codekurve/Cargo.toml` (path `src/lib.rs`), keep existing `[[bin]]` target unchanged.
- [x] 1.4 Grep `crate::` paths inside `commands.rs`/`incremental.rs`/`watch.rs` still resolve unchanged (module-declaration move, not a code move — no path rewrites expected).
- [x] 1.5 Run existing `crates/codekurve/tests/*` golden suite with zero edits; confirm pass.

## Phase 2: PR2 — Query Extraction: Search/Symbol/Relationships (req: graph-queries MODIFIED "Six Graph Query Commands")

- [ ] 2.1 `crates/codekurve/src/query.rs`: add `Session` enum (`Indexed{root,config,conn,project_id}` / `NotIndexed{root,config,reason}`), `Session::open(root)`.
- [ ] 2.2 `query.rs`: add `Page<T>{rows,total,truncated}` and `envelope(project,result,warnings,truncated,total:Option<usize>)` — `total` key emitted only when `Some`.
- [ ] 2.3 `query.rs`: `search(s,&SearchInput) -> Result<Page<SymbolHit>,CommandError>` — extract logic from `commands::search`.
- [ ] 2.4 `query.rs`: `get_symbol(s,id,ctx_lines) -> Result<SymbolDetail,CommandError>` — extract logic from `commands::symbol`.
- [ ] 2.5 `query.rs`: `relationships(s,kind:RelKind,&QueryArgs) -> Result<Page<StoredRelationship>,CommandError>` — extract shared logic behind `references`/`callers`/`callees`/`implementations`.
- [ ] 2.6 `query.rs`: `trace(s,&QueryArgs,to) -> Result<traverse::BfsOutcome,CommandError>` and `impact(s,&QueryArgs) -> Result<traverse::BfsOutcome,CommandError>` — extract from `commands::trace`/`commands::impact`.
- [ ] 2.7 `commands.rs`: rewrite `search`/`symbol`/`references`/`callers`/`callees`/`implementations`/`trace`/`impact` as thin wrappers (`Session::open` + `query::*` call + existing print logic), byte-identical stdout strings.
- [ ] 2.8 `commands.rs`: keep `#[allow(clippy::print_stdout)]` scope local to this module (prep for Phase 4's crate-level `#![deny]`).
- [ ] 2.9 Test: `crate::query::envelope(..,None)` produces byte-identical JSON to today's `search --json` golden string.
- [ ] 2.10 Test: direct `query::relationships(..)` call and `codekurve callers --symbol-id <id>` CLI invocation return identical structured results against the same fixture index (graph-queries "Library function reused by another consumer" scenario).
- [ ] 2.11 Run existing `crates/codekurve/tests/*` golden suite with zero edits; confirm pass (before/after diff of `references`/`callers`/`callees`/`implementations`/`trace`/`impact` stdout, with and without `--json`).

## Phase 3: PR3 — Query Extraction: Status/Doctor/Overview + Store Additions (req: mcp-server "project_status Tool", "doctor Tool"; design Interfaces)

- [ ] 3.1 `codekurve-store/src/repo.rs`: `StoredSymbol` gains `id` field; `search`/`find_by_name` select `s.id` (additive; CLI text output unchanged since it prints name/kind/path only).
- [ ] 3.2 `codekurve-store/src/repo.rs`: add `find_symbol_by_id(conn, id)` query.
- [ ] 3.3 `codekurve-store/src/repo.rs`: add `language_breakdown(conn, project_id)` query for `project_overview`.
- [ ] 3.4 `query.rs`: `status(s) -> Result<StatusData,CommandError>`, `overview(s) -> Result<OverviewData,CommandError>`, `doctor(s) -> DoctorReport` — extract from `commands::status`/`commands::project_overview` (new)/`commands::doctor`.
- [ ] 3.5 `query.rs`: `Session::warnings(&self) -> Vec<String>` — single stale-source helper (pending files / not-indexed reason), one wording for both CLI stderr and MCP `warnings`.
- [ ] 3.6 `commands.rs`: rewire `warn_if_stale` to call `query::warnings` internally instead of duplicating the pending-files check.
- [ ] 3.7 Test: `query::warnings(&Session)` returns identical `Vec<String>` regardless of caller (unit test, feeds both CLI and MCP paths).
- [ ] 3.8 Test: `Session::open` on missing config is fatal (`Err`); on missing/empty DB returns `NotIndexed` (degraded, not `Err`).
- [ ] 3.9 Run existing `crates/codekurve/tests/*` golden suite with zero edits; confirm `status`/`doctor` stdout unchanged.

## Phase 4: PR4 — MCP Skeleton, rmcp Pin, One-Tool Walking Skeleton (req: mcp-server "Stdio Transport Only", "stdout Carries JSON-RPC Only", "Single Project Root Per Server Instance"; design "rmcp Risk")

- [ ] 4.1 `cargo new --lib crates/codekurve-mcp`; add to workspace members.
- [ ] 4.2 `cargo add rmcp --features server,transport-io` in `crates/codekurve-mcp`, then rewrite to an exact `=X.Y.Z` pin in `Cargo.toml` (no caret/tilde range).
- [ ] 4.3 Verify handler shape (`#[tool_router]`, `#[tool]`, `ServerHandler::get_info`) against `docs.rs/rmcp/X.Y.Z` and `modelcontextprotocol/rust-sdk` `examples/servers/src/std_io.rs` at that same tag; note the verified version + fallback decision in the PR description.
- [ ] 4.4 `Cargo.toml` (workspace): add `[workspace.dependencies]` entries for `rmcp`, `tokio`, `schemars`, `serde_json`; `crates/codekurve-mcp/Cargo.toml` depends on `codekurve` (the new lib target) plus these.
- [ ] 4.5 `codekurve-core/src/config.rs`: add `Mcp { #[serde(default)] pub allow_reindex: bool }`, `Config { #[serde(default)] pub mcp: Mcp }` — same pattern as `[index.watch]`; older configs parse unchanged.
- [ ] 4.6 `crates/codekurve-mcp/src/lib.rs`: `#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]`; add one `fn log(msg)` = `eprintln!`, `#[allow]`-ed at that fn only.
- [ ] 4.7 `crates/codekurve-mcp/src/lib.rs`: `pub struct CodeKurve { session: Mutex<Session>, allow_reindex: bool }`, `pub fn run(root: &Path) -> Result<(), String>` — `Session::open` once, current-thread tokio runtime, `serve(rmcp::transport::stdio())`, `svc.waiting().await`.
- [ ] 4.8 `crates/codekurve-mcp/src/server.rs`: `ServerHandler` impl; `crates/codekurve-mcp/src/tools.rs`: register **one** tool, `codekurve_project_status`, body = `session.lock().unwrap()` (no `.await` while held) + `query::status` + `envelope(..)`.
- [ ] 4.9 `crates/codekurve/src/main.rs`: add `"mcp" => codekurve_mcp::run(&args.root)` dispatch arm; `crates/codekurve/src/cli.rs`: add `mcp` subcommand (no flags beyond existing `--root`).
- [ ] 4.10 Integration test (spawned-process): launch `codekurve mcp`, send `initialize` + `tools/list` + `codekurve_project_status` call over stdin, assert every stdout line parses as JSON-RPC (`jsonrpc == "2.0"`) under both default env and `RUST_LOG=trace`.
- [ ] 4.11 Manual: connect a real client (Claude Code or Codex) to `codekurve mcp`, confirm handshake + `codekurve_project_status` call succeed; record steps/output in the PR description.

## Phase 5: PR5 — Remaining Read Tools (req: mcp-server "Tool Registry", "search_symbols Tool Rejects Unsupported Filters", "get_symbol Reads Live Source and Flags Drift", "Query Tools Return the §28.3 Response Envelope", "Stale Warning Visible on Every Response")

- [ ] 5.1 `tools.rs`: `codekurve_search_symbols` — accepts `query`/`kinds`/`languages`/`path_prefix`/`limit`; unsupported filter value returns explicit `invalid params: filter not supported yet (supported: query, limit)` error, never silently ignored.
- [ ] 5.2 `query.rs`: `source_slice(path, span, ctx_lines)` — reads disk every call; `file_missing`/`span_out_of_range`/`non_utf8` → `stale:true` with reason; else `Some(text)` with `stale = index_pending > 0`.
- [ ] 5.3 `tools.rs`: `codekurve_get_symbol` — uses `query::get_symbol` + `source_slice`; sets stale flag per Confirmed Decision 4.
- [ ] 5.4 `tools.rs`: `codekurve_find_references`, `codekurve_find_callers`, `codekurve_find_callees`, `codekurve_find_implementations` — each maps to `query::relationships(s, RelKind::_, args)`.
- [ ] 5.5 `tools.rs`: `codekurve_trace_path`, `codekurve_analyze_impact` — map to `query::trace`/`query::impact`.
- [ ] 5.6 All 8 tools: response envelope includes path, line range, confidence, provenance per row; total count + truncation flag at result-set level; `warnings` populated from `Session::warnings()` on every response.
- [ ] 5.7 JSON schema (via `schemars`) for each tool's input, registered in `tools/list`.
- [ ] 5.8 Golden test per tool: fixture project, snapshot each `call_tool` result (8 fixtures).
- [ ] 5.9 Test: unsupported `kinds`/`languages`/`path_prefix` value → explicit error, one case per filter.
- [ ] 5.10 Test: capped result (more callers than cap) → `truncated:true` + total > returned rows; small result (under cap) → `truncated:false`.
- [ ] 5.11 Test: stale warning present when `pending_files > 0`, absent (empty/false) when `pending_files == 0` — asserted via a direct tool call, not a filesystem walk.

## Phase 6: PR6 — project_overview, doctor, Gated reindex (req: mcp-server "doctor Tool", "Missing or Stale Index Served Degraded, Never Auto-Indexed", "reindex Gated Off by Default")

- [ ] 6.1 `tools.rs`: `codekurve_project_overview` — maps to `query::overview` (counts + `language_breakdown`).
- [ ] 6.2 `tools.rs`: `codekurve_doctor` — maps to `query::doctor`, same checks as CLI `doctor` (schema/version compatibility, index integrity, config validity).
- [ ] 6.3 `tools.rs`: `codekurve_reindex` registered only when `self.allow_reindex`; body triggers existing `commands::reindex`/`incremental::apply_batch` path.
- [ ] 6.4 `server.rs`: `tools/list` conditionally omits `codekurve_reindex` when `allow_reindex == false`; calling `reindex` while disabled fails as unknown tool.
- [ ] 6.5 Test: `NotIndexed` session → query tools return degraded response/tool error with warning, no index run triggered; `project_status`/`doctor` still answer (degraded), never auto-index.
- [ ] 6.6 Test: stale index (`pending_files > 0`) is served as-is with the stale warning set — no auto-reindex triggered by a query tool call.
- [ ] 6.7 Spawned-process test (extends PR4's harness): `tools/list` omits `codekurve_reindex` by default; with `[mcp] allow_reindex = true`, `reindex` appears and a call triggers an index run.
- [ ] 6.8 Golden tests for `project_overview` and `doctor` tool responses.

## Phase 7: PR7 — Docs and Final Regression (req: mcp-server "AGENT_USAGE.md Documents the §28.4 Rules")

- [ ] 7.1 Create `docs/AGENT_USAGE.md`: document the 8 §28.4 rules verbatim (query before broad exploration; direct text search for literal strings; verify current source before editing; don't trust low-confidence edges for critical changes; use `trace_path` for flows; treat `analyze_impact` as a candidate list, not a guarantee; wait for watcher/run reindex after large changes; if a response says stale, read the current file).
- [ ] 7.2 `docs/AGENT_USAGE.md`: document client installation for connecting to `codekurve mcp` over stdio (Claude Code, Codex).
- [ ] 7.3 `README.md`: add MCP server section pointing to `docs/AGENT_USAGE.md` + quick-start config snippet.
- [ ] 7.4 Run the full pre-existing `crates/codekurve/tests/*` CLI golden suite one final time with zero edits; run all `codekurve-mcp` integration tests; confirm both pass on the merged chain.
