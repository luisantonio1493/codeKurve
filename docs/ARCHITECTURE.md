# Architecture

## Components and data flow

CLI and the MCP `stdio` server sit on top of shared application services
(project lifecycle, indexing, queries, diagnostics), which in turn drive
discovery/analysis, the query engine, and the SQLite store (plan §10).

## Crate boundaries and dependency direction (§11.2)

```text
codekurve (bin) ──> core, analysis, store, mcp   (composition root)
codekurve-mcp ─────> core, store
codekurve-analysis ─> core
codekurve-store ───> core
codekurve-core ────> (nothing internal)
```

`codekurve-core` is the dependency sink: domain types only (`Project`,
`Symbol`, `Relationship`, `Confidence`, `Provenance`, ...), never depends on
CLI, SQLite, or MCP. `codekurve` (the bin) holds no parsing logic or complex
SQL. Full crate responsibilities: plan §11.2.

**Phase 0 reality**: all crates are empty skeletons and currently declare no
internal dependencies (avoids unused-dependency clippy noise). The graph
above is the documented target, enforced as each crate gains real code.

## Concurrency (target shape, not yet implemented)

Main runtime handles CLI/MCP lifecycle, cancellation, and watcher
coordination; a worker pool handles hashing and parsing; SQLite writes are
serialized through a single writer. Full model: plan §49.

## No-network policy

The application must not depend on an HTTP client or any network I/O.
Adding a network-capable crate requires an ADR (plan §29.4).

## Deferred decisions (Phase 0)

- **Tracing infrastructure**: not introduced yet. `codekurve version`
  currently uses plain `println!` to stdout, no `tracing` subscriber. The
  observability plan (plan §30) calls for `tracing`; it enters once a
  command has real work to instrument, not for a single hardcoded print.
- **Full `CK_*` error model**: not introduced yet. Phase 0 has no fallible
  path (`version` cannot fail), so only a naming *convention* is assumed
  for future error codes; the concrete model lands with the first
  fallible command (plan §31).
- **`clap`**: not introduced yet. A single `version` subcommand does not
  justify a parser; args are hand-matched via `std::env::args()`. `clap`
  enters at plan §46 slice 13 once real subcommands (`init`, `index`)
  exist.
