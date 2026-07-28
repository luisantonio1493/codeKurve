//! CodeKurve library target — module-declaration move only (no code moved).
//!
//! Exists so `crates/codekurve-mcp` can depend on the same command/query
//! logic the CLI binary uses, without a second copy. `main.rs` consumes
//! these modules instead of declaring them locally.

pub mod commands;
pub mod incremental;
pub mod query;
pub mod watch;
