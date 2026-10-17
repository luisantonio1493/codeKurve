# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Changed

- MCP responses use `schema_version` 2: `null` fields and an empty `warnings` are omitted; `trace_path` returns only the nodes on the found path (plus `reached_count`) instead of every visited node; `analyze_impact`/`trace_path` rows carry `qualified_name`; rows no longer carry `path`/`name` (derivable from `qualified_name`) or the constant `confidence`/`provenance` on symbol rows; tool descriptions and schema text are about 23% shorter; `truncated_reason` is reported; `search_symbols` reports `truncated` when more matches exist. The CLI `--json` output is unchanged (version 1)
- Store: schema version 7 adds indexes on `source_file_id` (relationships, unresolved references) and `source_symbol_id` (unresolved references), applied automatically on the next open. `find_references`/`find_callers`/`find_callees`/`find_implementations`/`find_unresolved`/`find_routes` page and filter by confidence in SQL (`total` unchanged); rows that tie on the sort key now come back in a stable order. `analyze_impact`/`trace_path` look up node names in one batched query
- Search: `search_symbols` matches name prefixes through the full-text index (`RegisterMan` finds `RegisterManager`), ranks an exact name match first, and no longer matches the symbol kind or file path columns (`class` used to return every class; a path still matches through the qualified name)

### Fixed

- Index: an incremental `index` no longer fails with `UNIQUE constraint failed: unresolved_references.id` when several changed files define same-named symbols that another file references
- Index: an incremental pass keeps edges from untouched files into symbols of re-analyzed files instead of demoting them to unresolved, so the result equals a clean rebuild (on a 350-file C# project, 5 changed files used to lose 1,509 edges)
- Index: editing a file with named imports no longer leaves them unresolved ("not exported by target module"): an incremental pass now equals a clean rebuild (covered by six equivalence tests)
- Index: files that cannot be read or walked keep their rows but leave the index marked stale (`status` pending count, stderr warning) until they can be verified again
- Watch: edits to the root `tsconfig.json` or `.codekurve/config.toml` reload the configuration and reconcile
- Index: a directory the walk cannot read, or a file that cannot be read during a full reindex, is no longer treated as a deletion (the existing index is kept and the failure reported)
- Index: non-UTF-8 files (e.g. Latin-1 C#) are indexed instead of failing the batch
- Index: touch-only edits refresh the stored mtime; bytes and mtime are read as a consistent pair
- Index: changing config that affects analysis (ignore patterns, languages, symlinks, tsconfig aliases) triggers one full reindex; overlapping tsconfig aliases resolve by longest prefix
- Index: creating a module re-resolves files that already import it; an unreadable dependent no longer aborts the whole batch
- Watch: the watcher is registered before the startup reconcile; watcher errors and rescan requests trigger a full sweep; failed batches are retried with bounded backoff
- Analysis: `.tsx` files are parsed with the TSX grammar
- Analysis: by-name matches with more than 8 candidates are recorded as unresolved ("ambiguous: N candidates") instead of one low-confidence edge each

### Security

- `install`: write client configs and `.bak` backups atomically and refuse symlinks
- Enforce `ignore.patterns` during discovery (secrets, `.env`, minified, `dist`)
- Contain `get_symbol` source reads to the project root; cap snippet size, `ctx_lines` and search `limit`
- Scope symbol lookup by id to the project
- Cap AST recursion depth and set a parse timeout so deeply nested files cannot crash indexing
- Create the index database and project config owner-only on Unix
- Reject `storage.database` outside the project root; warn when a project config enables `mcp.allow_reindex` or `index.follow_symlinks`
- Match route-search text literally (escape `LIKE` wildcards); recover from a poisoned MCP session lock

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
