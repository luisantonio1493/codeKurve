//! Data-returning query layer (design "Print/return split"): every function
//! here returns structured data and never prints. `commands.rs` stays the
//! only module that owns stdout — it opens a [`Session`], calls into this
//! module, then prints the result exactly as before.
//!
//! PR2 landed the six graph-query commands (`references`/`callers`/
//! `callees`/`implementations`/`trace`/`impact`) plus the shared `Session`
//! and `envelope`. PR3 adds `search`/`get_symbol`/`status`/`overview`/
//! `doctor`, the degraded `Session::NotIndexed` variant, and
//! `Session::warnings` — the single stale-source feeding both CLI stderr and
//! (PR5+) MCP response `warnings`.

use std::path::{Path, PathBuf};

use codekurve_core::{Config, SourceSpan};
use codekurve_store::repo::{self, StoredRelationship, StoredSymbol, StoredUnresolved};
use codekurve_store::{db, migrations, traverse, Connection};

use crate::commands::{self, CommandError, QueryArgs};

/// Which of the four flat relationship queries [`relationships`] runs
/// (§5a.1) — one call site behind `references`/`callers`/`callees`/
/// `implementations` instead of four near-identical command bodies.
#[derive(Debug, Clone, Copy)]
pub enum RelKind {
    References,
    Callers,
    Callees,
    Implementations,
}

/// A project resolved once per session (design "Missing index"): `Indexed`
/// for the common case, `NotIndexed` when the project root/config are valid
/// but no completed index run exists yet — status/doctor/overview answer
/// degraded from this variant (MCP never auto-indexes, never hard-fails the
/// connection); the six graph-query commands plus `search`/`get_symbol`
/// still require `Indexed` (see [`Session::indexed`]) and fail exactly like
/// today's `require_indexed_project` did.
#[derive(Debug)]
pub enum Session {
    Indexed {
        root: PathBuf,
        config: Config,
        conn: Connection,
        project_id: String,
    },
    NotIndexed {
        root: PathBuf,
        config: Config,
        reason: String,
    },
}

impl Session {
    /// Fatal (`Err`, code 4) only when the project root/config can't be
    /// resolved — spec "Query before first index" still applies to callers
    /// that need [`Session::indexed`], but opening the session itself no
    /// longer fails just because the DB/project row is missing.
    pub fn open(root: &Path) -> Result<Self, CommandError> {
        let (root, config) = commands::load_project_config(root)?;
        match commands::open_project_index(&root, &config) {
            Ok((conn, project_id)) => Ok(Session::Indexed {
                root,
                config,
                conn,
                project_id,
            }),
            Err(e) => Ok(Session::NotIndexed {
                root,
                config,
                reason: e.message,
            }),
        }
    }

    pub fn root(&self) -> &Path {
        match self {
            Session::Indexed { root, .. } | Session::NotIndexed { root, .. } => root,
        }
    }

    pub fn config(&self) -> &Config {
        match self {
            Session::Indexed { config, .. } | Session::NotIndexed { config, .. } => config,
        }
    }

    /// The connection/project id pair every graph-query/search/get_symbol
    /// function needs. `NotIndexed` becomes the same code-4 `CommandError`
    /// `require_indexed_project` used to produce, with the exact same
    /// message — CLI behavior for these commands is unchanged.
    pub(crate) fn indexed(&self) -> Result<(&Connection, &str), CommandError> {
        match self {
            Session::Indexed {
                conn, project_id, ..
            } => Ok((conn, project_id)),
            Session::NotIndexed { reason, .. } => Err(CommandError {
                code: 4,
                message: reason.clone(),
            }),
        }
    }

    /// Single stale-source (design "Stale warning"): one wording feeds both
    /// `commands::warn_if_stale` (CLI stderr, via [`pending_warning`]) and
    /// every MCP response's `warnings` (PR5+). Empty for a freshly indexed,
    /// non-stale project; one entry for a stale index; one entry (the
    /// degraded reason) for `NotIndexed`.
    pub fn warnings(&self) -> Vec<String> {
        match self {
            Session::Indexed {
                conn, project_id, ..
            } => pending_warning(conn, project_id),
            Session::NotIndexed { reason, .. } => vec![reason.clone()],
        }
    }
}

