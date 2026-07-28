//! Tool bodies (design "Server Bootstrap"). PR4 landed one tool,
//! `codekurve_project_status`; PR5 adds the remaining eight read tools
//! (§28.2 tool registry, minus `project_overview`/`doctor`/`reindex`, which
//! are PR6 scope). Each body locks [`CodeKurve::session`], calls sync
//! `query::*` functions, and drops the guard before returning — no
//! `.await` while the lock is held (design "Concurrency").

use codekurve::commands::QueryArgs;
use codekurve::query::{self, RelKind, SearchInput};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{schemars, tool, tool_router, ErrorData as McpError};

use crate::server::CodeKurve;

/// `codekurve_project_status` takes no arguments (§28.2); the schema exists
/// so `tools/list` advertises an (empty) input shape rather than omitting
/// one.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectStatusInput {}

/// `search_symbols` (§28.2): `kinds`/`languages`/`path_prefix` are in the
/// schema so clients can discover the shape, but any non-`None` value is
/// rejected (spec "search_symbols Tool Rejects Unsupported Filters",
/// confirmed decision 3) — `repo::search` has no SQL predicate for them yet.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchSymbolsInput {
    pub query: String,
    pub kinds: Option<Vec<String>>,
    pub languages: Option<Vec<String>>,
    pub path_prefix: Option<String>,
    pub limit: Option<u32>,
}

/// `get_symbol` (§28.2): `ctx_lines` extra lines of context on each side of
/// the symbol's span; `include_source` (default `true`) lets a client skip
/// the disk read/staleness check entirely when it only wants the symbol's
/// location.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSymbolInput {
    pub id: String,
    pub ctx_lines: Option<u32>,
    pub include_source: Option<bool>,
}

/// Shared input shape for the four flat relationship tools
/// (`find_references`/`find_callers`/`find_callees`/`find_implementations`,
/// §28.2) — mirrors the CLI's `--symbol-id`/`--symbol-name`/
/// `--min-confidence`/`--limit`/`--offset` flags.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RelationshipInput {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub min_confidence: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// `trace_path` (§28.2): same subject resolution as [`RelationshipInput`]
/// plus the CLI's positional `to` target and `--depth`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TracePathInput {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub to: String,
    pub min_confidence: Option<String>,
    pub depth: Option<u32>,
}

