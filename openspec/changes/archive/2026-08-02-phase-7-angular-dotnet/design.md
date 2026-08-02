# Design: Phase 7 — Angular and .NET Aware

All 5 proposal open questions are resolved below (D4/D5 for Q1, D6/D7 for Q2, D11 for Q3, D12/D13 for Q4, D14 for Q5). Two PR chains sharing PR1–PR3.

Two proposal assumptions are corrected here, both grounded in current code rather than restated:

1. **The recognition pass cannot run on edges alone.** `cs_simple_type_name` (`csharp.rs:753`) deliberately drops type-argument lists, so `AddScoped<IRepo, Repo>()` and `DbSet<Invoice>` reach the edge model as the bare names `AddScoped`/`DbSet`; `cs_callee_name` (`csharp.rs:726`) keeps only the member name, so the `app`/`services` receiver is gone; and `typescript.rs` extracts no parameter symbols at all (zero matches for `Parameter`/`formal_parameters`). Decorator-argument object and array literals produce no edges in either analyzer. Every one of the five open questions needs syntax the edge model does not carry. D1 resolves this.
2. **Recognition runs *before* resolution, not after.** The proposal's pipeline put it last; cross-file target binding is exactly what `resolve.rs` already does, and a second resolver inside `frameworks/` is the thing to avoid. D2/D3 resolve this, and the provenance floor rule is what keeps a heuristic from being laundered into a fact on the way through.

The hard constraint is untouched: no framework-specific string appears under `languages/`, and `languages/csharp.rs` is not modified at all.

## Architecture Decisions

