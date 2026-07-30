# Tasks: Phase 5 — C#

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~2600–3400 |
| 400-line budget risk | High |
| Chained PRs recommended | Yes |
| Suggested split | PR1 → PR2 → PR3 → PR4 → PR5 → PR6 → PR7 |
| Delivery strategy | exception-ok |
| Chain strategy | feature-branch-chain |

Decision needed before apply: No
Chained PRs recommended: Yes
Chain strategy: feature-branch-chain
400-line budget risk: High

### Suggested Work Units

| Unit | Goal | Likely PR | Focused test command | Runtime harness | Rollback boundary |
|------|------|-----------|----------------------|-----------------|-------------------|
| 1 | Domain + storage groundwork, no behavior change: `LanguageId::CSharp`, `Visibility`, `is_record`, `RelationshipKind::Decorates`, IR fields, migration 0004, `symbol_key` `partial_ordinal` (base=tracker) | PR1 | `cargo test -p codekurve-core -p codekurve-store` | Index the existing TS fixture on a pre-0004 DB; assert migration applies without wipe and symbol ids are unchanged | Revert the enum/field additions + migration 0004 + `SCHEMA_VERSION` bump |
| 2 | `LanguageAnalyzer` trait; TS/JS extraction moved to `languages/typescript.rs`; `extract::analyze` becomes dispatch; `kind_matches` becomes per-language (base=PR1) | PR2 | `cargo test -p codekurve-analysis` + `cargo test --workspace` | Index `fixtures/typescript/basic`, byte-diff all CLI stdout against a pre-refactor baseline | Revert `languages/` + restore `extract.rs` body and the `resolve.rs` call site |
| 3 | C# analyzer part 1 — grammar pin, `.cs` discovery, symbols (namespace/class/interface/struct/record/enum/ctor/method/property/field/nested), visibility, `is_partial`, generics fingerprint, `Contains` (base=PR2) | PR3 | `cargo test -p codekurve-analysis csharp` | `codekurve index` + `search` over `fixtures/csharp/basic`, eyeball symbol kinds and visibility | Revert `languages/csharp.rs` + the `tree-sitter-c-sharp` dependency |
| 4 | C# analyzer part 2 — `using` directives, base list, calls, object creation, attributes/`Decorates`, `UsesType` (base=PR3) | PR4 | `cargo test -p codekurve-analysis csharp` | Single-file C# cases through `analyze`, assert edge kinds/spans/name texts | Revert the edge-emitting functions in `languages/csharp.rs` |
| 5 | Resolution — namespace-aware qualified names, using-scoped candidate lookup, language-filtered candidates, base-list class-vs-interface classification, unresolved rows with reasons (base=PR4) | PR5 | `cargo test -p codekurve-analysis resolve` | Multi-file C# fixture through extract+resolve; assert cross-file `Inherits`/`Implements`/`Calls` and preserved unresolved rows | Revert `resolve.rs` C#-path changes; C# stays file-local |
| 6 | Fixtures and goldens — `csharp-graph`, `fixtures/csharp/basic`, `vertical_slice_csharp.rs`, mixed-language fixture, partial-identity test, full TS regression re-run (base=PR5) | PR6 | `cargo test --workspace` | Full CLI round-trip on the C# fixture; confirm TS goldens pass with zero edits | Revert the new fixture dirs + test files |
| 7 | Docs — `docs/LANGUAGES.md` limitations, README language matrix (base=PR6, merges to tracker) | PR7 | N/A — doc-only | Manual read-through against the Known Limitations table | Revert `docs/LANGUAGES.md` + README section |

## Phase 1: PR1 — Domain + Storage Groundwork (req: symbol-index "Stable Symbol Key Excludes Position, Uses BLAKE3, and Disambiguates Partial Fragments", "Symbol Visibility Is a Language-Neutral Enum Independent of is_exported", "is_partial and is_record Modifiers on Symbols", "Schema Migration 0004 Adds Visibility and Modifier Columns Without Wiping Data", ".cs Discovery and Default Language List"; design "Migration 0004", "repo.rs")

