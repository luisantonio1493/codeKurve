//! Tool bodies (design "Server Bootstrap"). PR4 landed one tool,
//! `codekurve_project_status`; PR5 adds the remaining eight read tools
//! (§28.2 tool registry, minus `project_overview`/`doctor`/`reindex`, which
//! are PR6 scope). Each body locks [`CodeKurve::session`], calls sync
//! `query::*` functions, and drops the guard before returning — no
//! `.await` while the lock is held (design "Concurrency").

use codekurve::commands::{self, QueryArgs};
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

/// `codekurve_project_overview` takes no arguments — same reasoning as
/// [`ProjectStatusInput`].
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProjectOverviewInput {}

/// `codekurve_doctor` takes no arguments — same reasoning as
/// [`ProjectStatusInput`].
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DoctorInput {}

/// `codekurve_reindex` takes no arguments; only registered/callable when
/// `[mcp] allow_reindex = true` (`server.rs`'s `list_tools`/`call_tool`
/// gating, spec "reindex Gated Off by Default").
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReindexInput {}

/// `search_symbols` accepts only the filters it can execute. Unknown fields
/// are rejected rather than advertised and then refused at runtime.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchSymbolsInput {
    pub query: String,
    /// Max rows (default 50, max 500).
    pub limit: Option<u32>,
}

/// Find framework-recognized HTTP route bindings. `query` matches a verb,
/// path fragment, or both (for example `POST /api/PatientReferrals/Submit`).
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindRoutesInput {
    pub query: Option<String>,
    /// Max rows (default 50, max 500).
    pub limit: Option<u32>,
    /// Rows to skip (default 0).
    pub offset: Option<u32>,
}

/// `get_symbol` (§28.2): `ctx_lines` extra lines of context on each side of
/// the symbol's span; `include_source` (default `true`) lets a client skip
/// the disk read/staleness check entirely when it only wants the symbol's
/// location.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSymbolInput {
    pub id: String,
    /// Extra source lines on each side (default 0, max 200).
    pub ctx_lines: Option<u32>,
    /// Read the source (default true); false returns only the location.
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
    /// Confidence floor: exact|high|medium|low|unresolved. By-name matches are often `low`; leave unset to see them.
    pub min_confidence: Option<String>,
    /// Max rows (default 50, max 500).
    pub limit: Option<u32>,
    /// Rows to skip (default 0).
    pub offset: Option<u32>,
}

/// `find_unresolved`: every field optional — with none set the tool lists the
/// whole project's unresolved references. `target_text` is matched exactly
/// (the store's index is `(project_id, target_text)`), so pass the type/symbol
/// name as it appears in the source.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindUnresolvedInput {
    pub target_text: Option<String>,
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    /// Max rows (default 50, max 500).
    pub limit: Option<u32>,
    /// Rows to skip (default 0).
    pub offset: Option<u32>,
}

/// `trace_path` (§28.2): same subject resolution as [`RelationshipInput`]
/// plus the CLI's positional `to` target and `--depth`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TracePathInput {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub to: String,
    /// Confidence floor: exact|high|medium|low|unresolved. By-name matches are often `low`; leave unset to see them.
    pub min_confidence: Option<String>,
    /// Max hops (default 10).
    pub depth: Option<u32>,
}

/// `analyze_impact` (§28.2): same subject resolution as [`RelationshipInput`]
/// plus `--depth`; no `to` (reverse BFS explores everything reachable, no
/// single target).
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyzeImpactInput {
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    /// Confidence floor: exact|high|medium|low|unresolved. By-name matches are often `low`; leave unset to see them.
    pub min_confidence: Option<String>,
    /// Max hops (default 10).
    pub depth: Option<u32>,
}