/// The stale-index wording, computed once and reused by both
/// [`Session::warnings`] (Indexed case) and `commands::warn_if_stale`
/// directly — `search`/`symbol` don't build a full `Session`, so they call
/// this the same way `Session::warnings` does internally, instead of a
/// second copy of the pending-files check.
pub(crate) fn pending_warning(conn: &Connection, project_id: &str) -> Vec<String> {
    match repo::index_status(conn, project_id) {
        Ok(status) if status.pending_files > 0 => vec![format!(
            "index is stale ({} pending file(s)); run `codekurve index`",
            status.pending_files
        )],
        _ => Vec::new(),
    }
}

/// A page of results plus the total row count before truncation (§27.5).
pub struct Page<T> {
    pub rows: Vec<T>,
    pub total: usize,
    pub truncated: bool,
}

/// §27.5 JSON envelope. `total` is only emitted when `Some` — every CLI call
/// site passes `None`, so today's `--json` golden output stays
/// byte-identical; MCP (PR5+) passes `Some(page.total)`.
pub fn envelope(
    project: &str,
    result: serde_json::Value,
    warnings: Vec<String>,
    truncated: bool,
    total: Option<usize>,
) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "schema_version": 1,
        "project": project,
        "result": result,
        "warnings": warnings,
        "truncated": truncated,
    });
    if let Some(total) = total {
        envelope["total"] = serde_json::json!(total);
    }
    envelope
}

/// Shared body of `references`/`callers`/`callees`/`implementations`:
/// resolve the subject symbol, run the single indexed SELECT (§5a.1), then
/// paginate. Extracted from `commands::relationship_command`.
pub fn relationships(
    s: &Session,
    kind: RelKind,
    args: &QueryArgs,
) -> Result<Page<StoredRelationship>, CommandError> {
    let (conn, project_id) = s.indexed()?;
    let symbol_id = commands::resolve_symbol(conn, project_id, args.symbol_id, args.symbol_name)?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;
    let query_fn = match kind {
        RelKind::References => repo::references,
        RelKind::Callers => repo::callers,
        RelKind::Callees => repo::callees,
        RelKind::Implementations => repo::implementations,
    };

    let mut rows = query_fn(conn, project_id, &symbol_id, min_confidence)
        .map_err(|e| CommandError::from(e.to_string()))?;
    let total = rows.len();
    commands::paginate(&mut rows, args.limit, args.offset);
    let truncated = total > args.offset.unwrap_or(0) + rows.len();
    Ok(Page {
        rows,
        total,
        truncated,
    })
}

/// Extracted from `commands::trace`: bounded forward BFS (§26.4) from the
/// resolved source symbol to `to`, also resolved through
/// [`commands::resolve_symbol`] (an ambiguous target exits 6 exactly like an
/// ambiguous source).
pub fn trace(
    s: &Session,
    args: &QueryArgs,
    to: &str,
) -> Result<traverse::BfsOutcome, CommandError> {
    let (conn, project_id) = s.indexed()?;
    let from = commands::resolve_symbol(conn, project_id, args.symbol_id, args.symbol_name)?;
    let target = commands::resolve_symbol(conn, project_id, None, Some(to))?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;

    let adjacency = traverse::load_adjacency(conn, project_id, false)
        .map_err(|e| CommandError::from(e.to_string()))?;
    let caps = commands::bfs_caps(args.depth);
    Ok(traverse::bfs(
        &adjacency,
        &from,
        Some(&target),
        &caps,
        None,
        min_confidence,
    ))
}

