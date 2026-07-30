# Design: Phase 5 — C#

## Technical Approach

`extract::analyze(source, language, relative_path) -> Result<FileAnalysis>` keeps its signature and becomes a five-line dispatcher: `analyzer_for(language).analyze(source, relative_path)`. Every caller (`commands.rs`, `incremental.rs`, `resolve.rs` tests) is untouched.

The seam is three files under `crates/codekurve-analysis/src/languages/`. `extract.rs` retains only the dispatcher plus helpers that are genuinely tree-shape-agnostic (`span_of`, `find_child`, the fingerprint field-joiner). Trait-dependent shared machinery (the deferred-edge type and its same-file resolver, the unresolved-edge pusher) lives in `languages/mod.rs`, because it must call `kind_matches` through the analyzer. Everything that names a node kind lives in `typescript.rs` or `csharp.rs` — that is the testable form of the acceptance criterion "`extract.rs` contains no C#-specific and no TypeScript-specific node-kind string".

`ir.rs`, the resolution algorithm, the store schema shape, and every query stay language-agnostic. `resolve.rs` changes in exactly four places: candidates carry a `LanguageId`, `kind_matches` dispatches through the source file's analyzer, `Imports` splits a C# branch, and base-list entries get a classification pass.

## Module Layout

```
crates/codekurve-analysis/src/
├── extract.rs              dispatcher + tree-agnostic helpers (no node-kind strings)
└── languages/
    ├── mod.rs              LanguageAnalyzer, analyzer_for, PendingRel, resolve_pending
    ├── typescript.rs       TypeScriptAnalyzer — today's body, verbatim
    └── csharp.rs           CSharpAnalyzer — new
```

`analyzer_for` returns `&'static dyn LanguageAnalyzer` from three consts, no allocation. `TypeScriptAnalyzer` is a one-field struct precisely so it can serve both TS and JS while `analyze` has no `language` parameter:

```rust
pub struct TypeScriptAnalyzer { language: LanguageId }   // grammar picked from self.language
pub struct CSharpAnalyzer;

const TS: TypeScriptAnalyzer = TypeScriptAnalyzer { language: LanguageId::TypeScript };
const JS: TypeScriptAnalyzer = TypeScriptAnalyzer { language: LanguageId::JavaScript };
const CS: CSharpAnalyzer = CSharpAnalyzer;

pub fn analyzer_for(language: LanguageId) -> &'static dyn LanguageAnalyzer {
    match language {
        LanguageId::TypeScript => &TS,
        LanguageId::JavaScript => &JS,
        LanguageId::CSharp => &CS,
    }
}
```

### Helper split

