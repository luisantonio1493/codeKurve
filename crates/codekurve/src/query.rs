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

use codekurve_core::Config;
use codekurve_store::repo::{self, StoredRelationship, StoredSymbol};
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
            Ok(OverviewData {
                files: st.files,
                symbols: st.symbols,
                relationships: st.relationships,
                languages,
            })
        }
        Session::NotIndexed { .. } => Ok(OverviewData {
            files: 0,
            symbols: 0,
            relationships: 0,
            languages: Vec::new(),
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
