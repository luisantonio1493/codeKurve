//! [`CodeKurve`] — the `rmcp` [`ServerHandler`]. The tool bodies live in
//! [`crate::tools`]; this module only wires `get_info` and the
//! `#[tool_handler]` glue rmcp's macros need (design "Server Bootstrap").

use std::sync::Mutex;

use codekurve::query::Session;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool_handler, ErrorData as McpError, RoleServer, ServerHandler};

/// The one tool `codekurve_reindex` gates on (spec "reindex Gated Off by
/// Default") — `#[tool_router]` in `tools.rs` registers it unconditionally
/// (a compile-time list); the runtime gate lives entirely in this module's
/// `list_tools`/`call_tool` overrides below.
const REINDEX_TOOL_NAME: &str = "codekurve_reindex";

/// One project root's session plus the reindex gate (design "Concurrency"):
/// `std::sync::Mutex`, never held across an `.await` — every tool body locks
/// it, calls a sync `query::*` function, and drops the guard before
/// returning.
pub struct CodeKurve {
    pub(crate) session: Mutex<Session>,
    pub(crate) allow_reindex: bool,
}

#[tool_handler]
impl ServerHandler for CodeKurve {
    /// Overrides `#[tool_handler]`'s generated `list_tools` (task 6.4): same
    /// full list `tools::CodeKurve::tool_router()` builds, minus
    /// `codekurve_reindex` when `allow_reindex` is off.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::tool_router().list_all();
        if !self.allow_reindex {
            tools.retain(|tool| tool.name != REINDEX_TOOL_NAME);
        }
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    /// Overrides `#[tool_handler]`'s generated `call_tool` (task 6.4): a
    /// disabled `codekurve_reindex` fails exactly like calling any other
    /// unregistered tool name (`METHOD_NOT_FOUND`), not a distinct
    /// "forbidden" shape — spec requires it fail "as an unknown tool".
    /// Every other tool name delegates unchanged to the generated dispatch.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if request.name == REINDEX_TOOL_NAME && !self.allow_reindex {
            return Err(McpError::method_not_found::<CallToolRequestMethod>());
        }
        let tcc = ToolCallContext::new(self, request, context);
        Self::tool_router().call(tcc).await
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "CodeKurve exposes the project's code graph over MCP. Query before broad \
                 exploration; see docs/AGENT_USAGE.md for the full agent usage rules.",
            )
    }
}
