# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

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
