# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

## [0.3.0] - 2026-09-24

### Fixed

- `codekurve index` failed on ordinary real projects with `UNIQUE constraint
  failed: symbols.project_id, symbols.symbol_key` and indexed nothing:
  v0.2.11 could not index `dotnet/eShop`, `zod` or `effect`. Common constructs
  give two symbols in one file the same key: TypeScript interfaces of one name
  in different `namespace`s or merged declarations, same-named classes in two
  callbacks, C# explicit interface implementations (generated gRPC code), a
  generic and non-generic interface of one name. Later duplicates now get a
  distinct key; the first keeps its old one, so existing symbol ids do not
  change. Their qualified names are still identical (namespace, generic arity
  and explicit interface are not yet part of them).
- Apply `[ignore] patterns` during discovery. The patterns were parsed from
  `.codekurve/config.toml` but never used, so the default exclusions
  (`node_modules`, `dist`, `build`, `*.min.js`, `.env`, `secrets.*`, keys)
  only took effect when `.gitignore` also listed them. Files an existing index
  already holds that now match a pattern are removed on the next `index`. A
  pattern starting with `!` is rejected with a clear error instead of being
  silently ignored.

- MCP server: a tool that panicked never answered (the client waited
  forever), and it poisoned the session lock, so every later tool call
  panicked and never answered either. A panic now returns an `internal_error`
  response, and the next call reopens the session from disk.
- MCP server: tool bodies (SQLite queries, graph traversal, `reindex`) ran on
  the server's only runtime thread, so a slow call stopped it from reading
  any other message, pings and cancellations included. They now run on
  tokio's blocking pool. Tool calls are still serialized on the one session.

- MCP `get_symbol` and CLI `symbol` served stale source as current when a
  file was edited after indexing but stayed long enough to slice (the only
  check was the span's byte bounds): `get_symbol` returned shifted lines with
  `stale: false`, `symbol` printed them as `(live)`. Both now compare the
  file's content hash with the one stored at index time; a mismatch returns
  no source with `stale_reason: "file_changed"` (`(stale: …)` in the CLI).
  The MCP spec already required this ("get_symbol Reads Live Source and
  Flags Drift").
- Query latency no longer grows with project size: on a 10,000-file
  project MCP `search_symbols`/`find_callers` went from ~50-60 ms to under
  1 ms, `trace_path`/`analyze_impact` from ~100 ms to ~1 ms. Index writes now
  keep SQLite planner statistics current (none were ever collected, so
  lookups scanned whole tables); the stale warning on every tool call reads
  one row instead of counting four tables; and `trace`/`impact`/`export`
  fetch edges per visited node instead of loading the whole graph. Existing
  indexes pick up statistics on the next `codekurve index`, changes or not.
  Measured with the new `scripts/bench_queries.py`; see
  `docs/PERFORMANCE.md`.
- Traversal visits a node's edges in stored row order. Before, the order
  depended on the SQLite query plan, so among equal-length paths the one
  `trace` reported could differ between databases.

### Security

- A deeply nested source file could abort `codekurve index`, `watch` or the
  MCP server with an uncatchable stack overflow: ~20 KB of nested `[` in a
  `.ts` file killed `index`, and 5,000 levels killed the MCP server during
  `codekurve_reindex` (its 2 MiB blocking-pool stack). Extraction now runs
  on a dedicated thread with a 256 MiB stack reservation, and files nested
  deeper than 10,000 levels are skipped with a warning (indexed empty, not
  retried until they change). Cost: the extra depth walk adds ~6 % to cold
  index time (3.59 s -> 3.81 s on 10k files). `SECURITY_MODEL.md` no longer claims parse
  timeouts, memory budgets or redacted structured logs, none of which exist.

- `install.sh` and `install.ps1` (and so `codekurve update`) now verify the
  downloaded binary against the release's `SHA256SUMS` and refuse to install
  on a mismatch or a missing entry. Before this they installed whatever the
  download returned.
- Release workflow signs SLSA build provenance for every released file
  (`gh attestation verify`).
- Every GitHub Action in CI and Release is pinned to a commit SHA, and
  workflow tokens are read-only except the release `publish` job.

## 0.2.11 and earlier (history not split by version)

### Added

- Initialize Rust workspace (5 crates)
- Add governance documentation
- Add architecture decision records (0001-0010)
- Add cross-platform CI quality gates and licensing check
- Add `codekurve version` command
- Add typed error model and project configuration (`.codekurve/config.toml`)
- Add `codekurve init` command
- Add TypeScript/JavaScript file discovery honoring `.gitignore`
- Add Tree-sitter symbol extraction for classes and top-level functions
- Add SQLite storage with FTS5 (schema migration 0001) and symbol queries
- Add `index`, `search`, `symbol`, and `doctor` commands with live snippets
