//! Numbered, forward-only schema migrations (CODEKURVE_MASTER_PLAN.md §24.5).
//! Migration 0001 creates only the tables the Phase 1 vertical slice needs;
//! relationships, diagnostics, and run tracking arrive in later phases.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Schema version applied by this build.
pub const SCHEMA_VERSION: i64 = 5;

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

/// Migration 0003 (Phase 3 design "Migration 0003"): additive
/// `files.content_hash`/`modified_ns` for the change-detection engine, plus
/// the new `index_state` freshness table. Also wipes every project-data
/// table: ids move from `DefaultHasher` to BLAKE3 and `symbol_key` gains a
/// `signature_fingerprint` component, so every previously stored id/key is
/// invalid — this forces the one honest full reindex under the new scheme
/// instead of leaving corrupt-but-queryable rows around.
const MIGRATION_0003: &str = r#"
ALTER TABLE files ADD COLUMN content_hash TEXT;
ALTER TABLE files ADD COLUMN modified_ns INTEGER;

CREATE TABLE index_state (
    project_id TEXT PRIMARY KEY,
    pending_files INTEGER NOT NULL DEFAULT 0,
    last_verified_at TEXT,
    updated_at TEXT NOT NULL
);

DELETE FROM relationships;
DELETE FROM unresolved_references;
DELETE FROM symbols_fts;
DELETE FROM symbols;
DELETE FROM files;
DELETE FROM projects;
"#;

/// Migration 0004 (Phase 5 PR1, design "Migration 0004"): additive
/// `visibility`/`is_partial`/`is_record` columns for the language-neutral
/// `Visibility` enum and the C# `partial`/`record` modifiers. Unlike 0003,
/// this is **not** a wipe: none of the three columns participates in
/// `symbol_key`, so no existing symbol id changes and a reindex is not
/// forced.
const MIGRATION_0004: &str = r#"
ALTER TABLE symbols ADD COLUMN visibility TEXT NOT NULL DEFAULT 'default';
ALTER TABLE symbols ADD COLUMN is_partial INTEGER NOT NULL DEFAULT 0;
ALTER TABLE symbols ADD COLUMN is_record INTEGER NOT NULL DEFAULT 0;
"#;