| # | Decision | Choice | Rejected | Rationale |
|---|---|---|---|---|
| D1 | Recognition input | `frameworks/` re-parses candidate files with its own tree-sitter queries, gated by a source-text marker prefilter | (a) consume extracted edges only; (b) carry the `Tree` in `FileAnalysis` | (a) is provably insufficient — see the four evidence losses above. (b) breaks `FileAnalysis`'s `PartialEq` derive (`tree_sitter::Tree` has no `PartialEq`) and pins every tree in memory for the whole run. A second parse of only marker-matching files is the smaller cost and keeps `languages/` output byte-identical. |
| D2 | Prefilter | `source.contains(marker)` over a per-language `&[&str]` marker list (`"@Component"`, `"@Injectable"`, `"inject("`, `"Routes"` / `"[ApiController]"`, `"Http"`, `".Map"`, `".Add"`, `"DbSet<"`, `"[Function"`) before any parse | Always parse; a regex set | A repo with no framework usage pays one substring scan per file and produces zero framework edges (a success criterion). Markers are framework strings, and they live in `frameworks/` where they are allowed. Over-matching is harmless: a parse that finds no pattern emits nothing. |
| D3 | Pass placement | Inside `extract::analyze`, after `analyzer_for(language).analyze(...)` returns: `frameworks::recognize(source, language, &mut analysis)` | A standalone stage in `commands.rs`/`incremental.rs` | `analyze` is the single entry point every caller (`commands.rs`, `incremental.rs`, `watch`, tests) already routes through, and it is the only place holding both the source text and the finished per-file symbol list. One line, zero call-site ripple. |
| D4 | Framework edge targets | Always `EdgeTarget::Unresolved(<name as written>)`; `resolve.rs` binds them like any other name | Resolve inside `frameworks/` | Cross-file binding, multi-candidate splitting and unresolved-row bookkeeping already exist and are tested. Duplicating them for five edge kinds is a second resolver with its own bugs. |
| D5 | Provenance floor | `resolve_one` never upgrades provenance: an incoming `Heuristic` edge stays `Heuristic`, and its stored confidence is `min(recognition ceiling, resolution confidence)` on the `Exact > High > Medium > Low > Unresolved` order | Let resolution overwrite provenance/confidence as it does today | This is what makes "heuristics are clearly marked" structurally true instead of a review promise. Without it, a single-candidate `Injects` would persist as `Resolved`/`Exact` — a guess indistinguishable from a parsed fact. Enforced by a test asserting no row of a framework kind is ever `extracted`/`resolved`. |
| D6 | `kind_matches` for the 5 new kinds | One language-independent `frameworks::kind_matches(kind, sym)` consulted in `languages/mod.rs` *before* the per-analyzer table | A branch in each analyzer's `kind_matches` | The kinds are framework-level, not grammar-level; the answer is identical for TS and C#. Duplicating it invites divergence. Neither analyzer's existing table changes, so the phase-2/5 `kind_matches` sweep tests stay valid. |
| D7 | .NET slice split | **Two slices, split by evidence shape, not by feature**: PR5 = attribute-driven (MVC controllers **and** Azure Functions), PR6 = call-driven (minimal APIs, DI registration, middleware) + EF Core | One .NET slice; split as "controllers" vs "minimal APIs + Functions" | Azure Functions is `[Function("Name")]` + trigger attributes — structurally a controller, not a minimal API. Splitting by evidence shape gives each PR exactly one matcher to build and test; splitting by feature would make PR6 build both engines. |
| D8 | Pattern engine | Two matchers in `frameworks/`: `AttrPattern` (decorator/attribute name + argument slots) and `CallPattern` (method name + argument/type-argument shape) | One unified pattern DSL | Two ~40-line matchers with different inputs beat one abstraction over both. Angular reuses `AttrPattern` for decorators and the D14 object walker; .NET uses both. |
| D9 | `CallPattern` receiver | Matched on **method name + argument shape only**; the receiver expression is ignored | Require a receiver named `app`/`services`/`builder.Services` | The receiver is an untyped local (`app`, `builder`, `services`, `endpoints`, `_`) and the indexer has no type checker to say what it is. A receiver whitelist would be a name heuristic pretending to be a type check, and it breaks on every codebase that names the variable something else. Shape matching (e.g. `MapGet` requires a first string-literal argument) is the discriminator; a bare name match with a wrong shape scores `Low`, not `Medium`. |
| D10 | Where framework detail rides | The existing `reason: Option<String>` on the edge, with documented prefixes (`route:`, `di:`, `key:`, `trigger:`, `lifetime:`, `dbset:`) | New IR fields / a new `relationships` column | `reason` is already the extra-syntactic-context channel (`BASE_LIST_REASON`, `"static"`, `"alias:X"`), already persisted (`relationships.reason`), and already surfaced. Zero schema change for edge metadata. Framework edges are new rows, so no existing golden's `reason` changes. |
| D11 | EF Core generics exception | Exactly one rule: a property/field whose declared type is `generic_name` `DbSet` with **exactly one** type argument, declared in a class whose base list contains an entry named `DbContext` or ending in `DbContext` → one `PersistsTo` edge, source = the `DbContext` subclass, target = `Unresolved(<type argument>)`, `reason = "dbset:<PropertyName>"` | (a) general type-argument resolution; (b) reach the data layer through `Calls` on `DbSet` members instead | Phase 5's "generics are structural only, no edges" stands as policy; this is a single named exception, read by `frameworks/` during its own parse — `languages/csharp.rs` is not touched and still emits no generic edges. (b) fails because `_ctx.Invoices.Add(x)` resolves to the member name `Add`, which names no entity. **Bounding statement for the spec**: `PersistsTo` is the only edge in the system derived from a type argument, and it is derivable only from `DbSet<T>` inside a `DbContext` subclass. Nothing else about generics changes; a test asserts `List<Invoice>`, `Task<Invoice>` and `IQueryable<Invoice>` emit no edge. |
| D12 | Role tag storage | New `symbols.roles TEXT NOT NULL DEFAULT ''` column (migration 0005), holding a sorted comma-joined set of lowercase role tokens | (a) side table; (b) one boolean column per role; (c) derived at query time | Roles are 1:1 with a symbol and cardinality is ≤ 6, so a side table buys a join for nothing. (b) costs one migration per future role. (c) is impossible: roles are produced by the heuristic pass at index time and are not re-derivable at query time without re-running it. Filter is `WHERE ',' \|\| roles \|\| ',' LIKE '%,controller,%'`. `ponytail:` no index on `roles` — add one if role filtering ever shows up hot in the phase 6 benchmarks. |
| D13 | Role tags and identity | `roles` does **not** participate in `symbol_key`; `Symbol`/`ExtractedSymbol` gain `roles: Vec<FrameworkRole>` | Include roles in the key | Same guarantee migration 0004 gave: no stored id changes, no forced reindex, and re-running recognition with a wider catalogue never re-keys a symbol. |
| D14 | Array/metadata extraction shape | One walker `object_literal_entries(node, source) -> Vec<MetaKey>` over **any** object literal, returning `MetaKey { key, entries: Vec<MetaEntry { name, span }> }`, where an entry name comes from: bare identifier; member expression → last segment; `new X()` → `X`; object literal with a `useClass`/`useExisting`/`useFactory` key → that value, else its `provide` value | A decorator-argument-specific extractor | Angular's `@Component`/`@NgModule` metadata and a route-config entry are the *same* syntax (an object literal with array-valued keys); the only difference is what encloses them. One walker serves `providers`, `imports`, `declarations`, `canActivate`, `HTTP_INTERCEPTORS` and route objects, and each caller decides which keys it cares about. Non-array values (`path`, `component`) come back as a one-entry `MetaKey`, so route paths and components use the same shape. |
| D15 | TS parameter decorators | `Decorates` source = the enclosing **constructor symbol** (TS has no parameter symbols), `reason = "param:<index>"` | Synthesize `Parameter` symbols in `typescript.rs` | Parameter symbols would change TS symbol counts and every phase-2 golden. The index in `reason` is enough for `@Inject(TOKEN)` to be matched to its parameter by the recognition pass, and it stays framework-blind. |

