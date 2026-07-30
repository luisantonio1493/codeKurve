# Proposal: Phase 5 — C#

## Intent

CodeKurve only understands TypeScript/JavaScript. The target users are .NET teams (§ Fase 7 is Angular/.NET-aware), so a graph that stops at the frontend answers half of every real question: an agent asking "who calls this" gets nothing for the backend the frontend calls into. Phase 5 (§ "Fase 5 — C#") brings C# to basic parity with TypeScript — symbols, using directives, inheritance, implementation, calls, constructs, attributes, namespaces — with published limitations instead of pretended completeness, and zero TypeScript regression.

Phase 5 is also where the language layer stops being a coincidence. `extract::analyze` is one shared recursive tree-sitter walk that matches TS/JS node-kind strings inline; JavaScript reuses it only because TSX is a superset. C# shares no node vocabulary with it. Adding a second real language behind a `match` arm would make `extract.rs` a two-language file with no seam and would force an incompatible redesign in Phase 7. So this phase introduces the language-analyzer seam and moves TypeScript behind it, in the same change, before adding C#.

## Scope

### In Scope

**Architecture**
- A `LanguageAnalyzer` trait in `codekurve-analysis`, sized to exactly what TypeScript and C# each need today.
- Move the existing TypeScript/JavaScript extraction behind that trait (`languages/typescript.rs`), with `extract::analyze` reduced to dispatch. Behavior-preserving.
- Language-neutral `Visibility` on the IR, the domain `Symbol`, and the store.
- `RelationshipKind::Decorates`, shared by C# attributes and (later) TS decorators.
- `is_partial` and `is_record` modifiers on symbols (`record`/`record class` modeled as `SymbolKind::Class` + `is_record = true`; `record struct` → `SymbolKind::Struct` + `is_record = true`).
- Migration 0004: additive `symbols.visibility` + `symbols.is_partial` + `symbols.is_record`.

**C# vertical slice**
- `.cs` discovery (`LanguageId::CSharp`, `"csharp"` in the built-in default language list).
- Symbols: namespaces (block and file-scoped), classes, interfaces, structs, records, enums, constructors, methods, properties, fields, nested types.
- `using` directives (plain, `using static`, `using alias =`) as `Imports`.
- Base list: base class → `Inherits`, interface → `Implements`, disambiguated at resolve time.
- Direct invocations → `Calls`; object creation → `Constructs`.
- Attributes → `Decorates`, preserving the original attribute name text and the attribute's own source span.
- Visibility modifiers: all six C# levels, including the two compound levels.
- Generic type parameters and constraint clauses recorded structurally (names + constraint text in `signature_fingerprint` only; no edges are created from constraints in Phase 5).
- Namespace-aware `qualified_name`; using-scoped cross-file resolution; language-filtered resolution candidates.
- Fixtures: `csharp-graph` analysis fixture, `fixtures/csharp/basic` CLI vertical slice, mixed TS+C# regression fixture.
- Published limitations document.

### Out of Scope
- Semantic compilation (Roslyn, MSBuild, any out-of-process compiler). Staying in-process tree-sitter matches the existing "no daemon, no port, no network" constraints.
- NuGet / package resolution; BCL type resolution. External types stay `External`/unresolved.
- Extension-method resolution beyond direct syntactic evidence.
- Cross-file partial-type merging (see Known Limitations).
- Source generators, reflection, `dynamic`, runtime DI dispatch.
- Framework awareness: ASP.NET routing, Azure Functions triggers, EF Core mappings, DI container wiring (§ Fase 7).
- Solution/multi-project assembly resolution. The project model is a single named root (`[project] name`/`root` in `codekurve-core::config`) with no `.sln`/`.csproj` awareness, so "already supported" here means: every `.cs` file under the root is one flat project, regardless of csproj boundaries.
- Operators, indexers, events, delegates, top-level statements as indexed symbols.
- Angular/.NET cross-stack edges (frontend call → backend endpoint).

## Capabilities

### New Capabilities
- `csharp-analysis`: the C# analyzer, its node-kind mapping, namespace-aware qualified names, using-scoped resolution, C# visibility, partial flagging, attribute edges.

### Modified Capabilities
- `symbol-index`: `LanguageAnalyzer` seam; `Visibility`; `is_partial`; `is_record`; migration 0004; `.cs` discovery.
- `relationship-graph`: `RelationshipKind::Decorates`; resolution candidates filtered by language; base-list class-vs-interface disambiguation; `kind_matches` becomes per-language.

