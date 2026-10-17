# Security Model

Full source: plan §29.

## Threat model

Indexing secrets, escaping the project root, symlink/path traversal,
oversized or malformed files, malicious grammars, excessive CPU/RAM,
database corruption, log injection, oversized MCP output, compromised
dependencies, accidental execution of repository code, and reading
sensitive generated artifacts (plan §29.1).

## Controls

Never execute analyzed code, package-manager scripts, or repository
config; never shell out to `npm`/`dotnet`/`cargo`; canonicalize paths;
symlinks off by default; **no network**; dependency audit and SBOM; release
checksums (plan §29.2).

Resource limits, as implemented:

- **Input size**: `index.max_file_size_bytes` (default 2 MiB per file) and
  `index.max_total_files` (hard failure when exceeded); `[ignore] patterns`
  plus `.gitignore`.
- **Syntax depth**: extractors recurse per syntax-tree level, and a stack
  overflow aborts the process, so a crafted file could kill `index`,
  `watch` or the MCP server (~20 KB of nested `[` did). Files deeper than
  `MAX_SYNTAX_DEPTH` (10,000 levels) are skipped with a warning, and
  extraction runs on a thread with a 256 MiB stack reservation sized for
  that limit (`codekurve_analysis::extract::on_analysis_stack`).
- **Graph queries**: every traversal is capped by depth, node, edge and
  time (5 s) budgets and reports truncation.

Not implemented, so not claimed: a parse timeout (tree-sitter parses of
files within the size cap finish in seconds even when pathological, e.g.
~2.5 s for 1M nesting levels), an explicit memory budget, and structured
logging with content redaction (output is plain text; nothing logs file
contents).

## No-network

The application does not depend on an HTTP client. See
`docs/ARCHITECTURE.md` and plan §29.4.

## One scoped carve-out: `update` / `uninstall --binary`

`docs/adr/0012-update-via-install-script.md` scopes — does not supersede —
both the "never shell out" control above and ADR 0005. `codekurve update` and
the opt-in `codekurve uninstall --binary` spawn the published install script
(`install.sh` / `install.ps1`), which performs the download or removal.
CodeKurve gains no HTTP client and makes no network call from Rust; no
analysis path (`index`, `watch`, `mcp`, `tui`, any query) spawns a subprocess
or touches the network. Both paths are reachable only by the user typing the
command, print the exact command first, require `[y/N]` confirmation, and
refuse on a non-terminal stdin unless `--yes` is passed. ADR 0012 states the
supply-chain cost plainly.

## Ignored files

Respects `.gitignore` plus configurable exclusions; sensitive-file patterns
(`.env`, `*.pem`, `*.key`, `credentials*`, ...) are excluded by default but
the policy is configurable, not hardcoded (plan §29.3). `ignore.patterns`
is enforced by discovery for full, incremental and watcher indexing; an
invalid glob is a hard error.

## Untrusted project config

A cloned repository ships its own `.codekurve/config.toml`, so it is treated
as untrusted input:

- `storage.database` must be a relative path inside the project root;
  absolute paths and `..` are rejected.
- `mcp.allow_reindex` and `index.follow_symlinks` are honored, but `mcp`,
  `index` and `watch` print a warning on stderr when the project's own config
  enables either.

## Source reads

`get_symbol` resolves the symbol's file under the project root and refuses
absolute paths, `..`, and symlinked files or directories. Files above
`index.max_file_size_bytes` are not read, `ctx_lines` is capped at 200, and
the returned snippet is cut at `queries.max_snippet_bytes` on a character
boundary (`source_truncated: true`). Symbol lookup by id is scoped to the
project, and the MCP search `limit` is clamped to `queries.max_limit`.

## Storage

SQLite is local, no source-code duplication beyond what's required (plan
§0.8, §24).

## Data paths

- `.codekurve/index.db` — SQLite index, local to the project root, never
  transmitted over the network.
- Client MCP config files written by `codekurve install`: Claude Code
  (`<root>/.mcp.json`, project scope), Cursor (`<root>/.cursor/mcp.json`,
  project scope), Codex CLI (`$CODEX_HOME/config.toml` or
  `$HOME/.codex/config.toml`, user scope — Codex has no project-scoped
  config).
- Backups: `install` writes `<file>.bak` before any rewrite of an existing
  client config, letting a user roll back without git. Config and backup
  writes go through a temp file + rename, refuse symlinked targets, and keep
  the original file's permissions.
- On Unix the index database is created `0600`, `.codekurve/` `0700` and
  `config.toml` `0600`.

## Update process

No auto-update. Dependency and license audit runs in CI via `cargo-deny`
and `cargo-about` (`.github/workflows/ci.yml`, `deny.toml`, `about.toml`).
Tagged releases build a CycloneDX SBOM, a NOTICE report, and a `SHA256SUMS`
checksum file covering every platform binary
(`.github/workflows/release.yml`), and publish them as a public GitHub
Release (`gh release create`, `publish` job).

- **Checksum verification**: `install.sh` and `install.ps1` (and therefore
  `codekurve update`) download `SHA256SUMS` from the same release and refuse
  to install a binary whose SHA-256 does not match. This detects corrupted,
  truncated, or swapped downloads. It does **not** detect a compromised
  release, since whoever can replace the binary can replace `SHA256SUMS` too.
- **Build provenance**: releases after v0.2.11 carry a signed SLSA provenance
  attestation for every file in `SHA256SUMS`, tying it to this repository's
  release workflow and tag. Verify manually with
  `gh attestation verify <file> --repo luisantonio1493/codeKurve`; the
  installers do not check it.
- **CI supply chain**: every third-party GitHub Action is pinned to a commit
  SHA; workflow tokens are read-only except the `publish` job, which gets
  `contents`/`id-token`/`attestations` write.
- Not in place: binary code signing or notarization, verification of the
  install script itself (it is fetched from `main`).
