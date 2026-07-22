//! Domain core: shared types with no dependency on CLI, storage, or MCP.

pub mod config;
pub mod error;
pub mod project;

pub use config::Config;
pub use error::{Error, Result};