/// Extracted from `commands::impact`: bounded reverse BFS (§26.5) —
/// everything that potentially depends on the resolved symbol, never
/// guaranteed, truncated rather than silently incomplete.
pub fn impact(s: &Session, args: &QueryArgs) -> Result<traverse::BfsOutcome, CommandError> {
    let (conn, project_id) = s.indexed()?;
    let symbol_id = commands::resolve_symbol(conn, project_id, args.symbol_id, args.symbol_name)?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;

    let adjacency = traverse::load_adjacency(conn, project_id, true)
        .map_err(|e| CommandError::from(e.to_string()))?;
    let caps = commands::bfs_caps(args.depth);
    Ok(traverse::bfs(
        &adjacency,
        &symbol_id,
        None,
        &caps,
        None,
        min_confidence,
    ))
}

/// One §28.3 row (path, line range, confidence, provenance) for a
/// [`traverse::Reached`] node — `trace_path`/`analyze_impact`'s shared row
/// shape (MCP-only; the CLI's `trace_json`/`reached_json` stay unchanged).
/// One extra lookup per node (`find_symbol_by_id`) — `Reached` only carries
/// the id, not the node's own file/span; fixture-sized result sets make this
/// an acceptable cost rather than a second joined query.
pub fn reached_row(s: &Session, r: &traverse::Reached) -> Result<serde_json::Value, CommandError> {
    let (conn, _project_id) = s.indexed()?;
    let sym = repo::find_symbol_by_id(conn, &r.symbol_id)
        .map_err(|e| CommandError::from(e.to_string()))?;
    Ok(serde_json::json!({
        "symbol_id": r.symbol_id,
        "depth": r.depth,
        "path": sym.as_ref().map(|sym| sym.relative_path.clone()),
        "start_line": sym.as_ref().map(|sym| sym.span.start_line),
        "end_line": sym.as_ref().map(|sym| sym.span.end_line),
        "confidence": r.via.as_ref().map(|e| e.confidence.clone()),
        "provenance": r.via.as_ref().map(|e| e.provenance.clone()),
        "via_kind": r.via.as_ref().map(|e| e.kind.clone()),
        "predecessor": r.predecessor,
    }))
}

/// Every reached node as a §28.3 row (see [`reached_row`]), for `trace_path`/
/// `analyze_impact`'s MCP result.
pub fn bfs_rows(
    s: &Session,
    outcome: &traverse::BfsOutcome,
) -> Result<Vec<serde_json::Value>, CommandError> {
    outcome.reached.iter().map(|r| reached_row(s, r)).collect()
}

/// One §28.3 row (path, line range, confidence, provenance) for a
/// [`StoredRelationship`] — `find_references`/`find_callers`/`find_callees`/
/// `find_implementations`'s MCP result shape (the CLI's `relationship_json`
/// stays unchanged, no `path`).
pub fn relationship_row(r: &StoredRelationship) -> serde_json::Value {
    serde_json::json!({
        "source_symbol_id": r.source_symbol_id,
        "source_qualified_name": r.source_qualified_name,
        "target_symbol_id": r.target_symbol_id,
        "target_qualified_name": r.target_qualified_name,
        "target_external": r.target_external,
        "kind": r.kind,
        "path": r.source_relative_path,
        "start_line": r.start_line,
        "start_column": r.start_column,
        "confidence": r.confidence,
        "provenance": r.provenance,
        "reason": r.reason,
    })
}

/// [`unresolved`]'s filter. Every field is optional — no filter at all lists
/// the whole project's unresolved references, which is the common entry point
/// ("what did the analyzer give up on?"). Not `QueryArgs`: that shape
/// *requires* a subject symbol, and this query's whole point is that it works
/// without one.
pub struct UnresolvedFilter<'a> {
    pub target_text: Option<&'a str>,
    pub symbol_id: Option<&'a str>,
    pub symbol_name: Option<&'a str>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// The references the analyzer recorded but refused to turn into edges, with
