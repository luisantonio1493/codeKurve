# Roadmap

Full detail per phase: plan §43. Exit criteria are gates, not aspirations —
each phase's CI must pass on Windows, macOS, and Linux before moving on.

| Phase | Name | Status |
|---|---|---|
| 0 | Governance and scaffold | **Complete** (merged to `main`) |
| 1 | Minimal vertical slice | **Complete** (merged to `main`) |
| 2 | TypeScript graph | **Complete** (merged to `main`) |
| 3 | Incremental and watcher | **Complete** (merged to `main`) |
| 4 | MCP | **Complete** (merged to `main`) |
| 5 | C# | **Complete** (merged to `main`) — verify FAILED once on 3 critical gaps (CLI visibility output, `using static`/alias-reference test coverage), fixed and re-verified PASS before merge. See `openspec/changes/archive/2026-07-30-phase-5-csharp/verify-report.md`. |
| 6 | Enterprise hardening | **Complete** (working tree, not yet committed) — internal-pilot readiness: `max_total_files` hard-fail, `cargo-deny`/`cargo-about` CI audit, measured 100/1k/10k benchmarks, internal release artifacts (binaries/SBOM/checksums, CI-only), `codekurve install` for Claude Code/Cursor/Codex CLI. Public redistribution deferred pending `docs/LICENSING.md`. See `openspec/changes/archive/2026-07-30-phase-6-enterprise-hardening/verify-report.md`. |
| 7 | Angular and .NET aware | Not started |
| 8 | Pilot and evaluation | Not started |

## Phase 0 exit criteria

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -- version
```

All green on Windows, macOS, and Linux CI (plan §43).

## Deferred items (tracked, revisit when the trigger condition is met)

- **MSRV pinning**: `rust-version` left unset in `[workspace.package]`;
  pin once a minimum toolchain requirement actually matters.
- **Toolchain version pin**: `rust-toolchain.toml` currently floats on
  `stable`; pin to an exact version via ADR if reproducibility issues
  appear.
- **`[profile.release]` tuning**: not configured; revisit once real
  workloads exist to measure against (plan §38).
- **CI cache**: no cache action in Phase 0 (auditability over speed);
  reconsider only when CI slowness is measured, then pin and
  security-review the action.
