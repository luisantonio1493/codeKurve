# 0003. Tree-sitter for source parsing

## Context

CodeKurve needs incremental, error-tolerant, multi-language parsing to
extract symbols and relationships without depending on each language's own
compiler toolchain. See CODEKURVE_MASTER_PLAN.md §19 (analyzer architecture,
`LanguageAnalyzer` trait) and §20 (TypeScript/JavaScript syntactic scope).

## Decision

Use Tree-sitter grammars as the parsing layer feeding the per-language
`LanguageAnalyzer` implementations.

## Alternatives

- **Language-native compiler/LSP services (tsc, Roslyn, etc.)**: higher
  fidelity resolution but require running a separate language runtime/
  toolchain per language, is heavier and slower for a "no code execution"
  local tool, and does not scale to adding new languages cheaply.
- **Hand-written regex/heuristic parsers**: fast to start, but fragile,
  no real AST, no error recovery, unsustainable across multiple languages.
- **Full custom parser per language**: highest control, prohibitive cost
  for an MVP that must support TypeScript/JavaScript and C#.

## Consequences

- Adding a language means adding a Tree-sitter grammar dependency plus an
  analyzer implementation — not a new parser from scratch.
- Extraction accuracy is bounded by grammar coverage; anything beyond
  syntax (e.g. cross-file resolution) is handled by the resolution phase
  (§19), not the parser.
- Implementation (grammar integration, `codekurve-analysis` crate content)
  lands in a later phase; see `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
