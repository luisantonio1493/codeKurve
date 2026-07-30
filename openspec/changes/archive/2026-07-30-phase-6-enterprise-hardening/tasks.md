# Tasks: Phase 6 — Enterprise Hardening

## Phase 1 through Phase 5b — ALL COMPLETE

See prior iterations for unmodified complete phases. Phase 1 PR1, Phase 2 PR2, Phase 3 PR3, Phase 4 PR4, Phase 5a (JSON writer), Phase 5b (Codex CLI TOML writer + fan-out + doc closure) — all checkboxes [x] below.

### Phase 1 — max_total_files
- [x] 1.1 Add `max_total_files: usize` field to `Index` struct in `crates/codekurve-core/src/config.rs` with default `50_000`.
- [x] 1.2 Add `TooManyFiles` variant to `Error` enum in `crates/codekurve-core/src/error.rs`.
- [x] 1.3 Modify `discover()` in `crates/codekurve-analysis/src/discovery.rs` to enforce the limit and return early.
- [x] 1.4 Update all call-sites of `discover()` to handle the new error: `crates/codekurve/src/incremental.rs` (2 sites) and `crates/codekurve/src/commands.rs`.
- [x] 1.5 Add unit tests: `at_limit_is_accepted`, `over_limit_errors`, `zero_disables_the_limit` in `discovery.rs`.
- [x] 1.6 Add integration test in `crates/codekurve-bin/tests/` verifying CLI error on over-limit tempdir.
- [x] 1.7 Add unit tests to `config.rs`: default value round-trip and partial config with the field omitted still parses to the default.
- [x] 1.8 Run `cargo test --workspace` — all tests pass, no new regressions.

### Phase 2 — cargo-deny + NOTICE
- [x] 2.1 Create `deny.toml` with `[advisories] yanked = "deny"`, explicit SPDX allow-list in `[licenses]`, `[bans] multiple-versions = "warn"`, `[sources] unknown-registry = "deny"`. Every exception carries a `# reason:` comment.
- [x] 2.2 Create `about.toml` + `about.hbs` template for third-party license/NOTICE generation.
- [x] 2.3 Modify `.github/workflows/ci.yml`: new job `audit` running `cargo-deny check` (SHA-pinned action) and `cargo-about generate`, delete the trailing "Deferred" comment line.
- [x] 2.4 Verify CI runs the new job successfully and cargo-deny check passes with justified exceptions.

### Phase 3 — Benchmarks
- [x] 3.1 Create `scripts/gen_bench_fixture.py` to generate deterministic synthetic fixture trees (100/1k/10k files) with realistic call/inherit density.
- [x] 3.2 Create `scripts/bench.py` to run `codekurve index` against each fixture tier and report cold/warm medians + p95.
- [x] 3.3 Regenerate fixtures locally and run benchmarks for all three tiers.
- [x] 3.4 Update `docs/PERFORMANCE.md`: replace aspirational targets with measured numbers, include Hardware/OS/storage block, add exact reproduction command. Keep Budgets table and mark rows met/missed.
- [x] 3.5 Verify gitignore includes `fixtures/bench/` (generated, not committed).

### Phase 4 — Release workflow
- [x] 4.1 Create `.github/workflows/release.yml` with matrix build (ubuntu-22.04 x64, macos-latest aarch64 + x64, windows-latest x64).
- [x] 4.2 Build steps: checkout → rust-toolchain → `cargo build --release` per target → rename → upload-artifact per binary.
- [x] 4.3 Job `sbom`: install `cargo-cyclonedx` → generate SBOM → upload.
- [x] 4.4 Job `licenses`: reuse `cargo-about` from PR2 → upload license report.
- [x] 4.5 Job `bundle`: download all artifacts → flatten → `sha256sum * > SHA256SUMS` → self-check `sha256sum -c` → upload full bundle. **No publish step anywhere**.
- [x] 4.6 Test dispatch run manually; verify all 4 binaries + SBOM + NOTICE + SHA256SUMS exist and checksums validate.

### Phase 5a — JSON writer (Claude Code + Cursor)
- [x] 5a.1 Create `crates/codekurve/src/install.rs` module with `Client` enum, config target discovery, JSON/TOML writers.
- [x] 5a.2 Add `toml_edit = "0.22"` to `crates/codekurve/Cargo.toml` (zero new dependency tree nodes; 0.22.27 already transitive).
- [x] 5a.3 Wire `install` subcommand into `crates/codekurve-bin/src/cli.rs` + `main.rs`. Add `--client` flag and positional argument plumbing.
- [x] 5a.4 Implement JSON writer for Claude Code and Cursor: read `.mcp.json`/`.cursor/mcp.json`, merge codekurve entry, back up before write, fail loudly on malformed JSON.
- [x] 5a.5 Unit tests: JSON absent file, JSON existing file with foreign servers, idempotent re-run, malformed JSON rejection (no write), foreign servers preserved, backup creation.
- [x] 5a.6 Integration test: `assert_cmd` on `codekurve install --client claude-code --root <tmp>`, parse resulting `.mcp.json`.
- [x] 5a.7 Verify with real Claude Code handshake (recorded in PR description).
- [x] 5a.8 Update `docs/AGENT_USAGE.md` with `codekurve install` instructions for Claude Code + Cursor. Update `README.md` quick-start.
- [x] 5a.9 Run `cargo test --workspace` — all tests pass, no regressions from PR5a work.

