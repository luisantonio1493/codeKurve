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

/// Keeps the query planner's statistics (`sqlite_stat1`) current; call after
/// an index write commits. Without statistics SQLite assumes `project_id` is
/// selective, but a database holds a single project, so it picked
/// `(project_id, kind)` over `target_symbol_id` for relationship lookups and
/// scanned every symbol for FTS search: both linear in project size.
///
/// A database that has never been analyzed gets a full `ANALYZE` (tens of
/// milliseconds per 10k files); after that `PRAGMA optimize` re-analyzes
/// only the tables whose size drifted enough to matter, usually nothing.
pub fn refresh_planner_stats(conn: &Connection) -> Result<()> {
    let analyzed: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'sqlite_stat1')",
        [],
        |row| row.get(0),
    )?;
    conn.execute_batch(if analyzed {
        "PRAGMA optimize;"
    } else {
        "ANALYZE;"
    })?;
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
