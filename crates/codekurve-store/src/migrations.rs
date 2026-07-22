//! Numbered, forward-only schema migrations (CODEKURVE_MASTER_PLAN.md §24.5).
//! Migration 0001 creates only the tables the Phase 1 vertical slice needs;
//! relationships, diagnostics, and run tracking arrive in later phases.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Schema version applied by this build.
pub const SCHEMA_VERSION: i64 = 1;

const MIGRATION_0001: &str = r#"
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    config_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE files (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    language TEXT,
    size_bytes INTEGER NOT NULL,
    parse_status TEXT NOT NULL,
    parse_error TEXT,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, relative_path),
    FOREIGN KEY(project_id) REFERENCES projects(id)
);

CREATE TABLE symbols (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    symbol_key TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    language TEXT NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    provenance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    is_exported INTEGER NOT NULL DEFAULT 0,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, symbol_key),
    FOREIGN KEY(file_id) REFERENCES files(id)
);

CREATE INDEX idx_symbols_project_name ON symbols(project_id, name);
CREATE INDEX idx_symbols_project_qname ON symbols(project_id, qualified_name);
CREATE INDEX idx_symbols_file ON symbols(file_id);
CREATE INDEX idx_files_project_path ON files(project_id, relative_path);

CREATE VIRTUAL TABLE symbols_fts USING fts5(
    symbol_id UNINDEXED,
    name,
    qualified_name,
    kind,
    relative_path
);
"#;

/// Apply all pending migrations. Idempotent: already-applied versions are
/// skipped.
pub fn apply(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if current < 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATION_0001)
            .map_err(|e| Error::Migration(format!("0001: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, datetime('now'))",
            [],
        )?;
        tx.commit()?;
    }

    Ok(())
}
