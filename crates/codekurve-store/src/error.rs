//! Storage errors. Kept in the store crate so `codekurve-core` stays free of a
//! SQLite dependency (CODEKURVE_MASTER_PLAN.md §11.2).

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(String),
}