/// Migration 0005 (Phase 7 PR1, design "Model and Storage Changes"):
/// additive `symbols.roles` column for the comma-joined `FrameworkRole`
/// tag list. Like 0004, this is **not** a wipe: `roles` never participates
/// in `symbol_key`, so no existing symbol id changes and a reindex is not
/// forced.
const MIGRATION_0005: &str = r#"
ALTER TABLE symbols ADD COLUMN roles TEXT NOT NULL DEFAULT '';
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

    if current < 3 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATION_0003)
            .map_err(|e| Error::Migration(format!("0003: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (3, datetime('now'))",
            [],
        )?;
        tx.commit()?;
    }

    if current < 4 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATION_0004)
            .map_err(|e| Error::Migration(format!("0004: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (4, datetime('now'))",
            [],
        )?;
        tx.commit()?;
    }

    if current < 5 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATION_0005)
            .map_err(|e| Error::Migration(format!("0005: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (5, datetime('now'))",
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
    /// `SCHEMA_VERSION` (5), the pre-existing relationship-graph tables
    /// still exist, and Phase 3's `index_state` table (plus the
    /// `files.content_hash`/`modified_ns` columns) is present.
    #[test]
    fn fresh_database_reaches_schema_version_5() {
        let conn = db::open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), 5);

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
        assert!(table_exists("index_state"));

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

        let column_exists = |table: &str, column: &str| -> bool {
            conn.prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|c| c.ok())
                .any(|c| c == column)
        };
        assert!(column_exists("files", "content_hash"));
        assert!(column_exists("files", "modified_ns"));
        assert!(column_exists("symbols", "visibility"));
        assert!(column_exists("symbols", "is_partial"));
        assert!(column_exists("symbols", "is_record"));
        assert!(column_exists("symbols", "roles"));
    }

    /// Migrations must apply cleanly and idempotently on top of an
    /// already-migrated database (forward-only, per §24.5).
    #[test]
    fn migrations_apply_idempotently() {
        let conn = db::open_in_memory().unwrap();
        // Re-running apply() on an already-migrated connection must be a
        // no-op, not an error (idempotency).
        apply(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 5);
    }

    /// Phase 5 PR1 task 1.10 / symbol-index "Schema Migration 0004 ... Without
    /// Wiping Data", scenario "Migration applies to a populated index without
    /// wiping it": simulate a project indexed under `SCHEMA_VERSION = 3` (no
    /// `visibility`/`is_partial`/`is_record` columns), then apply migrations
    /// 0004+0005 (a single `apply()` call always drains every pending
    /// version) and assert every row and every `symbol_key`/`id` survives,
    /// with the new columns present at their documented defaults.
    #[test]
    fn migration_0004_applies_without_wiping_populated_v3_data() {
        let conn = db::open_in_memory().unwrap();
        // Roll the schema_migrations ledger back to "as if only 0001-0003
        // had run" by dropping the three PR1 columns and downgrading the
        // recorded version — apply() then re-applies 0004 on real data.
        conn.execute_batch(
            "ALTER TABLE symbols DROP COLUMN visibility;
             ALTER TABLE symbols DROP COLUMN is_partial;
             ALTER TABLE symbols DROP COLUMN is_record;
             ALTER TABLE symbols DROP COLUMN roles;
             DELETE FROM schema_migrations WHERE version IN (4, 5);",
        )
        .unwrap();
        assert_eq!(current_version(&conn).unwrap(), 3);

        conn.execute(
            "INSERT INTO projects(id, name, root_path, config_hash, created_at, updated_at)
             VALUES ('prj-1', 'demo', '/tmp/demo', 'hash', '0', '0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files(id, project_id, relative_path, language, size_bytes,
                 parse_status, generation, created_at, updated_at)
             VALUES ('fil-1', 'prj-1', 'src/a.ts', 'typescript', 10, 'ok', 1, '0', '0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols(id, project_id, file_id, symbol_key, name, qualified_name,
                 kind, language, start_byte, end_byte, start_line, start_column, end_line,
                 end_column, provenance, confidence, generation, created_at, updated_at)
             VALUES ('sym-1', 'prj-1', 'fil-1', 'preexisting-key', 'Foo', 'src/a.ts::Foo',
                 'class', 'typescript', 0, 10, 1, 0, 1, 10, 'tree-sitter', 'high', 1, '0', '0')",
            [],
        )
        .unwrap();

        let symbol_count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();

        apply(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 5);

        let symbol_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(symbol_count_before, symbol_count_after);

        let (id, key, visibility, is_partial, is_record, roles): (
            String,
            String,
            String,
            i64,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT id, symbol_key, visibility, is_partial, is_record, roles
                 FROM symbols WHERE id = 'sym-1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(id, "sym-1");
        assert_eq!(key, "preexisting-key", "pre-migration symbol_key unchanged");
        assert_eq!(visibility, "default");
        assert_eq!(is_partial, 0);
        assert_eq!(is_record, 0);
        assert_eq!(roles, "");
    }

    /// Phase 7 PR1 task 1.8 / symbol-index "Schema Migration 0005 Adds
    /// Role-Tag Storage Without Wiping Data": simulate a project indexed
    /// under `SCHEMA_VERSION = 4` (no `roles` column), then apply migration
    /// 0005 and assert every row and every `symbol_key`/`id` survives, with
    /// `roles` present and `''` for the pre-migration row.
    #[test]
    fn migration_0005_applies_without_wiping_populated_v4_data() {
        let conn = db::open_in_memory().unwrap();
        // Roll the ledger back to "as if only 0001-0004 had run" by dropping
        // the `roles` column and downgrading the recorded version.
        conn.execute_batch(
            "ALTER TABLE symbols DROP COLUMN roles;
             DELETE FROM schema_migrations WHERE version = 5;",
        )
        .unwrap();
        assert_eq!(current_version(&conn).unwrap(), 4);

        conn.execute(
            "INSERT INTO projects(id, name, root_path, config_hash, created_at, updated_at)
             VALUES ('prj-1', 'demo', '/tmp/demo', 'hash', '0', '0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files(id, project_id, relative_path, language, size_bytes,
                 parse_status, generation, created_at, updated_at)
             VALUES ('fil-1', 'prj-1', 'src/a.ts', 'typescript', 10, 'ok', 1, '0', '0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols(id, project_id, file_id, symbol_key, name, qualified_name,
                 kind, language, start_byte, end_byte, start_line, start_column, end_line,
                 end_column, provenance, confidence, visibility, is_partial, is_record,
                 generation, created_at, updated_at)
             VALUES ('sym-1', 'prj-1', 'fil-1', 'preexisting-key', 'Foo', 'src/a.ts::Foo',
                 'class', 'typescript', 0, 10, 1, 0, 1, 10, 'tree-sitter', 'high', 'default',
                 0, 0, 1, '0', '0')",
            [],
        )
        .unwrap();

        let symbol_count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();

        apply(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 5);

        let symbol_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(symbol_count_before, symbol_count_after);

        let (id, key, roles): (String, String, String) = conn
            .query_row(
                "SELECT id, symbol_key, roles FROM symbols WHERE id = 'sym-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, "sym-1");
        assert_eq!(key, "preexisting-key", "pre-migration symbol_key unchanged");
        assert_eq!(roles, "");
    }
}
