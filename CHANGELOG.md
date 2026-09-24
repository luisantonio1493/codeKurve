# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Fixed

- Apply `[ignore] patterns` during discovery. The patterns were parsed from
  `.codekurve/config.toml` but never used, so the default exclusions
  (`node_modules`, `dist`, `build`, `*.min.js`, `.env`, `secrets.*`, keys)
  only took effect when `.gitignore` also listed them. Files an existing index
  already holds that now match a pattern are removed on the next `index`. A
  pattern starting with `!` is rejected with a clear error instead of being
  silently ignored.

- MCP server: a tool that panicked never answered (the client waited
  forever), and it poisoned the session lock, so every later tool call
  panicked and never answered either. A panic now returns an `internal_error`
  response, and the next call reopens the session from disk.
- MCP server: tool bodies (SQLite queries, graph traversal, `reindex`) ran on
  the server's only runtime thread, so a slow call stopped it from reading
  any other message, pings and cancellations included. They now run on
  tokio's blocking pool. Tool calls are still serialized on the one session.

### Security

- `install.sh` and `install.ps1` (and so `codekurve update`) now verify the
  downloaded binary against the release's `SHA256SUMS` and refuse to install
  on a mismatch or a missing entry. Before this they installed whatever the
  download returned.
- Release workflow signs SLSA build provenance for every released file
  (`gh attestation verify`).
- Every GitHub Action in CI and Release is pinned to a commit SHA, and
  workflow tokens are read-only except the release `publish` job.

### Added

- Initialize Rust workspace (5 crates)
- Add governance documentation
- Add architecture decision records (0001-0010)
- Add cross-platform CI quality gates and licensing check
- Add `codekurve version` command
- Add typed error model and project configuration (`.codekurve/config.toml`)
- Add `codekurve init` command
- Add TypeScript/JavaScript file discovery honoring `.gitignore`
- Add Tree-sitter symbol extraction for classes and top-level functions
- Add SQLite storage with FTS5 (schema migration 0001) and symbol queries
- Add `index`, `search`, `symbol`, and `doctor` commands with live snippets
