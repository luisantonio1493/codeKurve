# 0009. TypeScript/JavaScript is the first supported language

## Context

The MVP must ship end-to-end value (init → index → query) for one language
before adding a second. See CODEKURVE_MASTER_PLAN.md §9 (MVP user stories:
US-002 indexing, framed around TS from the start) and §20 (TypeScript and
JavaScript syntactic scope, module resolution, qualified names, call
confidence levels).

## Decision

TypeScript and JavaScript (including `.tsx`/`.jsx`) are the first language
analyzer implemented. C# (§21) is the second, deliberately sequenced after
TS/JS is working end to end (§60 build order).

## Alternatives

- **C# first**: rejected — the master plan's build order (§60) and MVP
  stories (§9) are anchored on TypeScript; switching order provides no
  architectural benefit and delays validating the analyzer trait (§19)
  against a real, widely-used language.
- **Multiple languages in parallel from the start**: rejected by §5.10
  (simplicity before abstraction) — the analyzer registry (ADR 0010) is
  proven with one working analyzer before a second is added.

## Consequences

- `codekurve-analysis` implements the TypeScript/JavaScript analyzer first;
  `node_modules` is never indexed (§20.2).
- The `LanguageAnalyzer` trait and `AnalyzerRegistry` (ADR 0010) are
  validated against TS/JS before C# is added, reducing the risk of a
  TS-specific abstraction leaking into the trait contract.
- Implementation lands in a later phase; see `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
