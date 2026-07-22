# 0006. Source code is not duplicated into the index by default

## Context

The index stores symbol/relationship metadata, not a copy of the codebase.
See CODEKURVE_MASTER_PLAN.md §25 (source snippets without duplicating code).

## Decision

By default, CodeKurve does not persist full source text in the SQLite
index. Query responses that need source (e.g. a symbol snippet) read the
current file from disk at query time, using stored spans/hashes to locate
and validate the content, and mark the response stale if the on-disk hash no
longer matches.

## Alternatives

- **Always store full source in the DB**: simplest to query, but doubles
  storage, duplicates a source of truth that already exists (the working
  tree), and creates a staleness/consistency problem the moment a file
  changes outside an index run.
- **Store only on first query (lazy cache)**: adds cache-invalidation
  complexity with no MVP benefit over reading the file directly.

## Consequences

- Query latency depends on a disk read + span extraction, not a DB blob
  fetch — acceptable for a local tool (§5.9 requires measurement before
  performance claims).
- A future opt-in `[index] store_source = true` config may be added for
  portable/offline index use cases, but is explicitly out of scope for the
  MVP.
- Reduces risk of leaking full source contents through the index file
  itself if it is copied or shared.

## Status

Accepted (2026-07-22)
