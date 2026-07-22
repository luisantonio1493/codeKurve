# 0007. Every relationship carries explicit provenance and confidence

## Context

Static analysis cannot always resolve a reference exactly. CodeKurve must
not present a guess as a fact. See CODEKURVE_MASTER_PLAN.md §5.3 (explicit
provenance), §5.4 (explicit confidence), and the domain types in §17.4
(`Provenance`) and §17.5 (`Confidence`).

## Decision

Every `Relationship` record carries a `Provenance` (`Extracted`, `Resolved`,
or `Heuristic` in the MVP; `External` reserved for later) and a `Confidence`
(`Exact`, `High`, `Medium`, `Low`, `Unresolved`). Both fields are mandatory,
never inferred implicitly by callers, and surfaced in query results.

## Alternatives

- **Boolean "resolved" flag only**: loses the distinction between "found by
  direct extraction," "resolved via project-wide analysis," and "inferred
  heuristically" — collapses information CLI/MCP consumers need to trust or
  filter results.
- **No confidence field, best-effort silently**: rejected outright — hiding
  uncertainty is explicitly disallowed (§5.4: "tools must not hide
  uncertainty").

## Consequences

- CLI/MCP tools can and should expose a `--min-confidence` filter (see
  §20.4 for the TypeScript call-resolution example).
- Any future analyzer or resolver must populate both fields honestly rather
  than defaulting to `Exact`/`Extracted`.
- Implementation lands with the domain model in `codekurve-core` (later
  phase); see `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
