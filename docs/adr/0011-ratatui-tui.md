# 0011. `ratatui` + `crossterm` for the terminal UI

## Context

Everything CodeKurve knows is reachable today only one query at a time:
`codekurve search`, then copy a name, then `codekurve references
--symbol-name …`, then read the output and repeat. Walking a call graph —
the thing the index exists for — costs one process launch and one manual
copy-paste per hop. `codekurve install` has the same shape of problem: with
no client argument it prints a plan and asks a single `[y/N]`, so a user who
wants three of the five detected agents has to answer "no" and then run the
command three more times.

Both are interactive problems. Adding a TUI is the first time this project
needs to draw to a terminal rather than print to one.

This matters more than a normal dependency question, because the workspace
carries only **four** direct third-party dependencies today (`rmcp`,
`tokio`, `notify`, `toml_edit`, plus `serde_json`/`schemars`/`rusqlite`
beneath them). The dependency policy in `CONTRIBUTING.md` (plan §0.5, §12)
requires purpose, license, maintenance status, binary-size cost, security
impact, and the alternative considered before any addition.

## Decision

Add `ratatui` (0.30, `default-features = false`, feature `crossterm_0_29`)
and `crossterm` (0.29) to `[workspace.dependencies]`, used by exactly one
new crate, `crates/codekurve-tui`.

The dependency direction is one-way: `codekurve-tui` depends on `codekurve`
and is depended on only by `codekurve-bin`. Neither `codekurve`,
`codekurve-core`, `codekurve-store`, `codekurve-analysis` nor
`codekurve-mcp` gains a terminal dependency, so the library and the MCP
server stay headless.

`codekurve-tui` contains **no query logic**. Both screens are rendering
layers over existing public APIs:

| screen | reuses |
|---|---|
| explorer | `query::search`, `query::get_symbol`, `query::relationships`, `query::impact`, `Session::warnings` |
| installer picker | `install::plan`, `install::install_named` (which route to the same writers `install <client>` uses) |

### Purpose

- `ratatui`: immediate-mode widget/layout/diffing engine. Hand-rolling the
  double-buffered diff, the constraint solver and the widget set is a
  multi-week project whose bugs are invisible until someone resizes a window.