| Item (today in `extract.rs`) | Destination | Why |
|---|---|---|
| `analyze` | `extract.rs` (dispatch) | Public entry point, unchanged signature |
| `span_of`, `find_child` | `extract.rs`, `pub(crate)` | Pure tree/byte mechanics; no node-kind knowledge |
| `signature_fingerprint` | `extract.rs` as `fingerprint_fields(node, source, fields: &[&str])` | The *field names differ per language* (C# uses `type`, not `return_type`); the normalize-and-`\x1f`-join logic does not. TS passes `["type_parameters","parameters","return_type"]` → byte-identical output to today |
| `NO_SAME_FILE_MATCH_REASON` | `extract.rs` | Already a cross-module contract with `resolve.rs` |
| `PendingRel`, `resolve_pending`, `push_unresolved_edge` | `languages/mod.rs` | `resolve_pending` calls `kind_matches`, now a trait method → must take `analyzer: &dyn LanguageAnalyzer` |
| `CollectCtx`, `collect`, `collect_heritage`, `collect_imports`, `collect_exports`, `module_specifier`, `is_default_export`, `export_default_name`, `callee_name`, `constructor_name`, `type_name`, `referenced_type_name`, `reference_scope`, `is_top_level`, `method_kind`, `qualified_name`, `push_named`, `#[cfg(test)] mod tests` | `typescript.rs`, private | All name TS node kinds or TS-only concepts (`export`, `class_heritage`). Moved with **zero logic edits** |
| new: `CsCtx`, `collect`, `cs_qualified_name`, `visibility_of`, `has_modifier`, `collect_bases`, `collect_using`, `collect_attributes`, `cs_callee_name`, `created_type_name`, `field_declarators`, `cs_fingerprint`, `next_partial_ordinal` | `csharp.rs`, private | C#-only |

`CollectCtx` is **not** shared. `CsCtx` needs three fields TS has no use for (`namespace_stack: Vec<String>`, `type_stack: Vec<String>`, `partial_ordinals: HashMap<String, u32>`) and no `language` field (always `CSharp`). `push_named` is likewise **not** shared: the C# constructor derives a namespace-prefixed qualified name, reads visibility/`partial`/`record`, and handles declarations with no `name` field at all (`field_declaration`). Two ~30-line constructors beat one 10-parameter shared one; keeping them separate is also what guarantees the TS symbol bytes cannot shift.

## Architecture Decisions

| Question | Choice | Rejected | Rationale |
|---|---|---|---|
| Cross-language candidate filter | `same_resolution_domain(a, b)` free predicate in `languages/mod.rs`: `(TypeScript\|JavaScript, TypeScript\|JavaScript) \| (CSharp, CSharp)` | `a == b` on `LanguageId` | **`a == b` is a TypeScript regression**: today a `.ts` file resolves calls into `.js` symbols and vice versa. Grouping by resolution domain, not by id, is what keeps the existing goldens passing while still giving zero TS↔C# edges |
| Trait width | 3 methods, `&'static dyn` | 4th method for the resolution domain | The predicate is a property of the pair, not of one analyzer; a trait method would need a comparable group token — more surface for the same answer |
| Base-list edges | Always emitted as `Unresolved(name)` + `kind = UsesType` + `reason = BASE_LIST_REASON`, even for same-file bases; classified only in `resolve.rs` | Route them through `resolve_pending` like TS heritage | `resolve_pending` would bind a same-file base to `Local(key)` with kind `UsesType` and `resolve_one` passes non-`Unresolved` targets straight through — the entry would never be reclassified. One uniform path; cost is that a same-file base resolves at `High`/`Resolved` rather than `Exact`/`Extracted`. Honest, and identical to how a cross-file base already reads |
| C# `Imports` | Own branch in `resolve_one`; namespace-symbol lookup, never `resolve_module` | Reuse the TS module resolver | `resolve_module` implements relative paths, implicit extensions, `index.*` and tsconfig aliases — none of which exist in C# |
| Namespace symbol `name` | The namespace as written, fully dotted (`Acme.Billing`), for nested declarations too | Last segment only (`Billing`) | `using Acme.Billing` resolves through `SymbolTable::by_name`, which keys on `name`. The dotted form is what makes the lookup a plain hit instead of a new addressing dimension |
| New symbol fields in CLI output | `symbol`'s human output prints `visibility:`/`modifiers:` lines **only when non-default**; `--json` omits the keys when default | Always print them | Acceptance requires visibility to reach query output; Backward Compatibility forbids editing TS goldens. TS symbols are always `default`/`false`/`false`, so both hold with no special-casing beyond "omit when absent" — the same convention `target_external: null` already follows |
| `Defines` edges | Not emitted; `Contains` carries every nesting relation (namespace→type, type→member, type→nested type) | Emit `Defines` for declarations | TS emits none today and no query reads it; adding a C#-only edge kind usage would make graph shape language-dependent for no consumer |
| `partial_ordinal` storage | Field on `ExtractedSymbol`/`Symbol`, **no** `symbols` column | Persist a column | Key input only, exactly like `signature_fingerprint` in Phase 3; nothing queries it |
| `internal` as a resolution boundary | Never. Recorded as a `Visibility` value; confidence unchanged | Reduce confidence for internal targets | There is no `.sln`/`.csproj` model to say *which* assembly a file belongs to, so "internal" cannot be scoped to anything. A confidence penalty would encode a boundary the indexer cannot see: every cross-file reference to an `internal` type in a real single-project solution is legitimate and would be defamed. A guess dressed as a signal is worse than a documented gap (§ Known Limitations) |

## Grammar Pin

Workspace resolves `tree-sitter 0.25.10` and `tree-sitter-typescript 0.23.2`, whose only deps are `cc` + `tree-sitter-language 0.1.7` — i.e. the post-0.23 grammar convention that exports `LANGUAGE: LanguageFn` and depends on `tree-sitter-language`, not on `tree-sitter` itself. **Pin `tree-sitter-c-sharp = "0.23"`** (latest in that line, `0.23.1`), which follows the same convention and is therefore compatible with the existing core without touching it.

First task of PR3, before any extraction work: `cargo add tree-sitter-c-sharp@0.23 -p codekurve-analysis`, then assert the lockfile shows `tree-sitter-language` (not a second `tree-sitter`) as its dependency and that `parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into())` compiles. If it pulls a conflicting core, the fallback is aligning the core version — decided there, not mid-extraction.

## C# Node-Kind Mapping

Verify names against the pinned grammar's `node-types.json` as PR3's second task; this table is authoritative on the **outcome**, and where a grammar revision renames a node (`record_struct_declaration` vs a `struct` token inside `record_declaration`) the outcome column wins.

| Node kind | Symbol | Modifiers / fields read | Edges emitted |
|---|---|---|---|
| `compilation_unit` | — | — | recursion root; `source_local_key` for file-level edges is `relative_path` (same as TS) |
| `namespace_declaration` | `Namespace`, name = dotted `name` text | — | `Contains` → each child declaration; pushes `namespace_stack` |
| `file_scoped_namespace_declaration` | `Namespace`, same | — | same; applies to every following declaration in the file |
| `class_declaration` | `Class` | `partial` → `is_partial`; access modifiers → `visibility` | `Contains` from enclosing scope; `base_list`; `attribute_list` |
| `interface_declaration` | `Interface` | same | same |
| `struct_declaration` | `Struct` | same | same |
| `record_declaration` | `Class` + `is_record`; `Struct` + `is_record` when a `struct` keyword child is present | same | same |
| `enum_declaration` | `Enum` | same | `Contains` → members; `attribute_list` |
| `enum_member_declaration` | `Field`, parent = the enum | — | `Contains` (from enum); `attribute_list` |
| `constructor_declaration` | `Constructor`, name = declared identifier (the type name) | visibility | `Contains`; `attribute_list`; body walked with `scope = ctor key` |
| `method_declaration` | `Method` | visibility; `partial` → `is_partial` | `Contains`; `attribute_list`; body walked with `scope = method key` |
| `property_declaration` | `Property` | visibility | `Contains`; `attribute_list`; accessor bodies walked with `scope = property key` (accessors are not symbols) |
| `field_declaration` | one `Field` per `variable_declarator` inside its `variable_declaration` | visibility applies to all declarators | `Contains`; `attribute_list` (attached to every declarator) |
| `using_directive` | — | `static` / `alias` forms detected; `global` prefix ignored | `Imports`, source = `relative_path`, target `Unresolved(namespace text)`, `reason`: `None` \| `Some("static")` \| `Some("alias:<Name>")` |
| `base_list` | — | — | one entry per base: `UsesType` + `Unresolved(name)` + `reason = BASE_LIST_REASON`, span = the entry's own span |
| `invocation_expression` | — | callee from `function`: `identifier` → text, `member_access_expression` → `name` text, `generic_name` → `name` text, else skip | `Calls` (deferred via `PendingRel`, attributed to enclosing `scope`) |
| `object_creation_expression` | — | `type`: `identifier`/`qualified_name` → last segment, `generic_name` → `name` | `Constructs` (deferred) |
| `implicit_object_creation_expression` (`new()`) | — | no type name available | `UnresolvedReference` with reason `"target-typed new() has no type name at the call site"` — never dropped, never guessed |
| `attribute_list` → each `attribute` child | — | attribute name text as written | `Decorates`: source = the annotated declaration's key, target `Unresolved(<attribute name>)`, **span = `span_of(attribute)`** (the individual attribute, not the list), no name is special-cased |
| `type_parameter_list`, `type_parameter_constraints_clause` | — | text appended to `signature_fingerprint` | **none** — no `UsesType`, no edge of any kind (finalized) |
| everything else (operators, indexers, events, delegates, destructors, local functions, lambdas, top-level statements) | not indexed | — | — (published limitation) |

`cs_fingerprint(node, source)` = `fingerprint_fields(node, source, ["type_parameters", "parameters", "type"])`, with every `type_parameter_constraints_clause` child's normalized text `\x1f`-appended. That is the only place generic constraints appear in the output.

### Visibility

`visibility_of(node)` scans the declaration's `modifier` children: `public` → `Public`; `protected` + `internal` → `ProtectedInternal`; `private` + `protected` → `PrivateProtected`; `protected` → `Protected`; `internal` → `Internal`; `private` → `Private`; none → `Default`. Compound levels are checked **before** their single components, or `protected internal` collapses to `Protected`. `Default` means "no modifier written" — it is not C#'s implicit default (which differs by container), and the store never invents one.

### `qualified_name`

```rust
fn cs_qualified_name(relative_path: &str, ns: &[String], types: &[String], name: &str) -> String {
    let mut segs: Vec<&str> = ns.iter().map(String::as_str).collect();
    segs.extend(types.iter().map(String::as_str));
    segs.push(name);
    format!("{relative_path}::{}", segs.join("."))
}
```

→ `src/Billing/Invoice.cs::Acme.Billing.Invoice.Total`. Same two-component shape as TS (`path::dotted-name`), so `EdgeTarget::Global { file, qualified_name }`, `local_key == qualified_name`, `idx_symbols_project_qname`, and the composition root's Global→id lookup all work unchanged. The namespace is a prefix inside the second component, never a third addressing dimension. `ExtractedSymbol.parent` = the immediately enclosing declaration's `name` (a type name, or the namespace for a top-level type).

### Partial identity

`next_partial_ordinal(&mut CsCtx, qualified_name) -> u32` returns a per-file, per-qualified-name counter. `partial_ordinal = is_partial.then(|| next_partial_ordinal(..))`, so two `partial class Invoice` fragments in one file get ordinals 0 and 1 and therefore distinct `symbol_key`s under `UNIQUE(project_id, symbol_key)`. Fragments in *different* files already differ through the `relative_path` component. No merging, no canonical symbol.

## Resolution Changes

```rust
// ir.rs
pub struct FileAnalysis { pub language: LanguageId, /* ...unchanged... */ }

// resolve.rs
pub(crate) struct ProjectSymbol { pub file: String, pub qualified_name: String,
                                 pub kind: SymbolKind, pub language: LanguageId }
pub struct BaselineSymbol { /* ...unchanged... */ pub language: LanguageId }
```

1. **`kind_matches` dispatch.** `resolve_by_name` takes the source file's `LanguageId`, gets `analyzer_for(lang)`, and filters `same_resolution_domain(lang, ps.language) && analyzer.kind_matches(rel.kind, ps.kind)`. `TypeScriptAnalyzer::kind_matches` is today's `extract::kind_matches` body moved verbatim — that identity is the no-regression guarantee. `CSharpAnalyzer::kind_matches`: `Constructs` → `Class|Struct`; `Calls` → `Method|Constructor|Function`; `Inherits` → `Class|Struct`; `Implements` → `Interface`; `UsesType`/`References` → `Class|Struct|Interface|Enum`; `Imports` → `Namespace`; `Exports` → `false` (C# emits none); `_` → `true`.
2. **Base-list classification.** New branch in `resolve_one`: `UsesType` with `reason == Some(BASE_LIST_REASON)` → `resolve_base_entry`. Candidates = same-domain symbols named `text` with kind ∈ `{Class, Struct, Interface}`. One candidate → rewrite the edge kind by the *resolved* kind (`Interface` → `Implements`, `Class|Struct` → `Inherits`) and push `Global`/`Resolved`/`High`. Several → one `Low`/`Heuristic` edge per candidate, each classified by its own kind. Zero → `UnresolvedReference { relationship_kind: UsesType, reason: "base list entry not found in project; class vs interface undeterminable" }`. No `I`-prefix heuristic anywhere.
3. **C# `using`.** `Imports` from a C# file → `resolve_using`: candidates = C# `Namespace` symbols named exactly the directive text. One → `Global`/`High`. Several (the namespace is declared in several files) → one `Low` edge per candidate. Zero → `External(text)` — the same policy `node_modules` imports already follow, and the reason it is not an `UnresolvedReference`: a BCL/NuGet namespace genuinely *is* outside the project, not missing from it. `reason` (`static`/`alias:X`) rides through untouched.
4. **Snapshot.** `repo::resolution_snapshot` selects `s.language`; a `parse_language` reverse of `LanguageId::as_str` mirrors the existing `parse_symbol_kind`. `commands.rs` copies it into `BaselineSymbol`.

## Data Flow

```
discovery (.ts .js .cs)
      │
      ▼
extract::analyze(source, language, path)
      │  analyzer_for(language) ──▶ TypeScriptAnalyzer │ CSharpAnalyzer
      │                                    │                  │
      │                            (TS node kinds)    (C# node kinds)
      │                                    └────┬─────────────┘
      │                    languages::resolve_pending(analyzer, …)   same-file binding
      ▼
FileAnalysis { language, symbols(+visibility,is_partial,is_record,partial_ordinal),
               relationships, unresolved }
      │
      ▼
resolve::resolve_with(files, aliases, baseline)
      │  same_resolution_domain + analyzer.kind_matches   (zero TS↔C# edges)
      │  base-list → Inherits | Implements | UnresolvedReference
      │  C# Imports → namespace symbol | External
      ▼
repo::reindex → symbol_key(lang, path, kind, qname, fingerprint, partial_ordinal)
                symbols(+visibility, is_partial, is_record)      [migration 0004]
```

## Migration 0004

```sql
ALTER TABLE symbols ADD COLUMN visibility TEXT NOT NULL DEFAULT 'default';
ALTER TABLE symbols ADD COLUMN is_partial INTEGER NOT NULL DEFAULT 0;
ALTER TABLE symbols ADD COLUMN is_record INTEGER NOT NULL DEFAULT 0;
```

Three `ADD COLUMN`s with non-null defaults (SQLite's requirement for `NOT NULL`), each O(1) with no table rewrite. `SCHEMA_VERSION = 4` and a new `if current < 4 { … }` block in the same numbered style as 0001–0003. **No `DELETE`** — unlike 0003, none of the three columns feeds `symbol_key`, so no stored id changes and no reindex is forced. Pre-0004 rows read `default`/`0`/`0` until the next reindex.

### `repo.rs`

```rust
pub fn symbol_key(
    language: &str, relative_path: &str, kind: &str,
    qualified_name: &str, signature_fingerprint: &str,
    partial_ordinal: Option<u32>,
) -> String {
    let mut input = format!(
        "{language}\u{1f}{relative_path}\u{1f}{kind}\u{1f}{qualified_name}\u{1f}{signature_fingerprint}"
    );
    if let Some(ordinal) = partial_ordinal {
        input.push('\u{1f}');
        input.push_str(&ordinal.to_string());
    }
    blake3::hash(input.as_bytes()).to_hex().to_string()
}
```

`None` leaves the hashed input byte-for-byte identical to today's five-component `format!`, so every existing key and `symbol_id` survives. Pinned by a golden test asserting a hardcoded hex for `("typescript", "src/member.ts", "class", "src/member.ts::MemberService", "", None)`, captured before the signature change.

| Site | Change |
|---|---|
| `Symbol` (`codekurve-core/src/symbol.rs`) | `+ visibility: Visibility`, `+ is_partial: bool`, `+ is_record: bool`, `+ partial_ordinal: Option<u32>` |
| `ExtractedSymbol` (`ir.rs`) | same four fields; TS sets `Default`/`false`/`false`/`None` |
| `reindex`'s symbol `INSERT` | `+ visibility, is_partial, is_record` columns and params; `symbol_key(.., symbol.partial_ordinal)` |
| `StoredSymbol` | `+ visibility: String`, `+ is_partial: bool`, `+ is_record: bool` |
| `search_symbols`, `find_by_name`, `find_symbol_by_id` SELECTs | `+ s.visibility, s.is_partial, s.is_record`; `map_stored` reads indices 12–14 |
| `resolution_snapshot` | `+ s.language`; new `parse_language` |
| `commands.rs::symbol` | prints `visibility:` / `modifiers:` lines only when non-default; `--json` omits the keys when default. Every other `serde_json::json!` output shape is untouched |
| `Visibility` (`codekurve-core/src/symbol.rs`) | new enum + `as_str` (`public`,`protected`,`internal`,`private`,`protectedinternal`,`privateprotected`,`default`) + `parse_visibility` in `repo.rs` |
| `RelationshipKind` | `+ Decorates` → `"decorates"` |
| `LanguageId` | `+ CSharp`; `"cs"` extension; `"csharp"` name; `"csharp"` in `config.rs`'s default `index.languages` |

## Interfaces

```rust
pub trait LanguageAnalyzer {
    fn language(&self) -> LanguageId;
    fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis>;
    fn kind_matches(&self, rel: RelationshipKind, sym: SymbolKind) -> bool;
}

pub fn analyzer_for(language: LanguageId) -> &'static dyn LanguageAnalyzer;

/// TS↔JS still resolve together (existing behavior); C# is its own domain.
pub fn same_resolution_domain(a: LanguageId, b: LanguageId) -> bool;

pub(crate) const BASE_LIST_REASON: &str = "c# base list entry";

pub enum Visibility { Public, Protected, Internal, Private, ProtectedInternal, PrivateProtected, Default }
```

## PR Chain

Same seven units as the proposal, with the implementation-order dependencies made explicit. Each PR bases on the previous branch (feature-branch chain); PR1 bases on the tracker, PR7 merges to it.

| PR | Content | Hard dependency | Must be true when it lands |
|---|---|---|---|
| 1 | `LanguageId::CSharp`, `Visibility`, `is_partial`, `is_record`, `RelationshipKind::Decorates`, IR + `Symbol` fields, migration 0004, `symbol_key`'s 6th parameter, `parse_visibility`/`parse_language` | — | Zero behavior change. Golden `symbol_key(.., None)` hash test present and passing. 0004 applies to a populated pre-0004 DB without wiping it |
| 2 | `LanguageAnalyzer`, `analyzer_for`, `same_resolution_domain`, TS body moved to `typescript.rs`, `extract::analyze` reduced to dispatch, `kind_matches` per-language, `resolve.rs` call-site reshaped, `FileAnalysis.language` | PR1 (`LanguageId::CSharp` must exist for the `match` to be exhaustive; the new IR fields must exist for the moved constructor to compile) | Every TS fixture and CLI golden passes **unedited**. `TypeScriptAnalyzer::kind_matches` is byte-identical to the old function. `same_resolution_domain` keeps TS↔JS resolution alive |
| 3 | Grammar pin, `.cs` discovery, `csharp.rs` symbol extraction (namespace/class/interface/struct/record/enum/ctor/method/property/field/nested), visibility, `is_partial`, `partial_ordinal`, fingerprint + constraints, `Contains` | PR2 (trait must exist) | Grammar compatibility verified against the lockfile *before* extraction work. `cargo test -p codekurve-analysis csharp` green; symbols and visibility correct on `fixtures/csharp/basic` |
| 4 | `using` → `Imports`; `base_list` → `UsesType`+`BASE_LIST_REASON`; `invocation_expression` → `Calls`; `object_creation_expression` → `Constructs`; `implicit_object_creation_expression` → unresolved row; `attribute_list` → `Decorates` | PR3 (needs the symbols the edges attach to) | Edge kinds, name texts and spans asserted on single-file cases. Attribute spans are the attribute's own, not the list's |
| 5 | Language-filtered candidates, per-language `kind_matches` applied to real C# input, base-list classification, `resolve_using`, unresolved rows with reasons, `resolution_snapshot` language column | PR4 (the edges must exist to be resolved) | Cross-file `Inherits`/`Implements`/`Calls`/`Constructs` resolve; unresolved rows preserved with reasons; still zero TS golden edits |
| 6 | `csharp-graph` fixture + `csharp_graph_fixture.rs`, `fixtures/csharp/basic`, `vertical_slice_csharp.rs`, mixed TS+C# fixture + `mixed_language.rs`, `partial_identity.rs`, full TS regression re-run | PR5 | Zero cross-language edges asserted. `cargo test --workspace` green. No file under `fixtures/ts-graph/` modified |
| 7 | `docs/LANGUAGES.md` (published limitations), README language matrix | PR6 | Every row of the proposal's Known Limitations table appears in a user-visible document |

PR2 is the largest diff with the least new behavior, kept separate so a TypeScript regression bisects to a move. The PR1→PR2 dependency is compile-level, not cosmetic: PR2 cannot compile without PR1's enum variant and IR fields.

## Testing Strategy

| Layer | What | Approach |
|---|---|---|
| Unit | `symbol_key(.., None)` equals a hardcoded pre-migration hash; `Some(0) != Some(1) != None` | Extend `repo.rs`'s `symbol_key_*` tests |
| Unit | Migration 0004 on a populated v3 DB: row count and every `symbol_key`/`id` unchanged, three columns present with defaults | Extend `migrations.rs` tests; rename `fresh_database_reaches_schema_version_3` → `_4` |
| Unit | `TypeScriptAnalyzer::kind_matches` answers identically to the pre-refactor table | Exhaustive `(RelationshipKind, SymbolKind)` sweep asserted against a hardcoded expectation matrix |
| Unit | `same_resolution_domain`: TS↔JS true, C#↔TS false, C#↔C# true | Table test |
| Unit | C# visibility matrix (all six levels, both compounds), file-scoped vs block namespace, nested types, enum members, record vs `record struct`, target-typed `new()` | Single-source `analyze` cases in `csharp.rs`'s test module |
| Unit | Attribute `Decorates` name text and span; `where` constraints appear in the fingerprint and emit no edge | Single-source cases |
| Unit | Two `partial class` fragments in one file → distinct keys; one file each → distinct keys; non-partial matches the golden | `partial_identity.rs` |
| Integration | Multi-file C# project: per-kind edge counts, named cross-file `Inherits`/`Implements`/`Calls`/`Constructs`/`Imports`/`Decorates`, preserved unresolved rows with reasons | `csharp_graph_fixture.rs`, mirroring `relationship_graph_fixture.rs` |
| Integration | Mixed TS+C# project sharing a type name → zero cross-language edges, both languages indexed in one run | `mixed_language.rs` |
| Regression | Existing TS fixtures and CLI goldens pass with **zero edits** after PR2 and again after PR6 | Re-run unchanged; any diff under `fixtures/ts-graph/` is a review blocker |
| E2E | `init` → `index` → `search` → `symbol` → `callers`/`implementations` on a C# tree | `vertical_slice_csharp.rs`, mirroring `vertical_slice.rs` |

## Threat Matrix

N/A — no routing, shell, subprocess, VCS/PR automation, executable-file classification, or process-integration boundary. The new grammar is in-process (`tree-sitter-c-sharp`, no Roslyn, no MSBuild, no network). The only new filesystem surface is the `.cs` extension reaching `discovery::discover`, which reuses the existing ignore/UTF-8/size rules unchanged.

## Migration / Rollout

Additive and reindex-free. Opening a pre-0004 DB applies the three `ADD COLUMN`s in one transaction; existing rows keep every id and answer queries as before, reporting `default`/`false`/`false` until the next `codekurve index`. Rollback: revert the chain and set `SCHEMA_VERSION` back to 3 — the three columns are defaulted and unread by the prior build, so they can be left in place. C# rows vanish with the revert because nothing discovers `.cs` without `LanguageId::CSharp`.

## Open Questions

- [ ] PR3's unit description mentions `Contains`/`Defines`; this design emits `Contains` only (TS emits no `Defines` and no query reads it). Confirm the spec does not require `Defines`, or add it in PR3 for both languages rather than C# alone.
- [ ] `record_declaration` vs a separate `record_struct_declaration` node depends on the pinned grammar revision — resolved by reading `node-types.json` as PR3's second task, not by guessing here.
- [ ] Same-file C# base entries resolve at `High`/`Resolved` rather than `Exact`/`Extracted` (the uniform base-list path). If the C# fixture is asserted to `Exact` for same-file bases, the classification pass would have to move into `csharp.rs` too — flag before writing PR6's expectations.
