# Tasks: Phase 2 — TypeScript Relationship Graph

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~1900-2300 total across the chain |
| 400-line budget risk | High (slices 4 and 5 individually exceed 400 lines) |
| Chained PRs recommended | Yes |
| Suggested split | PR1 → PR2 → PR3 → PR4a → PR4b → PR5a → PR5b (7 PRs) |
| Delivery strategy | auto-forecast |
| Chain strategy | feature-branch-chain |

Decision needed before apply: No
Chained PRs recommended: Yes
Chain strategy: feature-branch-chain
400-line budget risk: High

Slices 4 ("Two-pass pipeline + resolution") and 5 ("CLI query commands") are each split into two PRs per the design's suggestion: resolution logic vs pipeline wiring, and repo query/traversal fns vs CLI dispatch.

### Suggested Work Units

| Unit | Goal | PR | Focused test command | Runtime harness | Rollback boundary |
|------|------|----|-----------------------|------------------|--------------------|
| 1 | Core IR + symbol_key fix | PR1 (base: tracker `phase-2-typescript-graph`) | `cargo test -p codekurve-analysis -p codekurve-store` | N/A — no CLI surface change yet, exercised via unit tests | Revert `symbol.rs`/`ir.rs`/`extract.rs`/`repo.rs` diff; no persisted prod data |
| 2 | Migration 0002 (empty edges) | PR2 (base: PR1) | `cargo test -p codekurve-store migrations::` | `codekurve doctor --root <tmp>` shows schema version 2 | Drop migration block, reset `SCHEMA_VERSION`; isolated additions |
| 3 | Intra-file relationship extraction | PR3 (base: PR2) | `cargo test -p codekurve-analysis relationship_extraction` | N/A — analysis-crate only, no CLI wiring yet | Revert `extract.rs` edge-emission diff; PR2 call sites unaffected |
| 4a | Resolution module (`resolve.rs`) | PR4a (base: PR3) | `cargo test -p codekurve-analysis resolve::` | N/A — pure library module, not yet wired into `index` | Delete `resolve.rs` + `mod resolve;` line; nothing depends on it yet |
| 4b | Two-pass pipeline wiring + acceptance fixture | PR4b (base: PR4a) | `cargo test -p codekurve-analysis --test relationship_graph_fixture` | `codekurve index --root <fixture-dir>` on the new multi-file TS fixture | Revert `commands::index` to PR3's single-loop shape; fixture/test files removable independently |
| 5a | Repo query fns + BFS traversal | PR5a (base: PR4b) | `cargo test -p codekurve-store repo:: traverse::` | N/A — library fns, no command callers yet | Revert `repo.rs`/new `traverse.rs`; PR4b pipeline unaffected |
| 5b | CLI query commands + exit codes | PR5b (base: PR5a) | `cargo test -p codekurve --test graph_queries` | `codekurve callers/trace/impact --root <tmp> --json` end-to-end | Revert `cli.rs`/`commands.rs`/`main.rs` dispatch + `CommandError`; PR5a fns stay unused but harmless |