## Module Layout

```
crates/codekurve-analysis/src/
├── extract.rs              dispatcher; + one `frameworks::recognize` call
├── languages/
│   ├── mod.rs              + frameworks::kind_matches consulted first
│   ├── typescript.rs       + decorator walking → generic Decorates (PR2)
│   └── csharp.rs           UNCHANGED
└── frameworks/
    ├── mod.rs              recognize(), marker prefilter, MetaKey/MetaEntry,
    │                       AttrPattern/CallPattern, kind_matches, confidence policy
    ├── angular.rs          Angular catalogue (PR4)
    └── dotnet.rs           .NET catalogue (PR5 attributes, PR6 calls + EF)
```

```rust
// frameworks/mod.rs
pub fn recognize(source: &str, language: LanguageId, analysis: &mut FileAnalysis);
pub fn kind_matches(kind: RelationshipKind, sym: SymbolKind) -> Option<bool>; // None = not a framework kind
pub(crate) struct MetaEntry { pub name: String, pub span: SourceSpan }
pub(crate) struct MetaKey   { pub key: String, pub entries: Vec<MetaEntry> }
pub(crate) fn object_literal_entries(node: Node, source: &[u8]) -> Vec<MetaKey>;
```

`recognize` returns early on a failed marker prefilter, parses once, walks, appends edges to `analysis.relationships` and sets `roles` on the matching entries of `analysis.symbols`. It never removes or mutates an existing edge.

## Q1 — Angular and .NET DI inference (resolved)

**Rule.** A class is a *DI host* only if recognition already tagged it with a framework role (`Component`, `Service`, `Controller`, `Repository`, `Decorator`). For a DI host only:

- each constructor parameter with an **explicit, non-builtin, non-generic, non-union declared type name** → `Injects`, target `Unresolved(<type name>)`, `reason = "di:ctor-param:<index>"`;
- each `inject(X)` call whose argument is a bare identifier, appearing inside the DI host's class body (field initializer or constructor body) → `Injects`, `reason = "di:inject-fn"`;
- a `@Inject(TOKEN)` parameter decorator overrides the parameter's declared type: target becomes `TOKEN`, `reason = "di:token:<index>"`.

Everything else emits **no edge and an `UnresolvedReference`** with a reason: no type annotation, a primitive/builtin (`string`, `number`, `boolean`, `any`, `unknown`, `object`, `Date`), a union/intersection/literal/anonymous type, a generic instantiation, `inject()` with a non-identifier argument, or a `@Inject` whose token is not a bare identifier. Nothing is guessed.

