//! Connection setup. Applies the PRAGMAs from CODEKURVE_MASTER_PLAN.md §24.1
//! and runs migrations on open.

use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;
use crate::migrations;

/// Open (or create) the index database at `path`, configure it, and migrate.
pub fn open(path: &Path) -> Result<Connection> {
    create_private_file(path)?;
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrations::apply(&conn)?;
    Ok(conn)
}

/// Create the database file owner-only (0600) before SQLite opens it, so the
/// index is never briefly world-readable. An existing file is left untouched;
/// `-wal`/`-shm` inherit the main file's mode.
#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

/// Open an in-memory database (tests and diagnostics).
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrations::apply(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    // Bulk indexing and BFS issue the same handful of statements thousands of
    // times; the default cache (16) is too small for the repo's query set.
    conn.set_prepared_statement_cache_capacity(64);
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -32768;
         PRAGMA temp_store = MEMORY;",
    )?;
    // No `mmap_size`: another process truncating a mapped database file
    // (e.g. a deleted and rebuilt index while `mcp`/`watch` runs) would
    // SIGBUS this one, and the page cache already covers the working set.
    Ok(())
}

/// Refuse every write on `conn` (`PRAGMA query_only`), for sessions that
/// must only read the index.
pub fn set_read_only(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(())
}

/// Run `PRAGMA optimize` so SQLite refreshes planner statistics for `conn`.
pub fn optimize(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA optimize;")?;
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

#[cfg(test)]
mod optimize_tests {
    use super::*;

    #[test]
    fn configure_enlarges_the_page_cache_and_keeps_temp_data_in_memory() {
        let conn = open_in_memory().unwrap();
        let pragma = |name: &str| -> i64 {
            conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(pragma("cache_size"), -32_768, "32 MiB page cache");
        assert_eq!(pragma("temp_store"), 2, "MEMORY");
    }

    #[test]
    fn optimize_succeeds_on_migrated_connection() {
        let conn = open_in_memory().unwrap();
        optimize(&conn).unwrap();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn open_creates_database_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");

        let conn = open(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600);

        // SQLite creates -wal/-shm with the main file's mode on Unix.
        for suffix in ["-wal", "-shm"] {
            let sidecar = dir.path().join(format!("index.db{suffix}"));
            if sidecar.exists() {
                assert_eq!(mode_of(&sidecar), 0o600, "{suffix} file mode");
            }
        }
        drop(conn);
    }
}
