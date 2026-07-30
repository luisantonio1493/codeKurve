# Design: Phase 6 — Enterprise Hardening

All 5 proposal open questions are resolved (hard-fail, synthetic fixtures, CI artifacts only, all 3 clients, measure-and-report). 5 chained PRs; only PR1/PR5 touch Rust.

## Architecture Decisions

| # | Decision | Choice | Rejected | Rationale |
|---|---|---|---|---|
| D1 | Over-limit enforcement site | Mid-walk short-circuit in `discovery::discover` | Post-walk count check | A 10k-cap breach on a 500k-file tree must not pay for the full walk. Cost: error cannot report the true total (message states the limit, not the overage). |
| D2 | Error type | New variant on existing `codekurve_core::Error` | New `DiscoveryError` enum; `Result<_, String>` | §56 "typed errors"; `codekurve-analysis` already depends on core for `LanguageId`. One variant, no new enum. |
| D3 | Dependency audit | `cargo-deny` via `EmbarkStudios/cargo-deny-action@v2` (SHA-pinned) | `cargo-audit` + `cargo-license` | One binary/config covers advisories+licenses+bans+sources; ci.yml already names it as the deferred choice. |
| D4 | NOTICE report | `cargo-about` (`about.toml` + `about.hbs`) | cargo-deny license check; `cargo-license` | Only cargo-about emits actual license *text* — attribution needs the text, not a gate or a CSV. |
| D5 | SBOM | `cargo-cyclonedx` | `cargo-sbom` | Maintained by the CycloneDX org itself and emits the format the proposal names; `cargo-sbom` is single-maintainer. |
| D6 | Linux runner | `ubuntu-22.04` for the release build (CI keeps `ubuntu-latest`) | `ubuntu-latest`; musl | ubuntu-latest (24.04, glibc 2.39) produces a binary that will not run on a 22.04 pilot box. 22.04 widens compat for one token; musl needs a cross toolchain. |
| D7 | macOS x64 | Second `macos-latest` matrix row with `--target x86_64-apple-darwin` | `macos-13` Intel runner | Identical steps across all 4 rows; no special-casing. |
| D8 | Checksums | Single `bundle` job on ubuntu running `sha256sum` over all downloaded artifacts | Per-OS hashing (`shasum`/`sha256sum`/`Get-FileHash`) | One `SHA256SUMS` file, one shell dialect, verifiable with `sha256sum -c` in the same job. |
| D9 | `install` never shells out | Direct config-file read/modify/write | `claude mcp add`, `cursor` CLI | SECURITY_MODEL.md control: "never shell out". Also works when the client CLI is absent. |
| D10 | TOML rewrite | `toml_edit` (format-preserving) | `toml` serialize round-trip | Codex `config.toml` is hand-written; a `toml` round-trip destroys comments/ordering. `toml_edit 0.22.27` is **already in Cargo.lock** (transitive via `toml`) — zero new tree nodes. |
| D11 | JSON key order | Accept `serde_json` alphabetical reordering of rewritten client JSON | `serde_json/preserve_order` | **Trap**: `preserve_order` is a unified workspace feature; it would flip every `json!` map from BTreeMap to insertion order and break the entire CLI/MCP golden suite. Reordering a user's `.mcp.json` is cosmetic and the `.bak` covers it. |
| D12 | Home dir | `std::env::var("HOME")`, `USERPROFILE` on Windows, `CODEX_HOME` override first | `dirs` crate | Stdlib suffices; no new dependency. |
| D13 | Bench tooling | `scripts/gen_bench_fixture.py` + `scripts/bench.py` | hyperfine; Rust bin/criterion | `scripts/check_licensing.py` already establishes Python-in-`scripts/` as the project convention. No new installed tool for maintainers. |

## PR1 — `max_total_files`

`crates/codekurve-core/src/config.rs`: add `pub max_total_files: usize` to `Index` (sibling of `max_file_size_bytes`), default `50_000` (5x the 10k "Large" tier in docs/PERFORMANCE.md). `Index` has no `#[serde(default)]` on its fields, so a config file naming `[index]` without the key fails to parse — matching existing field behavior; `#[serde(default)]` on the whole `index` section still covers files that omit the section.