#[tool_router(vis = "pub(crate)")]
impl CodeKurve {
    #[tool(description = "Report index freshness, counts, and staleness for the current project")]
    fn codekurve_project_status(
        &self,
        Parameters(ProjectStatusInput {}): Parameters<ProjectStatusInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
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
        let envelope = query::mcp_envelope(&project, result, warnings, false, None);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(
        description = "Search symbols by name or qualified name. Uses a partial-name fallback when exact full-text search has no results."
    )]
    fn codekurve_search_symbols(
        &self,
        Parameters(input): Parameters<SearchSymbolsInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
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
                // `name` and `path` are derivable from `qualified_name`
                // (`<path>::<Scope>.<name>`), and every symbol row is
                // tree-sitter / high confidence: none of that is repeated.
                serde_json::json!({
                    "id": sym.id,
                    "qualified_name": sym.qualified_name,
                    "kind": sym.kind,
                    "language": sym.language,
                    "start_line": sym.span.start_line,
                    "end_line": sym.span.end_line,
                })
            })
            .collect();
        drop(session);

        let envelope = query::mcp_envelope(
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
        description = "Find framework-recognized HTTP route bindings by method or path, with offset pagination"
    )]
    fn codekurve_find_routes(
        &self,
        Parameters(input): Parameters<FindRoutesInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
        let default_limit = session.config().queries.default_limit as usize;
        let max_limit = session.config().queries.max_limit as usize;
        // ponytail: a route row can carry a long framework path; keep one MCP
        // response comfortably bounded. Use offset to retrieve every page.
        const MAX_ROUTE_PAGE_SIZE: usize = 50;
        let limit = input
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(default_limit)
            .min(max_limit)
            .min(MAX_ROUTE_PAGE_SIZE);
        let page = query::routes(
            &session,
            input.query.as_deref(),
            limit,
            input.offset.map(|offset| offset as usize),
        )
        .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|r| query::drop_keys(query::prune_nulls(query::relationship_row(r)), &["path"]))
            .collect();
        drop(session);

        let envelope = query::mcp_envelope(
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
        let session = self.lock_session();
        let ctx_lines = input.ctx_lines.unwrap_or(0);
        let include_source = input.include_source.unwrap_or(true);
        let detail = query::get_symbol(&session, &input.id, ctx_lines)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let sym = &detail.symbol;

        let mut source_truncated = false;
        let (source, stale, reason) = if include_source {
            // Session::warnings() for an Indexed session is only ever the
            // pending-files wording (see query::pending_warning) — non-empty
            // here means index_pending > 0, the exact bit source_slice
            // needs, without a second `repo::index_status` round-trip.
            let index_pending = !warnings.is_empty();
            let config = session.config();
            let limits = query::SliceLimits {
                max_file_bytes: config.index.max_file_size_bytes,
                max_snippet_bytes: config.queries.max_snippet_bytes as usize,
            };
            let slice = query::source_slice(
                session.root(),
                &sym.relative_path,
                &sym.span,
                ctx_lines,
                index_pending,
                &limits,
            );
            source_truncated = slice.truncated;
            (slice.source, slice.stale, slice.reason)
        } else {
            (None, false, None)
        };

        let mut row = query::prune_nulls(serde_json::json!({
            "id": sym.id,
            "qualified_name": sym.qualified_name,
            "kind": sym.kind,
            "language": sym.language,
            "start_line": sym.span.start_line,
            "end_line": sym.span.end_line,
            "source": source,
            // Distinct from the project-level stale warning (spec
            // "get_symbol Reads Live Source and Flags Drift"): this flags
            // drift for *this* symbol's own span/file.
            "stale": stale,
            "stale_reason": reason,
        }));
        if source_truncated {
            row["source_truncated"] = serde_json::Value::Bool(true);
        }
        drop(session);

        let envelope = query::mcp_envelope(&project, row, warnings, false, Some(1));
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

    #[tool(
        description = "Explain a missing relationship: lists references the analyzer could not resolve, with the reason (e.g. external base class, no or ambiguous candidates). Use when find_callers/find_implementations come back empty. Filter by target_text (exact), symbol_id/symbol_name, or none for all"
    )]
    fn codekurve_find_unresolved(
        &self,
        Parameters(input): Parameters<FindUnresolvedInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
        // Same bound the relationship tools apply — a real project holds
        // hundreds of unresolved rows, so this is never unbounded (§28.3).
        let default_limit = session.config().queries.default_limit as usize;
        let max_limit = session.config().queries.max_limit as usize;
        let limit = input
            .limit
            .map(|l| l as usize)
            .unwrap_or(default_limit)
            .min(max_limit);
        let filter = query::UnresolvedFilter {
            target_text: input.target_text.as_deref(),
            symbol_id: input.symbol_id.as_deref(),
            symbol_name: input.symbol_name.as_deref(),
            limit: Some(limit),
            offset: input.offset.map(|o| o as usize),
        };
        let page = query::unresolved(&session, &filter)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|r| query::drop_keys(query::prune_nulls(query::unresolved_row(r)), &["path"]))
            .collect();
        drop(session);

        let envelope = query::mcp_envelope(
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

    #[tool(description = "Bounded forward path from one symbol to another")]
    fn codekurve_trace_path(
        &self,
        Parameters(input): Parameters<TracePathInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
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
        // Only the nodes ON the path: the search frontier (up to 500 nodes)
        // is not an answer to "how does A reach B".
        let rows = query::path_rows(&session, &outcome)
            .map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        let reached_count = outcome.reached.len();
        let truncated = outcome.truncated;
        let truncated_reason = outcome
            .truncated_reason
            .map(commands::truncation_reason_str);
        let path_found = outcome.path.is_some();
        drop(session);

        let result = serde_json::json!({
            "path_found": path_found,
            "path": rows,
            "reached_count": reached_count,
        });
        let mut envelope = query::mcp_envelope(&project, result, warnings, truncated, None);
        if let Some(reason) = truncated_reason {
            envelope["truncated_reason"] = serde_json::json!(reason);
        }
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
        let session = self.lock_session();
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
        let truncated_reason = outcome
            .truncated_reason
            .map(commands::truncation_reason_str);
        drop(session);

        let result = serde_json::json!({ "reached": rows });
        let mut envelope = query::mcp_envelope(&project, result, warnings, truncated, Some(total));
        if let Some(reason) = truncated_reason {
            envelope["truncated_reason"] = serde_json::json!(reason);
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(
        description = "Report project-wide counts, language breakdown, and framework-recognized route entry points"
    )]
    fn codekurve_project_overview(
        &self,
        Parameters(ProjectOverviewInput {}): Parameters<ProjectOverviewInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
        let data =
            query::overview(&session).map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        drop(session);

        let languages: Vec<_> = data
            .languages
            .iter()
            .map(|(language, files)| serde_json::json!({ "language": language, "files": files }))
            .collect();
        let result = serde_json::json!({
            "files": data.files,
            "symbols": data.symbols,
            "relationships": data.relationships,
            "languages": languages,
            "entry_points": data.entry_points.iter().map(|r| query::drop_keys(query::prune_nulls(query::relationship_row(r)), &["path"])).collect::<Vec<_>>(),
        });
        let envelope = query::mcp_envelope(&project, result, warnings, false, None);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    #[tool(
        description = "Run the same diagnostic checks as the CLI `doctor` command (sqlite/fts5 availability, schema version, config validity, index presence)"
    )]
    fn codekurve_doctor(
        &self,
        Parameters(DoctorInput {}): Parameters<DoctorInput>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
        let report = query::doctor(&session);
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        drop(session);

        let checks: Vec<_> = report
            .checks
            .iter()
            .map(|c| serde_json::json!({ "name": c.name, "ok": c.ok, "detail": c.detail }))
            .collect();
        let result = serde_json::json!({ "ok": report.ok, "checks": checks });
        let envelope = query::mcp_envelope(&project, result, warnings, false, None);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }

    /// Registered unconditionally by `#[tool_router]` (a compile-time list);
    /// `server.rs`'s `list_tools`/`call_tool` overrides do the actual
    /// `[mcp] allow_reindex` gating (task 6.4) — this body itself never
    /// checks the flag.
    #[tool(
        description = "Trigger an index run for the current project (only available when [mcp] allow_reindex = true)"
    )]
    fn codekurve_reindex(
        &self,
        Parameters(ReindexInput {}): Parameters<ReindexInput>,
    ) -> Result<CallToolResult, McpError> {
        let mut session = self.lock_session();
        let root = session.root().to_path_buf();
        let outcome =
            query::reindex(&root).map_err(|e| McpError::internal_error(e.message, None))?;
        // Reopen so subsequent tool calls (on this same locked session) see
        // the refreshed index, whether the session started `Indexed` or
        // `NotIndexed`.
        *session =
            query::Session::open(&root).map_err(|e| McpError::internal_error(e.message, None))?;
        let warnings = session.warnings();
        let project = session.config().project.name.clone();
        drop(session);

        let result = serde_json::json!({
            "files_changed": outcome.files_changed,
            "files_deleted": outcome.files_deleted,
            "fell_back_to_full_reindex": outcome.fell_back_to_full_reindex,
        });
        let envelope = query::mcp_envelope(&project, result, warnings, false, None);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }
}

