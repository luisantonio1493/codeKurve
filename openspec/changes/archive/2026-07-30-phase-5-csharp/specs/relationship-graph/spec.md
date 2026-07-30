# Delta for Relationship Graph

Phase 5 generalizes relationship extraction and resolution from TypeScript-only wording to a multi-language shape: a new `RelationshipKind::Decorates` shared by C# attributes (and later TS decorators), `kind_matches` becoming a per-language trait method, resolution candidates filtered by the reference's source language, and base-list class-vs-interface disambiguation performed at resolve time because C# has no `implements` keyword. Every existing TypeScript-observable relationship-graph behavior is unchanged (proposal: "zero TypeScript regression").

## MODIFIED Requirements

### Requirement: Relationship Kind Extraction Is Per-Language

The system MUST extract relationships of kind imports, exports, contains, extends, implements, calls, constructs, references, and decorates from each indexed language's syntactic scope, using that language's `LanguageAnalyzer` implementation. TypeScript/JavaScript extraction MUST continue to produce exactly the same relationship kinds and edges it produced before Phase 5.

(Previously: the system extracted relationships of kind imports, exports, contains, extends, implements, calls, constructs, and references from the TypeScript/JavaScript syntactic scope only; no `decorates` kind existed.)

#### Scenario: Class extends and implements (TypeScript, unchanged)

- GIVEN a file with `class Foo extends Base implements IFoo`
- WHEN the file is indexed
- THEN one `extends` edge Foo→Base and one `implements` edge Foo→IFoo exist, both `Resolved` provenance when the target is in-project

#### Scenario: C# attribute produces a decorates edge

- GIVEN a C# `[Serializable] public class Widget {}`
- WHEN the file is indexed
- THEN one `decorates` edge exists from `Widget` to a target carrying the attribute's original name text (`Serializable`) and the attribute's own source span

### Requirement: Two-Pass Whole-Project Resolution Filters Candidates by Source Language

Relationships MUST be resolved against a whole-project symbol table built after all files are parsed, and a reference's resolution candidates MUST be filtered to symbols of the same language as the reference's source file. A reference in a `.cs` file MUST NOT resolve to a TypeScript symbol, and vice versa, even when names collide.

(Previously: resolution ran against the whole-project symbol table with no language filter, because only one language was ever indexed in a project.)

#### Scenario: Cross-file call resolution within one language (unchanged)

- GIVEN `a.ts` calls a function exported from `b.ts`, both indexed in the same run
- WHEN indexing completes
- THEN the `calls` edge resolves to `b.ts`'s symbol regardless of parse order

#### Scenario: Same-name symbols in different languages never cross-resolve

- GIVEN a mixed-language project where a TypeScript file and a C# file each declare a symbol named `Invoice`
- WHEN both files are indexed in the same run
- THEN no reference in the C# file resolves to the TypeScript `Invoice` symbol, and no reference in the TypeScript file resolves to the C# `Invoice` symbol

## ADDED Requirements

### Requirement: `kind_matches` Is a Per-Language Trait Method

`kind_matches(RelationshipKind, SymbolKind) -> bool` MUST be implemented per language on its `LanguageAnalyzer`, and `resolve.rs` MUST dispatch through the analyzer of the reference's source language rather than through one shared, language-neutral function. TypeScript's `kind_matches` answers MUST be unchanged from before this method moved onto the trait.

#### Scenario: TypeScript kind_matches answers are unchanged

- GIVEN the same TypeScript relationship-kind/symbol-kind pairs checked before and after `kind_matches` became a trait method
- WHEN each pair is checked
- THEN the boolean answer is identical to before the change

#### Scenario: C# and TypeScript disagree on a kind/symbol pair without cross-contamination

- GIVEN a C# `Inherits` reference whose candidate is a `Struct` symbol (valid for C#, since `record struct` folds into `Struct`)
- WHEN `kind_matches` is dispatched through the C# analyzer for that candidate
- THEN it returns a per-language answer for C#, and this has no effect on any TypeScript `kind_matches` result for the same `RelationshipKind`/`SymbolKind` pair

### Requirement: Base List Class-vs-Interface Disambiguation at Resolve Time

For a C# base-list entry (a type listed after `:` on a class, struct, or interface declaration), the system MUST emit one pending reference per base-list entry at extraction time and classify it as `Inherits` or `Implements` only at resolve time, based on the resolved candidate's `SymbolKind` (`Class`/`Struct` → `Inherits`, `Interface` → `Implements`). No naming heuristic (e.g. an `I`-prefix convention) MUST be used to guess the classification.

#### Scenario: Base-list entry resolves to a class as Inherits

- GIVEN `public class Invoice : BillingDocument` where `BillingDocument` is an in-project class
- WHEN resolved
- THEN an `Inherits` edge Invoice→BillingDocument is created

#### Scenario: Base-list entry resolves to an interface as Implements

- GIVEN `public class Invoice : IBillable` where `IBillable` is an in-project interface
- WHEN resolved
- THEN an `Implements` edge Invoice→IBillable is created

#### Scenario: Unresolved base-list entry is recorded with a reason, never guessed

- GIVEN `public class Invoice : SomeBaseType` where `SomeBaseType` has zero in-project candidates
- WHEN resolved
- THEN an `unresolved_references` row is created with `relationship_kind = UsesType` and an explicit reason, and no `Inherits`/`Implements` edge is guessed from the name

### Requirement: `internal` Visibility Is Recorded But Never Reduces Resolution Confidence

A C# symbol's `internal` (or `ProtectedInternal`/`PrivateProtected`) visibility MUST be recorded on the symbol and MUST NOT reduce the confidence tier of any relationship edge resolving to or from it, because no `.sln`/`.csproj` assembly boundary exists in the single-flat-project model to enforce or approximate assembly-level scoping.

#### Scenario: An internal symbol resolves at the same confidence as a public one

- GIVEN two otherwise-identical C# classes in the same project, one `public` and one `internal`, each referenced once by an unambiguous same-project call
- WHEN both calls are resolved
- THEN both `calls` edges receive the same confidence tier; the `internal` symbol's visibility does not lower it

#### Scenario: internal is not enforced as a boundary

- GIVEN a call from one C# file to an `internal` symbol declared in a different notional assembly root that the flat single-project model does not distinguish
- WHEN resolved
- THEN the reference resolves exactly as it would if the symbol were `public`, since no project/assembly boundary is modeled or enforced
