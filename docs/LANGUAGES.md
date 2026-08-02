# Supported Languages

CodeKurve builds a local structural index. It does not compile source or model
runtime behavior. The tables below describe the currently indexed language
surface; unresolved or ambiguous references are retained rather than guessed.

## Coverage

| Language | Symbols | Relationships |
|---|---|---|
| TypeScript | Classes, interfaces, methods, and top-level functions | `contains`, `imports`, `exports`, `references`, `calls`, `constructs`, `inherits`, `implements`, `decorates` |
| JavaScript | Classes, methods, and top-level functions supported by the shared TypeScript/JavaScript analyzer | `contains`, `imports`, `exports`, `references`, `calls`, `constructs`, `inherits`, `implements`, `decorates` |
| C# | Namespaces; classes, interfaces, structs, records, enums; constructors, methods, properties, fields, enum members, and nested types | `contains`, `imports`, `calls`, `constructs`, `inherits`, `implements`, `uses-type`, `decorates` |

C# records retain their class/struct kind and record modifier. Enum members are
indexed as fields. The C# graph is resolved only against C# symbols; it does
not create edges to TypeScript or JavaScript symbols with the same name.

## C# Known Limitations

| Limitation | Effect on results |
|---|---|
| No semantic compilation (no Roslyn/MSBuild) | Resolution is syntactic; overload selection, inferred types, and implicit conversions are not modelled. |
| Partial types not merged | Each `partial` declaration is its own symbol. References may resolve to multiple fragments as a Low-confidence ambiguity set. |
| No NuGet / BCL resolution | Package and framework types are external or unresolved with a reason; `[HttpGet]`, `Task`, and `List<T>` do not resolve to definitions. |
| Generics are structural only | Type parameters and constraints are recorded in the signature fingerprint; type arguments and constraints create no type edges. |
| Extension methods | Only direct syntactic evidence is indexed; `obj.Extension()` does not resolve to the static extension method declaration. |
| Overload resolution | Calls resolve by name; multiple candidates produce one Low-confidence edge per candidate. |
| `using static` | The directive is recorded as an import, but members made visible by it are not resolved through it. |
| `using alias = X.Y` | The directive is recorded as an import, but alias-qualified references remain unresolved with an explicit reason. |
| `global using` | Treated as file-local; it is not applied project-wide. |
| Source generators | Generated code is not indexed; partial members supplied by generators can remain unresolved. |
| Reflection, `dynamic`, runtime DI | Reflection and `dynamic` are not modelled. DI registrations are recognized only from direct call-site evidence (`AddScoped<IFoo, Foo>()`, `AddDbContext<T>()`) by the framework pass; assembly scanning and any registration without a literal call site create no edges. See [docs/FRAMEWORKS.md](FRAMEWORKS.md). |
| No solution/project model | All `.cs` files below the root are one flat project. `.sln`/`.csproj` boundaries, project references, and assembly-level `internal` scope are not enforced. |
| Framework semantics live outside the analyzer | `languages/csharp.rs` itself is framework-blind. ASP.NET routing, Azure Functions triggers, EF Core `DbSet<T>` mappings, and DI wiring are recognized by the separate downstream pass and emit `handles-route`/`triggers`/`persists-to`/`registered-as` edges, always as `heuristic` provenance. See [docs/FRAMEWORKS.md](FRAMEWORKS.md) for the catalogue and its limits. |
| Not indexed as symbols | Operators, indexers, events, delegates, local functions, lambdas, and top-level statements are not symbols. |
| Target-typed `new()` | No type name is present at the call site, so construction is unresolved with a reason. |
| Decorator/attribute semantics | `decorates` is extracted for both C# attributes and TypeScript decorators, but only as a syntactic edge to the decorator name. What a decorator *does* (metadata arguments, generated members) is not modelled outside the framework catalogue. |
| Preprocessor directives | `#if` branches are parsed as written; configuration-conditional evaluation is not performed. |

For source-level behavior and confidence/provenance rules, see
[the architecture](ARCHITECTURE.md) and [data model](DATA_MODEL.md).
