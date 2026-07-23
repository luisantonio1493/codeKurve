//! Numbered, forward-only schema migrations (CODEKURVE_MASTER_PLAN.md §24.5).
//! Migration 0001 creates only the tables the Phase 1 vertical slice needs;
//! relationships, diagnostics, and run tracking arrive in later phases.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Schema version applied by this build.
pub const SCHEMA_VERSION: i64 = 2;

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

/// Migration 0002 (CODEKURVE_MASTER_PLAN.md §24.2/§24.4): the relationship
/// graph tables. Purely additive — no ALTER on any 0001 table; `Contains`
/// (parent/child) travels as a relationship row, not a `symbols` column
/// (design decision, keeps the index disposable and reindex-safe).
const MIGRATION_0002: &str = r#"
CREATE TABLE relationships (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    source_symbol_id TEXT NOT NULL,
    target_symbol_id TEXT,
    target_external TEXT,
    kind TEXT NOT NULL,
    provenance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    start_line INTEGER,
    start_column INTEGER,
    reason TEXT,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(source_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY(target_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY(source_file_id) REFERENCES files(id)
);

CREATE TABLE unresolved_references (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    source_symbol_id TEXT,
    source_file_id TEXT NOT NULL,
    relationship_kind TEXT NOT NULL,
    target_text TEXT NOT NULL,
    context_json TEXT,
    candidate_count INTEGER NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    confidence TEXT NOT NULL,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_relationships_source_kind ON relationships(source_symbol_id, kind);
CREATE INDEX idx_relationships_target_kind ON relationships(target_symbol_id, kind);
CREATE INDEX idx_relationships_project_kind ON relationships(project_id, kind);
CREATE INDEX idx_unresolved_project_target ON unresolved_references(project_id, target_text);
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

    if current < 2 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATION_0002)
            .map_err(|e| Error::Migration(format!("0002: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, datetime('now'))",
            [],
        )?;
        tx.commit()?;
    }

    Ok(())
}

/// The schema version currently applied to `conn` (0 if migrations never
/// ran). Surfaced by `codekurve doctor` (§24.5).
pub fn current_version(conn: &Connection) -> Result<i64> {
    let version = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Spec scenario "Fresh database migration": a fresh DB ends at
    /// `SCHEMA_VERSION` and both relationship-graph tables exist.
    #[test]
    fn fresh_database_reaches_schema_version_2() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), 2);

        let table_exists = |name: &str| -> bool {
            conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .is_ok()
        };
        assert!(table_exists("relationships"));
        assert!(table_exists("unresolved_references"));

        let index_exists = |name: &str| -> bool {
            conn.query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .is_ok()
        };
        assert!(index_exists("idx_relationships_source_kind"));
        assert!(index_exists("idx_relationships_target_kind"));
        assert!(index_exists("idx_relationships_project_kind"));
        assert!(index_exists("idx_unresolved_project_target"));
    }

    /// Migration 0002 must apply cleanly on top of an already-migrated 0001
    /// database (forward-only, idempotent per §24.5).
    #[test]
    fn migration_0002_applies_on_top_of_0001() {
        let conn = db::open_in_memory().unwrap();
        // Re-running apply() on an already-migrated connection must be a
        // no-op, not an error (idempotency).
        apply(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 2);
    }
}
