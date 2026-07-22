# 0008. SQLite writes are serialized through a single coordinated writer

## Context

Indexing is parallelized (parsing, hashing) but must not corrupt the index
or degenerate into per-symbol commits. See CODEKURVE_MASTER_PLAN.md §22.1
(parallelism: "SQLite uses a coordinated writer... do not open one write
connection per file... do not commit per symbol") and §49.1/§49.2
(concurrency model: SQLite writer as a serialized-transaction component
separate from the Rayon parsing pool).

## Decision

All SQLite mutations go through one coordinated writer component. Parsing
and hashing run in parallel (Rayon), but produce results that are batched
and handed to the single writer for transactional persistence. Read queries
may use a small connection pool under WAL, but writes are never issued
concurrently from multiple workers.

## Alternatives

- **One write connection per worker thread**: SQLite serializes writers
  internally anyway (`busy_timeout`), so this only adds lock contention and
  retry complexity for no throughput gain — explicitly rejected by §22.1.
- **Per-symbol/per-file commits**: guarantees are weaker (§5.6 requires a
  failed index run not leave a file partially updated) and is far slower
  than batched transactions.

## Consequences

- Backpressure between parsing and writing is handled via bounded channels
  (§22.2), not by fanning out DB connections.
- A full index run is either one transaction (MVP-simple path) or uses
  `index_generation`/staging (§22.3) as the codebase grows — both preserve
  the single-writer invariant.
- Implementation lands with `codekurve-store` (later phase); see
  `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
