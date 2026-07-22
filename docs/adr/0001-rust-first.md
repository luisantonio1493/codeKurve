# 0001. Rust as the implementation language

## Context

CodeKurve is a local-first code intelligence tool: parse a repository, persist
an index, and answer structural queries fast and safely. The runtime must
handle CPU-bound parsing/hashing at scale, ship a single cross-platform
binary, and avoid a class of memory-safety bugs while walking untrusted
source trees. See CODEKURVE_MASTER_PLAN.md §1 (executive summary, initial
tech list) and §60 (final startup decision).

## Decision

Implement CodeKurve in Rust, as a Cargo workspace, from the first commit.

## Alternatives

- **Go**: simpler concurrency model, weaker parser/FFI ecosystem for
  Tree-sitter grammars, GC pauses undesirable for large-repo indexing.
- **TypeScript/Node**: fastest to prototype, but single-threaded CPU-bound
  parsing and weaker memory/perf guarantees for an indexer meant to scale to
  large enterprise repos.
- **C++**: comparable performance, no memory safety, worse dependency/tooling
  ergonomics for a small team.

## Consequences

- Cross-platform CI must build/test on Linux, macOS, and Windows from day
  one (§37).
- `unsafe_code = "forbid"` is set workspace-wide (design decision, see
  `docs/ARCHITECTURE.md`); no FFI escape hatches without an explicit new ADR.
- The decision is revisited only per the exit criteria in §58 (throughput,
  memory, complexity, developer velocity measured after the first vertical
  slice) — not abandoned because the first parser integration is hard.

## Status

Accepted (2026-07-22)