/// Not part of `#[tool_router]` — the shared body of the four flat
/// relationship tools (task 5.4), kept as a plain method so it isn't itself
/// advertised as a tool.
impl CodeKurve {
    /// Lock the shared session, recovering from a poisoned mutex: one panicked
    /// tool call must not turn every later call into a panic too.
    fn lock_session(&self) -> std::sync::MutexGuard<'_, query::Session> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn relationship_result(
        &self,
        input: RelationshipInput,
        kind: RelKind,
    ) -> Result<CallToolResult, McpError> {
        let session = self.lock_session();
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
        let anchor_is_target = query::relationship_anchor_is_target(kind);
        let anchor = page
            .rows
            .first()
            .map(|r| query::relationship_anchor(r, anchor_is_target));
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|r| query::prune_nulls(query::relationship_row_compact(r, anchor_is_target)))
            .collect();
        drop(session);

        let envelope = query::mcp_envelope(
            &project,
            serde_json::json!({ "anchor": anchor, "rows": rows }),
            warnings,
            page.truncated,
            Some(page.total),
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(
            envelope.to_string(),
        )]))
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn poisoned_session_lock_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        codekurve_core::project::init(dir.path()).unwrap();
        let session = query::Session::open(dir.path()).unwrap();
        let server = Arc::new(CodeKurve {
            session: Mutex::new(session),
            allow_reindex: false,
        });

        let poisoner = Arc::clone(&server);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.session.lock().unwrap();
            panic!("poison the session lock");
        })
        .join();
        assert!(server.session.is_poisoned());

        let guard = server.lock_session();
        assert_eq!(guard.root(), dir.path().canonicalize().unwrap());
    }
}
