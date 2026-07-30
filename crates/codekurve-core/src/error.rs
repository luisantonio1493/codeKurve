//! Typed error model for the domain core. See CODEKURVE_MASTER_PLAN.md §56
//! ("use typed errors"). Intentionally minimal; grows with each vertical slice.

use std::path::PathBuf;

use thiserror::Error;

/// Convenience alias used across the core crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors surfaced by core operations.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse config: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error("failed to serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("project already initialized: {}", .0.display())]
    AlreadyInitialized(PathBuf),

    #[error("path does not exist: {}", .0.display())]
    PathNotFound(PathBuf),

    #[error("parse error: {0}")]
    Parse(String),

    #[error(
        "project exceeds index.max_total_files ({limit}); raise the limit in .codekurve/config.toml or narrow index.languages / ignore.patterns"
    )]
    TooManyFiles { limit: usize },
}
