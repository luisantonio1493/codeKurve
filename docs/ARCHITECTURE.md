# Architecture

## Components and data flow

The CLI, the MCP `stdio` server and the TUI sit on top of one application
crate (`codekurve`) that owns project lifecycle, indexing, queries and
diagnostics. It drives discovery/analysis (`codekurve-analysis`) and the
SQLite store (`codekurve-store`) (plan §10).

```text
discover (ignore rules, [ignore] patterns, size/count caps)
  -> extract (tree-sitter, per file) -> resolve (cross-file, confidence)
  -> store (one SQLite transaction per batch) -> refresh planner statistics
query: CLI / MCP / TUI -> codekurve::query -> store (indexed lookups, lazy BFS)
```

## Crate graph (as built)

```text
codekurve-bin ──> codekurve, codekurve-mcp, codekurve-tui, codekurve-core
codekurve-mcp ──> codekurve                  (+ rmcp, tokio)
codekurve-tui ──> codekurve, codekurve-core  (+ ratatui, crossterm)
codekurve ──────> codekurve-analysis, codekurve-store, codekurve-core
codekurve-analysis ─> codekurve-core         (+ tree-sitter, ignore)
codekurve-store ────> codekurve-core         (+ rusqlite bundled, blake3)
codekurve-core ─────> (nothing internal)
```

- `codekurve-core` is the dependency sink: domain types only (`Symbol`,
  `Relationship`, `Confidence`, `Provenance`, config, errors). It never
  depends on SQLite, MCP or the CLI.
- `codekurve` (the application crate) is the only crate the front ends call.
  `codekurve-mcp` and `codekurve-tui` go through `codekurve::query`, never
  through the store directly, so all three front ends share one query layer.
- `codekurve-bin` holds only argument parsing and dispatch. Arguments are
  parsed by hand (`crates/codekurve-bin/src/cli.rs`); `clap` has not been
  needed yet.
- `tokio` is confined to `codekurve-mcp`; every other crate is synchronous.

## Concurrency

- **Indexing** runs one batch at a time on a dedicated analysis thread
  (`extract::on_analysis_stack`, one per batch, 256 MiB stack reservation):
  the extractors recurse per syntax-tree level, and callers' stacks go down
  to tokio's 2 MiB. Within a batch, discovery, parsing and resolution are
  sequential, then one transaction writes it. This is fast enough for the
  budgets in `docs/PERFORMANCE.md` (10k files in ~3.4 s), so parsing has not
  been parallelized. Files deeper than `languages::MAX_SYNTAX_DEPTH` are
  skipped with a warning.
- **Single writer** (ADR 0008): SQLite in WAL mode; each batch is one
  transaction. The watcher applies batches one at a time on its own thread.
- **MCP server**: a current-thread tokio runtime reads JSON-RPC over stdio.
  Tool bodies are synchronous SQLite work, so each runs on tokio's blocking
  pool (`CodeKurve::blocking`) to keep the runtime thread free for other
  messages, pings and cancellations. Calls are serialized on one `Session`
  (one connection) behind a mutex; a panicking tool returns an error and the
  session is reopened from disk.

## Query performance

- Index writes refresh SQLite planner statistics
  (`codekurve_store::db::refresh_planner_stats`). Without them SQLite treats
  `project_id` as selective, which it is not (one project per database), and
  picks full-project scans.
- `trace`, `impact` and `export` walk the graph with a bounded BFS that
  fetches each visited node's edges on demand (`traverse::LazyAdjacency`), so
  their cost follows the BFS caps, not the project size.
- Measured with `scripts/bench_queries.py`; results in
  `docs/PERFORMANCE.md`.

## Errors

`codekurve-core` and `codekurve-store` use typed `thiserror` enums. The
application crate still passes most errors as `String` (and `CommandError`
with an exit code at the command boundary); moving it to a typed error is a
tracked follow-up in `docs/IMPROVEMENT_PLAN.md`.

## No-network policy

The application does not depend on an HTTP client or any network I/O.
Adding a network-capable crate requires an ADR (plan §29.4). The one scoped
exception is `codekurve update` / `uninstall --binary`, which spawn the
install script (ADR 0012, `docs/SECURITY_MODEL.md`).

## Not yet introduced

- **`tracing`**: output is plain `println!`/`eprintln!`; the MCP crate is
  compiler-restricted to stderr so stdout stays pure JSON-RPC.