/// the `reason` it recorded — same pagination/truncation semantics as
/// [`relationships`]. Answers "`find_implementations` came back empty, but
/// this class clearly implements something — what happened?".
pub fn unresolved(
    s: &Session,
    filter: &UnresolvedFilter,
) -> Result<Page<StoredUnresolved>, CommandError> {
    let (conn, project_id) = s.indexed()?;
    // Only resolve a subject symbol when one was actually named —
    // `resolve_symbol` errors on (None, None), which is a legitimate call
    // here (list everything), not a usage mistake.
    let symbol_id = match (filter.symbol_id, filter.symbol_name) {
        (None, None) => None,
        (id, name) => Some(commands::resolve_symbol(conn, project_id, id, name)?),
    };

    let mut rows = repo::unresolved(conn, project_id, filter.target_text, symbol_id.as_deref())
        .map_err(|e| CommandError::from(e.to_string()))?;
    let total = rows.len();
    commands::paginate(&mut rows, filter.limit, filter.offset);
    let truncated = total > filter.offset.unwrap_or(0) + rows.len();
    Ok(Page {
        rows,
        total,
        truncated,
    })
}

/// One §28.3 row for a [`StoredUnresolved`] — shared by the CLI's `--json`
/// and the `codekurve_find_unresolved` MCP tool (unlike `relationship_json`,
/// there is no pre-existing golden CLI shape to preserve, so one shape
/// serves both). No `start_line`/`provenance`: `unresolved_references` stores
/// neither (see [`StoredUnresolved`]); `path` locates the row instead.
pub fn unresolved_row(u: &StoredUnresolved) -> serde_json::Value {
    serde_json::json!({
        "source_symbol_id": u.source_symbol_id,
        "source_qualified_name": u.source_qualified_name,
        "path": u.source_relative_path,
        "kind": u.relationship_kind,
        "target_text": u.target_text,
        "reason": u.reason,
        "confidence": u.confidence,
        "candidate_count": u.candidate_count,
    })
}

/// `search`'s filter — just the free-text query plus an optional limit
/// override today; `kinds`/`languages`/`path_prefix` are PR5's job
/// (design "Unsupported search filters").
pub struct SearchInput<'a> {
    pub query: &'a str,
    pub limit: Option<u32>,
}

/// Extracted from `commands::search`: full-text search, `s.id` carried
/// through on every row (ponytail: reuses `repo::StoredSymbol` rather than a
/// parallel `SymbolHit` type — the two would have identical fields) so an
/// MCP client can chain a hit straight into [`get_symbol`].
pub fn search(s: &Session, q: &SearchInput) -> Result<Page<StoredSymbol>, CommandError> {
    let (conn, project_id) = s.indexed()?;
    let limit = q.limit.unwrap_or(s.config().queries.default_limit);
    let rows = repo::search(conn, project_id, q.query, limit)
        .map_err(|e| CommandError::from(e.to_string()))?;
    let total = rows.len();
    Ok(Page {
        rows,
        total,
        truncated: false,
    })
}

/// Framework-recognized HTTP route bindings, bounded and paginated by the caller.
pub fn routes(
    s: &Session,
    route_query: Option<&str>,
    limit: usize,
    offset: Option<usize>,
) -> Result<Page<StoredRelationship>, CommandError> {
    let (conn, project_id) = s.indexed()?;
    let mut rows = repo::routes(conn, project_id, route_query)
        .map_err(|e| CommandError::from(e.to_string()))?;
    let total = rows.len();
    commands::paginate(&mut rows, Some(limit), offset);
    let truncated = total > offset.unwrap_or(0) + rows.len();
    Ok(Page {
        rows,
        total,
        truncated,
    })
}

/// A single resolved symbol by id — the shape `get_symbol` needs to chain
/// off a `search` hit. `ctx_lines`-driven source snippet/staleness
/// (`source_slice`, design "get_symbol Staleness") is PR5 scope; this PR
/// only resolves the row.
pub struct SymbolDetail {
    pub symbol: StoredSymbol,
}

