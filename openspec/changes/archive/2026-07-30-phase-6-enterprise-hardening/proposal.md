# Proposal: Phase 6 — Enterprise Hardening (Internal Pilot Readiness)

## Intent

Codekurve already satisfies most enterprise-approval properties by architecture (no network, local storage, small dependency surface), but none of it is *proven or packaged*. Performance numbers are aspirational targets, dependency/license posture is unaudited, resource limits are partial, and there is no way to hand a colleague a working install. Phase 6 closes the gap between "architecturally safe" and "demonstrably safe and installable" so an internal pilot repo can be onboarded.

Success = a reviewer can read a threat model, an SBOM, a license report, and a real benchmark table, then install Codekurve on a pilot machine without a Rust toolchain.

## Scope

### In Scope
1. **Resource + config policy hardening**: add `max_total_files` (missing today; only `max_file_size_bytes`/`follow_symlinks` exist), enforce it in discovery, and verify sensitive-file exclusion is *implemented*, not just documented in SECURITY_MODEL.md.
2. **Dependency audit**: `cargo-deny` (advisories + licenses + bans) wired into CI, currently marked "Deferred" in ci.yml.
3. **Reproducible benchmark report**: execute the existing docs/PERFORMANCE.md methodology against 100/1k/10k-file fixture repos; replace target numbers with measured ones plus a repeatable command.
4. **Internal release artifacts**: new CI release workflow producing multi-platform binaries (macOS x64/aarch64, Linux x64, Windows x64), an SBOM (CycloneDX), a third-party license/NOTICE report, and SHA-256 checksums — published as internal/workflow artifacts, not a public channel.
5. **`codekurve install` subcommand**: auto-wire the MCP server into supported clients (Claude Code, Cursor, Codex CLI) by editing their config instead of manual `.mcp.json` surgery.
6. **Threat-model closure**: update SECURITY_MODEL.md with the now-real checksum/SBOM story and a documented data-paths section.

