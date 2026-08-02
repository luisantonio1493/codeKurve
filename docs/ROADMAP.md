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
| 6 | Enterprise hardening | **Complete** (merged to `main`) — internal-pilot readiness: `max_total_files` hard-fail, `cargo-deny`/`cargo-about` CI audit, measured 100/1k/10k benchmarks, release artifacts (binaries/SBOM/checksums), `codekurve install` for Claude Code/Cursor/Codex CLI. See `openspec/changes/archive/2026-07-30-phase-6-enterprise-hardening/verify-report.md`. |
| 7 | Angular and .NET aware | **Complete** (merged to `main`) — heuristic recognition pass for Angular (DI, routing, decorators) and .NET (attribute + call-driven controllers, minimal APIs, DI, EF Core), all framework edges `Heuristic`-provenance and confidence-floored. See `openspec/changes/archive/2026-08-02-phase-7-angular-dotnet/verify-report.md` and `docs/FRAMEWORKS.md`. |
| 8 | Pilot and evaluation | **In progress** — real-repo validation (one .NET, one Angular project) found and fixed 2 real bugs (an incremental-index foreign-key violation, unresolved symbol ids in `trace`/`impact` CLI output) that no fixture had caught. Index/query timing, memory footprint, and precision measured; still open: a large-repo memory measurement and the final continue/adjust/stop/scale-out decision. |

## Beyond the plan

Work that is **not** part of the master plan's phases 0–8. Listed here rather
than folded into a phase row, so the plan's scope stays an honest record of
what was designed up front versus what was added afterwards.

- **Terminal UI (`codekurve tui` + the interactive `codekurve install`
  picker)** — added on 2026-08-02, unplanned. A new leaf crate,
  `crates/codekurve-tui`, rendering the existing `query`/`install` layers;
  it introduces the workspace's first UI dependencies (`ratatui`,
  `crossterm`, +401 KiB on the release binary) and serves humans, whereas
  the plan's primary consumer is an agent over MCP. Justification, measured
  cost, and the exit condition for removing it: `docs/adr/0011-ratatui-tui.md`.

> **Public distribution note**: the repo is public and `v0.1.0` binaries are
> published on GitHub Releases (see [Installation](../README.md#installation)).
> Licensing (previously unresolved) is now MIT — see `docs/LICENSING.md`.

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