`crates/codekurve-core/src/error.rs`:
```rust
#[error("project exceeds index.max_total_files ({limit}); raise the limit in .codekurve/config.toml or narrow index.languages / ignore.patterns")]
TooManyFiles { limit: usize },
```

`crates/codekurve-analysis/src/discovery.rs`:
- `DiscoveryOptions` gains `pub max_total_files: usize`.
- `discover(root, options) -> Result<Vec<DiscoveredFile>, codekurve_core::Error>`.
- Short-circuit immediately after each successful `files.push(...)`: `if files.len() > options.max_total_files { return Err(Error::TooManyFiles { limit: options.max_total_files }) }`. Placed after push so exactly-at-limit passes.

Call-site ripple (3 sites, all already in `Result<_, String>` fns):
- `crates/codekurve/src/incremental.rs:72` (`detect`) and `:197` (`apply_via_full_reindex`) → `?` + `.map_err(|e| e.to_string())`.
- `crates/codekurve/src/commands.rs:1029` `discovery_options()` → populate the new field.
- Local `DiscoveryOptions` literals in both files' `#[cfg(test)]` helpers.

Consequence to state in the spec: `codekurve watch` also hard-fails per batch, because `detect` walks the whole tree even under a path filter. Deliberate — a repo that grew past the cap must not silently keep a partial index.

## PR2 — cargo-deny + NOTICE

`.github/workflows/ci.yml`: new job `audit`, `runs-on: ubuntu-latest`, not in the OS matrix (the dependency graph is OS-independent). Steps: checkout → `EmbarkStudios/cargo-deny-action@v2` with `command: check advisories licenses bans sources` → install `cargo-about` → `cargo about generate about.hbs > NOTICE-3rdparty.md` → `actions/upload-artifact@v4` (`name: license-report`). Delete the `cargo-deny` line from the trailing "Deferred" comment block (keep the aarch64-Linux / doc-tests / MSRV / caching deferrals).

New `deny.toml`: `[advisories] yanked = "deny"`; `[licenses] allow = [...]` explicit SPDX allow-list; `[bans] multiple-versions = "warn"` (workspace already carries duplicate transitive versions — `deny` here would fail on day one); `[sources] unknown-registry = "deny"`. Every `[advisories].ignore` / license exception entry carries an inline `# reason:` comment naming the advisory, why it is not exploitable here, and the revisit condition. Blanket ignores are forbidden.

New `about.toml` + `about.hbs`. `scripts/check_licensing.py` stays — it checks the project's own licensing posture, not third-party.

## PR3 — Benchmarks

- `scripts/gen_bench_fixture.py --tier {small,medium,large} --out fixtures/bench/<tier>`: deterministic seeded synthetic TS/C# tree at 100 / 1,000 / 10,000 files with realistic call/inherit density. `fixtures/bench/` is gitignored (generated, not committed) — keeps repo size flat and honors the proposal's "generate synthetically at runtime".
- `scripts/bench.py --tier <t> --runs 5`: cold (fresh `.codekurve/index.db`) and warm passes, `time.monotonic()` around `codekurve index`, reports median + p95 per docs/PERFORMANCE.md's stated method.
- `docs/PERFORMANCE.md`: replace "No benchmarks have been run yet" with a Measured table (tier, files, cold median, cold p95, warm median), a Hardware/OS/storage block, and the exact reproduce command. Keep the Budgets table alongside and mark each row met/missed. Per the confirmed decision, a missed budget is reported, not retuned; it becomes a follow-up change.
- Large tier is local/manual only, never per-PR CI.

## PR4 — `.github/workflows/release.yml`

Triggers: `workflow_dispatch` + `push: tags: ['v*']`. No publish step anywhere in the file — that absence is the design.

Job `build` (matrix, `fail-fast: false`):

| os | target | artifact |
|---|---|---|
| ubuntu-22.04 | x86_64-unknown-linux-gnu | codekurve-linux-x64 |
| macos-latest | aarch64-apple-darwin | codekurve-macos-aarch64 |
| macos-latest | x86_64-apple-darwin | codekurve-macos-x64 |
| windows-latest | x86_64-pc-windows-msvc | codekurve-windows-x64.exe |