## Approach

`extract::analyze(source, language, relative_path) -> Result<FileAnalysis>` keeps its signature and becomes a dispatcher over `LanguageAnalyzer` implementations. Every existing caller (`commands.rs`, `incremental.rs`) is untouched.

The trait carries only what both implementations need today:

```rust
pub trait LanguageAnalyzer {
    fn language(&self) -> LanguageId;
    fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis>;
    fn kind_matches(&self, rel: RelationshipKind, sym: SymbolKind) -> bool;
}
```

No registry plugin system, no per-node visitor framework, no config-driven language loading. `TypeScriptAnalyzer` handles both TypeScript and JavaScript exactly as today (grammar choice included); `CSharpAnalyzer` is the second implementation. `kind_matches` is on the trait rather than a widened neutral function because C# and TypeScript disagree about it (a C# `Inherits` target may be a class or struct — records fold into `Class`/`Struct` via `is_record`; widening one shared union would silently change TypeScript resolution outcomes, which is precisely the regression this phase must not have).

`ir.rs`, `resolve.rs`'s resolution algorithm, the store, and every query stay language-agnostic. `resolve.rs` changes only where it must: it dispatches `kind_matches` through the analyzer of the edge's source language, and it refuses candidates from a different language (a call in a `.cs` file never resolves to a TypeScript symbol that happens to share a name).

C# `qualified_name` stays path-prefixed — `src/Billing/Invoice.cs::Acme.Billing.Invoice.Total` — so `EdgeTarget::Global { file, qualified_name }` and every persisted-key scheme keep working unchanged. The namespace is a prefix inside the second component, not a new addressing dimension.

## Key Decisions

| Question | Decision | Rationale |
|---|---|---|
| Trait now or after a third language | Now, in this phase, before C# lands | A `match` arm over two unrelated node vocabularies has no seam; Phase 7 adds more languages and frameworks |
| Trait width | 3 methods, no registry/visitor/plugin surface | Only what TS and C# each need today; enough that a third language does not need a redesign |
| `kind_matches` | Per-language trait method | A shared widened union would change TypeScript resolution results |
| Visibility model | New neutral `Visibility` enum, independent of `is_exported` | C# access modifiers and the TS `export` keyword are different concepts; `is_exported` keeps meaning "declared with `export`" and stays `false` for C# |
| Compound C# modifiers | `ProtectedInternal` and `PrivateProtected` are their own values | Union vs intersection semantics; collapsing them onto `protected`/`internal` would be a wrong answer, not a coarse one |
| Attributes | `RelationshipKind::Decorates`, source = annotated declaration, target = attribute type; target text = original attribute name, span = the attribute's own span | One kind serves C# attributes and TS decorators; no new IR field needed to preserve name+span; no framework-specific semantics (`[HttpGet]` is just a name) |
| Partial types | One symbol per declaration, `is_partial = true`, no merging | Merging needs a whole-project pass and a canonical-symbol concept; deferred as a published limitation |
| Partial identity | `symbol_key` gains a trailing `partial_ordinal` component, emitted **only** for partial declarations | Two fragments of one type in one file would otherwise collide on `UNIQUE(project_id, symbol_key)`; non-partial keys stay byte-identical, so no TS reindex is forced |
| Later merge compatibility | A future merge adds a separate canonical type symbol plus fragment edges; fragment keys are never rekeyed | Nothing downstream may assume "one symbol = the type"; `resolve.rs` already treats multiple same-name candidates as a legitimate ambiguity set |
| Base list | Emit one pending entry per base; classify at resolve time by the resolved candidate's `SymbolKind` | C# has no `implements` keyword; only the project symbol table can tell class from interface |
| Unresolved base entry | `UnresolvedReference` with `relationship_kind = UsesType` and an explicit reason, never a guess | No `I`-prefix naming heuristic; "never silently pick" (§20.4/§27.4) |
| `using X.Y` | `Imports` to the namespace symbol; one Low-confidence edge per candidate when several files declare it; `External` when nothing in-project declares it | Same policy `node_modules` imports already follow |
| Records | No new `SymbolKind`; `record`/`record class` → `Class` + `is_record = true`, `record struct` → `Struct` + `is_record = true` | Avoids a kind every downstream consumer (CLI filters, MCP `search_symbols`) must tolerate for a phase-5 nicety; `is_record` still answers "was this a record" without widening the enum |
| Enum members | `SymbolKind::Field`, parent = the enum | No `EnumMember` kind for one phase's convenience |
| Grammar | `tree-sitter-c-sharp`, in-process | Same ecosystem as the existing `tree-sitter-typescript` pin; no Roslyn/subprocess anywhere in the plan |
| Default languages | `"csharp"` joins the built-in default list | Existing configs list their languages explicitly and are unaffected |
| Generic constraints | Structural only: names + constraint text in `signature_fingerprint`; no `UsesType` (or any) edges emitted from constraint clauses | Keeps Phase 5 generics scope at "record syntactically", not "resolve type arguments" |
| `internal` visibility | Recorded as its own `Visibility` value; resolution confidence is not reduced for it | No `.sln`/`.csproj` boundary exists to enforce assembly scope; approximating one via confidence would be a guess, not a signal — documented as a limitation instead |

