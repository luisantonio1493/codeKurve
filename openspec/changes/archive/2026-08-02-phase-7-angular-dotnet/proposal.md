# Proposal: Phase 7 — Angular and .NET Aware

## Intent

After phases 2 and 5, Codekurve knows TypeScript and C# *as languages* and nothing about the frameworks the target teams actually write. An agent can answer "who calls `InvoiceService.Total`" but not "which HTTP route reaches the database when a user submits this form" — and that second question is the one an Angular/.NET team asks. Today the graph breaks at every framework seam: an Angular constructor parameter is a type reference with no meaning, `[HttpGet("invoices/{id}")]` is a literal attribute name (phase 5 deliberately recorded it with zero semantics), `services.AddScoped<IRepo, Repo>()` is an ordinary call, and `DbSet<Invoice>` is a field whose generic argument phase 5 explicitly refused to link.

Phase 7 (§ "Fase 7 — Angular y .NET aware") closes those seams so a route-to-data-layer path is a traversable chain of typed edges, every framework edge carries provenance, and every inference is visibly a heuristic rather than a fact.

## Scope

### In Scope

**Model**
- New `RelationshipKind` variants from the pre-planned § 17.3 list, restricted to what this phase emits: `Injects`, `RegisteredAs`, `HandlesRoute`, `Triggers`, `PersistsTo`.
- Framework-role tags per § 17.2 (`Controller, Route, Service, Repository, Component, Decorator`) as tags on existing symbols — **not** new `SymbolKind` variants.
- Additive store migration for role tags and any new edge metadata.

**TypeScript extraction (framework-agnostic)**
- Decorator walking in `languages/typescript.rs`: class, method, property, and constructor-parameter decorators → generic `Decorates` edges carrying the literal decorator name and its own span, exactly as `csharp.rs` already does for attributes. No Angular name is special-cased at this layer.

**Framework recognition pass (heuristic, separate module)**
- Angular: `@Component`/`@Injectable` role tagging; constructor-parameter DI and `inject(Foo)` → `Injects`; `providers: [...]` → `RegisteredAs`; `Routes` config arrays → `HandlesRoute`, including `canActivate` guards and `HTTP_INTERCEPTORS` registrations; `standalone` component `imports: [...]`.
- .NET: `[ApiController]`/`[Route]`/`[HttpGet…]` controllers → `HandlesRoute`; minimal-API `app.MapGet/MapPost(...)` → `HandlesRoute`; `services.Add{Scoped,Transient,Singleton}<TService, TImpl>()` → `RegisteredAs`; Azure Functions `[Function]`/trigger attributes → `Triggers`; `app.Use*` middleware chains; `DbContext` subclasses and their `DbSet<T>` members → `PersistsTo`.
- Every edge emitted by this pass is `Provenance::Heuristic` with a calibrated `Confidence`, never `Extracted`/`Resolved`/`Exact`.

**Fixtures and docs**
- `fixtures/angular/` and `fixtures/dotnet/` trees each demonstrating a complete route → handler → injected service → data-layer chain, asserted end to end.
- Published limitations, in the phase-5 style.

### Out of Scope
- Roslyn, MSBuild, `tsc`, or any out-of-process type checker. The in-process tree-sitter-only constraint from phases 5 and 6 holds; framework knowledge is pattern recognition, not compilation.
- Angular HTML templates: `routerLink`, template bindings, `*ngIf`/control flow, component usage in markup. No HTML/template grammar is added.
- Legacy `@NgModule` `declarations`/`entryComponents` semantics beyond the `providers`/`imports` arrays already covered. Revisit if the phase 8 pilot repo needs it.
- Other TS decorator frameworks (NestJS, TypeORM, class-validator). They inherit the generic `Decorates` edges for free; no dedicated recognition.
- Blazor, Razor, MVC views, SignalR, gRPC, MassTransit/message-bus wiring (`Publishes`/`Subscribes` stay deferred).
- Runtime DI dispatch, reflection-based registration, assembly scanning, convention-based MVC routing with no attribute evidence.
- Cross-stack edges (an Angular `HttpClient` call linked to the .NET endpoint it hits). That is a phase 8+ question and needs a URL-matching model this phase does not build.
- General generic type-argument resolution. Phase 5's "generics are structural only" rule stands; any exception is bounded to what this phase's data-layer criterion demands (see open question 3).

## Capabilities

### New Capabilities
- `framework-awareness`: the heuristic recognition pass, its Angular and .NET pattern catalogue, framework-role tagging, and the provenance/confidence rules that keep inference distinguishable from fact.