Steps: checkout → `dtolnay/rust-toolchain@stable` with `targets: ${{ matrix.target }}` → `cargo build --release -p codekurve-bin --target ${{ matrix.target }}` → rename to the artifact name → `upload-artifact@v4` (`name: bin-${{ matrix.target }}`).

Job `sbom` (needs: none, ubuntu-22.04): install `cargo-cyclonedx` → `cargo cyclonedx --format json --all` → upload `sbom`. Job `licenses`: `cargo-about` → upload `license-report` (same recipe as PR2's step; PR2 lands it first, PR4 reuses it).

Job `bundle` (`needs: [build, sbom, licenses]`, ubuntu-22.04): `download-artifact@v4` with `path: dist` and no name (pulls all) → flatten → `sha256sum * > SHA256SUMS` → `sha256sum -c SHA256SUMS` as an in-job self-check → upload `codekurve-release-bundle` containing binaries + SBOM + NOTICE + SHA256SUMS.

## PR5 — `codekurve install`

New module `crates/codekurve/src/install.rs` (mirrors `watch`, called directly from `main.rs`; keeps the already-1000-line `commands.rs` untouched). `crates/codekurve/Cargo.toml` gains `toml_edit = "0.22"`.

```rust
pub enum Client { ClaudeCode, Cursor, Codex }
pub fn run(root: &Path, client: Option<&str>) -> Result<(), String>;
fn server_entry(exe: &Path, root: &Path) -> (String /*name*/, serde_json::Value);
fn write_json_client(path: &Path, entry: &Value) -> Result<(), String>;   // Claude Code + Cursor
fn write_codex_toml(path: &Path, exe: &Path, root: &Path) -> Result<(), String>;
fn backup(path: &Path) -> Result<(), String>;                              // <file>.bak
```

Binary path = `std::env::current_exe()?.canonicalize()?` — the running binary wires itself. Root = `root.canonicalize()`.

### Client config targets

| Client | Path | Scope | Format |
|---|---|---|---|
| Claude Code | `<root>/.mcp.json` | project | JSON, `mcpServers` object |
| Cursor | `<root>/.cursor/mcp.json` | project | JSON, `mcpServers` object |
| Codex CLI | `$CODEX_HOME/config.toml`, else `$HOME/.codex/config.toml` (Windows: `%USERPROFILE%\.codex\config.toml`) | user (no per-project config exists) | TOML, `[mcp_servers.<name>]` |

The scope asymmetry is forced: Codex has no project-scoped config. `install` must print which file it touched and at what scope.

JSON entry (Claude Code and Cursor share one shape):
```json
{"mcpServers":{"codekurve":{"command":"/abs/path/codekurve",
 "args":["mcp","--root","/abs/path/project"],"type":"stdio"}}}
```
**Must verify at implementation**: the repo's own docs (`docs/AGENT_USAGE.md:72`, `README.md`) currently emit `"transport": "stdio"`; current Claude Code uses `"type": "stdio"`. PR5 verifies against a live client and fixes both docs to match whatever it emits — the docs and the writer must never disagree.

Codex entry:
```toml
[mcp_servers.codekurve]
command = "/abs/path/codekurve"
args = ["mcp", "--root", "/abs/path/project"]
```

### Behavior rules
1. Missing file → create it (plus parent dir for `.cursor/`). No backup needed.
2. Existing file → `<file>.bak` copy **before** any write; overwrite an existing `.bak`.
3. Existing `codekurve` entry → overwrite it in place (idempotent re-run), leave sibling servers untouched.
4. Unparseable file, or `mcpServers`/`mcp_servers` present but not a table/object → **write nothing**, exit non-zero, print the exact JSON/TOML snippet for the user to paste manually. Same for an unknown `--client` value.
5. Bare `codekurve install` with no `--client` installs into every client whose config file or parent directory already exists, and reports each one. Never creates `~/.codex/` for a user who has no Codex.
6. No network, no subprocess — consistent with SECURITY_MODEL.md.

`crates/codekurve-bin/src/cli.rs`: add `pub client: Option<String>` + `"--client"` arm. `main.rs`: `"install" => install::run(&args.root, args.client.as_deref())`, plus `install` in the `USAGE` string.

## Doc closure (rides with its PR)
`docs/SECURITY_MODEL.md`: "max file size and total file count" becomes true at PR1; the checksum/SBOM sentences become true at PR4 → replace the "not yet implemented" update-process wording and add a Data Paths section (`.codekurve/index.db`, client config files, backups). `docs/LICENSING.md`: record public redistribution as deferred with the awaited decision.

## Testing Strategy

| PR | Layer | Test |
|---|---|---|
| 1 | Unit | `discovery.rs`: `at_limit_is_accepted` (N files, cap N) and `over_limit_errors` (N+1 files, cap N, asserts `TooManyFiles`). `config.rs`: default round-trip + `partial_config_fills_defaults` asserts the new default. |
| 1 | Integration | `crates/codekurve-bin/tests/`: `codekurve index` over an over-limit tempdir → exit 1, stderr contains `max_total_files`. |
| 2 | CI | `cargo deny check` green with every exception justified; local repro documented in the PR body. |
| 3 | Manual | `scripts/bench.py` twice on the same tier → medians within noise; numbers in PERFORMANCE.md match a pasted run log. |
| 4 | CI | One dispatched run produces all 4 binaries + SBOM + NOTICE + SHA256SUMS; `sha256sum -c` passes in-job. |
| 5 | Unit | Per client × {absent file, existing file with a foreign server, existing codekurve entry (idempotence), malformed file}. Assert: foreign servers preserved, `.bak` exists and holds the original bytes, malformed → `Err` **and file byte-identical**. |
| 5 | Integration | `assert_cmd`: `codekurve install --client claude-code --root <tmp>`, parse the resulting `.mcp.json`. |
| 5 | Manual | Real Claude Code handshake after install, recorded in the PR description (same bar Phase 4 PR4 used). |

## Threat Matrix

| Boundary | Applicability | Design response | RED test |
|---|---|---|---|
| Documentation-like paths | N/A — no file is classified as executable or executed; discovery only reads and parses. | — | — |
| Git repository selection | N/A — no `git` invocation anywhere in this phase. | — | — |
| Commit state | N/A — nothing stages or commits. | — | — |
| Push state | N/A — release workflow has no publish/push step by design. | — | — |
| PR commands | N/A — no PR automation. | — | — |

Supplementary boundary not covered by the canonical rows — **third-party config file mutation (PR5)**:

| Case | Expected behavior | RED test |
|---|---|---|
| Existing config with other MCP servers | Siblings preserved byte-for-byte in value | `preserves_foreign_servers` |
| Malformed JSON/TOML | No write, non-zero exit, manual snippet printed | `malformed_config_is_not_written` |
| `mcpServers` present but wrong type | Same as malformed | `wrong_shape_is_rejected` |
| Repeat invocation | Idempotent; entry updated, not duplicated | `install_twice_is_idempotent` |
| Any rewrite | `.bak` written first, holds pre-write bytes | `backup_precedes_write` |
| `$CODEX_HOME` set | Honored over `$HOME/.codex` | `codex_home_override` |

## Migration / Rollout

No data migration; no schema change; no index invalidation. `max_total_files` is additive config with a default.

## Rollback Boundaries

| PR | Revert action | Residual state |
|---|---|---|
| 1 | `git revert`; field, error variant, and the 3 call sites disappear together (one commit, must not be split) | None — no persisted data references the limit |
| 2 | Delete the `audit` job + `deny.toml`/`about.toml`/`about.hbs`; restore the ci.yml deferral comment | None; fmt/clippy/test/licensing-check unaffected |
| 3 | Docs-only revert of PERFORMANCE.md; `scripts/*bench*` and `fixtures/bench/` are additive and inert | `fixtures/bench/` on a dev machine — gitignored, deletable |
| 4 | Delete `.github/workflows/release.yml` | Prior workflow-run artifacts expire on GitHub's retention; nothing published anywhere |
| 5 | Revert `install.rs` + cli/main/Cargo.toml lines; manual `.mcp.json` editing remains the documented path | Client configs already written stay written — user rolls back with the `.bak` file, which is why rule 2 exists |

## Open Questions

- [x] Claude Code stdio key: `"type"` vs `"transport"` — PR5 confirmed against a live client and aligned docs. Resolved.
- [x] `[bans] multiple-versions = "warn"` vs `"deny"` — settled after PR2's first real run to `"warn"`.
