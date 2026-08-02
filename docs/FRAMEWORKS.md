# Framework Awareness

CodeKurve's language analyzers (`languages/typescript.rs`, `languages/csharp.rs`)
stay framework-blind: they extract syntactic facts only, and no
framework-specific string appears in either file. A separate, downstream
recognition pass (`codekurve-analysis/src/frameworks/`) re-parses candidate
files after extraction and pattern-matches a closed catalogue of Angular and
.NET idioms into framework-level graph edges and symbol role tags.

Every edge this pass produces is a guess, not a fact, and is marked as one
structurally (see "Confidence and provenance" below) — never by convention or
review discipline alone.

## New relationship kinds

Five `RelationshipKind` variants exist solely to carry framework-level
inference; the analyzers never emit them:

| Kind | Meaning | Emitted by |
|---|---|---|
| `Injects` | A DI-host class receives an instance of another type (constructor parameter, `inject(X)`, or `@Inject(TOKEN)`) | Angular |
| `RegisteredAs` | A type is registered into a DI/module/middleware container (`providers`, `imports`, `AddScoped`/`AddSingleton`/`AddTransient`, `Use*`/`UseMiddleware`) | Angular, .NET |
| `HandlesRoute` | A method, function, or route-table entry handles an HTTP route | Angular, .NET |
| `Triggers` | A function is invoked by an Azure Functions trigger attribute | .NET |
| `PersistsTo` | A `DbContext` subclass's `DbSet<T>` member persists entity `T` | .NET (EF Core) |

`Publishes`/`Subscribes` are deliberately **not** added — nothing in this
catalogue emits them, and an unpopulated enum variant would misrepresent the
graph.

## Framework role tags

`FrameworkRole` tags attach to existing symbols without introducing a new
`SymbolKind`: `Controller`, `Route`, `Service`, `Repository`, `Component`,
`Decorator`. A symbol can carry more than one role (e.g. an Angular service
whose class name ends in `Repository` gets both `Service` and `Repository`).
Roles are stored sorted and deduplicated in `symbols.roles`, do not
participate in `symbol_key`, and are queryable (`codekurve symbol` prints
`roles:` when non-empty; `--json` omits the key when empty).

## Angular catalogue

| Evidence | Role tag | Edge |
|---|---|---|
| `@Component`/`@Directive`/`@Pipe` on a class | `Component` (`Decorator` for `@Directive`/`@Pipe`) | — |
| `@Injectable` on a class | `Service`, plus `Repository` if the class name ends in `Repository`/`Store` | — |
| Constructor parameter, explicit non-builtin/non-generic/non-union type, on a DI host | — | `Injects`, `reason = "di:ctor-param:<index>"`, ceiling High |
| `inject(X)` bare-identifier call in a DI host's class body | — | `Injects`, `reason = "di:inject-fn"`, ceiling High |
| `@Inject(TOKEN)` parameter decorator | — | `Injects` to `TOKEN`, `reason = "di:token:<index>"`, ceiling Medium |
| `@Component({ providers: [...] })` / `@NgModule({ providers: [...] })` | — | `RegisteredAs` per entry, `reason = "key:providers"` |
| `@Component({ imports: [...] })` (standalone) / `@NgModule({ imports: [...] })` | — | `RegisteredAs` per entry, `reason = "key:imports"` |
| `{ provide: HTTP_INTERCEPTORS, useClass: X, multi: true }` | — | `RegisteredAs` to `X`, `reason = "key:providers:HTTP_INTERCEPTORS"` |
| Route object `{ path, component, canActivate, loadComponent }` inside a `Routes`-typed/named array | — | `HandlesRoute` from the array's const symbol to `Unresolved(<component>)`, `reason = "route:/<path>"`; one `RegisteredAs` per `canActivate` entry, `reason = "key:canActivate"` |
| `children: [...]` on a route object | — | recursion, nested routes prefixed with the parent path |
| `loadComponent: () => import('./x').then(m => m.XComponent)` | — | `HandlesRoute` to `Unresolved(XComponent)`, ceiling Medium |

