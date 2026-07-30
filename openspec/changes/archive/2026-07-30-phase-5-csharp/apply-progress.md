# Apply Progress: Phase 5 — C#

## Status

- Artifact store: OpenSpec
- Mode: Standard (`strict_tdd: false`)
- Delivery: `exception-ok` with `feature-branch-chain`; PR5 resolution work unit, then PR6 fixture/regression work unit, then PR7 documentation work unit
- Completed: 84/84 tasks

## Cumulative Completed Tasks

- PR1: tasks 1.1–1.12 — C# domain/storage groundwork and migration 0004.
- PR2: tasks 2.1–2.12 — `LanguageAnalyzer` seam and behavior-preserving TypeScript move.
- PR3: tasks 3.1–3.17 — C# symbol extraction, visibility, records, partial identities, and structural generics.
- PR4: tasks 4.1–4.11 — C# relationship extraction for using directives, base lists, calls, construction, and attributes.
- PR5: tasks 5.1–5.13 — language-filtered whole-project resolution, C# base/using resolution, and focused regression/runtime coverage.
- PR6: tasks 6.1–6.14 — C# graph/CLI/mixed fixtures, C# runtime slice, partial identity, and complete compatibility guards.
- PR7: tasks 7.1–7.5 — published language coverage and C# limitations, README entry point, manual limitation readback, and final workspace regression.

## Delivery Exception

- Maintainer-approved: `size:exception`.
- PR5 authored change budget: 459 lines (59 above the 400-line threshold).
- Rationale: this is one cohesive, verified resolution unit; an artificial split would harm review coherence.

## PR5 Implementation

- Added source-language to project and baseline resolution symbols; storage snapshots now read `symbols.language` and map it through `parse_language`.
- Restricted name resolution to the source language's resolution domain and analyzer-specific symbol-kind rules.
- Added the C# kind-match table, base-list class/interface classification, and namespace `using` resolution with preserved directive reasons.
- Added focused resolver coverage for cross-file base lists, unresolved bases, mixed-language isolation, visibility confidence, partial ambiguity, TypeScript parse-order regression, and a multi-file C# extract+resolve harness.


## PR6 Implementation

- Added a multi-file C# graph fixture with cross-file inheritance, implementation, calls, construction, namespace imports, attributes, partial fragments, generic constraints, and an explicit unresolved BCL base.
- Added C# single-file fixture coverage for visibility, namespaces, nesting, records, enum members, and target-typed construction.
- Added C# CLI vertical-slice, mixed-language isolation, and partial-identity integration tests; extended the incremental snapshot to compare migration-0004 default columns.
- TypeScript fixture sources and relationship expectations were not edited. Existing TypeScript vertical, relationship, and incremental suites pass unchanged in behavior.

## PR6 Delivery Exception

- Maintainer-approved: PR6-specific `size:exception`.
- Native authoritative changed-line count: 415 authored lines, including authored OpenSpec bookkeeping.
- Overage: 15 lines above the 400-line threshold.
- Rationale: the C# fixtures and regression guards are one cohesive, fully verified work unit; an artificial split would harm review coherence.
- Scope isolation: this approval applies only to PR6. PR5's separate 459-line exception remains distinct and unchanged.

## Work Unit Evidence

| Evidence | Result |
|---|---|
| Focused test | `cargo test -p codekurve-analysis resolve` — passed: 22 tests, 0 failed |
| Runtime harness | `resolve::tests::csharp_multifile_runtime_resolution_preserves_unresolved_rows` — passed; extract+resolve asserted cross-file `Inherits`, `Implements`, `Calls`, `Constructs`, `Imports`, and preserved `Missing` unresolved row |
| Full regression | `cargo test --workspace` — passed: all workspace unit, integration, and doc tests |
| Formatting | `cargo fmt --all --check` — passed |
| TS golden guard | `git diff -- fixtures/ts-graph ':(glob)**/*golden*'` — no output / zero edits |
| Rollback boundary | Revert PR5 deltas in `crates/codekurve-analysis/src/resolve.rs`, `crates/codekurve-analysis/src/languages/csharp.rs`, `crates/codekurve-store/src/repo.rs`, and `crates/codekurve/src/commands.rs`; prior PR1–PR4 extraction/storage behavior remains intact |