**The false-positive guard is the DI-host precondition.** An ordinary class constructor never produces `Injects`, so a plain `new Foo(bar)` graph is unchanged — that is what keeps the inference from flooding the graph.

**Confidence.** Stored confidence is `min(ceiling, resolution)` per D5, always at `Provenance::Heuristic`.

| Evidence (recognition ceiling) | Ceiling |
|---|---|
| Constructor parameter with explicit type, on a DI host | High |
| `inject(X)` with a bare identifier, in a DI host body | High |
| `@Inject(TOKEN)` parameter decorator | Medium |
| Name matched but the shape was partial (e.g. `.Add*` name with no type arguments) | Low |

| Resolution outcome | Resolution confidence |
|---|---|
| Exactly 1 same-domain candidate, kind `Class` | High |
| Exactly 1 same-domain candidate, kind `Interface` | **Medium** (cap) |
| >1 candidate | one `Low` edge per candidate (existing §20.4 never-pick-first policy) |
| 0 candidates | no edge; `UnresolvedReference` preserved |

The interface cap is deliberate and load-bearing for .NET: `IInvoiceRepository` names a contract, and only the `RegisteredAs` edge from `AddScoped<IInvoiceRepository, InvoiceRepository>()` says which implementation runs. The graph should say "Medium — and here is the registration edge", not "High — trust me".

Negative fixture (required): a non-injectable class sharing a name with an injectable service, asserted to produce two `Low` edges, not one `High` one.

## Q2 — .NET split (resolved)

Two slices, split by evidence shape (D7). Both catalogues are closed lists in `frameworks/dotnet.rs`.

### PR5 — attribute-driven (`AttrPattern`)

| Attribute evidence | Role tag | Edge |
|---|---|---|
| Class with `[ApiController]`, or a class named `*Controller` with any `[Route]`/`[Http*]` member | `Controller` | — |
| `[Route("tpl")]` on the class | — | prefix for its members |
| `[HttpGet("tpl")]`/`HttpPost`/`Put`/`Delete`/`Patch` on a method | `Route` | `HandlesRoute`, source = the method, target `Unresolved(<verb> <joined template>)` → resolves to `External` (a route is not a project symbol), `reason = "route:GET /api/invoices/{id}"` |
| Method with `[Function("Name")]` | `Route` | — |
| Parameter/method attribute ending in `Trigger` (`HttpTrigger`, `TimerTrigger`, `QueueTrigger`, `ServiceBusTrigger`, `BlobTrigger`) | — | `Triggers`, source = the function method, target `External(<attribute name>)`, `reason = "trigger:<first literal argument>"` |

Route targets are external by construction — a URL template is not a symbol. They are stored as `target_external`, exactly as a `node_modules` import already is, which keeps `HandlesRoute` traversable from the method without inventing a synthetic symbol.

### PR6 — call-driven (`CallPattern`) + EF Core

| Call evidence | Edge |
|---|---|
| `Map{Get,Post,Put,Delete,Patch}(<string literal>, <handler>)` | `HandlesRoute` from the **enclosing method** (usually `Main`/`Program`), `reason = "route:GET /path"`; plus, when the handler argument is an identifier or method group, a second `HandlesRoute` to `Unresolved(<handler name>)` |
| `MapGroup("prefix")` | recorded as a prefix for chained `Map*` calls on the same statement only; a prefix held in a variable is a published limitation |
| `Add{Scoped,Transient,Singleton}<TService, TImpl>()` | two `RegisteredAs` edges from the enclosing method (an edge source must be a symbol, and `TImpl` is only a name at this point): one to `Unresolved(TImpl)` with `reason = "lifetime:scoped;role:impl;service:TService"`, one to `Unresolved(TService)` with `reason = "lifetime:scoped;role:service;impl:TImpl"`. The pair is what lets a consumer join contract to implementation without a synthetic symbol |
| `Add*<T>()` single type argument, or `AddSingleton(typeof(A), typeof(B))` | same, ceiling `Low` (shape is partial, D9) |
| `Use<Name>()` / `UseMiddleware<T>()` | `RegisteredAs` to `Unresolved(<T or Name>)`, `reason = "key:middleware"` |
| `DbSet<T>` property in a `DbContext` subclass | `PersistsTo` per D11 |