- [x] 1.1 `codekurve-core/src/language.rs`: add `LanguageId::CSharp`; `.cs` extension mapping via `LanguageId::from_extension`; `"csharp"` as `as_str`/name.
- [x] 1.2 `codekurve-core/src/config.rs`: add `"csharp"` to the built-in default `index.languages` list; existing explicit configs stay unaffected.
- [x] 1.3 `codekurve-core/src/symbol.rs`: add `Visibility` enum (`Public`, `Protected`, `Internal`, `Private`, `ProtectedInternal`, `PrivateProtected`, `Default`) + `as_str`; add `RelationshipKind::Decorates` → `"decorates"`; add `Symbol.visibility: Visibility`, `Symbol.is_partial: bool`, `Symbol.is_record: bool`, `Symbol.partial_ordinal: Option<u32>`.
- [x] 1.4 `codekurve-analysis/src/ir.rs`: add the same four fields to `ExtractedSymbol`; every existing TS construction site sets `Visibility::Default` / `is_partial: false` / `is_record: false` / `partial_ordinal: None`.
- [x] 1.5 `codekurve-store/src/migrations.rs`: add MIGRATION_0004 (three additive `ALTER TABLE symbols ADD COLUMN` statements, `NOT NULL DEFAULT`), `SCHEMA_VERSION = 4`, no `DELETE`/wipe DML.
- [x] 1.6 `codekurve-store/src/repo.rs`: `symbol_key` gains a sixth parameter `partial_ordinal: Option<u32>`; `None` produces a byte-identical hashed input to the pre-Phase-5 five-component `format!`; add `parse_visibility`/`Visibility::as_str` mapping for persistence.
- [x] 1.7 `codekurve-store/src/repo.rs`: `reindex`'s symbol `INSERT`/`StoredSymbol` gain `visibility`, `is_partial`, `is_record` columns and params; `search_symbols`/`find_by_name`/`find_symbol_by_id` SELECTs read the new columns.
- [x] 1.8 Test: golden `symbol_key(lang, path, kind, qname, fingerprint, None)` equals a hardcoded pre-migration hash for a known TypeScript symbol.
- [x] 1.9 Test: `Some(0) != Some(1) != None` for `symbol_key`'s `partial_ordinal` component.
- [x] 1.10 Test: migration 0004 on a populated pre-0004 DB — row count and every `symbol_key`/`id` unchanged, three columns present with defaults (`default`/`0`/`0`); rename `fresh_database_reaches_schema_version_3` → `_4`.
- [x] 1.11 Test: `codekurve doctor` reports schema version 4 after migration.
- [x] 1.12 Run existing `crates/codekurve-analysis/tests/*` and `crates/codekurve-bin/tests/*` golden suites; all pass. One expectation edit was required and is in-scope for this PR: `codekurve-bin/tests/vertical_slice.rs::doctor_reports_fts5` asserted a hardcoded `schema: version 3` string, bumped to `4` (spec "Doctor reports post-migration schema version"). No `fixtures/ts-graph/` file or TS relationship/symbol-count expectation was touched.

## Phase 2: PR2 — LanguageAnalyzer Trait + TypeScript Move (req: symbol-index "Extraction Runs Behind a Per-Language Analyzer Seam Without Changing TypeScript Results"; relationship-graph "kind_matches Is a Per-Language Trait Method"; design "Module Layout", "Helper split")

