//! Domain core: shared types with no dependency on CLI, storage, or MCP.

pub mod config;
pub mod error;
pub mod language;
pub mod project;
pub mod symbol;

pub use config::Config;
pub use error::{Error, Result};
pub use language::LanguageId;
pub use symbol::{Confidence, Provenance, RelationshipKind, SourceSpan, Symbol, SymbolKind};
