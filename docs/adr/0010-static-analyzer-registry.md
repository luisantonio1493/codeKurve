# 0010. Static analyzer registry, no dynamic plugin system

## Context

CodeKurve will support more than one language analyzer over time. See
CODEKURVE_MASTER_PLAN.md §19.1 ("do not create dynamic plugins yet; use a
static registry — `AnalyzerRegistry`") and the simplicity principle in
§5.10 (no generic plugin engine until at least two analyzers exist and
work).

## Decision

Language analyzers are registered in a static, compile-time
`AnalyzerRegistry` implementing the `LanguageAnalyzer` trait (§19). No
dynamic loading (shared libraries, WASM plugins, external process plugins)
is implemented in the MVP.

## Alternatives

- **Dynamic plugin system (dylib/WASM)**: adds a stable ABI/versioning
  surface, sandboxing, and loader complexity with zero current demand —
  explicitly deferred by §5.10 until at least two analyzers exist and a
  real need for third-party extensibility appears.
- **Config-driven analyzer selection with reflection-like dispatch**: no
  benefit over a static registry in a compiled language; adds indirection
  without solving a real problem.

## Consequences

- Adding a language means adding a crate/module implementing
  `LanguageAnalyzer` and registering it statically — a compile-time change,
  not a runtime plugin install.
- A future dynamic plugin system, if ever needed, requires its own ADR
  superseding this one, once a concrete third-party extensibility need
  exists.
- Implementation lands with `codekurve-analysis` (later phase); see
  `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
