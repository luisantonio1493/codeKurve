# Delta for C# Analysis

New capability. C# reaches basic parity with TypeScript's relationship graph: symbols, `using` directives, inheritance, implementation, calls, object creation, attributes, and namespaces, resolved in-process with `tree-sitter-c-sharp`, with published limitations instead of pretended completeness (proposal Intent, "In Scope: C# vertical slice").

## Requirements

### Requirement: C# Symbol Extraction Covers Types, Members, and Namespaces

The C# analyzer MUST extract, as indexed symbols: namespaces (both block-scoped `namespace X { }` and file-scoped `namespace X;`), classes, interfaces, structs, records (per the `is_record` modifier requirement), enums, constructors, methods, properties, fields, and nested types. Enum members MUST index as `SymbolKind::Field` with the enum as parent; no `SymbolKind::EnumMember` variant MUST be introduced. Operators, indexers, events, delegates, local functions, lambdas, and top-level statements MUST NOT be indexed as symbols.

#### Scenario: File-scoped namespace

- GIVEN `namespace Acme.Billing; public class Invoice {}`
- WHEN indexed
- THEN a namespace symbol for `Acme.Billing` and a class symbol for `Invoice` are indexed, with `Invoice`'s `qualified_name` reflecting the `Acme.Billing` prefix

#### Scenario: Block-scoped namespace with nested class

- GIVEN `namespace Acme.Billing { public class Invoice { private class LineItem {} } }`
- WHEN indexed
- THEN `Invoice` and its nested `LineItem` are both indexed as distinct symbols, `LineItem` scoped under `Invoice`

#### Scenario: Enum members index as Field

- GIVEN `public enum Status { Draft, Sent, Paid }`
- WHEN indexed
- THEN `Status` is indexed as `SymbolKind::Enum`, and `Draft`, `Sent`, `Paid` are each indexed as `SymbolKind::Field` with `Status` as parent

#### Scenario: Constructor, method, property, and field are all indexed

- GIVEN a class with one constructor, one method, one property, and one field
- WHEN indexed
- THEN all four are indexed as distinct symbols of the corresponding `SymbolKind`, each `contains`-linked to the class

### Requirement: `using` Directives Become Imports Edges

Every `using` directive MUST produce an `Imports` relationship: a plain `using X.Y;` and a `using static X.Y;` MUST both resolve to the target namespace symbol (one Low-confidence edge per candidate when multiple in-project files declare the same namespace, `External` when nothing in-project declares it); a `using alias = X.Y;` MUST also be recorded as `Imports`. Member references made visible by `using static` MUST NOT be resolved through it, and alias-qualified references MUST be recorded as unresolved with an explicit reason, per the published limitations.

#### Scenario: Plain using resolves to an in-project namespace

- GIVEN `using Acme.Billing;` in a file, and `Acme.Billing` declared by an in-project namespace
- WHEN indexed
- THEN an `Imports` edge resolves to that namespace symbol

#### Scenario: using static is recorded without resolving member visibility

- GIVEN `using static Acme.Billing.TaxRules;` followed by a call to `Calculate()` made visible only by the static using
- WHEN indexed
- THEN the `using static` directive is recorded as an `Imports` edge, and the `Calculate()` call is recorded as unresolved with an explicit reason, not silently resolved through the static using

#### Scenario: using alias is recorded, alias-qualified reference is unresolved

- GIVEN `using Billing = Acme.Billing;` and a later reference `Billing.Invoice`
- WHEN indexed
- THEN the alias directive is recorded as `Imports`, and the alias-qualified reference `Billing.Invoice` is recorded in `unresolved_references` with an explicit reason

### Requirement: Base List Entries Are Emitted as Pending References

For every base-list entry on a class, struct, or interface declaration, the C# analyzer MUST emit one pending reference (classified into `Inherits` or `Implements` by `relationship-graph`'s resolve-time disambiguation), never guessing the classification from the entry's own syntax.

#### Scenario: Multiple base-list entries each produce their own pending reference

- GIVEN `public class Invoice : BillingDocument, IBillable, IAuditable`
- WHEN extracted
- THEN three pending references are emitted, one per base-list entry, each independently resolvable

### Requirement: Calls and Object Creation Produce Calls and Constructs Edges

A direct method invocation MUST produce a `Calls` relationship; an object-creation expression (`new Foo(...)`) MUST produce a `Constructs` relationship targeting `Foo`. A target-typed `new()` (no type name at the call site) MUST produce an unresolved `Constructs` reference with an explicit reason, never a guess.

#### Scenario: Direct call produces a Calls edge

- GIVEN `var total = CalculateTotal(items);` where `CalculateTotal` is an in-project function
- WHEN indexed
- THEN a `Calls` edge resolves to `CalculateTotal`

#### Scenario: Object creation produces a Constructs edge

- GIVEN `var invoice = new Invoice();`
- WHEN indexed
- THEN a `Constructs` edge resolves to the `Invoice` class

#### Scenario: Target-typed new() is unresolved, not guessed

- GIVEN `Invoice invoice = new();`
- WHEN indexed
- THEN a `Constructs` reference is recorded in `unresolved_references` with an explicit reason, since no type name appears at the call site