/// `analyze_impact` (§28.2): same subject resolution as [`RelationshipInput`]
/// plus `--depth`; no `to` (reverse BFS explores everything reachable, no
/// single target).
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyzeImpactInput {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub min_confidence: Option<String>,
    pub depth: Option<u32>,
}

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

    #[tool(
        description = "Full-text search over symbol name/qualified name/kind/path (query, limit only — kinds/languages/path_prefix are rejected, not yet supported)"
    )]
    fn codekurve_search_symbols(
        &self,
        Parameters(input): Parameters<SearchSymbolsInput>,
    ) -> Result<CallToolResult, McpError> {
        reject_unsupported_search_filters(&input)?;

        let session = self.session.lock().unwrap();
        let search_input = SearchInput {
            query: &input.query,
            limit: input.limit,
        };
        let page = query::search(&session, &search_input)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|sym| {
                serde_json::json!({
                    "id": sym.id,
                    "name": sym.name,
                    "qualified_name": sym.qualified_name,
                    "kind": sym.kind,
                    "language": sym.language,
                    "path": sym.relative_path,
                    "start_line": sym.span.start_line,
                    "end_line": sym.span.end_line,
                    // ponytail: `reindex` hardcodes provenance/confidence at
                    // write time (codekurve-store::repo::insert_file) — no
                    // per-symbol variance yet to read back, so these mirror
                    // that constant rather than a stored column.
                    "confidence": "high",
                    "provenance": "tree-sitter",
                })
            })
            .collect();
        drop(session);

        let envelope = query::envelope(
            &project,
            serde_json::Value::Array(rows),
            warnings,
            page.truncated,
            Some(page.total),
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(
        description = "Resolve one symbol by id; reads the current source from disk on every call and flags drift from the indexed span"
    )]
    fn codekurve_get_symbol(
        &self,
        Parameters(input): Parameters<GetSymbolInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session.lock().unwrap();
        let ctx_lines = input.ctx_lines.unwrap_or(0);
        let include_source = input.include_source.unwrap_or(true);
        let detail = query::get_symbol(&session, &input.id, ctx_lines)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let sym = &detail.symbol;

        let (source, stale, reason) = if include_source {
            let path = session.root().join(&sym.relative_path);
            // Session::warnings() for an Indexed session is only ever the
            // pending-files wording (see query::pending_warning) — non-empty
            // here means index_pending > 0, the exact bit source_slice
            // needs, without a second `repo::index_status` round-trip.
            let index_pending = !warnings.is_empty();
            let slice = query::source_slice(&path, &sym.span, ctx_lines, index_pending);
            (slice.source, slice.stale, slice.reason)
        } else {
            (None, false, None)
        };

        let row = serde_json::json!({
            "id": sym.id,
            "name": sym.name,
            "qualified_name": sym.qualified_name,
            "kind": sym.kind,
            "language": sym.language,
            "path": sym.relative_path,
            "start_line": sym.span.start_line,
            "end_line": sym.span.end_line,
            "confidence": "high",
            "provenance": "tree-sitter",
            "source": source,
            // Distinct from the project-level stale warning (spec
            // "get_symbol Reads Live Source and Flags Drift"): this flags
            // drift for *this* symbol's own span/file.
            "stale": stale,
            "stale_reason": reason,
        });
        drop(session);

        let envelope = query::envelope(&project, row, warnings, false, Some(1));
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(description = "Find every relationship that references a symbol")]
    fn codekurve_find_references(
        &self,
        Parameters(input): Parameters<RelationshipInput>,
    ) -> Result<CallToolResult, McpError> {
        self.relationship_result(input, RelKind::References)
    }

    #[tool(description = "Find call sites that call a symbol")]
    fn codekurve_find_callers(
        &self,
        Parameters(input): Parameters<RelationshipInput>,
    ) -> Result<CallToolResult, McpError> {
        self.relationship_result(input, RelKind::Callers)
    }

    #[tool(description = "Find calls made by a symbol")]
    fn codekurve_find_callees(
        &self,
        Parameters(input): Parameters<RelationshipInput>,
    ) -> Result<CallToolResult, McpError> {
        self.relationship_result(input, RelKind::Callees)
    }

    #[tool(description = "Find symbols that implement or inherit/extend a symbol")]
    fn codekurve_find_implementations(
        &self,
        Parameters(input): Parameters<RelationshipInput>,
    ) -> Result<CallToolResult, McpError> {
        self.relationship_result(input, RelKind::Implementations)
    }

    #[tool(description = "Bounded forward path from one symbol to another")]
    fn codekurve_trace_path(
        &self,
        Parameters(input): Parameters<TracePathInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session.lock().unwrap();
        let args = QueryArgs {
            root: session.root(),
            symbol_id: input.symbol_id.as_deref(),
            symbol_name: input.symbol_name.as_deref(),
            min_confidence: input.min_confidence.as_deref(),
            depth: input.depth,
            limit: None,
            offset: None,
            json: false,
        };
        let outcome = query::trace(&session, &args, &input.to)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let rows = query::bfs_rows(&session, &outcome)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let total = rows.len();
        let truncated = outcome.truncated;
        let path_found = outcome.path.is_some();
        drop(session);

        let result = serde_json::json!({ "reached": rows, "path_found": path_found });
        let envelope = query::envelope(&project, result, warnings, truncated, Some(total));
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(
        description = "Bounded reverse traversal — everything that potentially depends on a symbol"
    )]
    fn codekurve_analyze_impact(
        &self,
        Parameters(input): Parameters<AnalyzeImpactInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session.lock().unwrap();
        let args = QueryArgs {
            root: session.root(),
            symbol_id: input.symbol_id.as_deref(),
            symbol_name: input.symbol_name.as_deref(),
            min_confidence: input.min_confidence.as_deref(),
            depth: input.depth,
            limit: None,
            offset: None,
            json: false,
        };
        let outcome = query::impact(&session, &args)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let rows = query::bfs_rows(&session, &outcome)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let total = rows.len();
        let truncated = outcome.truncated;
        drop(session);

        let result = serde_json::json!({ "reached": rows });
        let envelope = query::envelope(&project, result, warnings, truncated, Some(total));
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }
}

/// Not part of `#[tool_router]` — the shared body of the four flat
/// relationship tools (task 5.4), kept as a plain method so it isn't itself
/// advertised as a tool.
impl CodeKurve {
    fn relationship_result(
        &self,
        input: RelationshipInput,
        kind: RelKind,
    ) -> Result<CallToolResult, McpError> {
        let session = self.session.lock().unwrap();
        // ponytail: no MCP-specific cap config exists yet; reuse
        // `[queries] default_limit`/`max_limit`, the same bound `search`
        // already applies, rather than inventing a second knob.
        let default_limit = session.config().queries.default_limit as usize;
        let max_limit = session.config().queries.max_limit as usize;
        let limit = input
            .limit
            .map(|l| l as usize)
            .unwrap_or(default_limit)
            .min(max_limit);
        let args = QueryArgs {
            root: session.root(),
            symbol_id: input.symbol_id.as_deref(),
            symbol_name: input.symbol_name.as_deref(),
            min_confidence: input.min_confidence.as_deref(),
            depth: None,
            limit: Some(limit),
            offset: input.offset.map(|o| o as usize),
            json: false,
        };
        let page = query::relationships(&session, kind, &args)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let rows: Vec<_> = page.rows.iter().map(query::relationship_row).collect();
        drop(session);

        let envelope = query::envelope(
            &project,
            serde_json::Value::Array(rows),
            warnings,
            page.truncated,
            Some(page.total),
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }
}

/// Spec "search_symbols Tool Rejects Unsupported Filters" (confirmed
/// decision 3): `kinds`/`languages`/`path_prefix` are in the schema for
/// discoverability, but the store can't filter on any of them yet — reject
/// explicitly, one message naming every supported filter, rather than
/// silently dropping the value.
fn reject_unsupported_search_filters(input: &SearchSymbolsInput) -> Result<(), McpError> {
    if input.kinds.is_some() || input.languages.is_some() || input.path_prefix.is_some() {
        return Err(McpError::invalid_params(
            "invalid params: filter not supported yet (supported: query, limit)",
            None,
        ));
    }
    Ok(())
}