- `crossterm`: the cross-platform terminal backend — raw mode, the alternate
  screen, and key events on Linux/macOS/**Windows console**. Windows is a
  first-class CI target (`docs/ROADMAP.md`), which rules out the
  termios-only options.

### Licenses

Both crates are **MIT**. The 58 crates they add transitively break down as:

| license | crates |
|---|---|
| `MIT OR Apache-2.0` (incl. `MIT/Apache-2.0`, `Apache-2.0/MIT`) | 36 |
| `MIT` | 18 |
| `Apache-2.0` (`approx`) | 1 |
| `Apache-2.0 OR BSL-1.0` (`ryu`) | 1 |
| `Zlib` (`foldhash`) | 1 |
| *(plus the workspace's own `codekurve-tui`, MIT)* | 1 |

All permissive; no copyleft (no GPL/LGPL/AGPL/MPL/EPL) anywhere in the new
subtree. One allowlist change was required: **`Zlib`** was added to
`about.toml`'s `accepted` list and `deny.toml`'s `allow` list, for
`foldhash 0.2.0` (reached via `ratatui-core` → `kasuari` → `hashbrown`).
Zlib is OSI-approved, permissive, and imposes no reciprocity or
source-disclosure obligation — it belongs in the same bucket as the MIT/BSD
entries already there. `cargo about generate about.hbs` and `cargo deny
check licenses` both pass with that one addition.

### Maintenance status

- `ratatui` — active fork-successor of `tui-rs` (unmaintained since 2023),
  now the de-facto Rust TUI library; multi-maintainer organisation
  (`ratatui/ratatui`), regular releases, 0.30.2 current. The 0.30 split into
  `ratatui-core` / `ratatui-widgets` / `ratatui-crossterm` is a sign of
  ongoing investment, not abandonment.
- `crossterm` — the standard cross-platform terminal crate, depended on by
  most of the Rust CLI ecosystem, 0.29 current.

Neither is a single-author crate with a stale release history, which is the
failure mode this policy exists to catch.

### Binary-size cost (measured, not estimated)

`cargo build --release -p codekurve-bin` on macOS/aarch64, same tree, only
the `codekurve-tui` dependency and its two call sites in `main.rs` differing:

| build | bytes |
|---|---|
| without the TUI | 16,738,336 |
| with the TUI | 17,149,120 |
| **delta** | **+410,784 B (+401 KiB, +2.45 %)** |

`default-features = false` on `ratatui` is what keeps it there: it drops the
calendar widget, the `ratatui-macros` helpers and the layout cache, none of
which these two screens use.

Honest framing: 401 KiB is real but small next to the ~16 MB the bundled
SQLite and the Tree-sitter grammars already cost. If this project were a
300 KiB binary the answer would have been different.

### Security impact

- **No network.** ADR 0005 holds. Neither crate opens a socket. The one
  crate worth naming is `mio` (via `crossterm`'s `events` feature): `mio` is
  *capable* of TCP/UDP, but `crossterm` depends on it with
  `features = ["os-poll"]` only — `mio`'s `net` feature is never enabled, so
  no socket type is even compiled in. Verified in
  `crossterm-0.29.0/Cargo.toml`. `cargo deny check advisories bans sources`
  passes.
- **No shell-out.** `SECURITY_MODEL.md` holds: `crossterm` writes escape
  sequences to a file descriptor; it spawns no process.
- **No `unsafe` in our code.** `codekurve-tui` inherits
  `unsafe_code = "forbid"` through `[lints] workspace = true`. The
  dependencies themselves contain `unsafe` (`mio`, `parking_lot`, `winapi`
  and the terminal FFI necessarily do) — that is the cost of not writing
  those `ioctl` calls ourselves, and it is the same trade already accepted
  for `rusqlite` and Tree-sitter.
- **New failure mode, mitigated:** a TUI that panics with raw mode still on
  leaves the user's shell unusable. `ratatui::try_init()` installs a panic
  hook that disables raw mode and leaves the alternate screen *before* the
  panic propagates (`ratatui-0.30.2/src/init.rs`, `set_panic_hook`); both
  screens go through it and both call `ratatui::restore()` on the normal
  path.
- **No behaviour change to any non-interactive path.** The installer picker
  is reached only when there is no client argument, `--yes` was not passed,
  **and** stdin is a terminal — the exact conditions under which the `[y/N]`
  prompt already appeared. Scripted, piped and agent-driven installs run the
  identical code they ran before.

## Alternatives

- **No TUI — stay CLI-only.** Zero dependencies, zero bytes, and the status
  quo keeps working for scripts and agents (which is the majority of this
  tool's traffic). Rejected because it leaves the graph-walking use case
  unserved for humans: nothing about `search → copy id → references → copy
  id → references` gets better with more CLI polish, and the alternative
  users actually reach for is "grep the repo instead", which is the exact
  behaviour CodeKurve exists to replace. Weakest part of this decision: the
  TUI serves humans, and CodeKurve's primary consumer is an agent over MCP.
  It is an addition for a secondary audience, and that is why it is scoped
  to one leaf crate that nothing else depends on and that can be deleted in
  one commit.
- **Hand-rolled ANSI, no dependency.** Print escape sequences directly, read
  bytes from stdin, parse them. Viable for the installer picker alone (~150
  lines). Rejected for the pair, because the explorer needs a resizable
  two-pane layout with scrolling lists and per-cell diffing, and because the
  input half is the trap: raw mode via `termios` requires `unsafe` FFI
  (forbidden workspace-wide) or a `libc` dependency, escape-sequence
  decoding is a real parser with real edge cases, and **Windows console
  input is a completely different API** — reimplementing `crossterm`'s
  platform layer badly, for a CI matrix that includes `windows-latest`. The
  "no dependency" version is not dependency-free, it is `libc` + `winapi` +
  our own bugs.
- **`cursive`.** Higher-level, callback/retained-mode. Heavier dependency
  tree, less direct control over rendering, and a smaller ecosystem than
  `ratatui` today.
- **`dialoguer`/`inquire` for the picker only, no explorer.** Would solve
  the multi-select install with a much smaller dependency, but leaves the
  explorer unbuilt and would still be a new dependency — and then a second
  one later when the explorer lands. One TUI stack for both screens beats
  two libraries.
- **`ratatui` with default features.** Rejected: it pulls the calendar
  widget, `ratatui-macros` and the layout cache for no use here. Measured
  saving is the reason `default-features = false` is in the manifest.

## Consequences

- The workspace gains its first UI dependency, and the direct-dependency
  count goes from 4 to 6. The dependency policy's bar rises accordingly: any
  *further* TUI dependency (a text-input widget crate, a syntax highlighter)
  needs its own justification, not a reference to this ADR.
- `docs/ROADMAP.md` records this as scope beyond the master plan's phases
  0–8 — it was not planned, it was added.
- `about.toml` and `deny.toml` now accept `Zlib`. Reverting the TUI should
  revert that entry too, once nothing else needs it.
- Rendering and the event loop are deliberately untested; the state machines
  (`explorer::Explorer`, `picker::Picker`) hold no `ratatui` or terminal
  types precisely so selection, navigation and toggling are testable without
  a pty. Terminal drawing is verified by running the screens, not by
  asserting on frame buffers.
- If the TUI proves unused, deleting `crates/codekurve-tui`, the two
  `main.rs` call sites, the two `[workspace.dependencies]` entries and the
  `Zlib` allowlist line removes it completely — nothing else in the
  workspace references it. That is the exit condition for this ADR.

## Status

Accepted (2026-08-02)