**DI-host precondition (the false-positive guard):** a class only produces
`Injects` edges from its constructor if recognition already tagged it with a
role (`Component`, `Service`, `Controller`, `Repository`, `Decorator`). An
ordinary, untagged class's constructor emits nothing — `new Foo(bar)` never
becomes a fabricated dependency edge.

Every other DI shape — no type annotation, a primitive/builtin type
(`string`, `number`, `boolean`, `any`, `unknown`, `object`, `Date`), a
union/intersection/literal/anonymous type, a generic instantiation,
`inject()` with a non-identifier argument, or `@Inject` with a non-identifier
token — emits **no edge**, only an `UnresolvedReference` with an explicit
reason. Nothing is guessed.

## .NET catalogue

### Attribute-driven (controllers, Azure Functions)

| Evidence | Role tag | Edge |
|---|---|---|
| Class with `[ApiController]`, or a `*Controller`-named class with any `[Route]`/`[Http*]` member | `Controller` | — |
| `[Route("tpl")]` on the class | — | prefix for its members' route templates |
| `[HttpGet("tpl")]`/`HttpPost`/`Put`/`Delete`/`Patch` on a method | `Route` | `HandlesRoute`, source = the method, target `External(<verb> <class+method template joined>)`, `reason = "route:GET /api/invoices/{id}"` |
| Method with `[Function("Name")]` | `Route` | — |
| Parameter/method attribute ending in `Trigger` (`HttpTrigger`, `TimerTrigger`, `QueueTrigger`, `ServiceBusTrigger`, `BlobTrigger`) | — | `Triggers`, source = the function method, target `External(<attribute name>)`, `reason = "trigger:<first literal argument>"` |

Route targets resolve to `External` rather than a synthetic symbol — a URL
template is not a project symbol — exactly as an unresolved `node_modules`
import already does, keeping `HandlesRoute` traversable from the method.

### Call-driven (minimal APIs, DI registration, middleware) + EF Core

| Evidence | Edge |
|---|---|
| `Map{Get,Post,Put,Delete,Patch}(<string literal>, <handler>)` | `HandlesRoute` from the enclosing method (usually `Main`/`Program`), `reason = "route:GET /path"`; plus, when the handler is an identifier or method group, a second `HandlesRoute` to `Unresolved(<handler name>)` |
| `MapGroup("prefix")` | recorded as a prefix for chained `Map*` calls on the same statement only |
| `Add{Scoped,Transient,Singleton}<TService, TImpl>()` (2 type arguments) | two `RegisteredAs` edges from the enclosing method: `Unresolved(TImpl)` with `reason = "lifetime:scoped;role:impl;service:TService"`, `Unresolved(TService)` with `reason = "lifetime:scoped;role:service;impl:TImpl"` |
| `Add*<T>()` (1 type argument), or `AddSingleton(typeof(A), typeof(B))` | same pair, ceiling Low (partial shape) |
| `Use<Name>()` / `UseMiddleware<T>()` | `RegisteredAs` to `Unresolved(<T or Name>)`, `reason = "key:middleware"` |
| `DbSet<T>` property/field in a `DbContext` subclass | `PersistsTo`, source = the `DbContext` subclass, target `Unresolved(<T>)`, `reason = "dbset:<PropertyName>"` |

`Add*`/`Use*` are matched by **exact name from a closed list, never by
prefix** — `AddSomethingCustom` produces no edge, not a silent Low-confidence
guess.

## Confidence and provenance

Every framework edge carries `Provenance::Heuristic` and a non-`Exact`
confidence. The **provenance floor** (design D5) is what keeps a heuristic
from being laundered into a fact during cross-file resolution:

> An incoming `Heuristic` edge never upgrades to `Extracted`/`Resolved`.
> Stored confidence = `min(recognition ceiling, resolution confidence)`,
> ordered `Exact > High > Medium > Low > Unresolved`.