`Add*` and `Use*` are matched by exact name from the closed list, never by prefix — `AddSomethingCustom` is a published limitation, not a silent Low-confidence guess.

## Q5 — Array-literal shape, applied to Angular (resolved)

`object_literal_entries` (D14) is the single extractor. Angular consumption:

| Source | Key | Edge |
|---|---|---|
| `@Component`/`@Directive`/`@Pipe` on a class | — | role `Component` (`Decorator` for `@Directive`/`@Pipe`) |
| `@Injectable` on a class | — | role `Service`; role `Repository` additionally when the class name ends in `Repository`/`Store` |
| `@Component({ providers: [...] })`, `@NgModule({ providers: [...] })` | `providers` | `RegisteredAs` per entry, `reason = "key:providers"` |
| `@Component({ imports: [...] })` (standalone), `@NgModule({ imports: [...] })` | `imports` | `RegisteredAs` per entry, `reason = "key:imports"` |
| `{ provide: HTTP_INTERCEPTORS, useClass: X, multi: true }` | resolved by the `useClass` rule | `RegisteredAs` to `X`, `reason = "key:providers:HTTP_INTERCEPTORS"` |
| A route object `{ path, component, canActivate: [...], loadComponent }` inside an array assigned to a `Routes`-typed or `Routes`-named const | `path`/`component`/`canActivate` | `HandlesRoute` from the **const variable symbol** to `Unresolved(<component>)`, `reason = "route:/<path>"`; one `RegisteredAs` per `canActivate` entry, `reason = "key:canActivate"` |
| `children: [...]` | recursion | nested route objects walked with the parent path prefixed |