/// Extracted from `commands::symbol`'s resolution step, generalized to
/// resolve by `id` (unambiguous) instead of by name (which the CLI's
/// `symbol <name>` command still does directly, since it may report several
/// same-named hits) — needs [`repo::find_symbol_by_id`] (PR3 store
/// addition).
pub fn get_symbol(s: &Session, id: &str, _ctx_lines: u32) -> Result<SymbolDetail, CommandError> {
    let (conn, _project_id) = s.indexed()?;
    let symbol = repo::find_symbol_by_id(conn, id)
        .map_err(|e| CommandError::from(e.to_string()))?
        .ok_or_else(|| CommandError::from(format!("no symbol with id {id:?}")))?;
    Ok(SymbolDetail { symbol })
}

/// `get_symbol`'s live-source read (design "`get_symbol` Staleness",
/// confirmed decision 4): promoted from `commands::snippet`'s bounds check,
/// made explicit rather than a text marker. Reads disk on every call —
/// never the indexed/cached span.
pub struct SourceSlice {
    pub source: Option<String>,
    pub stale: bool,
    pub reason: Option<&'static str>,
}

/// `path` is the file on disk (project root joined with the symbol's
/// `relative_path`); `ctx_lines` extra lines of context on each side of the
/// symbol's line span; `index_pending` is whether the project's stored
/// freshness metadata shows pending files (`Session::warnings()` non-empty
/// for an `Indexed` session) — folded into `stale` even when the read itself
/// succeeds, per confirmed decision 4.
pub fn source_slice(
    path: &Path,
    span: &SourceSpan,
    ctx_lines: u32,
    index_pending: bool,
) -> SourceSlice {
    let Ok(bytes) = std::fs::read(path) else {
        return SourceSlice {
            source: None,
            stale: true,
            reason: Some("file_missing"),
        };
    };
    if span.end_byte > bytes.len() {
        return SourceSlice {
            source: None,
            stale: true,
            reason: Some("span_out_of_range"),
        };
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return SourceSlice {
            source: None,
            stale: true,
            reason: Some("non_utf8"),
        };
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = span.start_line.saturating_sub(ctx_lines as usize).max(1);
    let end = (span.end_line + ctx_lines as usize).min(lines.len());
    let snippet = if start >= 1 && start <= end && end <= lines.len() {
        lines[(start - 1)..end].join("\n")
    } else {
        String::new()
    };
    SourceSlice {
        source: Some(snippet),
        stale: index_pending,
        reason: None,
    }
}

/// `codekurve status`'s data, plus the degraded (`NotIndexed`) shape a
/// missing index reports instead of failing (design "Missing index").
pub struct StatusData {
    pub schema_version: i64,
    pub files: usize,
    pub symbols: usize,
    pub relationships: usize,
    pub relationships_unresolved: usize,
    pub pending_files: i64,
    pub last_verified_at: Option<String>,
    pub stale: bool,
}

/// New data function backing the future `codekurve_project_status` MCP tool
/// (PR4+); `commands::status` keeps its own independent, fatal-on-missing
/// preamble unchanged (task hard constraint: zero edits to existing golden
/// tests).
pub fn status(s: &Session) -> Result<StatusData, CommandError> {
    match s {
        Session::Indexed {
            conn, project_id, ..
        } => {
            let st = repo::index_status(conn, project_id)
                .map_err(|e| CommandError::from(e.to_string()))?;
            let schema_version =
                migrations::current_version(conn).map_err(|e| CommandError::from(e.to_string()))?;
            Ok(StatusData {
                schema_version,
                files: st.files,
                symbols: st.symbols,
                relationships: st.relationships,
                relationships_unresolved: st.relationships_unresolved,
                pending_files: st.pending_files,
                last_verified_at: st.last_verified_at,
                stale: st.pending_files > 0,
            })
        }
        Session::NotIndexed { .. } => Ok(StatusData {
            schema_version: 0,
            files: 0,
            symbols: 0,
            relationships: 0,
            relationships_unresolved: 0,
            pending_files: 0,
            last_verified_at: None,
            stale: true,
        }),
    }
}

/// `project_overview`'s data (design "`project_overview` content"): counts
/// plus a per-language file breakdown, nothing else until an agent-usage
/// need appears.
pub struct OverviewData {
    pub files: usize,
    pub symbols: usize,
    pub relationships: usize,
    pub languages: Vec<(String, usize)>,
    pub entry_points: Vec<StoredRelationship>,
}

pub fn overview(s: &Session) -> Result<OverviewData, CommandError> {
    match s {
        Session::Indexed {
            conn, project_id, ..
        } => {
            let st = repo::index_status(conn, project_id)
                .map_err(|e| CommandError::from(e.to_string()))?;
            let languages = repo::language_breakdown(conn, project_id)
                .map_err(|e| CommandError::from(e.to_string()))?;
            let entry_points = routes(s, None, 20, None)?.rows;
            Ok(OverviewData {
                files: st.files,
                symbols: st.symbols,
                relationships: st.relationships,
                languages,
                entry_points,
            })
        }
        Session::NotIndexed { .. } => Ok(OverviewData {
            files: 0,
            symbols: 0,
            relationships: 0,
            languages: Vec::new(),
            entry_points: Vec::new(),
        }),
    }
}

/// One `doctor` check result (name/ok/detail), mirroring
/// `commands::doctor`'s `report()` lines.
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub ok: bool,
}

