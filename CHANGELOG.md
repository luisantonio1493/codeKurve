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