### Out of Scope (non-goals)
- **Public redistribution**: `curl | sh` / `irm | iex` install scripts, GitHub Releases publishing, any public download URL. Deferred pending docs/LICENSING.md (see Risks).
- Binary signing / notarization / codesigning.
- aarch64 Linux (plan marks it "Futuro").
- Memory-budget enforcement and new timeout/cancellation semantics — cancellation already tracked separately; a memory budget is speculative without benchmark evidence. Revisit after slice 2 measurements.
- **"Cleanup"** (vague in the plan): scoped to removing stale artifacts/dead config discovered during the above slices, opportunistically. No standalone refactor slice.
- **"Config policies"** (vague in the plan): scoped to items 1 + 6 only — resource limits and exclusion policy. No new policy engine, no config schema redesign, no per-repo policy files.
- Choosing an OSS license (business/legal decision, not this phase's work).

## Capabilities

### New Capabilities
- `release-packaging`: how release artifacts are produced and attested — binary matrix, SBOM, license report, checksums, and installation/wiring flow. New spec area; existing specs (`symbol-index`, `relationship-graph`, `csharp-analysis`, `mcp-server`, ...) are all analysis capabilities, so this is packaging/ops in kind and does not extend them.

### Modified Capabilities
- `symbol-index`: discovery gains a `max_total_files` limit with defined over-limit behavior (deterministic truncation vs. hard error — decide in spec phase).

## Approach

Four independent, disjoint slices; only slice 1 touches core Rust logic, so they can be chained without heavy conflict risk.

| Slice | Content | Kind |
|---|---|---|
| PR1 | `max_total_files` + enforcement + config policy verification | Rust, localized |
| PR2 | cargo-deny in CI + license/NOTICE report generation | CI/config |
| PR3 | Benchmark fixtures (100/1k/10k) + measured PERFORMANCE.md + repeatable runner | Fixtures + docs |
| PR4 | Release workflow: binary matrix, SBOM, checksums (internal artifacts only) | CI/YAML |
| PR5 | `codekurve install` MCP client auto-wiring | Rust, new subcommand |

Prefer off-the-shelf tooling over bespoke: `cargo-deny` for audit/licenses, `cargo-cyclonedx` (or `cargo-sbom`) for SBOM, `cargo-about` for NOTICE, `shasum`/`sha256sum` in CI for checksums. No custom SBOM/report code.

Slices 1–3 are unblocked by licensing. Slice 4 stops at "artifacts exist and are verifiable"; the publish step is deliberately absent. Slice 5 writes only to local client configs, no network, so it is also unblocked.

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `crates/codekurve-core/src/config.rs` | Modified | New `max_total_files` field + default |
| `crates/codekurve-analysis/src/discovery.rs` | Modified | Enforce total-file cap |
| `crates/codekurve/src/commands.rs` | New | `install` subcommand |
| `.github/workflows/ci.yml` | Modified | cargo-deny job |
| `.github/workflows/release.yml` | New | Binary matrix, SBOM, checksums |
| `docs/PERFORMANCE.md` | Modified | Measured numbers replace targets |
| `docs/SECURITY_MODEL.md` | Modified | Checksums/SBOM/data-paths closure |
| `docs/LICENSING.md` | Modified | Record the deferral explicitly |
| `deny.toml`, `about.toml` | New | Audit config |
| `fixtures/bench/` | New | 100/1k/10k-file fixture generation |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| **Licensing unresolved** blocks public distribution | High (already true) | Scope phase to internal artifacts only; record deferral in LICENSING.md; public channel becomes a follow-up change once resolved. Not a blocker for slices 1–5. |
| cargo-deny surfaces existing advisories/incompatible licenses | Medium | Triage in PR2: `deny.toml` allow-list with a written justification per exception rather than blanket ignores |
| Benchmarks miss budget targets | Medium | Report measured reality; treat target revision or an optimization follow-up as a separate change, do not silently retune targets to fit |
| 10k-file fixture is slow/large in CI | Medium | Generate synthetically at runtime; run the large tier locally/nightly, not per-PR |
| MCP client config formats drift | Medium | Support the three named clients only; fail loudly with manual instructions on unknown clients |
| Scope creep from vague "cleanup"/"config policies" | Medium | Explicitly bounded in Out of Scope above |
| Multi-platform CI build flakiness/cost | Low-Med | Reuse the existing 3-OS matrix; add aarch64 macOS via cross-compile target |

## Prerequisites

- Pilot repository selected (plan exit criterion) — a user decision, needed before pilot sign-off, not before implementation.
- Licensing decision — **not** a prerequisite for this phase as scoped; only for the deferred public-distribution follow-up.

## Rollback Plan

Per slice, independently revertable:
- PR1: revert commit; `max_total_files` defaults to effectively-unlimited, so no data migration and no index invalidation.
- PR2/PR4: delete/disable the workflow job; CI returns to fmt/clippy/test/licensing-check.
- PR3: docs-only revert.
- PR5: revert the subcommand; manual `.mcp.json` editing remains the documented path. `install` must back up any client config it rewrites so a user-level rollback is possible without git.

## Success Criteria

- [x] `max_total_files` configurable, enforced, and covered by a test at the limit boundary
- [x] `cargo-deny check` passes in CI with every exception justified in `deny.toml`
- [x] docs/PERFORMANCE.md contains measured numbers for 100/1k/10k tiers plus the exact command to reproduce them
- [x] A CI run produces, for all four platforms: binary, SBOM, license report, SHA-256 checksums
- [x] `codekurve install` wires at least one supported MCP client end-to-end on a clean machine with no Rust toolchain
- [x] SECURITY_MODEL.md documents data paths and no-network behavior with no open TODOs
- [x] docs/LICENSING.md explicitly records public redistribution as deferred, with the decision it awaits
- [x] Internal review completed against a selected pilot repository

## Proposal question round

5 open questions were resolved during sdd-spec phase:

1. **Over-limit behavior**: hard-fail (safer, no silently partial index) — DECIDED.
2. **Pilot repo**: synthetic fixtures — DECIDED.
3. **Distribution channel**: CI workflow artifacts, no release publishing — DECIDED.
4. **Slice 5 client list**: Claude Code + Cursor + Codex CLI — DECIDED.
5. **Benchmark failure policy**: measure and report, optimize separately — DECIDED.

## Sizing / Chaining Forecast

Comparable to or larger than phase-5-csharp (~2600–3400 lines merged, delivered as chained PR1–PR7).

| Slice | Est. authored lines | Budget risk (400) |
|---|---|---|
| PR1 | 150–250 | Low |
| PR2 | 150–250 | Low |
| PR3 | 300–500 (fixture generator + docs) | Medium |
| PR4 | 250–400 (YAML-heavy) | Medium |
| PR5 | 400–700 (per-client config handling + tests) | High — may split into PR5a/PR5b |

Total estimate: **1250–2100 authored lines**, lower than phase 5 because it is infra/YAML/docs-heavy rather than parser-heavy.

- Decision needed before apply: Yes
- Chained PRs recommended: Yes
- 400-line budget risk: High

Recommend a feature-branch chain: PR1 → PR2 → PR3 → PR4 → PR5, each targeting the previous branch. PR5 is the one likely to need splitting (config-format handling vs. detection/wiring). Slices 1–4 are genuinely independent and could also land in parallel against the tracker branch if conflict risk stays near zero.
