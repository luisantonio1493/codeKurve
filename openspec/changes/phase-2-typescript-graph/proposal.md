# Proposal: Phase 2 — TypeScript Relationship Graph

## Intent

Phase 1 ships a flat symbol index with a placeholder `qualified_name = name` and no edges. It answers "where is this defined" but not "where is it used, who calls it, what breaks if I change it". Phase 2 turns the flat index into a real TypeScript relationship graph, queryable from the CLI, with per-edge provenance and confidence (plan §0.15, §17.4-5).

## Scope

### In Scope
- Relationship extraction: imports, exports, contains, extends/inherits, implements, calls, constructs, references, unresolved (§17.3, §20).
- Real `qualified_name` computation replacing the Phase 1 placeholder (§20.3), with module resolution order per §20.2.
- Two-pass pipeline: parse per-file to `FileAnalysis` IR (§18) → build whole-project symbol table → resolve → persist (§22). This restructuring is its own review-sized slice, distinct from extraction.
- Migration 0002: `relationships` + `unresolved_references` tables and their indexes (§24.2) — additive, no ALTER of 0001.
- Resolution module in `codekurve-analysis`.
- 6 hand-rolled CLI commands: references, callers, callees, implementations, trace, impact.
- Ambiguity handling at both layers (query-time vs resolution-time).

### Out of Scope
- C#/Angular/.NET framework awareness (later phases, §20.5).
- MCP server (Phase 4); incremental/watcher indexing (Phase 3).
- petgraph (benchmark first, §50.2); full §27.3 exit-code taxonomy; clap.

## Capabilities

### New Capabilities
- `relationship-graph`: edge model, migration 0002, resolution pipeline, provenance/confidence.
- `graph-queries`: the 6 CLI commands + traversal (BFS over SQL adjacency list).

### Modified Capabilities
- `symbol-index`: real `qualified_name`; `symbol_key` stops embedding `start_byte` (§16.3) so edge targets stay stable across reindex.

## Approach

Extend the IR to `FileAnalysis{symbols, relationships, unresolved, diagnostics}`. Pass 1 parses each file to IR. Pass 2 builds a project symbol table, resolves references against it, and persists edges in one transaction (§22.3). Traversal for trace/impact is hand-rolled BFS over an SQL-loaded adjacency list — no petgraph until benchmarked (§50.1).

Key locked decisions: contains/parent as relationship rows (not an ALTER); ambiguous call sites → Low-confidence resolved edges (§20.4), `unresolved_references` reserved for zero-candidate/insufficient-context; add exit codes 6 (ambiguous) and 4 (index missing) only, defer the rest of §27.3.

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `crates/codekurve-core/src/symbol.rs` | Modified | Add SymbolKinds (§17.2), RelationshipKind, qualified_name/parent fields |
| `crates/codekurve-analysis/src/extract.rs` | Modified | Emit FileAnalysis IR; parse imports/exports/heritage/calls/new |
| `crates/codekurve-analysis/` (new resolution mod) | New | Project symbol table + reference resolution |
| `crates/codekurve-store/src/migrations.rs` | Modified | Migration 0002; SCHEMA_VERSION=2 |
| `crates/codekurve-store/src/repo.rs` | Modified | Real qualified_name; stable symbol_key; persist edges |
| `crates/codekurve/src/{cli,commands,main}.rs` | Modified | 6 new commands; exit codes 4 and 6; --min-confidence/--depth flags |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Pipeline restructuring blows the 400-line budget | High | Auto-forecast splits into chained PRs; keep extraction, pipeline, migration, commands as separate slices |
| symbol_key change breaks Phase 1 reindex identity | Med | Ship key fix with real qualified_name in the same slice; drop/rebuild index (no prod data) |
| Over-resolving ambiguous calls produces false edges | Med | Confidence tiers (§20.4); never silently pick first (§27.4); provenance on every edge |
| Traversal unbounded on large graphs | Low | depth/nodes/edges/time limits + `truncated: true` (§50.3) |

## Rollback Plan

Migration 0002 is additive; drop `relationships`/`unresolved_references`, set SCHEMA_VERSION back to 1. Revert the feature-branch chain. Phase 1 commands keep working since no 0001 table is altered. The `symbol_key` change requires a full reindex either way (no persisted prod data).

## Dependencies

- Phase 1 (symbol index, SQLite store, hand-rolled CLI) — done.
- No new crates. Reuse tree-sitter, rusqlite already in the workspace.

## Success Criteria

- [ ] All 9 relationship kinds extracted from a complex TypeScript fixture with correct provenance/confidence.
- [ ] `qualified_name` matches §20.3 examples; symbol_key stable across reindex.
- [ ] The 6 commands return correct results; ambiguous names exit 6, missing index exits 4.
- [ ] `trace`/`impact` traverse the graph with enforced depth/node limits and truncation signal.
- [ ] `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace` green after every slice; `unsafe_code = "forbid"` holds.