### Modified Capabilities
- `relationship-graph`: TypeScript decorators produce `Decorates` edges; the new `Injects`/`RegisteredAs`/`HandlesRoute`/`Triggers`/`PersistsTo` kinds and their resolution rules.
- `symbol-index`: framework-role tags on symbols plus the additive migration that stores them.

## Approach

Recommended approach from exploration (approach 1 of 3): **generic extraction first, heuristic framework recognition as a separate pass.**

```
discover → extract (language, facts only) → resolve (names) → recognize (frameworks, heuristics) → store
```

The language analyzers stay framework-blind. `typescript.rs` gains decorator walking that knows nothing about Angular, mirroring what `csharp.rs` already does for attributes — which also means every other TS decorator framework becomes visible at zero extra cost. A new `frameworks/` module then consumes already-extracted and already-resolved edges (`Decorates` names, `Calls`, `Constructs`, type references) and pattern-matches them into the § 17.3 edge kinds.

Rationale:
- It is the phase-5 precedent, not a new idea. Phase 5's spec requirement "Attributes Produce Decorates Edges" explicitly forbids framework special-casing inside the analyzer; putting `@Component` recognition into `typescript.rs` would contradict a rule the codebase already enforces.
- It keeps `Provenance::Extracted` (syntactic fact) and `Provenance::Heuristic` (inference) in physically separate code paths, so a guess cannot accidentally be written as a fact. That separation is what makes the "heuristics clearly marked" exit criterion structurally true rather than a review promise.
- Framework catalogues evolve on a different clock than grammars. A new Angular idiom should not reopen the TypeScript analyzer.
- The exit criterion falls out for free: a route-to-data-layer path is just `HandlesRoute → Injects → Calls → PersistsTo`, traversable by the existing MCP path/trace tools with no MCP-layer change, since those tools are already `RelationshipKind`-shape agnostic.

Rejected: (2) baking recognition into the language analyzers — cheaper-looking, contradicts the phase-5 rule, conflates fact and guess, and is hard to fixture independently. (3) real type-checker integration via `tsc`/Roslyn — higher accuracy but violates the in-process/no-network/no-MSBuild constraint that phases 5 and 6 both committed to.

## Open Questions (for design phase)

These are deliberately unresolved here. `sdd-design` must answer all five before implementation starts.

| # | Question | Why it blocks design |
|---|---|---|
| 1 | **Angular DI inference**: how is a constructor parameter type or `inject(Foo)` argument matched to the injectable class, and what confidence does a partial/ambiguous match get? | More inference surface than any prior phase attempted; a wrong answer floods the graph with false `Injects` edges |
| 2 | **.NET sub-feature split**: minimal APIs and Azure Functions are call-expression patterns, structurally unlike attribute-based controllers. Are they one .NET slice or two, and do they share a recognition mechanism? | Determines PR chain shape and whether one pattern engine covers both |
| 3 | **EF Core `DbSet<T>`**: phase 5 explicitly emits no edges from generic type arguments. Does phase 7 take a documented, narrowly-scoped exception, or reach the data layer another way? | Without a decision, the route→data-layer exit criterion has no implementation path |
| 4 | **Framework-role tag storage**: new column on `symbols`, a side table, or derived at query time? No prior phase decided this. | Drives the migration and whether tags are queryable/filterable |
| 5 | **Array-literal patterns**: Angular `imports: [...]`, `providers: [...]`, `canActivate: [...]`, and route arrays have no precedent in the phase 2–5 edge model. What is the extraction shape? | Several in-scope Angular features depend on one shared answer |

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `crates/codekurve-core/src/symbol.rs` | Modified | New `RelationshipKind` variants; framework-role tag representation |
| `crates/codekurve-analysis/src/languages/typescript.rs` | Modified | Decorator walking → generic `Decorates` edges |
| `crates/codekurve-analysis/src/languages/csharp.rs` | Unchanged | Attribute extraction already generic; recognition reads its output |
| `crates/codekurve-analysis/src/frameworks/mod.rs` | New | Recognition pass entry point, provenance/confidence policy |
| `crates/codekurve-analysis/src/frameworks/angular.rs` | New | Angular pattern catalogue |
| `crates/codekurve-analysis/src/frameworks/dotnet.rs` | New | ASP.NET / DI / Azure Functions / EF Core catalogue |
| `crates/codekurve-analysis/src/resolve.rs` | Modified | Hand off resolved edges to the recognition pass |
| `crates/codekurve-store/src/migrations.rs`, `repo.rs` | Modified | Migration 0005 (additive), persist role tags |
| `fixtures/angular/`, `fixtures/dotnet/` | New | End-to-end route→data-layer fixture trees |
| `crates/codekurve-analysis/tests/` | New | `angular-graph`, `dotnet-graph` fixtures + assertions |
| `docs/LANGUAGES.md` or `docs/FRAMEWORKS.md` | New/Modified | Published heuristic limitations and confidence semantics |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Heuristic edges are wrong often enough to be noise | High | Every framework edge is `Provenance::Heuristic` with calibrated confidence; fixtures assert exact edge counts so false positives are visible, not silent; consumers can filter by provenance |
| Angular DI matching produces false `Injects` edges | Med-High | Open question 1 must define the matching rule and its confidence ladder before code; negative fixtures (same-named non-injectable class) required |
| EF Core need reopens phase 5's generics decision broadly | Med | Any exception must be narrowly scoped and written into the spec as an exception, not a policy reversal (open question 3) |
| Framework catalogue grows without bound | Med | The In Scope list is the contract; every other pattern lands in published limitations, not in code |
| Angular and .NET together exceed a reviewable change size | High | Two separate PR chains, split as below; .NET may split again per open question 2 |
| Role tags force a full reindex | Low | Migration is additive and tags do not participate in `symbol_key`, same guarantee as migration 0004 |
| Recognition pass slows indexing on large repos | Low-Med | Pass operates on already-extracted edges in memory, no re-parse; measured against the phase 6 benchmark tiers |