Recognition assigns a **ceiling** based on how direct the evidence is
(e.g. an explicit constructor-parameter type is High; a partial `Add*<T>()`
shape is Low). Resolution then assigns its own confidence based on how many
candidate symbols match that name:

| Resolution outcome | Resolution confidence |
|---|---|
| Exactly 1 same-domain candidate, kind `Class` | High |
| Exactly 1 same-domain candidate, kind `Interface` | Medium (capped) |
| More than 1 candidate | one Low edge per candidate |
| 0 candidates | no edge; `UnresolvedReference` preserved |

The stored confidence is the lower of the two. This means an interface-typed
dependency (`IInvoiceRepository`) never reads as "High — trust me": only the
paired `RegisteredAs` edge from the DI registration (`AddScoped<IInvoiceRepository, InvoiceRepository>()`)
says which implementation actually runs.

No framework edge is ever stored as `extracted` or `resolved`. A route →
data-layer path (`HandlesRoute → Injects → Calls → PersistsTo`) is
traversable via the existing MCP path/trace tools without any MCP-layer
change, since those tools are already `RelationshipKind`-shape agnostic — but
every hop through a framework kind stays visibly heuristic.

## Published limitations

These are deliberate scope boundaries, not defects. Framework recognition is
in-process, tree-sitter-only pattern matching — the same constraint phases 5
and 6 already committed to for C#. No Roslyn, MSBuild, `tsc`, or other
out-of-process type checker is invoked.

| Limitation | Effect on results |
|---|---|
| Angular HTML templates | `routerLink`, template bindings, `*ngIf`/control flow, and component usage inside markup produce no edges. No HTML/template grammar is added. |
| Legacy `@NgModule` fields | Only `providers` and `imports` arrays are read. `declarations`/`entryComponents` and other legacy fields are not covered. |
| Other TS decorator frameworks | NestJS, TypeORM, class-validator, and similar inherit the generic `Decorates` edge for free (from PR2's decorator walking) but have no dedicated recognition catalogue. |
| Blazor, Razor, MVC views, SignalR, gRPC, message buses | No `Publishes`/`Subscribes` edges exist; MassTransit/message-bus wiring, SignalR hubs, gRPC services, and Razor/Blazor views are not modelled. |
| Runtime/reflection-based DI | Assembly scanning, reflection-based registration, and any DI wiring without direct attribute or call-site evidence produce no edge. |
| Convention-based MVC routing | Routes inferred purely by naming convention (no `[Route]`/`[Http*]` attribute) are not recognized. |
| `MapGroup` prefix in a variable | A `MapGroup("prefix")` result is only applied as a prefix to `Map*` calls chained on the *same statement*. If the group result is assigned to a variable and used later, the prefix is lost — a published limitation, not a bug. |
| `Add*`/`Use*` matched by exact name only | The .NET call-driven catalogue is a closed list matched by exact method name, never by prefix. A custom extension method like `AddSomethingCustom` produces no edge rather than a guessed Low-confidence one. |
| EF Core's single narrow exception | The only generic-type-argument-derived edge in the system is `DbSet<T>` inside a `DbContext` subclass → `PersistsTo`. General generic type-argument resolution is out of scope; `List<T>`, `Task<T>`, `IQueryable<T>`, and `Dictionary<K,V>` produce no edge regardless of context. |
| Cross-stack edges | An Angular `HttpClient` call is never linked to the .NET endpoint it hits — that needs a URL-matching model this phase does not build. |
| No new `SymbolKind` | Framework roles are tags on existing symbols (`FrameworkRole`), never a new kind; a `Controller` is still a `Class`. |

For C#'s pre-existing structural limitations (no semantic compilation, no
NuGet/BCL resolution, generics are structural only, etc.), see
[docs/LANGUAGES.md](LANGUAGES.md).
