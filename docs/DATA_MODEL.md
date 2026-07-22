# Data Model

Not implemented in Phase 0 — this documents the design target so future
slices build against a stable reference.

## Schema

SQLite with WAL, `PRAGMA foreign_keys = ON`, FTS5 for symbol search. Core
tables include `projects`, `files`, symbols, and edges. Full schema: plan
§24.

## Identity (§16)

- `file_key` = `BLAKE3(project_id + normalized_relative_path)`.
- `content_hash` = `BLAKE3(file_bytes)` — the source of truth for change
  detection, not `mtime`.
- `symbol_key` = deterministic hash of `(language, relative_path,
  symbol_kind, qualified_name, signature_fingerprint)`; `symbol_id` is the
  persisted internal identifier.

## Domain types (§17)

`LanguageId`, `SymbolKind`, `RelationshipKind`, `Provenance` (`Extracted`,
`Resolved`, `Heuristic`), `Confidence` (`Exact`, `High`, `Medium`, `Low`,
`Unresolved`), `SourceSpan`. Every edge carries confidence and provenance;
no relationship is presented as definitive if it was resolved heuristically
(plan §0.15).

## Deferred decision

Early introduction of `index_generation` tracking is deferred to the store
implementation phase (Phase 1+), not part of Phase 0 scaffolding.
