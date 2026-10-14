# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Fixed

- Index: a directory the walk cannot read, or a file that cannot be read during a full reindex, is no longer treated as a deletion (the existing index is kept and the failure reported)
- Index: non-UTF-8 files (e.g. Latin-1 C#) are indexed instead of failing the batch
- Index: touch-only edits refresh the stored mtime; bytes and mtime are read as a consistent pair
- Index: changing config that affects analysis (ignore patterns, languages, symlinks, tsconfig aliases) triggers one full reindex; overlapping tsconfig aliases resolve by longest prefix
- Index: creating a module re-resolves files that already import it; an unreadable dependent no longer aborts the whole batch
- Watch: the watcher is registered before the startup reconcile; watcher errors and rescan requests trigger a full sweep; failed batches are retried with bounded backoff
- Analysis: `.tsx` files are parsed with the TSX grammar
- Analysis: by-name matches with more than 8 candidates are recorded as unresolved ("ambiguous: N candidates") instead of one low-confidence edge each

### Security

- `install`: write client configs and `.bak` backups atomically and refuse symlinks
- Enforce `ignore.patterns` during discovery (secrets, `.env`, minified, `dist`)
- Contain `get_symbol` source reads to the project root; cap snippet size, `ctx_lines` and search `limit`
- Scope symbol lookup by id to the project
- Cap AST recursion depth and set a parse timeout so deeply nested files cannot crash indexing
- Create the index database and project config owner-only on Unix
- Reject `storage.database` outside the project root; warn when a project config enables `mcp.allow_reindex` or `index.follow_symlinks`
- Match route-search text literally (escape `LIKE` wildcards); recover from a poisoned MCP session lock

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
