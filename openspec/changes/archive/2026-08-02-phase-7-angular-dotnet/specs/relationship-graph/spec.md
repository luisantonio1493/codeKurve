# Delta for Relationship Graph

Phase 7 extends TypeScript extraction to produce `Decorates` edges (mirroring C#'s attribute extraction since phase 5) and adds five new `RelationshipKind` variants — `Injects`, `RegisteredAs`, `HandlesRoute`, `Triggers`, `PersistsTo` — emitted exclusively by the `framework-awareness` recognition pass, never by a language analyzer. Every existing TypeScript/C# relationship-graph behavior is unchanged.

## MODIFIED Requirements

### Requirement: Relationship Kind Extraction Is Per-Language

The system MUST extract relationships of kind imports, exports, contains, extends, implements, calls, constructs, references, and decorates from each indexed language's syntactic scope, using that language's `LanguageAnalyzer` implementation. TypeScript/JavaScript extraction MUST walk class, method, property, and constructor-parameter decorators into `decorates` edges carrying the decorator's literal name and its own source span, mirroring how `csharp.rs` already extracts attributes. No Angular-specific (or any other framework-specific) decorator name MUST be special-cased inside `languages/typescript.rs`.

(Previously: TypeScript/JavaScript extraction produced imports, exports, contains, extends, implements, calls, constructs, and references only; TypeScript decorators were not walked into `decorates` edges — only C# attributes were.)

#### Scenario: Class extends and implements (TypeScript, unchanged)

- GIVEN a file with `class Foo extends Base implements IFoo`
- WHEN the file is indexed
- THEN one `extends` edge Foo→Base and one `implements` edge Foo→IFoo exist, both `Resolved` provenance when the target is in-project

#### Scenario: C# attribute produces a decorates edge (unchanged)

- GIVEN a C# `[Serializable] public class Widget {}`
- WHEN the file is indexed
- THEN one `decorates` edge exists from `Widget` to a target carrying the attribute's original name text (`Serializable`) and the attribute's own source span

#### Scenario: TypeScript class decorator produces a decorates edge

- GIVEN a TypeScript `@Component({...}) export class InvoiceList {}`
- WHEN the file is indexed
- THEN one `decorates` edge exists from `InvoiceList` to a target carrying the literal decorator name `Component` and the decorator's own span, with no Angular semantics inferred at this layer

#### Scenario: TypeScript constructor-parameter decorator produces a decorates edge

- GIVEN a TypeScript constructor parameter annotated `@Inject(TOKEN) private svc: Foo`
- WHEN the file is indexed
- THEN a `decorates` edge exists carrying the literal decorator name `Inject`, independent of any framework recognition

## ADDED Requirements

### Requirement: Framework-Level Relationship Kinds Exist and Are Emitted Only By the Recognition Pass

`RelationshipKind` MUST include `Injects`, `RegisteredAs`, `HandlesRoute`, `Triggers`, and `PersistsTo`. These five kinds MUST be emitted exclusively by the `framework-awareness` recognition pass; no `LanguageAnalyzer` implementation under `languages/` MUST emit any of them directly.

#### Scenario: A language analyzer never emits a framework-level kind

- GIVEN the extraction output of `languages/typescript.rs` and `languages/csharp.rs` for any fixture
- WHEN the emitted relationship kinds are inspected
- THEN none of `Injects`, `RegisteredAs`, `HandlesRoute`, `Triggers`, `PersistsTo` appears; only the recognition pass's output contains them

#### Scenario: Framework-level edges resolve against already-resolved symbols

- GIVEN an `Injects` edge produced by the recognition pass
- WHEN its source and target are inspected
- THEN both reference symbols already present in the resolved symbol table, established by the existing per-language resolution requirements

### Requirement: TypeScript Kind Matching Extends to Decorates Without TypeScript Regression

TypeScript's `kind_matches` implementation MUST accept `decorates` edges the same way C#'s does, and every pre-existing TypeScript `kind_matches` answer for non-decorates kinds MUST be unchanged.

#### Scenario: Existing TypeScript kind_matches answers are unaffected

- GIVEN the same TypeScript relationship-kind/symbol-kind pairs checked before and after decorator extraction is added
- WHEN each non-decorates pair is checked
- THEN the boolean answer is identical to before the change