/// New data function backing the future `codekurve_doctor` MCP tool (PR4+):
/// same sqlite/fts5/schema probe `commands::doctor` runs, plus the session's
/// own resolved root/config/index state (never re-reads the config file —
/// `Session::open` already did). `commands::doctor` keeps its own
/// independent implementation unchanged (it must keep reporting partial
/// results even when `root` itself fails to canonicalize, which a
/// `Session`-gated version — fatal on bad root — could never do).
pub fn doctor(s: &Session) -> DoctorReport {
    let mut checks = Vec::new();
    let mut ok = true;

    match db::open_in_memory() {
        Ok(probe) => {
            checks.push(DoctorCheck {
                name: "sqlite",
                ok: true,
                detail: "available (bundled)".to_string(),
            });
            let fts5 = db::has_fts5(&probe);
            checks.push(DoctorCheck {
                name: "fts5",
                ok: fts5,
                detail: if fts5 { "available" } else { "MISSING" }.to_string(),
            });
            ok &= fts5;

            match migrations::current_version(&probe) {
                Ok(version) => {
                    let schema_ok = version == migrations::SCHEMA_VERSION;
                    checks.push(DoctorCheck {
                        name: "schema",
                        ok: schema_ok,
                        detail: format!(
                            "version {version} (expected {})",
                            migrations::SCHEMA_VERSION
                        ),
                    });
                    ok &= schema_ok;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        name: "schema",
                        ok: false,
                        detail: e.to_string(),
                    });
                    ok = false;
                }
            }
        }
        Err(e) => {
            checks.push(DoctorCheck {
                name: "sqlite",
                ok: false,
                detail: e.to_string(),
            });
            ok = false;
        }
    }

    checks.push(DoctorCheck {
        name: "project root",
        ok: true,
        detail: s.root().to_string_lossy().into_owned(),
    });
    checks.push(DoctorCheck {
        name: "config",
        ok: true,
        detail: ".codekurve/config.toml".to_string(),
    });

    if let Session::NotIndexed { reason, .. } = s {
        checks.push(DoctorCheck {
            name: "index",
            ok: false,
            detail: reason.clone(),
        });
        ok = false;
    }

    DoctorReport { checks, ok }
}

