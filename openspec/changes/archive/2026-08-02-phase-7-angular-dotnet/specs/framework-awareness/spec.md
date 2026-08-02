# Framework Awareness Specification

## Purpose

A heuristic recognition pass that consumes already-extracted, already-resolved edges (`Decorates`, `Calls`, `Constructs`, type references) and pattern-matches them into framework-level relationships — Angular DI/routing/components and .NET controllers/minimal APIs/DI/Functions/EF Core — so a route-to-data-layer path is a traversable chain of typed edges. Every inference this pass emits is structurally distinguishable from a syntactic fact.

## Requirements

### Requirement: Recognition Runs as a Separate Pass Downstream of Extraction and Resolution

The framework recognition pass MUST run after language extraction and reference resolution complete. It MAY perform its own marker-gated re-parse of source text to recover data the extractor's edge model does not carry (generic type arguments, call receivers, decorator call arguments, TS parameter symbols) — the extracted edge set alone (`Decorates`, resolved `Calls`/`Constructs`, type references) is insufficient. No language analyzer under `languages/` MUST contain a framework-specific string literal (e.g. `"Component"`, `"HttpGet"`, `"AddScoped"`).

#### Scenario: Language analyzers stay framework-blind

- GIVEN the full source of `languages/typescript.rs` and `languages/csharp.rs`
- WHEN searched for framework-specific identifiers (Angular/ASP.NET/EF Core names)
- THEN no such identifier is found; only generic decorator/attribute extraction exists

#### Scenario: Recognition pass is marker-gated before it re-parses

- GIVEN a file whose extracted symbols/edges contain no framework marker prefix (`TS_MARKERS`/`CS_MARKERS`)
- WHEN the recognition pass runs
- THEN it skips the file without re-parsing its source text

#### Scenario: Recognition pass re-parses only marker-matched files

- GIVEN a file whose extracted output contains a framework marker prefix
- WHEN the recognition pass runs
- THEN it re-parses that file's source text and produces framework edges from the recovered detail (type arguments, call receivers, decorator arguments), never from language analyzers themselves

### Requirement: Every Framework Edge Carries Heuristic Provenance and a Non-Exact Confidence

Every edge emitted by the recognition pass MUST carry `Provenance::Heuristic` and a confidence value other than `Exact`. No framework edge MUST ever be recorded with `Provenance::Extracted` or `Provenance::Resolved`.

#### Scenario: An Injects edge is never presented as a fact

- GIVEN an Angular constructor parameter matched to an injectable class
- WHEN the `Injects` edge is emitted
- THEN it carries `Provenance::Heuristic` and a non-`Exact` confidence value

#### Scenario: No framework edge kind is ever Extracted or Resolved

- GIVEN a full fixture project with Angular and .NET framework usage indexed
- WHEN every `Injects`/`RegisteredAs`/`HandlesRoute`/`Triggers`/`PersistsTo` edge in the store is inspected
- THEN none carries `Provenance::Extracted` or `Provenance::Resolved`

### Requirement: Angular Recognition Covers Components, DI, Routes, Guards, Interceptors, and Standalone Imports

The Angular catalogue MUST role-tag `@Component`/`@Injectable` classes, emit `Injects` edges from constructor-parameter DI and `inject(Foo)` calls, emit `RegisteredAs` edges from `providers: [...]` arrays, and emit `HandlesRoute` edges from `Routes` config arrays including `canActivate` guards and `HTTP_INTERCEPTORS` registrations, plus recognize standalone component `imports: [...]`.

#### Scenario: Constructor DI produces an Injects edge

- GIVEN an Angular component whose constructor takes a parameter typed to an `@Injectable` service in the same project
- WHEN the recognition pass runs
- THEN an `Injects` edge from the component to the service is emitted with `Provenance::Heuristic`

#### Scenario: Route array produces HandlesRoute with a guard

- GIVEN a `Routes` array entry with a `component` and a `canActivate` guard
- WHEN the recognition pass runs
- THEN a `HandlesRoute` edge links the route path to the component, and the guard relationship is recorded per the design's chosen shape

### Requirement: .NET Recognition Covers Controllers, Minimal APIs, DI Registrations, Azure Functions, Middleware, and EF Core

The .NET catalogue MUST emit `HandlesRoute` edges from `[ApiController]`/`[Route]`/`[HttpGet…]` controllers and from minimal-API `app.MapGet/MapPost(...)` call expressions, `RegisteredAs` edges from `services.Add{Scoped,Transient,Singleton}<TService, TImpl>()` calls, `Triggers` edges from Azure Functions `[Function]`/trigger attributes, and `PersistsTo` edges from `DbContext` subclasses and their `DbSet<T>` members.

#### Scenario: Attribute-based controller route

- GIVEN a class decorated `[ApiController]` with a method decorated `[HttpGet("invoices/{id}")]`
- WHEN the recognition pass runs
- THEN a `HandlesRoute` edge is emitted from the route to the handler method, `Provenance::Heuristic`

#### Scenario: Minimal-API route is recognized without an attribute

- GIVEN `app.MapGet("/invoices/{id}", handler)`
- WHEN the recognition pass runs
- THEN a `HandlesRoute` edge is emitted for that route, using the same edge kind as the attribute-based case

#### Scenario: DbSet member produces a PersistsTo edge

- GIVEN a `DbContext` subclass with a `DbSet<Invoice> Invoices` member
- WHEN the recognition pass runs
- THEN a `PersistsTo` edge links the context (or the service depending on it) to `Invoice`

### Requirement: An End-to-End Route-to-Data-Layer Path Is Traversable

For both the Angular and the .NET fixture trees, the chain from a route definition through DI-injected services to the data-access layer MUST be traversable as a sequence of typed edges answerable by the existing MCP path/trace tools, with no MCP-layer change required.

#### Scenario: .NET controller route reaches EF Core through a service

- GIVEN a fixture with a controller route, a DI-registered service the controller depends on, and a `DbSet<T>` the service uses
- WHEN a path/trace query runs from the route to the `DbSet<T>` member
- THEN a path exists composed of `HandlesRoute` → `Injects` → `Calls` → `PersistsTo` edges

#### Scenario: .NET minimal-API route reaches EF Core through a service

- GIVEN a fixture with a minimal-API route, an injected service, and a `DbSet<T>` the service uses
- WHEN a path/trace query runs from the route to the `DbSet<T>` member
- THEN a traversable path exists, matching the shape of the controller-based case

### Requirement: Framework Catalogue Coverage Is Bounded and Published

Patterns outside the proposal's in-scope list (Angular templates, `@NgModule` legacy fields beyond `providers`/`imports`, other TS decorator frameworks beyond generic `Decorates`, Blazor/Razor/SignalR/gRPC, message-bus wiring, runtime/reflection-based DI, convention-based routing) MUST NOT be recognized, and the recognition pass's limitations MUST be published in a user-visible document alongside the confidence semantics.

#### Scenario: A project with no framework usage produces zero framework edges

- GIVEN a plain TypeScript/C# project using no Angular or ASP.NET patterns
- WHEN indexed
- THEN zero `Injects`/`RegisteredAs`/`HandlesRoute`/`Triggers`/`PersistsTo` edges are produced, and phase 2/5 output is unaffected

#### Scenario: Out-of-scope pattern is not recognized

- GIVEN an Angular template with `routerLink` or `*ngIf` bindings
- WHEN indexed
- THEN no framework edge is derived from the template, consistent with the published limitations
