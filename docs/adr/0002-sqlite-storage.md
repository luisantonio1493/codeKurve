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
- Open/deferred point: whether `rusqlite`'s `bundled` feature (statically
  linked SQLite) is acceptable for enterprise distribution, or whether a
  system-linked SQLite is required instead, is not yet decided — to be
  confirmed when the store crate is implemented (Phase 1+), not in Phase 0.
- Implementation (schema, PRAGMAs, migrations) lands in a later phase; see
  `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