- [x] 2.1 `codekurve-analysis/src/languages/mod.rs`: define `pub trait LanguageAnalyzer { fn language(&self) -> LanguageId; fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis>; fn kind_matches(&self, rel: RelationshipKind, sym: SymbolKind) -> bool; }`.
- [x] 2.2 `languages/mod.rs`: `analyzer_for(LanguageId) -> &'static dyn LanguageAnalyzer` over `TS`/`JS`/`CS` consts (CS stubbed until PR3; `CSharpAnalyzer` may be a minimal placeholder if needed to keep the match exhaustive).
- [x] 2.3 `languages/mod.rs`: `same_resolution_domain(a: LanguageId, b: LanguageId) -> bool` — `(TypeScript|JavaScript, TypeScript|JavaScript) | (CSharp, CSharp)`.
- [x] 2.4 `languages/mod.rs`: move `PendingRel`, `resolve_pending`, `push_unresolved_edge` from `extract.rs`; `resolve_pending` now takes `analyzer: &dyn LanguageAnalyzer` and dispatches `kind_matches` through it.
- [x] 2.5 `languages/typescript.rs`: move `CollectCtx`, `collect`, `collect_heritage`, `collect_imports`, `collect_exports`, `module_specifier`, `is_default_export`, `export_default_name`, `callee_name`, `constructor_name`, `type_name`, `referenced_type_name`, `reference_scope`, `is_top_level`, `method_kind`, `qualified_name`, `push_named`, and the existing `#[cfg(test)] mod tests` verbatim (zero logic edits) behind `impl LanguageAnalyzer for TypeScriptAnalyzer`; `TypeScriptAnalyzer { language: LanguageId }` const-instantiated as `TS`/`JS`.
- [x] 2.6 `extract.rs`: reduce `analyze` to `analyzer_for(language).analyze(source, relative_path)`; retain only `span_of`, `find_child`, `fingerprint_fields` (renamed/generalized from `signature_fingerprint`, taking `fields: &[&str]`), and `NO_SAME_FILE_MATCH_REASON`; assert no C#-specific and no TypeScript-specific node-kind string remains in `extract.rs`.
- [x] 2.7 `codekurve-analysis/src/ir.rs`: add `FileAnalysis.language: LanguageId`.
- [x] 2.8 `codekurve-analysis/src/resolve.rs`: reshape the `kind_matches` call site to take the edge's source `LanguageId`, look up `analyzer_for(lang)`, and call `analyzer.kind_matches(..)`; `TypeScriptAnalyzer::kind_matches` is today's `extract::kind_matches` body moved verbatim.
- [x] 2.9 Test: exhaustive `(RelationshipKind, SymbolKind)` sweep asserting `TypeScriptAnalyzer::kind_matches` answers are identical to the pre-refactor table.
- [x] 2.10 Test: `same_resolution_domain` table test — TS↔JS true, C#↔TS false, C#↔C# true.
- [x] 2.11 Test: same TypeScript source analyzed before/after the seam refactor produces an identical `FileAnalysis` (symbols, relationships, unresolved references).
- [x] 2.12 Run full `crates/codekurve-analysis/tests/*` and `crates/codekurve-bin/tests/*` golden suites (including `relationship_graph_fixture.rs`, `vertical_slice.rs`, `incremental_golden.rs`) with **zero edits**; byte-diff CLI stdout with/without `--json` against a pre-refactor baseline.

## Phase 3: PR3 — C# Analyzer Part 1: Symbols (req: csharp-analysis "C# Symbol Extraction Covers Types, Members, and Namespaces", "All Six C# Visibility Levels Round-Trip", "Generic Type Parameters and Constraints Are Recorded Structurally Only", "Partial Classes Are Flagged, Not Merged", "Namespace-Aware Qualified Names Stay Path-Prefixed"; symbol-index "is_partial and is_record Modifiers on Symbols" (record scenarios), ".cs Discovery and Default Language List"; design "Grammar Pin", "C# Node-Kind Mapping", "Visibility", "qualified_name", "Partial identity")