`loadComponent: () => import('./x').then(m => m.XComponent)` → `HandlesRoute` to `Unresolved(XComponent)` at ceiling `Medium` (the member expression's last segment is the evidence). A `loadChildren` with no extractable member name → `UnresolvedReference`.

## Data Flow

```
discovery (.ts .js .cs)
      │
      ▼
extract::analyze(source, language, path)
      │  analyzer_for(language).analyze(...)         ← framework-blind, byte-identical output
      │  frameworks::recognize(source, language, &mut analysis)
      │      marker prefilter → own parse → AttrPattern / CallPattern / object_literal_entries
      │      appends Injects|RegisteredAs|HandlesRoute|Triggers|PersistsTo
      │      (Provenance::Heuristic + ceiling confidence, EdgeTarget::Unresolved|External)
      │      sets ExtractedSymbol.roles
      ▼
resolve::resolve_with(...)
      │  frameworks::kind_matches consulted before the per-analyzer table
      │  D5 provenance floor: Heuristic stays Heuristic; confidence = min(ceiling, resolution)
      ▼
repo::reindex → symbols(+roles)  [migration 0005]; relationships(reason = route:/di:/key:/…)
      ▼
MCP path/trace: HandlesRoute → Injects → Calls → PersistsTo   (no MCP-layer change)
```

## Model and Storage Changes

```rust
// codekurve-core/src/symbol.rs
pub enum RelationshipKind { /* … */ Injects, RegisteredAs, HandlesRoute, Triggers, PersistsTo }
//   as_str: "injects", "registeredas", "handlesroute", "triggers", "persiststo"

/// Framework role tag (plan §17.2). Never a SymbolKind variant.
pub enum FrameworkRole { Controller, Route, Service, Repository, Component, Decorator }
//   as_str: lowercase; Symbol.roles is stored sorted + deduped
pub struct Symbol { /* … */ pub roles: Vec<FrameworkRole> }
```

`Publishes`/`Subscribes` from §17.3 are **not** added — nothing emits them this phase, and an unpopulated variant is a lie in the enum.

Migration 0005:
```sql
ALTER TABLE symbols ADD COLUMN roles TEXT NOT NULL DEFAULT '';
```
One `ADD COLUMN` with a non-null default, O(1), no table rewrite, no `DELETE`. `SCHEMA_VERSION = 5`. `roles` feeds no `symbol_key` component, so every stored id survives (same guarantee as 0004).

| Site | Change |
|---|---|
| `ExtractedSymbol` (`ir.rs`) | `+ roles: Vec<FrameworkRole>`, empty at every existing construction site |
| `reindex` symbol INSERT | `+ roles` column, value = sorted `as_str` joined by `,` |
| `StoredSymbol`, `search_symbols`/`find_by_name`/`find_symbol_by_id` | `+ s.roles`; `parse_roles` mirrors `parse_visibility` |
| `commands.rs::symbol` | prints `roles:` only when non-empty; `--json` omits the key when empty (the phase-5 convention) |
| `languages/mod.rs` | `kind_matches` consults `frameworks::kind_matches` first |
| `resolve.rs` | D5 provenance floor in `resolve_one`; `External` allowed as a recognition-supplied target |

`frameworks::kind_matches`: `Injects` → `Class\|Interface`; `RegisteredAs` → `Class\|Interface\|Variable`; `HandlesRoute` → `Class\|Method\|Function\|Variable`; `Triggers` → `Class\|Method\|Function`; `PersistsTo` → `Class\|Struct\|Interface`; anything else → `None` (fall through to the analyzer).

## PR Chain

| PR | Content | Depends on | Must be true when it lands |
|---|---|---|---|
| 1 | 5 `RelationshipKind` variants, `FrameworkRole`, `roles` on `Symbol`/`ExtractedSymbol`, migration 0005, repo read/write, `parse_roles` | — | Zero behavior change. 0005 applies to a populated 0004 DB with every id unchanged. |
| 2 | TS decorator walking → generic `Decorates` (class/method/property/constructor-parameter, D15) | 1 | Decorator name text and own span asserted; no framework string in `typescript.rs`; phase-2 goldens unedited. |
| 3 | `frameworks/mod.rs` scaffolding: `recognize`, marker prefilter, `AttrPattern`/`CallPattern`, `object_literal_entries`, `kind_matches`, D5 provenance floor in `resolve.rs` | 2 | Empty catalogues → zero framework edges on every existing fixture. Floor rule unit-tested with a synthetic Heuristic edge. |
| 4 | Angular catalogue: roles, DI (Q1), routes + guards + interceptors + standalone imports (Q5) | 3 | Every Angular edge is `Heuristic`; negative same-name fixture asserted. |
| 5 | .NET attribute-driven: controllers → `HandlesRoute`, Azure Functions → `Triggers` (Q2 slice A) | 3 (parallel with 4) | Route templates joined class+method; `csharp.rs` diff is empty. |
| 6 | .NET call-driven: minimal APIs, `Add*` registrations, middleware, EF Core `DbSet<T>` → `PersistsTo` (Q2 slice B, Q3) | 5 | `List<T>`/`Task<T>`/`IQueryable<T>` emit no edge; `csharp.rs` diff still empty. |
| 7 | `fixtures/angular/`, `fixtures/dotnet/`, end-to-end chain assertions, full phase-2/5 regression re-run | 4, 6 | Both route→data-layer chains traverse via existing MCP tools; zero edits under `fixtures/ts-graph/` or `fixtures/csharp/`. |
| 8 | `docs/FRAMEWORKS.md`: catalogue, confidence semantics, published limitations | 7 | Every "no edge" rule above appears in a user-visible document. |

PR4 and PR5/PR6 branch from PR3 in parallel — they share no file. PR4 and PR6 are each likely to split once written; the natural seams are Angular DI vs Angular routes, and .NET calls vs EF Core.

## Testing Strategy

| Layer | Test |
|---|---|
| Unit | Migration 0005 on a populated v4 DB: row count, every `symbol_key`/`id` unchanged, `roles` present and `''` |
| Unit | **Provenance floor**: a synthetic `Heuristic`/`High` edge with one candidate resolves to `Heuristic`/`High`, never `Resolved`/`Exact` |
| Unit | `frameworks::kind_matches` table sweep; both analyzers' existing sweeps still pass unchanged |
| Unit | Marker prefilter: a file with no marker is never parsed (assert via a call counter) and yields zero edges |
| Unit | TS decorators: class/method/property/param, name text, own span, `param:<index>` reason |
| Unit | `object_literal_entries`: bare identifier, member expression, `new X()`, `{provide, useClass}`, `{provide, useExisting}`, nested arrays, non-array value |
| Unit | Angular DI ladder: every row of the ceiling table, plus every "no edge" case emitting an `UnresolvedReference` with its reason |
| Unit | Angular negative: same-named non-injectable class → two `Low` edges |
| Unit | .NET route template join (class `[Route]` + method `[HttpGet]`), Azure Functions `[Function]` + trigger |
| Unit | .NET `Add*` with 2 / 1 / 0 type arguments → `Medium` / `Low` / no edge; `Map*` with and without a literal first argument |
| Unit | **EF Core bound**: `DbSet<Invoice>` in a `DbContext` subclass → one `PersistsTo`; `DbSet<Invoice>` outside a `DbContext` → none; `List<Invoice>`, `Task<Invoice>`, `IQueryable<Invoice>`, `Dictionary<A,B>` → none |
| Integration | `angular-graph` / `dotnet-graph`: exact per-kind edge counts (false positives are visible, not silent) |
| Integration | **No framework edge is ever `extracted`/`resolved`** — asserted across every fixture |
| Integration | A plain TS + plain C# project produces zero framework edges and zero roles |
| Regression | Phase 2 and phase 5 fixtures and CLI goldens unedited; `git diff --exit-code` over `crates/codekurve-analysis/src/languages/csharp.rs` is empty for the whole chain |
| Static | Grep check (a test, following `scripts/check_licensing.py`'s precedent): no file under `languages/` contains any marker from the `frameworks/` catalogues |
| E2E | `index` → MCP path/trace: route → controller/component → injected service → `PersistsTo` entity, on both fixtures |
| Perf | Recognition cost measured against the phase 6 small/medium benchmark tiers; reported, not tuned |

## Threat Matrix

N/A — no routing, shell, subprocess, VCS/PR automation, or executable classification. No new grammar, crate, network call, or filesystem surface: the recognition pass parses source already read and already parsed once, in process.

## Migration / Rollout

Additive and reindex-free. A pre-0005 DB gains one defaulted column and keeps every id; it reports no roles and no framework edges until the next `codekurve index`. Because roles and framework edges are index-time products, an existing index is *stale*, not wrong — which is acceptable per §5.5 (the index is disposable).

## Rollback Boundaries

| PR | Revert | Residual |
|---|---|---|
| 1 | Revert; set `SCHEMA_VERSION` back to 4 and leave `roles` in place (defaulted, unread by the prior build) | None |
| 2 | Independently revertable; `Decorates` edges disappear, nothing depends on them | None |
| 3–6 | `frameworks/` is a leaf consumer: deleting it removes every framework edge and role and leaves the phase 2/5 graph byte-identical | None |
| 7–8 | Fixtures and docs only | None |

## Open Questions

All five proposal questions are resolved above. Two implementation-first verification tasks remain — the phase-5 "verify against `node-types.json` before writing extraction" convention, not deferred decisions:

- [ ] **PR2 task 1**: confirm the pinned `tree-sitter-typescript 0.23.2` node kinds for decorators (`decorator`, its `call_expression` vs bare `identifier` form, and whether parameter decorators appear as `decorator` children of `required_parameter`) before writing the walker. The outcome column of D15 wins over any name in this document.
- [ ] **PR6 task 1**: confirm `tree-sitter-c-sharp 0.23`'s node for a `DbSet<Invoice>` property type (`generic_name` with a `type_argument_list`) and for `AddScoped<A,B>()`'s type-argument list on the `member_access_expression`'s `generic_name`. D11's rule is stated in outcome terms and survives a node rename.
