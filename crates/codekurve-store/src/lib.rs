//! Persistence layer: single-writer SQLite storage for the analysis graph.

pub mod db;
pub mod error;
pub mod migrations;
pub mod repo;
pub mod traverse;

pub use error::{Error, Result};
pub use repo::{FileInput, IndexOutcome, StoredSymbol};
pub use rusqlite::Connection;
