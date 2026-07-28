//! [`CodeKurve`] — the `rmcp` [`ServerHandler`]. The tool bodies live in
//! [`crate::tools`]; this module only wires `get_info` and the
//! `#[tool_handler]` glue rmcp's macros need (design "Server Bootstrap").

use std::sync::Mutex;

use codekurve::query::Session;
use rmcp::model::{ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{tool_handler, ServerHandler};

/// One project root's session plus the reindex gate (design "Concurrency"):
/// `std::sync::Mutex`, never held across an `.await` — every tool body locks
/// it, calls a sync `query::*` function, and drops the guard before
/// returning.
pub struct CodeKurve {
    pub(crate) session: Mutex<Session>,
    // ponytail: read by PR6's `codekurve_reindex` gating (task 6.4), unused
    // until that tool exists — this PR only wires `codekurve_project_status`.
    #[allow(dead_code)]
    pub(crate) allow_reindex: bool,
}

#[tool_handler]
impl ServerHandler for CodeKurve {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "CodeKurve exposes the project's code graph over MCP. Query before broad \
                 exploration; see docs/AGENT_USAGE.md for the full agent usage rules.",
            )
    }
}