## Rollback Plan

Revert the feature-branch chain. Migration 0005 is additive and no new column participates in `symbol_key`, so stored symbol ids are unchanged and no reindex is forced — a prior build simply reads no role tags. The `frameworks/` module is a leaf consumer: removing it removes all framework edges and leaves the phase 2/5 graph byte-identical. The TypeScript decorator extraction is separately revertable; if only it is kept, `Decorates` edges remain and nothing depends on them. The index is disposable regardless (§ 5.5).

## Dependencies

- Phases 2, 3, 4, 5, 6 — all archived. No new crate, no new grammar, no new external tool.

## Success Criteria

- [ ] An Angular fixture yields a traversable chain from a route definition through its component and injected service to the data-access call, answerable by existing MCP path/trace tools.
- [ ] A .NET fixture yields the same chain from a controller route and from a minimal-API route through DI-registered services to a `DbContext`.
- [ ] Every edge emitted by the recognition pass carries `Provenance::Heuristic` and a non-`Exact` confidence; a test asserts no framework edge is ever `Extracted` or `Resolved`.
- [ ] TypeScript `Decorates` edges carry the literal decorator name and the decorator's own span, with no Angular name special-cased in `languages/typescript.rs`.
- [ ] No framework-specific string appears in any file under `languages/`; a test or grep-based check enforces it.
- [ ] Phase 2 and phase 5 fixtures and CLI goldens pass unedited; a project with no framework usage produces zero framework edges.
- [ ] Migration 0005 applies to a populated index without wiping it; pre-migration symbol ids unchanged.
- [ ] Heuristic limitations and confidence semantics published in a user-visible document.
- [ ] `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` green after every PR in each chain.

## Proposal question round

Not run interactively — this proposal was produced from the archived exploration. The five entries in **Open Questions** are the questions that need answers; `sdd-design` resolves them, and any user correction to scope should land before design starts.

## Sizing / Chaining Forecast

Two independent chains, since Angular and .NET touch different analyzers and share only the model/migration groundwork.

| Slice | Content | Est. authored lines | Budget risk (400) |
|---|---|---|---|
| PR1 | Model + storage groundwork: new `RelationshipKind` variants, role tags, migration 0005 | 200–300 | Low |
| PR2 | TS decorator extraction → generic `Decorates` edges | 250–400 | Medium |
| PR3 | `frameworks/` pass scaffolding + provenance policy | 200–300 | Low |
| PR4 | Angular recognition (components, services, DI, routes, guards, interceptors, standalone imports) | 500–800 | High |
| PR5 | .NET recognition part A: controllers + DI registrations | 400–600 | High |
| PR6 | .NET recognition part B: minimal APIs, Azure Functions, middleware, EF Core | 400–600 | High |
| PR7 | Fixtures, end-to-end assertions, regression re-run | 400–600 | High |
| PR8 | Published limitations docs | 100–200 | Low |

Total estimate: **2450–3800 authored lines**, comparable to phase 5.

- Decision needed before apply: Yes — open questions 1–5 must be resolved in design first
- Chained PRs recommended: Yes
- Chain strategy: feature-branch-chain, PR1 → PR2 → PR3 → PR4, with PR5/PR6 branching from PR3 in parallel if conflict risk stays near zero
- 400-line budget risk: High

PR4, PR5, and PR6 are each likely to split further once the design answers open questions 1, 2, and 5.
