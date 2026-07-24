# Design: Phase 2 — TypeScript Relationship Graph

## Technical Approach

Replace `extract.rs`'s flat `Vec<Symbol>` with a per-file `FileAnalysis` IR (§18), then run a
whole-project two-pass pipeline (§22): pass 1 parses each file to IR with stable local keys; pass 2
builds a project symbol table, resolves imports/references, and persists symbols + edges + unresolved
in one transaction. Traversal for `trace`/`impact` is hand-rolled BFS over an SQL-loaded adjacency
list — no petgraph (§50.1). Delivered as a 5-slice feature-branch chain, workspace green after each.

## Architecture Decisions

| Decision | Choice | Rejected | Rationale |
|----------|--------|----------|-----------|
| Domain enums vs IR structs placement | Enums (`SymbolKind`+14, `RelationshipKind`, `Provenance`, `Confidence`) in **core**; IR structs (`FileAnalysis`/`ExtractedSymbol`/`ExtractedRelationship`/`UnresolvedReference`) in **analysis**; store keeps its own persist-input structs referencing core enums | IR in core; IR in store | Store already depends on core only. Keeping IR in analysis and bridging in the binary (composition root) avoids a store→analysis dep and preserves the current direction. |
| contains / parent | **relationship rows** (`Contains` edges), no `parent_symbol_id` column | ALTER symbols to add `parent_symbol_id` | Proposal-locked; migration 0002 stays purely additive, no 0001 ALTER. `parent` still travels in-IR for qualified-name computation. |
| Ambiguous call target | **Low-confidence resolved edge** to each candidate; `unresolved_references` only for zero-candidate/insufficient-context | Silently pick first (§27.4 forbids); drop | §20.4 tiers; every edge keeps provenance/confidence; never lose a reference (§18.3). |
| Graph traversal | Hand-rolled BFS over `HashMap<sym_id, Vec<edge>>` loaded from SQL, depth/node caps + `truncated` | petgraph; recursive CTE | §50.1 mandates benchmark-first; adjacency BFS is a few lines and enough for MVP. |
| symbol_key | Drop `start_byte`; key tuple = `language / relative_path / kind / qualified_name` (§16.3), same `DefaultHasher` `hash_id` | Keep byte; adopt BLAKE3 now | Edge targets must survive reindex; BLAKE3 is a separate Phase 3 concern. |
| `CommandError{code,message}` | Land in **slice 5** (CLI queries) only; existing `String` errors keep mapping to code 1 via `From<String>` | Slice 1 refactor | Exit codes 4/6 are only observable once query commands exist; earlier is speculative (YAGNI). |
| Signature/visibility columns | **Deferred** — not needed by the 6 queries | Add now per §24.2 | Unused columns are dead weight; add when `overview`/signatures need them. |

## Data Flow

    discover ─→ [pass 1] extract::analyze(file) ─→ FileAnalysis{symbols,rels,unresolved}
                                                          │  (all files held in Vec, MVP batch)
                                                          ▼
                              [pass 2] resolve::resolve(&mut analyses)
                                  ├─ build SymbolTable (by_qualified, by_name, exports/file)
                                  ├─ module resolution (§20.2)  ─→ edge target_symbol_id | target_external
                                  └─ ref/call resolution (§20.4 confidence tiers)
                                                          ▼
                              repo::reindex(project, resolved)  ── one tx ──→ symbols + relationships + unresolved_references + FTS

`commands::index` changes: today it builds `Vec<FileInput>` and calls `reindex` in the discovery loop.
New shape — loop only does pass 1 into `Vec<FileAnalysis>`; after the loop, one `resolve` call, then one
`reindex` with the resolved project.

## Interfaces / Contracts

```rust
// core: enums only (persisted, shared)
enum RelationshipKind { Defines, Contains, Imports, Exports, References,
    Calls, Constructs, Inherits, Implements, Overrides, UsesType, Reads, Writes }
enum Provenance { Extracted, Resolved, Heuristic }
enum Confidence { Exact, High, Medium, Low, Unresolved }

// analysis: IR (§18). local_key = qualified_name within a file.
struct FileAnalysis { file: AnalyzedFile, symbols: Vec<ExtractedSymbol>,
    relationships: Vec<ExtractedRelationship>, unresolved: Vec<UnresolvedReference>, diagnostics: Vec<..> }
struct ExtractedSymbol { local_key, name, qualified_name, kind, language, span, parent: Option<String>, is_exported }
struct ExtractedRelationship { source_local_key, target: EdgeTarget, kind, span, provenance, confidence, reason }
enum EdgeTarget { Local(String), Global{file,qname}, External(String), Unresolved(String) }
struct UnresolvedReference { source_local_key: Option<String>, relationship_kind, target_text, context, candidate_count, reason, confidence }

// analysis::resolve — SymbolTable keyed for cross-file lookup
struct SymbolTable { by_qualified: HashMap<(RelPath,Qname), GlobalKey>,
    by_name: HashMap<String, Vec<GlobalKey>>, exports: HashMap<RelPath, HashMap<String,GlobalKey>> }
fn resolve(analyses: &mut [FileAnalysis], root: &Path, tsconfig: Option<&TsConfig>) -> ResolutionReport;
fn resolve_module(importer: &RelPath, specifier: &str, files: &FileSet) -> Option<RelPath>; // §20.2 order
```

