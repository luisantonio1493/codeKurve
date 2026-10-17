# Improvement Roadmap

This roadmap captures the findings from the September 2026 audit. It preserves valid CLI, configuration, and MCP contracts while correcting unsafe or misleading behavior incrementally.

## Summary

Prioritize security policy enforcement and index correctness before speed or binary-size work. Retain Rust, SQLite, and the existing crate boundaries.

| Area | Verified gap |
| --- | --- |
| Security | Configured ignore patterns and snippet limits are not enforced; installers do not verify published checksums. |
| Reliability | Discovery failures can be treated as deletions; configuration and watcher events can leave stale index data. |
| Performance | Graph queries load all relationships before applying limits; some endpoints page after materializing all results. |
| Architecture | Indexing logic depends on command helpers and some error handling relies on strings. |
| Usability | The CLI silently accepts invalid input; documented paths do not all work; the TUI hides incomplete results. |
| Size | Release-size optimizations already exist; current reproducible measurements are missing. |

## Phase 1: Establish a Reproducible Baseline

- Run Rust tests using an isolated target directory without deleting existing build artifacts.
- Make benchmark output identify commit, binary hash, Cargo profile, toolchain, and platform.
- Measure full indexing, no-op indexing, single-file changes, watcher bursts, graph traversal, query latency, peak RSS, SQLite size, and release artifact size.
- Exercise 100-, 1,000-, and 10,000-file fixtures, including dense and disconnected graphs.

**Acceptance:** Results are reproducible and distinguish measured performance from budgets or targets.

## Phase 2: Enforce Existing Security Policy

- Apply `ignore.patterns` consistently to full indexing, incremental indexing, and watcher indexing; fail clearly for invalid patterns.
- Enforce search, source-read, and UTF-8-safe snippet limits at the actual read/query boundary.
- Enforce project-root containment for source reads and reject path or symlink escapes.
- Scope externally reachable symbol lookup by project.
- Verify the release SHA-256 manifest before either installer replaces an executable; preserve the prior binary on failure.

**Acceptance:** Tests cover exclusion patterns, large requests, Unicode boundaries, cross-project access, path manipulation, and missing or mismatched checksums.

## Phase 3: Make the Index Correct and Recoverable

- Distinguish confirmed deletion from discovery or read failure; retain the last complete index generation and expose degraded status.
- Persist and compare an effective analysis-input fingerprint: analyzer version, relevant configuration, and TypeScript aliases.
- Correct invalidation for created, renamed, and removed modules.
- Register the watcher before reconciliation, reconcile queued changes, and treat root/error/rescan events as reconciliation work.
- Retain failed watcher work with bounded retries and visible health reporting.

**Acceptance:** Incremental results equal a clean rebuild, and transient failures neither erase data nor claim a verified index.

## Phase 4: Reduce Unnecessary Work

- Fetch graph adjacency in bounded frontier batches and start resource accounting before SQL materialization.
- Move ordering, filtering, and pagination to SQLite; report totals and truncation accurately.
- Discover only the affected path or subtree for precise events; reserve full scans for ambiguous events or policy changes.
- Refresh metadata for unchanged-content touches.
- Re-use prepared SQLite statements and batch large parameter lists.
- Share parsed-file context between extraction and framework recognition only when profiling proves it worthwhile.

**Acceptance:** Functional parity is preserved and improvements are supported by the Phase 1 measurements.

## Phase 5: Improve Architecture, CLI, and TUI

- Move indexing orchestration and conversions into typed application services while retaining current crate boundaries.
- Replace internal string-matched failures with typed errors.
- Reject unknown flags, missing values, invalid numbers, conflicting selectors, and extra arguments; invalid mutations must exit with code 2 and make no changes.
- Make documented positional graph selectors and JSON/limit options work consistently.
- Add offset pagination to MCP search and reuse it in the TUI.
- Surface total counts and truncation reasons in the TUI.
- Run TUI queries in a bounded, debounced worker; discard obsolete responses and keep input responsive.
- Improve narrow-terminal layouts, error visibility, cursor behavior, and redirected-output safety.

**Acceptance:** README examples run, JSON contracts are tested, paged results are complete, and terminal tests cover slow queries, Unicode, narrow screens, and large result sets.

## Phase 6: Control Releases, Size, and Documentation

- Publish raw and compressed binary size, build duration, and size attribution per supported platform.
- Keep the full default product; do not split CLI/MCP/TUI or remove bundled SQLite without a measured benefit.
- Build with locked dependencies and pinned build tooling; limit release write permissions to publication work.
- Require checks for the exact release tag before publishing.
- Update architecture, security, and performance documentation to label facts as implemented, measured, or planned.
- Run performance reporting on a schedule and for releases before introducing any enforcement threshold.

**Acceptance:** Release evidence is reproducible, permissions are least-privilege, and documentation matches the current implementation.

## Compatibility Contracts

- Preserve valid command names, MCP tools, and configuration.
- Add optional search `offset` with default `0`.
- Correct pagination metadata and add truncation reasons without removing existing fields.
- Document stricter validation as a correction for invalid invocations.
- Use versioned, transactional storage migrations; do not erase indexes to resolve incompatibility.
- Preserve local operation, stdio MCP transport, and disabled-by-default MCP reindexing.

## Delivery Order

1. Baseline measurements
2. Security enforcement
3. Index correctness and watcher recovery
4. Query and indexing efficiency
5. Application architecture and usability
6. Release hardening, size tracking, and documentation

Every phase closes with relevant unit/integration tests, formatting, Clippy, CI-equivalent Rust checks, CLI/MCP contract checks, and evidence for performance claims.

