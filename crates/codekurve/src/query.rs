//! Data-returning query layer (design "Print/return split"): every function
//! here returns structured data and never prints. `commands.rs` stays the
//! only module that owns stdout — it opens a [`Session`], calls into this
//! module, then prints the result exactly as before.
//!
//! PR2 scope: the six graph-query commands (`references`/`callers`/
//! `callees`/`implementations`/`trace`/`impact`) plus the shared `Session`
//! and `envelope`. `search`/`get_symbol`/`status`/`overview`/`doctor` and
//! `Session::warnings`/`NotIndexed` land in PR3 — `get_symbol` in particular
//! needs `StoredSymbol.id`/`find_symbol_by_id` (PR3 store additions), so
//! extracting it here would reach ahead of this PR's base.

use std::path::{Path, PathBuf};

use codekurve_core::Config;
use codekurve_store::repo::{self, StoredRelationship};
use codekurve_store::{traverse, Connection};

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

/// A project resolved and ready for querying — opened once per graph-query
/// command. PR3 adds the degraded `NotIndexed` variant and `warnings()`;
/// PR2's `open` keeps `require_indexed_project`'s current fatal-on-missing
/// behavior unchanged.
pub struct Session {
    pub root: PathBuf,
    pub config: Config,
    pub conn: Connection,
    pub project_id: String,
}

impl Session {
    /// Fatal (`Err`, code 4) on missing config/DB/project row — spec "Query
    /// before first index". Delegates to
    /// [`commands::require_indexed_project`] so both entry points share one
    /// preamble (including the stale-index stderr warning).
    pub fn open(root: &Path) -> Result<Self, CommandError> {
        let (root, config, conn, project_id) = commands::require_indexed_project(root)?;
        Ok(Self {
            root,
            config,
            conn,
            project_id,
        })
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
    let symbol_id =
        commands::resolve_symbol(&s.conn, &s.project_id, args.symbol_id, args.symbol_name)?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;
    let query_fn = match kind {
        RelKind::References => repo::references,
        RelKind::Callers => repo::callers,
        RelKind::Callees => repo::callees,
        RelKind::Implementations => repo::implementations,
    };

    let mut rows = query_fn(&s.conn, &s.project_id, &symbol_id, min_confidence)
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
    let from =
        commands::resolve_symbol(&s.conn, &s.project_id, args.symbol_id, args.symbol_name)?;
    let target = commands::resolve_symbol(&s.conn, &s.project_id, None, Some(to))?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;

    let adjacency = traverse::load_adjacency(&s.conn, &s.project_id, false)
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
    let symbol_id =
        commands::resolve_symbol(&s.conn, &s.project_id, args.symbol_id, args.symbol_name)?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;

    let adjacency = traverse::load_adjacency(&s.conn, &s.project_id, true)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 2.9: `envelope(.., None)` produces byte-identical JSON to
    /// today's `--json` golden shape — no `total` key, same five fields.
    #[test]
    fn envelope_without_total_matches_existing_shape() {
        let v = envelope("demo", serde_json::json!([1, 2]), vec!["w".into()], true, None);
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
}
