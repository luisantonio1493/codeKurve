# Delta for Symbol Index

Phase 5 moves TypeScript/JavaScript extraction behind a `LanguageAnalyzer` seam and adds C# as the second implementation, without changing any TypeScript-observable result. It adds a language-neutral `Visibility` enum plus `is_partial`/`is_record` modifiers to the symbol model, migration 0004 to persist them additively, and `.cs` discovery with `"csharp"` in the default language list (proposal: "the language layer stops being a coincidence").

## MODIFIED Requirements

### Requirement: Stable Symbol Key Excludes Position, Uses BLAKE3, and Disambiguates Partial Fragments

`symbol_key` MUST be the BLAKE3 hash of `(language, relative_path, symbol_kind, qualified_name, signature_fingerprint, partial_ordinal)`, where `partial_ordinal` MUST be `None` for every non-partial declaration and MUST NOT change the hashed input compared to the pre-Phase-5 five-component tuple when it is `None`. `partial_ordinal` MUST be `Some(n)` only for a `partial` declaration, distinguishing multiple fragments of the same type that would otherwise collide on `UNIQUE(project_id, symbol_key)`.

(Previously: `symbol_key` was the BLAKE3 hash of the five-component tuple `(language, relative_path, symbol_kind, qualified_name, signature_fingerprint)` with no partial-fragment disambiguation, because no analyzed language had a `partial` declaration concept.)

#### Scenario: Non-partial key is byte-identical to the pre-Phase-5 hash

- GIVEN a TypeScript symbol previously hashed under the five-component tuple
- WHEN `symbol_key` is computed with `partial_ordinal: None` after Phase 5 ships
- THEN the resulting BLAKE3 hash equals the pre-Phase-5 golden hash for that symbol, byte for byte

#### Scenario: Two partial fragments in one file get distinct keys

- GIVEN a C# file declaring `partial class Widget` twice with different members in each declaration
- WHEN the file is indexed
- THEN each declaration is its own symbol with a distinct `symbol_key` (distinct `partial_ordinal`), and neither collides with `UNIQUE(project_id, symbol_key)`

#### Scenario: Partial fragments across files keep their own identity

- GIVEN `partial class Widget` declared once in `a.cs` and once in `b.cs`
- WHEN both files are indexed
- THEN each fragment is indexed as its own symbol with `is_partial = true` and a `symbol_key` distinct from the other fragment's

## ADDED Requirements

### Requirement: Extraction Runs Behind a Per-Language Analyzer Seam Without Changing TypeScript Results

`extract::analyze(source, language, relative_path)` MUST dispatch to a `LanguageAnalyzer` implementation selected by `language`, and the TypeScript/JavaScript implementation's extraction logic, node-kind matching, and output MUST be unchanged from before the seam was introduced. `extract.rs` MUST contain no C#-specific and no TypeScript-specific node-kind string; each language's node-kind matching MUST live in its own `languages/` module.

#### Scenario: TypeScript extraction is unaffected by the seam

- GIVEN the same TypeScript source file analyzed once before and once after the `LanguageAnalyzer` refactor
- WHEN `extract::analyze` runs both times
- THEN the returned `FileAnalysis` (symbols, relationships, unresolved references) is identical

#### Scenario: A C# file is analyzed by its own analyzer

- GIVEN a `.cs` file
- WHEN `extract::analyze` runs
- THEN it dispatches to the C#-specific `LanguageAnalyzer` implementation, not the TypeScript one

### Requirement: Symbol Visibility Is a Language-Neutral Enum Independent of `is_exported`

Every indexed symbol MUST carry a `visibility` field with one of: `Public`, `Private`, `Protected`, `Internal`, `ProtectedInternal`, `PrivateProtected`, or `Default` (unspecified/package-private), independent of `is_exported`. `is_exported` MUST continue to mean only "declared with the TypeScript `export` keyword" and MUST be `false` for every C# symbol, since C# has no equivalent keyword.

