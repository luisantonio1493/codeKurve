# 0012. Self-update by spawning the published install script

## Context

Upgrading CodeKurve today means remembering and re-typing the install
one-liner from the README. That is friction users hit on every release, and
"how do I update?" is not a question a CLI should make people leave the
terminal to answer. The same applies in reverse: removing the binary means
finding `install.sh --uninstall` in the docs.

Two existing promises stand in the way, and both are advertised in
`README.md`'s "Security promise":

- ADR 0005 (`0005-no-network-no-telemetry.md`): "CodeKurve makes no outbound
  network requests and collects no telemetry, **in any mode**. No HTTP client
  crate is added to the workspace without a new ADR explicitly superseding
  this one."
- `docs/SECURITY_MODEL.md`: "never shell out" — whose stated rationale is not
  executing untrusted **project** code during analysis.

The "no network" claim is a differentiator for a tool that reads people's
proprietary source. Weakening it to save a copy-paste would be a bad trade.

## Decision

Add `codekurve update` and an opt-in `codekurve uninstall --binary`. Neither
performs any network I/O from Rust. Both **spawn the already-published
install script** (`install.sh` / `install.ps1` at the repo root), which is
what downloads or deletes the release binary.

Concretely:

- **CodeKurve gains no HTTP client dependency**, and no Rust code in this
  workspace opens a socket. ADR 0005's substance is intact for every analysis
  path: `index`, `watch`, `mcp`, `tui`, and every query command spawn nothing
  and reach no network. The network lives in a shell script the user can read
  before running, not inside the binary.
- `crates/codekurve/src/update.rs` is the **one and only** place CodeKurve
  spawns a subprocess. It is reachable only by a user explicitly typing
  `codekurve update` or `codekurve uninstall --binary` — never from `index`,
  `watch`, `mcp`, `tui`, any query command, or any automatic path. There is
  no update check, no background poll, no "a new version is available"
  banner.
- The exact command is **printed verbatim before anything runs**, followed by
  a `[y/N]` confirmation.
- **A non-terminal stdin refuses** rather than proceeding, and says to pass
  `--yes`. This differs from `install.rs`'s `confirm` helper, which
  auto-proceeds when stdin is not a terminal: `install`/`uninstall` only write
  local config files and back them up first, whereas these paths download and
  replace — or delete — an executable. Silently doing that in a scripted
  context is a materially worse failure mode than a prompt nobody answers.
  `--yes` remains the explicit opt-in for automation.
- Removing the binary is **opt-in via `--binary`**, never the default. Plain
  `codekurve uninstall` keeps its existing behaviour exactly: agent configs
  only. Escalating a config-only command into one that deletes an executable
  would be a surprising, hard-to-undo default, and it would contradict the
  reasoning above that the subprocess is only ever reached by explicit user
  intent. This is a deliberate divergence from codegraph, whose `uninstall`
  removes both by default with `--keep-cli` to opt out; recorded here so it is
  a choice, not an oversight.
- The install-script URLs are constants in `update.rs`, so README and code
  cannot drift silently.
- The child's exit status is propagated: a failed installer makes
  `codekurve update` exit non-zero and say so.

Commands, verbatim:

| | Unix | Windows |
|---|---|---|
| `update` | `sh -c "curl -fsSL <install.sh> \| sh"` | `powershell -NoProfile -Command "irm <install.ps1> \| iex"` |
| `uninstall --binary` | `sh -c "curl -fsSL <install.sh> \| sh -s -- --uninstall"` | `powershell -NoProfile -Command "&([scriptblock]::Create((irm <install.ps1>))) -Uninstall"` |

`install.ps1` had no uninstall switch before this change (despite the docs
claiming one); a `-Uninstall` switch mirroring `install.sh --uninstall` was
added to it here.

This ADR **scopes** ADR 0005 and the never-shell-out control. It does not
supersede either. ADR 0005 keeps its Status; both it and
`docs/SECURITY_MODEL.md` cross-reference this carve-out.

## Alternatives

- **Print the command and exit** (a `codekurve update` that only tells you
  what to paste): rejected — barely better than the README, and it still
  leaves the user to type a `curl … | sh` by hand, which is the same
  supply-chain exposure with an extra step.
- **A built-in downloader with an HTTP client** (`ureq`/`reqwest`, resolve the
  latest release, verify a checksum, atomically replace the binary): rejected.
  It requires a *real* supersede of ADR 0005 rather than a scoped carve-out,
  adds a dependency (and its transitive TLS stack) to a workspace that
  currently has none, and weakens the headline "no network, in any mode" claim
  that differentiates this project — for functionality a 40-line shell script
  already performs. It would also duplicate the OS/arch detection, version
  resolution, and PATH handling `install.sh` already does correctly.
- **Do nothing, keep documenting the one-liner**: rejected as the status quo
  the users actually complained about, though it remains the fallback for
  anyone who declines the prompt.

## Consequences

- **Honest cost: running a remote install script is a supply-chain surface.**
  `codekurve update` fetches and executes code from `raw.githubusercontent.com`
  at the moment it runs. A compromised GitHub account, a compromised release,
  or a MITM on that host would execute arbitrary code as the invoking user.
  This is exactly the exposure of the `curl | sh` line in the README — no
  more, but no less, and moving it behind a subcommand makes it easier to
  invoke without thinking about it. The mitigations actually implemented are
  modest and are listed rather than oversold:
  - the exact command is printed verbatim before anything runs, so the user
    can read and reject it;
  - an explicit `[y/N]` confirmation is required;
  - a non-terminal stdin refuses outright instead of proceeding;
  - the scripts are in this repo, reviewable and diffable, and CodeKurve
    fetches nothing else;
  - nothing automatic ever reaches this code path.

  Not implemented, and therefore not claimed: signature verification, checksum
  verification of the fetched script, binary notarization, or pinning.
  `install.sh` downloads release binaries over HTTPS without verifying the
  published `SHA256SUMS`; closing that gap is a separate change to the script.
- Dependency review keeps flagging any network-capable crate as an ADR-0005
  violation. This ADR grants no exemption for one.
- Reviewers must keep the subprocess confined to `update.rs`. A second spawn
  site anywhere else needs its own ADR.
- `install.ps1` now takes a `-Uninstall` switch; its README/AGENT_USAGE
  references are now true rather than aspirational.

## Status

Accepted (2026-08-02)
