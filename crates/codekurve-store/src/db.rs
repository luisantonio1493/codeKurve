//! Connection setup. Applies the PRAGMAs from CODEKURVE_MASTER_PLAN.md §24.1
//! and runs migrations on open.

use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;
use crate::migrations;

/// Open (or create) the index database at `path`, configure it, and migrate.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::apply(&conn)?;
    Ok(conn)
}

/// Open an in-memory database (tests and diagnostics).
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::apply(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

/// Whether this SQLite build supports FTS5 (surfaced by `doctor`, §24.1).
pub fn has_fts5(conn: &Connection) -> bool {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE temp.__fts5_probe USING fts5(x);
         DROP TABLE temp.__fts5_probe;",
    )
    .is_ok()
}