Every PR: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace` must be green before moving to the next PR in the chain.

**Skill match**: `chained-pr` (registry `gentle-ai-chained-pr`) — feature-branch-chain, PR1 targets tracker branch, each child PR targets the immediate previous PR branch, only the tracker merges to `main`.

**Design/spec note**: spec text says `symbol_key = BLAKE3(...)`; design explicitly rejects adopting BLAKE3 now (Phase 3 concern) and keeps the existing `DefaultHasher`-based `hash_id`, only dropping `start_byte` from the key tuple. Tasks below follow the design decision (more specific rationale); flagged as a risk in the phase result.

## PR1: Core IR + symbol_key (base: tracker branch)

- [x] 1.1 `crates/codekurve-core/src/symbol.rs`: extend `SymbolKind` from 2 to the full set from plan §17.2 (verify exact list against the plan doc during apply); add `RelationshipKind` (13 variants: Defines, Contains, Imports, Exports, References, Calls, Constructs, Inherits, Implements, Overrides, UsesType, Reads, Writes), `Provenance{Extracted,Resolved,Heuristic}`, `Confidence{Exact,High,Medium,Low,Unresolved}`.
- [x] 1.2 `crates/codekurve-analysis/src/ir.rs` (new): `FileAnalysis{file,symbols,relationships,unresolved,diagnostics}`, `ExtractedSymbol{local_key,name,qualified_name,kind,language,span,parent:Option<String>,is_exported}`, `ExtractedRelationship{source_local_key,target:EdgeTarget,kind,span,provenance,confidence,reason}`, `EdgeTarget{Local,Global,External,Unresolved}`, `UnresolvedReference{..}` per design §Interfaces.
- [x] 1.3 `crates/codekurve-analysis/src/extract.rs`: rename `extract_symbols` → `analyze(source, language, relative_path) -> Result<FileAnalysis>`; compute real `qualified_name` per §20.3 (`relative_path::Name`, `relative_path::Class.method`); populate `parent`; relationships/unresolved stay empty.
- [x] 1.4 `crates/codekurve-store/src/repo.rs`: `symbol_key` drops `start_byte` from the tuple (`language/relative_path/kind/qualified_name`, same `hash_id`); `qualified_name` comes from the IR, not `= name`.
- [x] 1.5 `crates/codekurve/src/commands.rs`: bridge `extract::analyze` output into `repo::FileInput` (composition-root mapping); `index()` behavior unchanged otherwise.
- [x] 1.6 Test: `crates/codekurve-analysis/src/extract.rs` unit test — nested method qualified_name (`src/services/member.service.ts::MemberService.getEligibility`, spec scenario "Nested member qualified name").
- [x] 1.7 Test: `crates/codekurve-store/src/repo.rs` unit test — reindexing with only `start_byte` changed on a later symbol keeps `symbol_key` stable (spec scenario "Reindex after unrelated edit").

## PR2: Migration 0002 (base: PR1)

- [x] 2.1 `crates/codekurve-store/src/migrations.rs`: add `MIGRATION_0002` (`relationships`, `unresolved_references` DDL + indexes per §24.2/§24.4), bump `SCHEMA_VERSION = 2`, extend `apply()` for the `current < 2` branch.
- [x] 2.2 `crates/codekurve-store/src/repo.rs`: add `persist_relationships`/`persist_unresolved` (insert-only, called with empty vecs for now); add `DELETE ... WHERE project_id` for both new tables inside the existing `reindex` tx.
- [x] 2.3 `crates/codekurve/src/commands.rs` `doctor()`: report `SCHEMA_VERSION` via a new `codekurve_store::migrations::current_version(&conn)` helper.
- [x] 2.4 Test: `crates/codekurve-store/src/migrations.rs` — fresh DB ends at version 2, both tables exist (spec scenario "Fresh database migration").
- [x] 2.5 Test: `crates/codekurve/tests/vertical_slice.rs` (extend) — `doctor` stdout contains `[ok] schema: version 2`.

## PR3: Intra-file relationship extraction (base: PR2)

**Split note (apply-time)**: 3.1's full scope (Contains + heritage + same-file Calls/Constructs) measured ~549 changed lines in one branch, clearly over the 400-line budget — split into three chained sub-PRs: PR3a (Contains + Inherits/Implements, 406/-38), PR3b (Calls/Constructs, 123/-24, base PR3a), and PR3c (Imports/Exports, 316 changed lines, base PR3b). 3.2 was out of the PR3a/PR3b apply run's given scope and stayed `[ ]` until PR3c closed the gap.

- [x] 3.1 `crates/codekurve-analysis/src/extract.rs`: emit `Contains` (class→method/property), `Inherits`/`Implements` (heritage clause), same-file `Calls`/`Constructs` — all `Provenance::Extracted`, `EdgeTarget::Local` when in-file resolvable else `Unresolved(text)`. (done across PR3a + PR3b, see split note)
- [x] 3.2 `crates/codekurve-analysis/src/extract.rs`: emit `Imports`/`Exports` edges with `EdgeTarget::Unresolved(module_specifier)` (module resolution deferred to PR4a). (done in PR3c; local named/default exports without a `from` clause resolve against the file's own symbols via the same deferred-resolution machinery as heritage edges, per §20.4's "never silently pick" — cross-file targets stay `Unresolved(module_specifier)`)
- [x] 3.3 `crates/codekurve-store/src/repo.rs`: wire `persist_relationships` (PR2) to real same-file edges from `FileAnalysis.relationships`. (test-proven with real `Contains` edge data; commands.rs pipeline wiring stays PR4b per design's stated runtime harness)
- [x] 3.4 `crates/codekurve-analysis/tests/fixtures/ts-graph/`: new fixture dir; add `heritage.ts` (`class Foo extends Base implements IFoo`) and `contains.ts` (class + 2 methods).
- [x] 3.5 Test: `crates/codekurve-analysis/tests/relationship_extraction.rs` (new) — asserts one `extends` + one `implements` edge (spec scenario "Class extends and implements") and two `contains` rows (spec scenario "Contains hierarchy"). Plus extract.rs unit tests for the same-file `Calls`/`Constructs` cases (method calling a sibling method; `new` expression).

## PR4a: Resolution module (base: PR3c)

**Split note (apply-time)**: base is PR3c (`phase-2/typescript-graph-pr3c-imports-exports`), not PR3 directly — PR3 was split into PR3a/PR3b/PR3c, see PR3's split note above. The full resolution module (symbol table + module resolution + reference/call resolution + tests) measured ~740 changed lines in one branch, clearly over the 400-line budget — split into two chained sub-PRs: **PR4a-1** (`resolve.rs` new file: `SymbolTable` + `resolve_module` + helpers, ~314 changed lines) and **PR4a-2** (base: PR4a-1; extends the same file with `resolve()`/by-name/import/export resolution + `extract.rs`'s small `pub(crate)` exposures, budget-sized separately). `SymbolTable` ships as `{by_name, exports}` (no separate `by_qualified` map — its only use case, existence/kind lookup, is already covered by `by_name`; dropped as unused, YAGNI). `SymbolTable::build` also carries a documented fallback (PR4a-1): since `extract.rs` (PR3c) never emits an `Exports` edge for direct declaration exports (`export function x() {}`), every top-level Class/Function/Interface symbol is additionally registered under its own name so the common `export function x(){}` + `import { x }` pattern still resolves (`is_exported` isn't tracked yet, so this may over-register a not-actually-exported symbol — accepted MVP over-inclusion).

- [x] 4a.1 `crates/codekurve-analysis/src/resolve.rs` (new): `SymbolTable{by_name, exports}` built from `&[FileAnalysis]`. (PR4a-1)
- [x] 4a.2 `resolve.rs`: `resolve_module()` implementing §20.2 order — relative path → exact file → implicit `.ts/.tsx/.js/.jsx` → `index.*` → `tsconfig.json` `compilerOptions.paths` single-`*` prefix alias → external node. (PR4a-1)
- [x] 4a.3 `resolve.rs`: `resolve()` — import/export edge resolution + call/construct confidence tiers (§20.4): Exact (same-file), High (single cross-file candidate), Low (multi-candidate — one edge per candidate, never pick first), zero-candidate → `UnresolvedReference`. (PR4a-2, base PR4a-1; also exposes `crate::extract::{kind_matches, NO_SAME_FILE_MATCH_REASON}` as `pub(crate)` so cross-file resolution reuses the same kind-compatibility rule and can tell a same-file-export-miss apart from a module-specifier re-export)
- [x] 4a.4 `crates/codekurve-analysis/src/lib.rs`: add `pub mod resolve;`. (PR4a-1)
- [x] 4a.5 Test: table test — implicit-extension + `index.*` resolves Exact (scenario "Implicit extension resolution"); tsconfig alias resolves; relative-to-missing-file is Unresolved; external package → external node (scenario "External package import"); `SymbolTable` direct-declaration-export fallback. (PR4a-1)
- [x] 4a.6 Test: table test — cross-file call resolves High to the single candidate (PR4a-2); multi-candidate cross-file calls → Low-confidence/Heuristic-provenance edges, no unresolved row (scenario "Multi-candidate call is not unresolved" / "Ambiguous member call", PR4a-2); 0-candidate call → `UnresolvedReference` with `candidate_count=0` (scenario "Zero-candidate import" principle, PR4a-2); end-to-end named-import and external-import resolution through `resolve()` (PR4a-2).

**Known gaps carried forward to PR4b** (documented, not silently mis-resolved): a `"default"`-reason import/export binding always resolves to `UnresolvedReference` (extraction never tags which local export is literally `export default`, see extract.rs's `collect_exports` doc comment); a namespace import (`* as ns`) or `export * from` resolves to a synthetic `EdgeTarget::Global{file, qualified_name: "*"}` module-level placeholder rather than a real `Module` symbol (none is emitted yet) and `export *` re-exported names are not propagated into the re-exporting file's own `exports` table (no transitive/fixed-point pass).

## PR4b: Two-pass pipeline wiring + acceptance fixture (base: PR4a)

**Split note (apply-time)**: the full scope (pipeline restructuring + tsconfig loader + persistence wiring + multi-file acceptance fixture + acceptance test) measured ~549 changed lines in one branch, over the 400-line budget — split into two chained sub-PRs, mirroring the PR3a/b/c and PR4a-1/PR4a-2 precedent: **PR4b-1** (pipeline restructuring + persistence wiring, 4b.1/4b.2/4b.3/4b.6, ~396 changed lines) and **PR4b-2** (base: PR4b-1; acceptance fixture + integration test, 4b.4/4b.5/4b.7, ~137 changed lines).

**PR4b-1 apply-time notes**: (1) `RelationshipInput`/`UnresolvedReferenceInput` (PR2) identify symbols by pre-resolved id strings, but ids are only assigned inside `repo::reindex`'s own insert loop — `repo.rs` now exposes deterministic `file_id`/`symbol_key`/`symbol_id` helpers (used internally by `reindex` too, single source of truth) so `commands.rs` can precompute matching ids and build the source/target maps before calling `reindex`, without `codekurve-store` depending on `codekurve-analysis`. (2) `relationships.source_symbol_id` is `NOT NULL` (§24.2), but `Imports`/`Exports` edges are extracted at file level, not symbol level, and no `Module` symbol kind is emitted by `extract::analyze` yet (documented PR4a gap) — `commands.rs` now synthesizes one `SymbolKind::Module` symbol per file (keyed by its own relative path) as a stand-in source, closing the gap without touching the analysis crate.

- [x] 4b.1 `crates/codekurve/src/commands.rs`: restructure `index()` — pass 1 loop only builds `Vec<FileAnalysis>` (collect parse errors, no per-file persist); pass 2: one `resolve::resolve(&mut analyses, &aliases)` call; one `repo::reindex(project, resolved)` call. (PR4b-1)
- [x] 4b.2 `crates/codekurve/src/commands.rs`: minimal `tsconfig.json` loader (parses `compilerOptions.paths` only, hand-rolled narrow-shape scan — no `serde_json`, deliberately scoped to PR5b) passed into `resolve::resolve`. (PR4b-1)
- [x] 4b.3 `crates/codekurve-store/src/repo.rs`: `reindex()` persists resolved relationships + unresolved refs in the same tx as symbols (spec "Atomic persistence on failure"). (PR4b-1)
- [ ] 4b.4 `crates/codekurve-analysis/tests/fixtures/ts-graph/`: extend to a multi-file fixture covering all 9 kinds (imports, exports, contains, extends, implements, calls, constructs, references, unresolved) + `expected_relationships.json` (kind/source/target/provenance/confidence tuples) — the fixture infra this phase needs.
- [ ] 4b.5 Test (acceptance gate, §34.2-34.3): `crates/codekurve-analysis/tests/relationship_graph_fixture.rs` (new) — runs discovery+`analyze`+`resolve` over the fixture, asserts every `expected_relationships.json` row present with matching provenance/confidence.
- [x] 4b.6 Test: `crates/codekurve-store/src/repo.rs` — `reindex_rolls_back_completely_on_relationship_error` — a relationship row violating the `source_symbol_id` FK leaves zero partial rows (spec scenario "Atomic persistence on failure"). (PR4b-1)
- [ ] 4b.7 Test: `crates/codekurve/tests/vertical_slice.rs` (extend) — `codekurve index` stdout reports relationship counts.

## PR5a: Repo query fns + BFS traversal (base: PR4b)

- [ ] 5a.1 `crates/codekurve-store/src/repo.rs`: add `references`/`callers`/`callees`(kind=Calls)/`implementations`(kind=Implements|Inherits) — single indexed SELECTs, filterable by `min_confidence`.
- [ ] 5a.2 `crates/codekurve-store/src/traverse.rs` (new): `load_adjacency(project_id) -> HashMap<SymbolId, Vec<Edge>>`; `bfs()` forward (trace) and reverse (impact) with depth/node/edge/time caps + `Truncated{reason}`.
- [ ] 5a.3 `crates/codekurve-store/src/repo.rs`: `find_candidates_by_name` — all matches with qualified_name, for CLI ambiguity handling (PR5b).
- [ ] 5a.4 Test: `repo.rs` — callers/callees/references/implementations against seeded relationships (spec scenarios "Callers of a symbol", "Min-confidence filter").
- [ ] 5a.5 Test: `traverse.rs` — path found within depth (scenario "Path found within depth"); depth exceeded → `truncated:true, reason:max_depth` (scenario "Depth limit exceeded"); reverse impact truncation (scenario "Impact truncation").

## PR5b: CLI query commands (base: PR5a)

- [ ] 5b.1 `crates/codekurve/src/commands.rs`: `CommandError{code:u8, message:String}` + `impl From<String> for CommandError` (code 1) — lands here only, per design.
- [ ] 5b.2 `crates/codekurve/Cargo.toml`: add `serde_json = "1"` (JSON envelope, no existing dep covers it).
- [ ] 5b.3 `crates/codekurve/src/cli.rs`: extend hand-rolled `Args` parsing for `--depth`, `--min-confidence`, `--json`, `--symbol-id`/`--symbol-name`, `--limit`/`--offset` (no clap).
- [ ] 5b.4 `crates/codekurve/src/commands.rs`: add `references`/`callers`/`callees`/`implementations`/`trace`/`impact` fns using PR5a repo/traverse fns; bare-name ambiguity → `CommandError{code:6}` listing all candidates (spec "Ambiguous name lookup"); missing index → `CommandError{code:4}` (spec "Query before first index").
- [ ] 5b.5 `crates/codekurve/src/commands.rs`: `--json` envelope `{schema_version, project, result, warnings, truncated}` via `serde_json` (spec "JSON envelope shape").
- [ ] 5b.6 `crates/codekurve/src/main.rs`: dispatch the 6 new commands; map `CommandError.code` to `ExitCode`.
- [ ] 5b.7 Test: `crates/codekurve/tests/graph_queries.rs` (new) — callers returns confidence/provenance; `--min-confidence high` filters Low out; bare-name ambiguity exits 6 with both candidates listed; qualified name resolves exit 0; query before index exits 4; `--json` output has all 5 envelope fields.