/// Backing function for `codekurve_reindex` (design "reindex Gated Off by
/// Default", spec "reindex Gated Off by Default"): the same
/// setup/detect/apply_batch path `commands::index` drives, minus the
/// `println!`s — this crate's callers (the MCP server) must never write to
/// stdout outside their own JSON-RPC response. Runs a fresh `setup_index`
/// (its own `Connection`) rather than reusing `Session`'s — the caller (the
/// tool body) reopens its `Session` afterward so subsequent tool calls see
/// the refreshed index (works whether the session started `Indexed` or
/// `NotIndexed`).
pub fn reindex(root: &Path) -> Result<crate::incremental::BatchOutcome, CommandError> {
    let mut setup = commands::setup_index(root).map_err(CommandError::from)?;
    let force_full = commands::analyzer_version_changed(&setup.conn, &setup.project_id)
        .map_err(CommandError::from)?;
    let changes = crate::incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        None,
        force_full,
    )
    .map_err(CommandError::from)?;
    let ctx = crate::incremental::IndexContext {
        root: &setup.root,
        project_id: &setup.project_id,
        aliases: &setup.aliases,
        options: &setup.options,
        full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
    };
    let outcome = crate::incremental::apply_batch(&mut setup.conn, &ctx, &changes)
        .map_err(CommandError::from)?;
    codekurve_store::repo::set_analyzer_version(
        &setup.conn,
        &setup.project_id,
        commands::ANALYZER_VERSION,
    )
    .map_err(|e| CommandError::from(e.to_string()))?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 2.9: `envelope(.., None)` produces byte-identical JSON to
    /// today's `--json` golden shape — no `total` key, same five fields.
    #[test]
    fn envelope_without_total_matches_existing_shape() {
        let v = envelope(
            "demo",
            serde_json::json!([1, 2]),
            vec!["w".into()],
            true,
            None,
        );
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        assert!(!obj.contains_key("total"));
        assert_eq!(obj["schema_version"], 1);
        assert_eq!(obj["project"], "demo");
        assert_eq!(obj["truncated"], true);
    }

    /// `total` appears, and only, when `Some` — the MCP-facing shape.
    #[test]
    fn envelope_with_total_adds_one_key() {
        let v = envelope("demo", serde_json::json!([]), vec![], false, Some(3));
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 6);
        assert_eq!(obj["total"], 3);
    }

    /// Task 3.8: no `.codekurve/config.toml` at all — `Session::open` is
    /// fatal, exactly like `require_indexed_project` used to be.
    #[test]
    fn session_open_missing_config_is_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Session::open(tmp.path()).unwrap_err();
        assert_eq!(err.code, 4);
    }

    /// Task 3.8: config present (`init` ran) but no `codekurve index` yet —
    /// `Session::open` succeeds, degraded (`NotIndexed`), not `Err`.
    #[test]
    fn session_open_missing_db_is_not_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        codekurve_core::project::init(tmp.path()).unwrap();

        let session = Session::open(tmp.path()).unwrap();
        assert!(matches!(session, Session::NotIndexed { .. }));
        // The six graph-query commands still fail fatally (code 4) off this
        // degraded session — same behavior `require_indexed_project` gave.
        assert_eq!(session.indexed().unwrap_err().code, 4);
    }

    /// Task 3.7: `Session::warnings` (the `Indexed` arm) and
    /// `commands::warn_if_stale`'s direct call both go through
    /// [`pending_warning`] — same connection, same project id, same vec.
    #[test]
    fn warnings_wording_identical_regardless_of_caller() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/a.ts"),
            "export function f(): void {}\n",
        )
        .unwrap();
        codekurve_core::project::init(tmp.path()).unwrap();

        let mut setup = commands::setup_index(tmp.path()).unwrap();
        let changes = crate::incremental::detect(
            &setup.conn,
            &setup.project_id,
            &setup.root,
            &setup.options,
            None,
            false,
        )
        .unwrap();
        let ctx = crate::incremental::IndexContext {
            root: &setup.root,
            project_id: &setup.project_id,
            aliases: &setup.aliases,
            options: &setup.options,
            full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
        };
        crate::incremental::apply_batch(&mut setup.conn, &ctx, &changes).unwrap();
        setup
            .conn
            .execute(
                "UPDATE index_state SET pending_files = 2 WHERE project_id = ?1",
                [&setup.project_id],
            )
            .unwrap();

        let session = Session::open(tmp.path()).unwrap();
        let via_session = session.warnings();
        let (conn, project_id) = session.indexed().unwrap();
        let via_free_fn = pending_warning(conn, project_id);
        assert_eq!(via_session, via_free_fn);
        assert_eq!(
            via_session,
            vec!["index is stale (2 pending file(s)); run `codekurve index`".to_string()]
        );
    }
}
