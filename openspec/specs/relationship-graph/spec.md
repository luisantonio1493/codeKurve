# Relationship Graph Specification

## Purpose

Turn the Phase 1 flat symbol index into a TypeScript/JavaScript relationship graph: imports, exports, contains, extends, implements, calls, constructs, references, and unresolved edges, each with provenance and a confidence tier, resolved against real qualified names (plan §17.3, §18, §20).

## Requirements

### Requirement: Relationship Kind Extraction

The system MUST extract relationships of kind imports, exports, contains, extends, implements, calls, constructs, and references from the §20.1 TypeScript/JavaScript syntactic scope.

#### Scenario: Class extends and implements

- GIVEN a file with `class Foo extends Base implements IFoo`
- WHEN the file is indexed
- THEN one `extends` edge Foo→Base and one `implements` edge Foo→IFoo exist, both `Resolved` provenance when the target is in-project

#### Scenario: Contains hierarchy

- GIVEN a class with two methods
- WHEN the file is indexed
- THEN two `contains` rows link the class to each method; migration 0002 does not `ALTER` the `symbols` table to express this

### Requirement: Real Qualified Name Computation

Every symbol's `qualified_name` MUST be computed per §20.3, replacing the Phase 1 placeholder (`qualified_name = name`).

#### Scenario: Nested member qualified name

- GIVEN `src/services/member.service.ts` defines `class MemberService { getEligibility() {} }`
- WHEN indexed
- THEN the method's `qualified_name` is `src/services/member.service.ts::MemberService.getEligibility`

### Requirement: Module Resolution Order

Import targets MUST resolve in order: relative path → exact file → implicit extension (`.ts`, `.tsx`, `.js`, `.jsx`) → `index.*` → `tsconfig.json` path aliases → external package (registered as an external node, not indexed). `node_modules` MUST NOT be indexed.

#### Scenario: Implicit extension resolution

- GIVEN `import { x } from './utils'` and a sibling `utils.ts`
- WHEN resolution runs
- THEN the import edge resolves to `utils.ts`'s exported symbol with confidence `Exact`

#### Scenario: External package import

- GIVEN `import { z } from 'zod'`
- WHEN resolution runs
- THEN the target is recorded as an external node; no `unresolved_references` row or indexing error is produced

### Requirement: Confidence Tiers on Calls and Constructs

Call and construct edges MUST carry confidence Exact, High, Medium, or Low per §20.4.

#### Scenario: Exact local call

- GIVEN `function a() { b(); }` where `b` is a local same-file function
- WHEN indexed
- THEN the `calls` edge a→b has confidence `Exact`

#### Scenario: Ambiguous member call

- GIVEN two unrelated classes each define `getEligibility()` and the call site's receiver type cannot be determined
- WHEN indexed
- THEN a `calls` row is created with confidence `Low` and provenance `Heuristic`, not silently dropped and not silently resolved as if certain

### Requirement: Unresolved Reference Handling

A reference with zero resolution candidates, or insufficient context to attempt resolution, **or whose previously resolved target symbol was deleted**, MUST be recorded in `unresolved_references` and never dropped silently (§18.3). A reference with one or more candidates MUST NOT be recorded there.

(Phase 3 addition: references whose previously resolved targets are deleted must convert to unresolved, never silently dropped or left pointing at a nonexistent symbol row.)

#### Scenario: Zero-candidate import

- GIVEN `import { Missing } from './nonexistent'`
- WHEN indexed
- THEN an `unresolved_references` row is created with `target_text = './nonexistent'`, `candidate_count = 0`, and a `reason`

#### Scenario: Multi-candidate call is not unresolved

- GIVEN a call site with 3 same-name method candidates project-wide
- WHEN indexed
- THEN a Low-confidence `relationships` row is created for the best candidate; no `unresolved_references` row is created for this reference

#### Scenario: Deleting a symbol's file unresolves its inbound edges

- GIVEN `src/b.ts` has a `Resolved` `calls` edge into a function exported from `src/a.ts`
- WHEN `src/a.ts` is deleted and the batch is applied
- THEN the `Resolved` edge is removed and an `unresolved_references` row is created recording the now-missing target, not silently dropped and not left pointing at a nonexistent symbol row

### Requirement: Two-Pass Whole-Project Resolution

Relationships MUST be resolved against a whole-project symbol table built after all files are parsed, not single-file context alone (§22.3).

#### Scenario: Cross-file call resolution

- GIVEN `a.ts` calls a function exported from `b.ts`, both indexed in the same run
- WHEN indexing completes
- THEN the `calls` edge resolves to `b.ts`'s symbol regardless of parse order

### Requirement: Affected-Set Resolution for Incremental Batches

For an incremental batch, relationships MUST be re-resolved against the changed files' symbols plus every symbol whose existing relationships reference (or previously referenced) a changed or deleted symbol — the affected set — rather than the whole project's symbol table. A full reindex (whether run directly or as the oversized-batch fallback) MUST still resolve against the whole-project symbol table built after all files are parsed.

(Previously: relationships were always resolved against a whole-project symbol table built after all files are parsed, with no incremental/affected-set mode, because every index run was a full reindex.)

#### Scenario: Cross-file call resolution within a full reindex

- GIVEN `a.ts` calls a function exported from `b.ts`, both indexed in the same full reindex
- WHEN indexing completes
- THEN the `calls` edge resolves to `b.ts`'s symbol regardless of parse order

#### Scenario: Incremental batch re-resolves only affected dependents

- GIVEN `b.ts` exports a function called from `a.ts`, both previously indexed, and only `b.ts`'s export signature changes
- WHEN the incremental batch containing `b.ts` is applied
- THEN `a.ts`'s call edge into `b.ts` is re-resolved as part of the affected set, but unrelated files with no relationship to `b.ts`'s changed symbol are not re-resolved

### Requirement: Atomic Persistence Per Batch

One batch's relationships and unresolved references (whether the batch is a full reindex or an incremental per-file batch) MUST persist in a single atomic transaction; a failed or interrupted batch MUST leave no partial `relationships` or `unresolved_references` rows visible.

(Previously: atomicity was scoped to "one index run", which was always a full reindex; there was no smaller incremental-batch unit.)

#### Scenario: Atomic persistence on incremental batch failure

- GIVEN an incremental batch that fails mid-resolution
- WHEN the batch aborts
- THEN no partial `relationships` or `unresolved_references` rows from that batch are visible, and the previously committed graph state is unchanged

### Requirement: Schema Migration 0002

Migration 0002 MUST add `relationships` and `unresolved_references` tables plus their §24.4 indexes, without altering any 0001 table, and bump `SCHEMA_VERSION` to 2.

#### Scenario: Fresh database migration

- GIVEN an empty database
- WHEN `codekurve index` runs
- THEN migrations 0001 then 0002 apply in order and `doctor` reports schema version 2

#### Scenario: Rollback

- GIVEN a database at schema version 2 with no persisted production data
- WHEN rollback is performed
- THEN dropping `relationships`/`unresolved_references` and resetting `SCHEMA_VERSION` to 1 leaves Phase 1 commands fully functional

## MODIFIED Requirements (Phase 5)

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

## ADDED Requirements (Phase 5)

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