#### Scenario: TypeScript symbols default to unaffected visibility

- GIVEN an existing TypeScript symbol indexed before Phase 5
- WHEN reindexed after Phase 5 ships
- THEN its `visibility` is `Default`, its `is_exported` value is unchanged from before, and no TypeScript query result changes

#### Scenario: C# visibility is recorded independently of export state

- GIVEN a C# `public class Widget`
- WHEN indexed
- THEN the symbol's `visibility` is `Public` and its `is_exported` is `false`

### Requirement: `is_partial` and `is_record` Modifiers on Symbols

Every indexed symbol MUST carry boolean `is_partial` and `is_record` fields, both defaulting to `false` for every non-C# symbol and for every C# symbol that is not a `partial` declaration or a `record`/`record struct` respectively. `record`/`record class` MUST index as `SymbolKind::Class` with `is_record = true`; `record struct` MUST index as `SymbolKind::Struct` with `is_record = true`. No new `SymbolKind::Record` variant MUST be introduced.

#### Scenario: record class indexes as Class with is_record

- GIVEN `public record Invoice(decimal Total);`
- WHEN indexed
- THEN the symbol has `kind = Class`, `is_record = true`

#### Scenario: record struct indexes as Struct with is_record

- GIVEN `public record struct Point(int X, int Y);`
- WHEN indexed
- THEN the symbol has `kind = Struct`, `is_record = true`

#### Scenario: partial class is flagged without merging

- GIVEN `partial class Widget` declared in two files
- WHEN indexed
- THEN both declarations are indexed as separate symbols with `is_partial = true`, and no merge into one canonical symbol occurs

### Requirement: Schema Migration 0004 Adds Visibility and Modifier Columns Without Wiping Data

Migration 0004 MUST add `symbols.visibility` (`TEXT NOT NULL DEFAULT 'default'`), `symbols.is_partial` (`INTEGER NOT NULL DEFAULT 0`), and `symbols.is_record` (`INTEGER NOT NULL DEFAULT 0`), additively, without altering or wiping any existing table, and MUST bump `SCHEMA_VERSION` to 4. Because none of the three columns participates in `symbol_key`, no existing symbol id MUST change as a result of this migration.

(Previously: `SCHEMA_VERSION` was 3, with no `visibility`, `is_partial`, or `is_record` columns on `symbols`.)

#### Scenario: Migration applies to a populated index without wiping it

- GIVEN a project previously indexed under `SCHEMA_VERSION = 3`
- WHEN `codekurve index` first runs after upgrading
- THEN migration 0004 applies, existing rows survive, pre-0004 symbols read `visibility = 'default'`, `is_partial = false`, `is_record = false`, and every pre-migration symbol id is unchanged

#### Scenario: Doctor reports post-migration schema version

- GIVEN a project indexed after migration 0004 applied
- WHEN `codekurve doctor` runs
- THEN it reports schema version 4

### Requirement: `.cs` Discovery and Default Language List

File discovery MUST classify files with the `.cs` extension as `LanguageId::CSharp` via the same `LanguageId::from_extension` mechanism used for other languages, and the built-in default `index.languages` list MUST include `"csharp"` alongside the existing defaults. An existing project's explicit `index.languages` configuration MUST be unaffected by this default-list change.

#### Scenario: A new project indexes C# by default

- GIVEN a project with no explicit `index.languages` configuration and a `.cs` file under its root
- WHEN `codekurve index` runs
- THEN the `.cs` file is discovered, classified as `LanguageId::CSharp`, and indexed

#### Scenario: An existing project's explicit language list is unaffected

- GIVEN a project whose config explicitly sets `index.languages = ["typescript"]`
- WHEN `codekurve index` runs after upgrading to Phase 5
- THEN `.cs` files under the root are still not indexed, since the explicit list was not implicitly widened