| PR6 delivery exception | Maintainer-approved `size:exception`: 415 authored lines including OpenSpec bookkeeping (15 over); PR5 exception remains separately scoped |
| PR6 focused tests | `cargo test -p codekurve-analysis --test csharp_graph_fixture --test mixed_language --test partial_identity && cargo test -p codekurve-bin --test vertical_slice_csharp --test incremental_golden && cargo test -p codekurve-analysis --test relationship_graph_fixture && cargo test -p codekurve-bin --test vertical_slice` — passed: 10 integration tests, 0 failed |
| PR6 runtime harness | `vertical_slice_csharp_init_index_search_symbol_callers_and_implementations` — passed: real binary executed `init` → `index` (3 files) → `search` → `symbol` → `callers` → `implementations` |
| PR6 full gates | `cargo fmt --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace` — all passed (132 test cases; 0 failed) |
| PR6 TS guard | `relationship_graph_fixture`, `vertical_slice`, and `incremental_golden` passed; no PR6 edit under `crates/codekurve-analysis/tests/fixtures/ts-graph/` |
| PR6 unsafe guard | Workspace `Cargo.toml` retains `unsafe_code = "forbid"` |
| PR6 rollback boundary | Remove only PR6 fixture dirs and test files plus the incremental snapshot-column assertion and analysis test dev-dependency; PR1–PR5 behavior remains intact |


## PR7 Implementation

- Created `docs/LANGUAGES.md` as the canonical detailed language-coverage and C# limitations document.
- Added a concise README language section that links to the canonical document.
- Manually read back each published limitation against the accepted proposal and design table; all 17 rows are present with equivalent effects.

## PR7 Work Unit Evidence

| Evidence | Result |
|---|---|
| Documentation readback | `docs/LANGUAGES.md` and README link manually read after writing; coverage matrix lists TypeScript, JavaScript, and C# symbol/relationship surface |
| Limitation-table mapping | All 17 accepted proposal/design rows map one-to-one to the `C# Known Limitations` table below |
| Full-chain regression | `cargo test --workspace` — passed: 91 tests, 0 failed; 5 doctest binaries ran with 0 tests |
| Runtime harness | N/A — this PR changes documentation only and has no runtime boundary; prior PR6 CLI runtime evidence remains applicable to the cumulative chain |
| Rollback boundary | Revert `docs/LANGUAGES.md`, the README language section, and this PR7 OpenSpec bookkeeping only; PR1–PR6 source and fixtures remain intact |

### PR7 Limitation-Table Readback Mapping

| Accepted proposal/design row | Published row in `docs/LANGUAGES.md` |
|---|---|
| No semantic compilation | No semantic compilation (no Roslyn/MSBuild) |
| Partial types not merged | Partial types not merged |
| No NuGet / BCL resolution | No NuGet / BCL resolution |
| Generics are structural only | Generics are structural only |
| Extension methods | Extension methods |
| Overload resolution | Overload resolution |
| `using static` | `using static` |
| `using alias = X.Y` | `using alias = X.Y` |
| `global using` | `global using` |
| Source generators | Source generators |
| Reflection, `dynamic`, runtime DI | Reflection, `dynamic`, runtime DI |
| No solution/project model | No solution/project model |
| No framework semantics | No framework semantics |
| Not indexed as symbols | Not indexed as symbols |
| Target-typed `new()` | Target-typed `new()` |
| TS decorators | TS decorators |
| Preprocessor directives | Preprocessor directives |

## Remaining Tasks

None — 84/84 tasks complete.
