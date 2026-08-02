# Licensing

**Status: MIT** (chosen 2026-08-02). See `LICENSE` at the repo root for the
full text.

Why MIT: CodeKurve is a public tool inspired by `colbymchenry/codegraph`,
which is itself MIT-licensed; matching that gives users and any downstream
consumer the same, well-understood permissive terms. All dependencies are
already restricted to a permissive allowlist (`about.toml`'s `accepted`
list — MIT, Apache-2.0, BSD-2-Clause, ISC, Unicode-3.0, CC0-1.0, MIT-0,
Unlicense) and `cargo about generate` runs clean against it, so MIT
introduces no new compatibility conflict.

Every crate's `Cargo.toml` sets `license.workspace = true`, resolving to
`license = "MIT"` in `[workspace.package]` (`Cargo.toml` at the repo root).

- No code from Graphify, CodeGraph, or any other proprietary tool is
  copied — general ideas and patterns only, with inspiration documented
  without assuming license compatibility.

## History

- **Pending (through 2026-08-01)**: no license chosen, "do not redistribute"
  (all rights reserved by default).
- **2026-08-02**: repository made public, `v0.1.0` release binaries published
  on GitHub Releases, to unblock real installation for Phase 8's pilot — a
  distribution decision, not yet a licensing one.
- **2026-08-02 (later same day)**: MIT chosen and applied.