Module resolution order (§20.2): relative path → exact file → implicit `.ts/.tsx/.js/.jsx` → `index.*` →
simple `tsconfig.json` `compilerOptions.paths` prefix aliases → else external node (`target_external`, no
symbol row, `node_modules` never indexed).

Query fns in `repo` back the commands: `references`/`callers`(target+kind=Calls)/`callees`(source+kind=Calls)/
`implementations`(kind=Implements/Inherits) are single indexed SELECTs; `trace`/`impact` load adjacency then BFS.

## Storage

Migration 0002 (`SCHEMA_VERSION=2`), additive only — creates `relationships` and `unresolved_references`
per §24.2 verbatim, plus indexes `relationships(source_symbol_id,kind)`, `(target_symbol_id,kind)`,
`(project_id,kind)`, `unresolved_references(project_id,target_text)`. `repo::reindex` gains a `DELETE ...
WHERE project_id` for both tables inside the existing tx and inserts resolved edges/unresolved alongside
symbols. `symbol_key` input drops `start_byte`; `qualified_name` comes from IR, not `= name`.

## Testing Strategy

| Layer | What | Approach |
|-------|------|----------|
| Unit | qualified_name (§20.3), module resolution order, confidence tiers, symbol_key stability across reindex | table tests in analysis/store |
| Unit | BFS depth cap + `truncated` flag | in-memory adjacency fixture |
| Integration | 9 relationship kinds from a complex TS fixture with expected provenance/confidence | fixture dir + `expected_relationships.json` (§34) |
| E2E | 6 commands: correct results, ambiguous→exit 6, missing index→exit 4, `--min-confidence`/`--depth`/`--json` | CLI integration test |

## Threat Matrix

N/A — no routing, shell, subprocess, VCS/PR automation, executable-file classification, or process-integration boundary. Module resolution reads project files inside the root only; `node_modules` and external packages are never opened.

## Migration / Rollout

Additive migration; rollback = drop the two tables, `SCHEMA_VERSION` back to 1. The `symbol_key` change
forces a full reindex, but there is no persisted prod data and the index is disposable (§5.5).

## Slice Boundaries (feature-branch chain, 400-line budget)

| # | Slice | Scope | Budget |
|---|-------|-------|--------|
| 1 | Core IR + symbol_key | core enums (16 SymbolKinds, RelationshipKind/Provenance/Confidence); analysis IR structs; `extract` returns `FileAnalysis` (empty relationships); store maps ExtractedSymbol; drop start_byte + real single-file qualified_name | Med |
| 2 | Migration 0002 | DDL + indexes, `SCHEMA_VERSION=2`; repo persist-edges/unresolved fns (called with empty vecs) | Low |
| 3 | Relationship extraction | `extract` emits imports/exports/heritage/calls/new/contains as intra-file edges (Extracted, local/unresolved targets); persist them | Med |
| 4 | Two-pass pipeline + resolution | `resolve.rs` (SymbolTable, module resolution, ref/call tiers); restructure `commands::index`; persist resolved + unresolved | **High** |
| 5 | CLI query commands | 6 commands + `--depth/--min-confidence/--json`; `CommandError{code,message}`; exit codes 4/6; repo query fns + BFS | **High** |

Slices 4 and 5 are the budget risks; the tasks phase may split each (resolution vs pipeline wiring; repo
query fns vs CLI dispatch). This feeds the Review Workload Forecast.

## Open Questions

- [ ] tsconfig alias depth — **recommend** minimal: parse `compilerOptions.paths` single-`*` prefix mappings only; ignore `baseUrl` chains and wildcards mid-segment. Anything richer is deferred.
- [ ] Hold-all-analyses-in-memory for pass 2 — **recommend** accept for MVP (§22.2 allows batch); revisit with bounded channels only when a large repo OOMs.