- [x] 3.1 First task: `cargo add tree-sitter-c-sharp@0.23 -p codekurve-analysis`; assert the lockfile shows `tree-sitter-language` (not a second `tree-sitter`) as its dependency.
- [x] 3.2 Second task: verify node-kind names against the pinned grammar's `node-types.json`, resolving the `record_declaration` vs `record_struct_declaration` open question before writing extraction code.
- [x] 3.3 `codekurve-analysis/src/languages/csharp.rs`: `CSharpAnalyzer` struct (unit), `impl LanguageAnalyzer for CSharpAnalyzer` skeleton with `language()` returning `LanguageId::CSharp`; wire `analyzer_for(LanguageId::CSharp)` to it (replacing PR2's stub).
- [x] 3.4 `csharp.rs`: `CsCtx` (`namespace_stack: Vec<String>`, `type_stack: Vec<String>`, `partial_ordinals: HashMap<String, u32>`) and its own `collect`/`push_named` — not shared with `CollectCtx`.
- [x] 3.5 `csharp.rs`: symbol extraction for `namespace_declaration`/`file_scoped_namespace_declaration` (`Namespace`, dotted name), `class_declaration`/`interface_declaration`/`struct_declaration` (visibility, `is_partial`), `record_declaration` (→ `Class`+`is_record` or `Struct`+`is_record`), `enum_declaration` + `enum_member_declaration` (→ `Field`, parent = enum), `constructor_declaration`, `method_declaration`, `property_declaration`, `field_declaration` (one `Field` per `variable_declarator`), nested types — per the node-kind mapping table.
- [x] 3.6 `csharp.rs`: `Contains` edges from enclosing scope to each declaration (namespace→type, type→member, type→nested type); `discovery.rs` comment updated for the no-longer-TS/JS-only extension filter.
- [x] 3.7 `csharp.rs`: `visibility_of(node)` — scans `modifier` children; compound levels (`protected internal`, `private protected`) checked before their single components.
- [x] 3.8 `csharp.rs`: `cs_qualified_name(relative_path, ns, types, name)` — `relative_path::Namespace.Type.member` dotted form, same two-component shape as TS.
- [x] 3.9 `csharp.rs`: `next_partial_ordinal(&mut CsCtx, qualified_name) -> u32`; `partial_ordinal = is_partial.then(|| next_partial_ordinal(..))`.
- [x] 3.10 `csharp.rs`: `cs_fingerprint(node, source)` = `fingerprint_fields(node, source, ["type_parameters", "parameters", "type"])` with each `type_parameter_constraints_clause` child's normalized text `\x1f`-appended; no edge emitted from generics. (Adapted: `type_parameters` is a named field only on `interface_declaration`/`method_declaration` in the pinned grammar — `class`/`struct`/`record` expose the same `type_parameter_list` as an unnamed child instead, and a method's return type field is `returns`, not `type`; `cs_fingerprint` reads both field name and structural fallback so the *outcome* — generics recorded, no edges — holds for every declaration kind, per design's "outcome column wins" note.)
- [x] 3.11 Test: C# visibility matrix — all six levels including both compounds, asserted distinct.
- [x] 3.12 Test: file-scoped vs block-scoped namespace; nested types; enum members index as `Field` with enum parent.
- [x] 3.13 Test: `record`/`record class` → `Class`+`is_record`; `record struct` → `Struct`+`is_record`.
- [x] 3.14 Test: generic class with `where` constraint — fingerprint contains type param name + constraint text, no edge created.
- [x] 3.15 Test: two `partial class` fragments in one file → distinct `partial_ordinal`/`symbol_key`; fragments across files each keep their own identity (ties into PR1's golden but exercised here with real C# input).
- [x] 3.16 Runtime harness: `codekurve index` + `search` over a scratch C# tree (full `fixtures/csharp/basic` fixture is PR6's job), eyeball symbol kinds and visibility.
- [x] 3.17 `cargo test -p codekurve-analysis csharp` green.

## Phase 4: PR4 — C# Analyzer Part 2: Relationships (req: csharp-analysis "using Directives Become Imports Edges", "Base List Entries Are Emitted as Pending References", "Calls and Object Creation Produce Calls and Constructs Edges", "Attributes Produce Decorates Edges Preserving Name and Span"; relationship-graph "Relationship Kind Extraction Is Per-Language" (Decorates scenario); design "C# Node-Kind Mapping" using/base_list/invocation/object_creation/attribute rows)

- [x] 4.1 `csharp.rs`: `collect_using` — `using_directive` → `Imports`, detecting `static`/`alias =` forms and `global` prefix (ignored); target `Unresolved(namespace text)`; `reason: None | Some("static") | Some("alias:<Name>")`.
- [x] 4.2 `csharp.rs`: `collect_bases` — `base_list` → one entry per base: `UsesType` + `Unresolved(name)` + `reason = BASE_LIST_REASON`, span = the entry's own span; never routed through `resolve_pending`.
- [x] 4.3 `csharp.rs`: `cs_callee_name` — `invocation_expression` → `Calls` (deferred via `PendingRel`, attributed to enclosing scope); callee resolution from `identifier`/`member_access_expression`/`generic_name`.
- [x] 4.4 `csharp.rs`: `created_type_name` — `object_creation_expression` → `Constructs` (deferred); `implicit_object_creation_expression` (target-typed `new()`) → `UnresolvedReference` with reason `"target-typed new() has no type name at the call site"`, never dropped or guessed.
- [x] 4.5 `csharp.rs`: `collect_attributes` — each `attribute` child of an `attribute_list` → `Decorates`, source = annotated declaration's key, target `Unresolved(<attribute name>)`, span = `span_of(attribute)` (the individual attribute, not the list); no attribute name special-cased.
- [x] 4.6 Test: plain `using X.Y;`/`using static X.Y;`/`using Alias = X.Y;` each produce an `Imports` edge with the correct `reason`.
- [x] 4.7 Test: `public class Invoice : BillingDocument, IBillable, IAuditable` emits three independent pending base-list references.
- [x] 4.8 Test: direct invocation → `Calls`; `new Foo()` → `Constructs`; `Foo invoice = new();` → unresolved `Constructs` with explicit reason.
- [x] 4.9 Test: `[Serializable] public class Widget {}` → `Decorates` edge, target text `Serializable`, span covers only the attribute text, not the class declaration; `[HttpGet]` recorded literally with no framework semantics inferred.
- [x] 4.10 Runtime harness: single-file C# cases through `analyze`, assert edge kinds/spans/name texts.
- [x] 4.11 `cargo test -p codekurve-analysis csharp` green.

## Phase 5: PR5 — Resolution (req: relationship-graph "Two-Pass Whole-Project Resolution Filters Candidates by Source Language", "Base List Class-vs-Interface Disambiguation at Resolve Time", "internal Visibility Is Recorded But Never Reduces Resolution Confidence"; csharp-analysis "Namespace-Aware Qualified Names Stay Path-Prefixed" (cross-file), "Unresolved References Are Preserved With Explicit Reasons, Never Dropped or Guessed", "Partial Classes Are Flagged, Not Merged" (ambiguity-set resolution scenario); design "Resolution Changes")

- [x] 5.1 `resolve.rs`: `ProjectSymbol`/`BaselineSymbol` gain `language: LanguageId`; `repo::resolution_snapshot` selects `s.language`; new `parse_language` mirroring `parse_symbol_kind`; `commands.rs` copies it into `BaselineSymbol`.
- [x] 5.2 `resolve.rs`: `resolve_by_name` filters candidates by `same_resolution_domain(lang, ps.language) && analyzer.kind_matches(rel.kind, ps.kind)` using the reference's source-file analyzer.
- [x] 5.3 `csharp.rs`: `impl LanguageAnalyzer::kind_matches for CSharpAnalyzer` — `Constructs` → `Class|Struct`; `Calls` → `Method|Constructor|Function`; `Inherits` → `Class|Struct`; `Implements` → `Interface`; `UsesType|References` → `Class|Struct|Interface|Enum`; `Imports` → `Namespace`; `Exports` → `false`; `_` → `true`.
- [x] 5.4 `resolve.rs`: new branch in `resolve_one` for `UsesType` with `reason == Some(BASE_LIST_REASON)` → `resolve_base_entry`; candidates = same-domain symbols named `text` with kind ∈ `{Class, Struct, Interface}`; one candidate → rewrite edge kind by resolved kind (`Interface`→`Implements`, `Class|Struct`→`Inherits`) at `Resolved`/`High`; several → one `Low`/`Heuristic` edge per candidate classified by its own kind; zero → `UnresolvedReference` with explicit reason, no naming heuristic.
- [x] 5.5 `resolve.rs`: `resolve_using` — C# `Imports` branch, own function (not `resolve_module`); candidates = C# `Namespace` symbols named exactly the directive text; one → `Global`/`High`; several → one `Low` edge per candidate; zero → `External(text)`; `reason` (`static`/`alias:X`) rides through untouched.
- [x] 5.6 Test: cross-file base-list entry resolves to an in-project class as `Inherits`, to an in-project interface as `Implements`.
- [x] 5.7 Test: unresolved base-list entry (zero in-project candidates) → `unresolved_references` row with `relationship_kind = UsesType` and explicit reason, no guessed edge.
- [x] 5.8 Test: mixed-language project with same-name `Invoice` symbol in a `.ts` and a `.cs` file — no cross-language resolution either direction.
- [x] 5.9 Test: `internal`/`ProtectedInternal`/`PrivateProtected` C# symbols resolve at the same confidence tier as `public` ones for an otherwise-identical unambiguous call.
- [x] 5.10 Test: a reference to a partial type with two fragment candidates resolves as one Low-confidence edge per fragment, not a single guessed resolution.
- [x] 5.11 Test: cross-file `.ts` call resolution (existing TS behavior) still resolves regardless of parse order — no regression from the language filter.
- [x] 5.12 Runtime harness: multi-file C# fixture through extract+resolve; assert cross-file `Inherits`/`Implements`/`Calls`/`Constructs`/`Imports` and preserved unresolved rows.
- [x] 5.13 `cargo test -p codekurve-analysis resolve` green; still zero TS golden edits.

## Phase 6: PR6 — Fixtures, Goldens, Regression Guards (req: proposal "Backward Compatibility Guarantee for TypeScript" items 1–6; proposal "Test Fixtures Plan"; symbol-index partial-fragment scenarios; csharp-analysis full-slice scenarios)

- [x] 6.1 Create `crates/codekurve-analysis/tests/fixtures/csharp-graph/project/`: 2 namespaces across 3+ files, interface + implementation in different files, base class in a third file, cross-namespace `using`, calls and object creation across files, attributes, a partial class split across two files, a generic class with a `where` constraint.
- [x] 6.2 `crates/codekurve-analysis/tests/csharp_graph_fixture.rs`: extract + resolve over that project; assert per-`RelationshipKind` counts, named cross-file `Inherits`/`Implements`/`Calls`/`Constructs`/`Imports`/`Decorates` edges, and that unresolved BCL references are preserved as rows with reasons — same shape as `relationship_graph_fixture.rs`.
- [x] 6.3 Add `csharp-graph/*.cs` single-file cases in the same dir: visibility matrix (all six levels incl. both compounds), file-scoped vs block namespace, nested types, enum members, records vs `record struct`, target-typed `new()`.
- [x] 6.4 Create `fixtures/csharp/basic/` (repo-root CLI round-trip source tree).
- [x] 6.5 `crates/codekurve-bin/tests/vertical_slice_csharp.rs`: full CLI `init` → `index` → `search` → `symbol` → `callers`/`implementations`, asserting stdout — mirrors `vertical_slice.rs`.
- [x] 6.6 Create `fixtures/mixed/` (TS and C# in one project sharing a type name) + `crates/codekurve-analysis/tests/mixed_language.rs`: asserts zero cross-language edges (Backward Compatibility item 5) and that both languages are indexed in one run.
- [x] 6.7 Create `partial_identity.rs` (`crates/codekurve-analysis/tests/` or store tests): two partial fragments of one type in one file get distinct keys; two fragments across files each keep their own; non-partial key equals the pre-migration golden hash from PR1.
- [x] 6.8 Regression guard (Backward Compatibility item 1): re-run `crates/codekurve-analysis/tests/relationship_graph_fixture.rs` and the whole `fixtures/ts-graph/` assertion set (RelationshipKind counts, cross-file edges, unresolved-reference preservation) — assert **zero edits** to expectations.
- [x] 6.9 Regression guard (Backward Compatibility item 2): re-run `crates/codekurve-bin/tests/*` CLI goldens (`vertical_slice.rs`, `incremental_golden.rs`) unchanged; TypeScript stdout byte-identical with and without `--json`.
- [x] 6.10 Regression guard (Backward Compatibility item 3): confirm the golden `symbol_key(lang, path, kind, qname, fingerprint, None)` hash test from PR1 still passes unmodified after C# lands.
- [x] 6.11 Regression guard (Backward Compatibility item 4): a TypeScript-only project indexed under migration 0004 produces the same symbol ids and the same relationship rows as before the migration — extend `incremental_golden.rs`'s digest query over the new columns' defaults.
- [x] 6.12 Regression guard (Backward Compatibility item 6): confirm no diff exists under `fixtures/ts-graph/` or to any TS golden expectation as part of this PR's diff.
- [x] 6.13 Runtime harness: full CLI round-trip on the C# fixture; confirm TS goldens pass with zero edits.
- [x] 6.14 `cargo test --workspace` green; `cargo fmt --check`; `cargo clippy -D warnings`; `unsafe_code = "forbid"` holds.

## Phase 7: PR7 — Docs (req: proposal Acceptance Criteria "Limitations are published in a user-visible document"; design "Known Limitations")

- [x] 7.1 Create `docs/LANGUAGES.md`: publish every row of the proposal's Known Limitations table (no semantic compilation, partial types not merged, no NuGet/BCL resolution, generics structural-only, extension methods, overload resolution, `using static`, `using alias`, `global using`, source generators, reflection/dynamic/DI, no solution/project model, no framework semantics, unindexed constructs, target-typed `new()`, TS decorators, preprocessor directives).
- [x] 7.2 `docs/LANGUAGES.md`: supported-language matrix (TypeScript/JavaScript/C#) with symbol/relationship-kind coverage per language.
- [x] 7.3 `README.md`: add/update language matrix section pointing to `docs/LANGUAGES.md`.
- [x] 7.4 Manual read-through of `docs/LANGUAGES.md` against the Known Limitations table — confirm every row appears verbatim or equivalently.
- [x] 7.5 Final full-chain regression: run `cargo test --workspace` one more time on the merged chain; confirm all C# and TS suites green.