### Requirement: Attributes Produce Decorates Edges Preserving Name and Span

Every attribute application on a declaration MUST produce a `Decorates` relationship whose source is the annotated declaration and whose target text is the attribute's original name as written (no framework-specific special-casing, e.g. `[HttpGet]` is recorded as the literal name `HttpGet`), and whose span is the attribute's own source span, not the annotated declaration's span.

#### Scenario: Attribute on a class produces a Decorates edge with its own span

- GIVEN `[Serializable]\npublic class Invoice {}`
- WHEN indexed
- THEN a `Decorates` edge exists from `Invoice`, target text `Serializable`, with a span covering only the `[Serializable]` attribute text, not the whole class declaration

#### Scenario: No attribute name is special-cased

- GIVEN `[HttpGet]\npublic IActionResult Get() { ... }`
- WHEN indexed
- THEN the `Decorates` edge's target text is the literal string `HttpGet`, with no routing/framework semantics inferred (per Known Limitations, § no framework semantics)

### Requirement: All Six C# Visibility Levels Round-Trip

Every one of the six C# access levels — `public`, `private`, `protected`, `internal`, `protected internal`, and `private protected` — MUST round-trip through IR → store → query output as distinct `Visibility` values, with `protected internal` (`ProtectedInternal`) and `private protected` (`PrivateProtected`) distinguishable from plain `protected` and `internal`.

#### Scenario: All six levels are distinct after indexing

- GIVEN a class with one member at each of the six access levels
- WHEN indexed and queried (e.g. via `codekurve symbol`)
- THEN each member reports its own distinct `visibility` value, and `protected internal`/`private protected` are never conflated with `protected`/`internal`

### Requirement: Generic Type Parameters and Constraints Are Recorded Structurally Only

Generic type parameter names and `where` constraint clause text MUST be recorded in `signature_fingerprint`. No relationship edge (including `UsesType`) MUST be created from a generic type parameter or its constraints.

#### Scenario: Generic class with a where constraint records fingerprint, no edges

- GIVEN `public class Repository<T> where T : IComparable {}`
- WHEN indexed
- THEN `Repository`'s `signature_fingerprint` includes `T` and the `IComparable` constraint text, and no `UsesType` (or other) edge is created linking `Repository` to `IComparable`

#### Scenario: Type argument at a generic instantiation site does not resolve

- GIVEN `var repo = new Repository<Invoice>();`
- WHEN indexed
- THEN the `Constructs` edge resolves `Repository`, but no edge links `Repository` to `Invoice` through the type argument (per Known Limitations, generics are structural only)

### Requirement: Partial Classes Are Flagged, Not Merged

Every `partial` declaration MUST be indexed as its own symbol with `is_partial = true`. No cross-declaration merge into one canonical symbol MUST occur; a reference to the type MAY resolve to multiple fragments as a legitimate ambiguity set (Low confidence, one edge per candidate), per the existing multi-candidate resolution policy.

#### Scenario: Partial fragments in different files are indexed independently

- GIVEN `partial class Widget { public void A() {} }` in `a.cs` and `partial class Widget { public void B() {} }` in `b.cs`
- WHEN indexed
- THEN two `Widget` symbols exist, both `is_partial = true`, neither merged, and each retains its own members

#### Scenario: A reference to a partial type resolves as an ambiguity set

- GIVEN a call site referencing `Widget` where two partial fragments exist
- WHEN resolved
- THEN one Low-confidence edge per fragment candidate is created, not a single guessed resolution

### Requirement: Namespace-Aware Qualified Names Stay Path-Prefixed

A C# symbol's `qualified_name` MUST include its namespace as a prefix inside the same path-prefixed addressing scheme used by every other language (`relative_path::Namespace.Owner.member`), so `EdgeTarget::Global { file, qualified_name }` and every persisted-key scheme continue to work unchanged. The namespace MUST NOT introduce a new addressing dimension.

#### Scenario: Namespaced method qualified name

- GIVEN `src/Billing/Invoice.cs` with `namespace Acme.Billing; public class Invoice { public decimal Total() { ... } }`
- WHEN indexed
- THEN the method's `qualified_name` is `src/Billing/Invoice.cs::Acme.Billing.Invoice.Total`

### Requirement: Unresolved References Are Preserved With Explicit Reasons, Never Dropped or Guessed

References the C# analyzer cannot resolve — BCL/NuGet types, target-typed `new()`, alias-qualified names, and unresolved base-list entries — MUST be recorded in `unresolved_references` with an explicit reason, never silently dropped and never resolved by a naming-convention guess.

#### Scenario: BCL type reference is unresolved with a reason

- GIVEN a method parameter typed `List<Invoice>` where `List<T>` is a BCL type not present in the project
- WHEN indexed
- THEN the reference to `List` is recorded as unresolved (or `External`) with an explicit reason, and no fabricated resolution to an in-project symbol is created

#### Scenario: Every listed unresolved case carries a reason, no silent drops

- GIVEN target-typed `new()`, an alias-qualified reference, and an unresolved base-list entry all present in one fixture
- WHEN indexed
- THEN each produces its own `unresolved_references` row with a distinct, explicit reason, and none is silently omitted from the output