## Migration Impact

The TypeScript extractor move is a real migration, not a rename.

1. `crates/codekurve-analysis/src/extract.rs` (~730 lines) is today one file: the public `analyze`, a `CollectCtx`, a recursive `collect`, and ~20 TS-specific helpers (`collect_heritage`, `collect_imports`, `collect_exports`, `module_specifier`, `is_default_export`, `method_kind`, `qualified_name`, `signature_fingerprint`, …). Its body moves to `languages/typescript.rs` behind `impl LanguageAnalyzer for TypeScriptAnalyzer`, with no logic edits.
2. `extract.rs` keeps the public `analyze` entry point (now dispatch) plus the shared `span_of`/`find_child`-class helpers that both analyzers use. `extract::kind_matches` is currently `pub(crate)` and consumed by `resolve.rs`; it becomes a trait method, so `resolve.rs`'s call site changes shape (it must know the edge's source language) even though its answers for TS do not change.
3. `Visibility`, `is_partial`, and `is_record` are additive fields on `ir::ExtractedSymbol` and `codekurve_core::Symbol`. Every existing TS construction site sets `Visibility::Default` / `is_partial: false` / `is_record: false`.
4. `codekurve-store`: migration 0004 adds three columns with defaults (`visibility TEXT NOT NULL DEFAULT 'default'`, `is_partial INTEGER NOT NULL DEFAULT 0`, `is_record INTEGER NOT NULL DEFAULT 0`). Unlike migration 0003, it does **not** wipe project data: none of the three columns participate in `symbol_key`, so no stored id changes. Pre-0004 rows read as `default`/`false`/`false` until the next reindex — honest, not corrupt.
5. `repo::symbol_key` gains a sixth parameter, `partial_ordinal: Option<u32>`. With `None` the hashed input string is byte-for-byte what today's five-component `format!` produces, so every existing TypeScript key and symbol id survives the migration unchanged. This is asserted by a golden test on a known key, not by inspection.

Risk profile of the migration: large mechanical diff, small semantic diff. It is deliberately its own PR (PR2) with zero new features, so a regression can be bisected to a move rather than to C#.

## Backward Compatibility Guarantee for TypeScript

The exit criterion "no regresión TypeScript" is stated here as testable claims, each one a check that fails loudly:

1. `crates/codekurve-analysis/tests/relationship_graph_fixture.rs` and the whole `fixtures/ts-graph/` assertion set (RelationshipKind counts, specific cross-file edges, unresolved-reference preservation) pass **unchanged** — no edited expectations, no added tolerance — after the trait refactor and after the C# analyzer lands.
2. `crates/codekurve-bin/tests/*` CLI goldens (including `vertical_slice.rs` and `incremental_golden.rs`) pass unchanged; TypeScript stdout is byte-identical with and without `--json`.
3. Golden assertion: `symbol_key(lang, path, kind, qname, fingerprint, None)` equals a hardcoded pre-migration hash for a known TypeScript symbol.
4. A TypeScript-only project indexed under migration 0004 produces the same symbol ids and the same relationship rows as before the migration (existing `incremental_golden.rs` digest query, extended over the new columns' defaults).
5. A mixed TS+C# fixture produces zero cross-language edges: no TS symbol resolves a C# reference and vice versa.
6. Any diff to a file under `fixtures/ts-graph/` or to a TS golden expectation is a review blocker for this change, not a routine edit.

## Test Fixtures Plan

Mirrors the existing dual convention (analysis-level multi-file fixture + CLI vertical slice).

| Fixture | Location | Purpose |
|---|---|---|
| `csharp-graph/project/` | `crates/codekurve-analysis/tests/fixtures/csharp-graph/project/` | Multi-file C# project: 2 namespaces across 3+ files, interface + implementation in different files, base class in a third, cross-namespace `using`, calls and object creation across files, attributes, a partial class split across two files, a generic class with a `where` constraint |
| `csharp_graph_fixture.rs` | `crates/codekurve-analysis/tests/` | Extract + resolve over that project; asserts per-`RelationshipKind` counts, named cross-file `Inherits`/`Implements`/`Calls`/`Constructs`/`Imports`/`Decorates` edges, and that unresolved BCL references are preserved as rows with reasons — same shape as `relationship_graph_fixture.rs` |
| `csharp-graph/*.cs` single-file cases | same dir | Visibility matrix (all six levels incl. both compounds), file-scoped vs block namespace, nested types, enum members, records vs `record struct`, target-typed `new()` |
| `fixtures/csharp/basic/` | repo `fixtures/` | CLI round-trip source tree |
| `vertical_slice_csharp.rs` | `crates/codekurve-bin/tests/` | Full CLI: `init` → `index` → `search` → `symbol` → `callers`/`implementations`, asserting stdout — mirrors `vertical_slice.rs` |
| `fixtures/mixed/` + `mixed_language.rs` | `fixtures/`, `crates/codekurve-analysis/tests/` | TS and C# in one project sharing a type name; asserts zero cross-language edges and that both languages are indexed in one run |
| TS regression (existing) | `fixtures/ts-graph/`, `fixtures/typescript/basic/` | Reused **unedited** as the trait-refactor regression suite (see Backward Compatibility) |
| `partial_identity.rs` | `crates/codekurve-analysis/tests/` or store tests | Two partial fragments of one type in one file get distinct keys; two fragments across files each keep their own; non-partial key equals the pre-migration golden hash |

## Acceptance Criteria

- [ ] A multi-file C# fixture project indexes end to end; `codekurve search`/`symbol`/`callers`/`callees`/`implementations`/`references`/`trace`/`impact` all answer against it.
- [ ] Cross-file resolution works: interface implemented in another file, base class in another file, call and object creation across namespaces, all resolved with provenance and confidence.
- [ ] All six C# visibility levels round-trip through IR → store → query output, with `protected internal` and `private protected` distinct from `protected` and `internal`.
- [ ] Attributes appear as `Decorates` edges carrying the original attribute name text and the attribute's own span. No attribute name is special-cased.
- [ ] Every partial declaration is indexed, flagged `is_partial`, and two fragments in one file do not collide.
- [ ] Generic type parameters and `where` constraints are recorded in `signature_fingerprint`; no edges are created from them.
- [ ] Unresolved references (BCL, NuGet, target-typed `new()`, alias-qualified names, unresolved base entries) are preserved as `unresolved_references` rows with explicit reasons — never dropped, never guessed.
- [ ] `extract.rs` contains no C#-specific and no TypeScript-specific node-kind string; both live in their own `languages/` module.
- [ ] Every TypeScript claim in "Backward Compatibility Guarantee" holds, with no edits to existing TS fixtures or golden expectations.
- [ ] Migration 0004 applies to an existing populated index without wiping it, and pre-0004 symbol ids are unchanged.
- [ ] Limitations are published in a user-visible document (not only in this proposal).
- [ ] `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` green after every PR in the chain; `unsafe_code = "forbid"` holds.

## Known Limitations (to be published)

Every deferred item is a limitation a user can hit, so each one is documented, not just omitted.

| Limitation | Effect on results |
|---|---|
| No semantic compilation (no Roslyn/MSBuild) | Resolution is syntactic; overload selection, inferred types and implicit conversions are not modelled |
| Partial types not merged | Each `partial` declaration is its own symbol; a reference to the type may resolve to several fragments as a legitimate ambiguity set (Low confidence, one edge per candidate) |
| No NuGet / BCL resolution | References to package or framework types are `External` or unresolved with a reason; `[HttpGet]`, `Task`, `List<T>` do not resolve to definitions |
| Generics are structural only | No instantiation resolution and no constraint edges: `Repository<Invoice>` does not link to `Invoice` through the type argument, and `where T : IComparable` creates no `UsesType` edge; type parameter names and constraint text are recorded in `signature_fingerprint`, nothing more |
| Extension methods | Only direct syntactic evidence; `obj.Extension()` does not resolve to the static extension method's declaring type |
| Overload resolution | Calls resolve by name; multiple overloads produce one Low-confidence edge per candidate |
| `using static` | Recorded as `Imports`; member references made visible by it are not resolved through it |
| `using alias = X.Y` | Recorded as `Imports`; alias-qualified references are recorded as unresolved with an explicit reason |
| `global using` | Treated as file-local; not applied project-wide |
| Source generators | Generated code is not indexed; partial members it supplies appear unresolved |
| Reflection, `dynamic`, runtime DI | Not modelled at all; no edge is inferred from a container registration |
| No solution/project model | All `.cs` files under the root are one flat project; `.sln`/`.csproj` boundaries, per-project references and assembly-level `internal` scoping are ignored, so `internal` is recorded as its own `Visibility` value but not enforced as a resolution boundary, and resolution confidence is not reduced for it |
| No framework semantics | No routing, trigger, EF mapping or DI edges (§ Fase 7) |
| Not indexed as symbols | Operators, indexers, events, delegates, local functions, lambdas, top-level statements |
| Target-typed `new()` | No type name at the call site → unresolved `Constructs` with a reason |
| TS decorators | `Decorates` exists and is shared, but the TypeScript side is not wired in this phase |
| Preprocessor directives | `#if` branches are parsed as written by the grammar; no configuration-conditional evaluation |

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `crates/codekurve-core/src/language.rs` | Modified | `LanguageId::CSharp`; `cs` extension; `"csharp"` name |
| `crates/codekurve-core/src/symbol.rs` | Modified | `Visibility` enum; `RelationshipKind::Decorates`; `Symbol.visibility`/`is_partial`/`is_record` |
| `crates/codekurve-core/src/config.rs` | Modified | `"csharp"` in the default `index.languages` |
| `crates/codekurve-analysis/src/extract.rs` | Modified | Reduced to dispatch + shared helpers; TS body moved out |
| `crates/codekurve-analysis/src/languages/mod.rs` | New | `LanguageAnalyzer` trait, `analyzer_for(LanguageId)` |
| `crates/codekurve-analysis/src/languages/typescript.rs` | New (moved) | Existing TS/JS extraction, unchanged logic |
| `crates/codekurve-analysis/src/languages/csharp.rs` | New | C# analyzer |
| `crates/codekurve-analysis/src/ir.rs` | Modified | `visibility`, `is_partial`, `is_record` on `ExtractedSymbol` (still language-agnostic) |
| `crates/codekurve-analysis/src/resolve.rs` | Modified | Per-language `kind_matches` dispatch; language-filtered candidates; base-list classification |
| `crates/codekurve-analysis/src/discovery.rs` | Modified | `.cs` picked up via `LanguageId::from_extension` (comment about the TS/JS-only extension filter updated) |
| `crates/codekurve-store/src/migrations.rs` | Modified | Migration 0004 (additive); `SCHEMA_VERSION = 4` |
| `crates/codekurve-store/src/repo.rs` | Modified | `symbol_key` `partial_ordinal`; persist/read `visibility`/`is_partial`/`is_record` |
| `crates/codekurve-analysis/Cargo.toml` | Modified | `tree-sitter-c-sharp` |
| `docs/LANGUAGES.md`, `README.md` | New/Modified | Published limitations, supported-language matrix |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| The trait refactor silently changes TS results | Med | Refactor is its own PR with zero features; existing TS fixtures/goldens must pass unedited |
| `tree-sitter-c-sharp` incompatible with the workspace `tree-sitter 0.25` | Med | First task of the C# PR is to pin a release whose `tree-sitter` requirement the workspace satisfies (same situation as the existing `tree-sitter-typescript = 0.23` pin); fallback is aligning the core version, decided before any extraction work |
| Namespace-qualified names break `resolve.rs`'s path-based addressing | Med | Namespace stays a prefix inside `qualified_name`; `EdgeTarget::Global` shape unchanged; covered by the cross-file fixture |
| Partial fragments explode ambiguity and flood Low-confidence edges | Med | Existing §20.4 policy already handles multi-candidate sets; fixture asserts the exact counts so the blast radius is visible, not surprising |
| Cross-language name collisions produce false edges | Med | Language-filtered candidates + the mixed-language fixture asserting zero cross-language edges |
| Trait grows into a plugin framework | Med | Three methods, fixed; a fourth needs a design-phase justification tied to a real TS or C# need |
| Migration 0004 forces an unnecessary full reindex | Low | Neither new column touches `symbol_key`; golden test on a pre-migration key hash |
| C# node-kind coverage drifts toward endless scope | Med | The in/out lists above are the contract; anything else lands in Known Limitations rather than in code |

## Rollback Plan

Revert the feature-branch chain. Migration 0004 is additive: drop the two columns (or leave them; they are defaulted and unread by the prior build) and set `SCHEMA_VERSION` back to 3. No stored TypeScript id changes, so a rollback needs no reindex — and the index is disposable anyway (§5.5). C# rows disappear with the revert because nothing discovers `.cs` files without `LanguageId::CSharp`.

## Dependencies

- Phase 2 (relationship graph, IR, resolver), Phase 3 (incremental/watcher), Phase 4 (MCP) — all archived.
- New crate: `tree-sitter-c-sharp` (exact pin decided in PR3, verified against the workspace `tree-sitter` version).
- No other new dependency. No Roslyn, no MSBuild, no subprocess, no network.

## PR Breakdown

### Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~2600–3400 |
| 400-line budget risk | High |
| Chained PRs recommended | Yes |
| Suggested split | PR1 → PR2 → PR3 → PR4 → PR5 → PR6 → PR7 |
| Delivery strategy | auto-forecast |
| Chain strategy | feature-branch-chain |

Decision needed before apply: No
Chained PRs recommended: Yes
Chain strategy: feature-branch-chain
400-line budget risk: High

### Suggested Work Units

| Unit | Goal | Likely PR | Focused test command | Runtime harness | Rollback boundary |
|------|------|-----------|----------------------|-----------------|-------------------|
| 1 | Domain + storage groundwork, no behavior change: `LanguageId::CSharp`, `Visibility`, `is_record`, `RelationshipKind::Decorates`, IR fields, migration 0004, `symbol_key` `partial_ordinal` (base=tracker) | PR1 | `cargo test -p codekurve-core -p codekurve-store` | Index the existing TS fixture on a pre-0004 DB; assert migration applies without wipe and symbol ids are unchanged | Revert the enum/field additions + migration 0004 + `SCHEMA_VERSION` bump |
| 2 | `LanguageAnalyzer` trait; TS/JS extraction moved to `languages/typescript.rs`; `extract::analyze` becomes dispatch; `kind_matches` becomes per-language (base=PR1) | PR2 | `cargo test -p codekurve-analysis` + `cargo test --workspace` | Index `fixtures/typescript/basic`, byte-diff all CLI stdout against a pre-refactor baseline | Revert `languages/` + restore `extract.rs` body and the `resolve.rs` call site |
| 3 | C# analyzer part 1 — grammar pin, `.cs` discovery, symbols (namespace/class/interface/struct/record/enum/ctor/method/property/field/nested), visibility, `is_partial`, generics fingerprint, `Contains`/`Defines` (base=PR2) | PR3 | `cargo test -p codekurve-analysis csharp` | `codekurve index` + `search` over `fixtures/csharp/basic`, eyeball symbol kinds and visibility | Revert `languages/csharp.rs` + the `tree-sitter-c-sharp` dependency |
| 4 | C# analyzer part 2 — `using` directives, base list, calls, object creation, attributes/`Decorates`, `UsesType` (base=PR3) | PR4 | `cargo test -p codekurve-analysis csharp` | Single-file C# cases through `analyze`, assert edge kinds/spans/name texts | Revert the edge-emitting functions in `languages/csharp.rs` |
| 5 | Resolution — namespace-aware qualified names, using-scoped candidate lookup, language-filtered candidates, base-list class-vs-interface classification, unresolved rows with reasons (base=PR4) | PR5 | `cargo test -p codekurve-analysis resolve` | Multi-file C# fixture through extract+resolve; assert cross-file `Inherits`/`Implements`/`Calls` and preserved unresolved rows | Revert `resolve.rs` C#-path changes; C# stays file-local |
| 6 | Fixtures and goldens — `csharp-graph`, `fixtures/csharp/basic`, `vertical_slice_csharp.rs`, mixed-language fixture, partial-identity test, full TS regression re-run (base=PR5) | PR6 | `cargo test --workspace` | Full CLI round-trip on the C# fixture; confirm TS goldens pass with zero edits | Revert the new fixture dirs + test files |
| 7 | Docs — `docs/LANGUAGES.md` limitations, README language matrix (base=PR6, merges to tracker) | PR7 | N/A — doc-only | Manual read-through against the Known Limitations table | Revert `docs/LANGUAGES.md` + README section |

PR2 is the largest single diff and the one with the least new behavior; it is kept separate precisely so a TypeScript regression bisects to a move. PR6 is where the exit criteria "proyecto C# de prueba" and "no regresión TypeScript" are actually proven; PR7 is where "limitations published" is.