### Phase 5b — Codex CLI TOML writer + fan-out + doc closure
- [x] 5b.1 `Client::Codex` variant added to the existing enum in `crates/codekurve/src/install.rs` (PR5a's module, not rewritten).
- [x] 5b.2 `crates/codekurve/Cargo.toml`: added `toml_edit = "0.22"` — zero new dependency tree nodes (0.22.27 was already in `Cargo.lock` transitively via `toml`, confirmed unchanged after build).
- [x] 5b.3 Codex config path resolution verified against a live installed Codex CLI on this machine (`codex --version` → `codex-cli 0.146.0`): `$CODEX_HOME/config.toml` else `$HOME/.codex/config.toml` (`%USERPROFILE%\.codex\config.toml` on Windows), `codex_config_path()` fn. This machine's real `~/.codex/config.toml` already contains live `[mcp_servers.*]` tables (e.g. `[mcp_servers.engram]`) written by Codex itself with plain `command`/`args` keys, no `type`/`transport` key — ground truth, matches design's PR5 sketch exactly, no deviation needed (unlike PR5a's Cursor surprise).
- [x] 5b.4 `write_codex_toml` via `toml_edit::DocumentMut`: parses (or starts fresh), merges `[mcp_servers.codekurve]` without disturbing sibling tables, comments, or formatting elsewhere in the file; backs up before any rewrite (`config.toml.bak`); fails loudly with no write on unparseable TOML or `mcp_servers` present-but-not-a-table.
- [x] 5b.5 Client name for CLI: `"codex-cli"` (matches spec's literal scenario text `codekurve install codex-cli`, and PR5a's manual-instructions placeholder string that already said "codex-cli is not yet supported").
- [x] 5b.6 `run()` dispatches Codex through `codex_config_path()` (ignores `--root`-relative path) and prints `(user scope)` vs `(project scope)` for Claude Code/Cursor — surfaces the scope asymmetry per design.
- [x] 5b.7 `--client` flag / positional client arg (already wired in PR5a's `cli.rs`/`main.rs`) needed no changes — `Client::parse` now recognizes `"codex-cli"`, `main.rs`'s `"install"` arm and USAGE string are client-name-agnostic.
- [x] 5b.8 Tests (10 new, 16 total in `install.rs`): `codex_toml_created_fresh_no_backup`, `codex_toml_preserves_comments_and_foreign_servers`, `codex_toml_install_twice_is_idempotent`, `malformed_codex_toml_is_rejected_and_file_untouched`, `codex_toml_wrong_shape_mcp_servers_is_rejected`, `codex_home_override_is_honored`, `unsupported_client_message_lists_codex_cli` (existing 9 PR5a tests unchanged and still passing).
- [x] 5b.9 `docs/AGENT_USAGE.md`: Codex section renamed "Codex CLI", documents `codekurve install codex-cli`, user-scope path resolution, no `type` key — matches PR5a's pattern for Claude Code/Cursor.
- [x] 5b.10 `README.md`: quick-start line now lists all three clients (`claude-code` / `cursor` / `codex-cli`).
- [x] 5b.11 `docs/SECURITY_MODEL.md`: "Update process" section rewritten — removed stale "not yet implemented" wording for release checksums/SBOM (both true since PR4); added a new "Data paths" section listing `.codekurve/index.db`, all three client config paths, and the `.bak` backup mechanism.
- [x] 5b.12 `docs/LICENSING.md` reviewed — already correctly states "Status: pending", "Do not redistribute this repository" as the deferral note; no stale wording found, no change needed.
- [x] 5b.13 `cargo build --workspace` and `cargo test --workspace` both green: 0 failures across every crate (26 test binaries/suites, includes the 16-test `install.rs` unit suite, golden CLI/MCP suites, `codekurve-store` 29 tests, all unchanged and passing).
- [x] 5b.14 `cargo clippy -p codekurve --all-targets`: `install.rs` changes are clippy-clean; only the one pre-existing unrelated `commands.rs:1176` warning remains (noted in PR5a, untouched by this PR).

## SUMMARY

**ALL phase-6-enterprise-hardening tasks (PR1 through PR5b) are [x] COMPLETE.**

Implementation-complete date: 2026-07-30 18:30:58.
