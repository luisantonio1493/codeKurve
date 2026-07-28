//! Tool bodies (design "Server Bootstrap"): one tool for this PR,
//! `codekurve_project_status`. Each body locks [`CodeKurve::session`], calls
//! a sync `query::*` function, and drops the guard before returning — no
//! `.await` while the lock is held (design "Concurrency").

use codekurve::query;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{schemars, tool, tool_router, ErrorData as McpError};

use crate::server::CodeKurve;

/// `codekurve_project_status` takes no arguments (§28.2); the schema exists
/// so `tools/list` advertises an (empty) input shape rather than omitting
/// one.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectStatusInput {}

#[tool_router(vis = "pub(crate)")]
impl CodeKurve {
    #[tool(description = "Report index freshness, counts, and staleness for the current project")]
    fn codekurve_project_status(
        &self,
        Parameters(ProjectStatusInput {}): Parameters<ProjectStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session.lock().unwrap();
        let data =
            query::status(&session).map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        drop(session);

        let result = serde_json::json!({
            "schema_version": data.schema_version,
            "files": data.files,
            "symbols": data.symbols,
            "relationships": data.relationships,
            "relationships_unresolved": data.relationships_unresolved,
            "pending_files": data.pending_files,
            "last_verified_at": data.last_verified_at,
            "stale": data.stale,
        });
        let envelope = query::envelope(&project, result, warnings, false, None);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }
}
