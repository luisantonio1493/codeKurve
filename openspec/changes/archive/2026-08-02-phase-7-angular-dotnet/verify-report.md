# Verification Report — phase-7-angular-dotnet

**Mode**: full artifacts (proposal/design/tasks/specs all present + apply-progress)
**Verdict**: PASS WITH WARNINGS

## Completeness

- tasks.md: 158/158 checkboxes `[x]`, 0 unchecked. All 8 PRs marked complete, matches apply-progress.

## Test/build evidence

- `cargo test --workspace`: all suites green, 0 failures (codekurve-analysis frameworks tests, resolve.rs D5 floor tests, repo.rs migration 0005 tests, angular_graph/dotnet_graph e2e, MCP tools, etc.)
- `cargo clippy --all-targets --workspace`: 0 errors, 2 pre-existing style warnings (needless_lifetimes in angular.rs test helper, manual_contains in commands.rs test) — non-blocking style nits, not phase-7 regressions of substance. (Both were fixed in a later commit as part of getting CI green — see git history.)
- `cargo fmt --check`: fails (exit 1) on `crates/codekurve/src/commands.rs` and `crates/codekurve-analysis/src/languages/csharp.rs` — confirmed via `git stash` that the same diff locations exist without phase-7's changes at all (pre-existing repo-wide fmt drift, not introduced by this change). (Also fixed in a later commit.)

## Spec compliance matrix (framework-awareness, relationship-graph, symbol-index)

- Req "Angular Recognition Covers Components, DI, Routes, Guards, Interceptors, Standalone Imports" — SATISFIED, covered by frameworks/angular.rs + tests.
- Req ".NET Recognition Covers Controllers, Minimal APIs, DI, Azure Functions, Middleware, EF Core" — SATISFIED, frameworks/dotnet.rs + tests (route join, Add*/Use*, DbSet<T> exception, negative List<T>/Task<T>/IQueryable<T> tests).
- Req "Every Framework Edge Carries Heuristic Provenance and Non-Exact Confidence" — SATISFIED. `resolve.rs::resolve_framework_edge`/`push_framework_edge` implement the D5 floor exactly: provenance carried verbatim from `rel.provenance` (never upgraded), confidence = `min_confidence(rel.confidence, resolution_confidence)`. Unit test `provenance_floor_never_upgrades_a_heuristic_framework_edge` and the integration assertion "no framework kind ever Extracted/Resolved" both pass.
- Req "An End-to-End Route-to-Data-Layer Path Is Traversable" — SATISFIED via angular_graph.rs/dotnet_graph.rs e2e fixtures, no MCP-layer change.
- Req "Framework Catalogue Coverage Is Bounded and Published" — SATISFIED, docs/FRAMEWORKS.md published-limitations table matches design.md's "no edge" rules and proposal's out-of-scope list verbatim-equivalent.
- relationship-graph MODIFIED "Relationship Kind Extraction Is Per-Language" (TS decorator walking → decorates) — SATISFIED, PR2.
- relationship-graph ADDED "Framework-Level Relationship Kinds ... Emitted Only By Recognition Pass" — SATISFIED. `RelationshipKind` gains exactly `Injects/RegisteredAs/HandlesRoute/Triggers/PersistsTo`; `Publishes`/`Subscribes` correctly absent (deliberate omission, documented).
- relationship-graph ADDED "TypeScript Kind Matching Extends to Decorates" — SATISFIED.
- symbol-index ADDED "Framework-Role Tags ... Without a New SymbolKind" — SATISFIED. `FrameworkRole` enum (Controller/Route/Service/Repository/Component/Decorator) is a tag field on `Symbol`/`ExtractedSymbol`, not a `SymbolKind` variant; `roles` excluded from `symbol_key`.
- symbol-index ADDED "Schema Migration 0005" — SATISFIED, `SCHEMA_VERSION = 5`.
- symbol-index ADDED "Role Tags Are Queryable" — SATISFIED, `codekurve symbol` prints `roles:` via `roles_suffix()`.

## CRITICAL — spec/design/implementation contradiction (process defect, not a functional bug) — RESOLVED before archive

`specs/framework-awareness/spec.md`'s requirement "Recognition Runs as a Separate Pass Downstream of Extraction and Resolution" and its scenario "Recognition pass reads extracted output, not source text" literally stated the pass "consum[es] their output ... rather than parsing source itself" and "without re-parsing the source file". This was factually false against the actual implementation: `design.md` explicitly overturned this proposal assumption (D1 — the recognition pass cannot run on edges alone; `cs_simple_type_name`/`cs_callee_name` drop type arguments/receivers, TS extracts no parameter symbols), and `frameworks/mod.rs::recognize` does its own marker-gated tree-sitter re-parse of the source text. `docs/FRAMEWORKS.md` already correctly stated "re-parses candidate files after extraction". So: design.md, code, and docs/FRAMEWORKS.md were all internally consistent and correct; only the delta spec.md text had never been synced back after the design phase's justified correction.

**Resolution**: `specs/framework-awareness/spec.md` was patched before archive to match D1 (recognition is marker-gated and re-parses source), replacing the stale requirement/scenario text with two accurate scenarios ("Recognition pass is marker-gated before it re-parses" / "Recognition pass re-parses only marker-matched files"). This is reflected in the merged main spec at `openspec/specs/framework-awareness/spec.md`.

## WARNING

- `cargo fmt --check` failed at verify time but was confirmed pre-existing (identical diffs present with phase-7's changes stashed out). Not a phase-7 regression. Fixed in a follow-up commit before this archive.
- 2 clippy warnings (non-error, pre-existing style patterns) in test helper code touched by phase-7 (angular.rs, commands.rs test additions) — cosmetic only. Fixed in a follow-up commit before this archive.

## Documented deviations reviewed — both legitimate

- PR5's `EdgeTarget::External` bypass of the by-name resolver for route targets: legitimate. `resolve_one`'s match arm only intercepts `EdgeTarget::Unresolved`; `External` targets fall through unchanged, preserving `rel.provenance`/`rel.confidence` verbatim from the recognition pass — the D5 floor is not bypassed, it never applied to non-Unresolved targets in the first place (routes are external-by-construction per D1/D2).
- PR7's narrowed zero-edge regression test scope (`empty_catalogues_produce_zero_framework_edges_on_every_fixture` explicitly excludes `fixtures/angular/`/`fixtures/dotnet/`): legitimate and documented in-code — those fixtures exist specifically to exercise the now-populated catalogues and are asserted non-empty by `angular_graph.rs`/`dotnet_graph.rs` instead.

## Framework-blindness grep check

Confirmed via direct grep of `languages/csharp.rs` and `languages/typescript.rs`: all matches of framework-specific strings (`Component`, `Injectable`, `HttpGet`, `DbSet`, `AddScoped`, etc.) appear only inside doc comments, `#[cfg(test)]` test fixtures, or the file's own static grep-check test — zero production/branching-logic occurrences. Independently enforced by `frameworks/mod.rs`'s own test `no_frameworks_marker_leaks_into_languages_module`, which strips comments/tests before asserting.
