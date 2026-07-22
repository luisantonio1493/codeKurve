# 0002. SQLite as the index storage engine

## Context

The extracted symbol/relationship graph needs a durable, queryable, portable
store that a single local process can own without external infrastructure.
See CODEKURVE_MASTER_PLAN.md §24 (schema, PRAGMAs, FTS5 requirement) and the
non-negotiable principle that the index is disposable and rebuildable (§5.5).

## Decision

Use SQLite (via `rusqlite`) with WAL journal mode and FTS5 as the sole
persistence layer for the index. No embedded graph database, no external
DB server.

## Alternatives

- **Embedded graph DB (e.g. sled + custom graph layer)**: no mature,
  auditable, cross-platform embedded graph store fits the "no exotic
  runtime dependency" constraint; would require building query/FTS
  primitives from scratch.
- **External DB server (Postgres, etc.)**: violates local-first (§5.1) and
  adds an operational dependency the tool must not require.
- **Flat files / custom binary format**: no transactional guarantees
  (§5.6), no FTS, reinvents query capability SQLite already provides.

## Consequences

- Index is a single file, trivially disposable/rebuildable (§5.5); never the
  sole source of truth.
- Concurrency is constrained to SQLite's model: WAL for concurrent reads,
  one coordinated writer for mutations (see ADR 0008).
- `rusqlite` is used with the `bundled` feature (statically linked SQLite),
  decided when the store crate landed in Phase 1. Rationale: Windows has no
  system SQLite, so the 3-OS CI matrix needs a self-contained build; `bundled`
  guarantees FTS5 is compiled in and pins a reproducible SQLite version; SQLite
  is public domain, so it does not conflict with the pending licensing stance
  (`docs/LICENSING.md`). Revisit only if enterprise distribution or binary size
  forces a system-linked build.
- Implementation (schema, PRAGMAs, migrations) lands in a later phase; see
  `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
