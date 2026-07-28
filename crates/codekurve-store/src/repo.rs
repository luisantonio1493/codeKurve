//! Repository operations for the Phase 1 vertical slice: register a project,
//! rebuild its index transactionally, and query symbols. See
//! CODEKURVE_MASTER_PLAN.md §24 (schema) and §Fase 1 (scope).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use codekurve_core::{Confidence, Provenance, RelationshipKind, SourceSpan, Symbol, SymbolKind};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::error::Result;

/// One discovered file plus the symbols extracted from it.
pub struct FileInput {
    pub relative_path: String,
    pub language: String,
    pub size_bytes: u64,
    /// BLAKE3 digest of the file's bytes at read time (Phase 3 "Content Hash
    /// Tracked Per File") — the change-detection engine's confirm step.
    pub content_hash: String,
    /// Filesystem mtime, nanoseconds since epoch, at read time.
    pub modified_ns: i64,
    pub symbols: Vec<Symbol>,
}

/// One relationship edge to persist (§24.2 `relationships`). Ids are
/// already resolved by the caller — `reindex` only stores rows, it never
/// looks up or infers a target. ponytail: no caller populates this yet;
/// intra-file extraction (PR3) and cross-file resolution (PR4b) decide how
/// ids get resolved before calling `reindex`.
pub struct RelationshipInput {
    pub source_symbol_id: String,
    pub target_symbol_id: Option<String>,
    pub target_external: Option<String>,
    pub kind: RelationshipKind,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub source_file_id: String,
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    pub reason: Option<String>,
}

/// One unresolved reference to persist (§24.2 `unresolved_references`) —
/// zero-candidate or insufficient-context targets that never become a
/// `relationships` row (§27.4: never silently pick first).
pub struct UnresolvedReferenceInput {
    pub source_symbol_id: Option<String>,
    pub source_file_id: String,
    pub relationship_kind: RelationshipKind,
    pub target_text: String,
    pub context_json: Option<String>,
    pub candidate_count: u32,
    pub reason: String,
    pub confidence: Confidence,
}

/// Counts returned by a reindex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexOutcome {
    pub files: usize,
    pub symbols: usize,
}

/// A symbol as read back from storage (display model). `id` (PR3, additive):
/// lets a caller chain a `search`/`find_by_name` hit straight into
/// `find_symbol_by_id` — CLI text output is unaffected since it only prints
/// name/kind/path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSymbol {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub language: String,
    pub relative_path: String,
    pub span: SourceSpan,
}

/// Deterministic file storage id. Exposed so a caller (the `codekurve`
/// composition root, PR4b) can precompute relationship source/target ids
/// before the DB round-trip that `reindex` performs — `reindex` uses this
/// same function internally, so the two never drift apart.
pub fn file_id(project_id: &str, relative_path: &str) -> String {
    hash_id("fil", &format!("{project_id}/{relative_path}"))
}

/// §16.3 + Phase 3: BLAKE3 over the 5-tuple, still excluding `start_byte`.
/// `\x1f` (unit separator) delimits components so a path containing `/`
/// cannot shift a boundary and forge another symbol's key.
pub fn symbol_key(
    language: &str,
    relative_path: &str,
    kind: &str,
    qualified_name: &str,
    signature_fingerprint: &str,
) -> String {
    let input = format!(
        "{language}\u{1f}{relative_path}\u{1f}{kind}\u{1f}{qualified_name}\u{1f}{signature_fingerprint}"
    );
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Deterministic symbol storage id from a precomputed `file_id`/`symbol_key`
/// — see `file_id`'s doc comment.
pub fn symbol_id(file_id: &str, symbol_key: &str) -> String {
    hash_id("sym", &format!("{file_id}/{symbol_key}"))
}

/// Register or update a project by its canonical root path. Returns its id.
pub fn upsert_project(
    conn: &Connection,
    name: &str,
    root_path: &str,
    config_hash: &str,
) -> Result<String> {
    let id = hash_id("prj", root_path);
    let ts = now_ts();
    conn.execute(
        "INSERT INTO projects(id, name, root_path, config_hash, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(root_path) DO UPDATE SET
             name = excluded.name,
             config_hash = excluded.config_hash,
             updated_at = excluded.updated_at",
        params![id, name, root_path, config_hash, ts],
    )?;
    let id = conn.query_row(
        "SELECT id FROM projects WHERE root_path = ?1",
        params![root_path],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Rebuild the whole index for a project in a single transaction. The index is
/// disposable (§5.5): the previous generation is wiped and replaced.
pub fn reindex(
    conn: &mut Connection,
    project_id: &str,
    files: &[FileInput],
    relationships: &[RelationshipInput],
    unresolved: &[UnresolvedReferenceInput],
) -> Result<IndexOutcome> {
    let tx = conn.transaction()?;

    // Relationships/unresolved first: they FK-reference symbols/files, so
    // they must go before those tables are cleared (foreign_keys = ON).
    tx.execute(
        "DELETE FROM relationships WHERE project_id = ?1",
        params![project_id],
    )?;
    tx.execute(
        "DELETE FROM unresolved_references WHERE project_id = ?1",
        params![project_id],
    )?;
    tx.execute(
        "DELETE FROM symbols_fts WHERE symbol_id IN
             (SELECT id FROM symbols WHERE project_id = ?1)",
        params![project_id],
    )?;
    tx.execute(
        "DELETE FROM symbols WHERE project_id = ?1",
        params![project_id],
    )?;
    tx.execute(
        "DELETE FROM files WHERE project_id = ?1",
        params![project_id],
    )?;

    let ts = now_ts();
    let mut outcome = IndexOutcome {
        files: 0,
        symbols: 0,
    };

    for file in files {
        outcome.files += 1;
        outcome.symbols += insert_file_and_symbols(&tx, project_id, &ts, file)?;
    }

    persist_relationships(&tx, project_id, &ts, relationships)?;
    persist_unresolved(&tx, project_id, &ts, unresolved)?;
    // Phase 3 "Freshness Metadata Written Inside the Data Transaction": a
    // full reindex is itself a (maximal) batch, so it verifies the whole
    // project and clears any pending count in the same transaction.
    mark_verified(&tx, project_id, &ts)?;

    tx.commit()?;
    Ok(outcome)
}

/// Inserts (or, via `ON CONFLICT`, updates) one file's row and every symbol
/// it owns. Shared by `reindex` (whole project, prior rows already wiped by
/// the caller) and `apply_incremental` (one changed file at a time, prior
/// rows cleared by `delete_file_owned_rows` first) — both need identical
/// files/symbols/symbols_fts insert logic. Returns the symbol count written.
fn insert_file_and_symbols(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    file: &FileInput,
) -> Result<usize> {
    let file_id = file_id(project_id, &file.relative_path);
    tx.execute(
        "INSERT INTO files(id, project_id, relative_path, language, size_bytes,
             content_hash, modified_ns, parse_status, generation, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ok', 1, ?8, ?8)
         ON CONFLICT(project_id, relative_path) DO UPDATE SET
             language = excluded.language,
             size_bytes = excluded.size_bytes,
             content_hash = excluded.content_hash,
             modified_ns = excluded.modified_ns,
             updated_at = excluded.updated_at",
        params![
            file_id,
            project_id,
            file.relative_path,
            file.language,
            file.size_bytes as i64,
            file.content_hash,
            file.modified_ns,
            ts,
        ],
    )?;

    let mut count = 0;
    for symbol in &file.symbols {
        let symbol_key = symbol_key(
            symbol.language.as_str(),
            &file.relative_path,
            symbol.kind.as_str(),
            &symbol.qualified_name,
            &symbol.signature_fingerprint,
        );
        let symbol_id = symbol_id(&file_id, &symbol_key);
        let qualified = &symbol.qualified_name;
        tx.execute(
            "INSERT INTO symbols(id, project_id, file_id, symbol_key, name, qualified_name,
                 kind, language, start_byte, end_byte, start_line, start_column,
                 end_line, end_column, provenance, confidence, generation,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 'tree-sitter', 'high', 1, ?15, ?15)",
            params![
                symbol_id,
                project_id,
                file_id,
                symbol_key,
                symbol.name,
                qualified,
                symbol.kind.as_str(),
                symbol.language.as_str(),
                symbol.span.start_byte as i64,
                symbol.span.end_byte as i64,
                symbol.span.start_line as i64,
                symbol.span.start_column as i64,
                symbol.span.end_line as i64,
                symbol.span.end_column as i64,
                ts,
            ],
        )?;
        tx.execute(
            "INSERT INTO symbols_fts(symbol_id, name, qualified_name, kind, relative_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                symbol_id,
                symbol.name,
                qualified,
                symbol.kind.as_str(),
                file.relative_path
            ],
        )?;
        count += 1;
    }
    Ok(count)
}

/// Deletes one file's own symbols (+ FTS shadow) and its own outbound
/// relationships/unresolved rows (as source). Does NOT touch the `files` row
/// itself — `insert_file_and_symbols`'s `ON CONFLICT` upserts it, and
/// `delete_file` removes it outright for a genuine delete.
fn delete_file_owned_rows(tx: &Transaction, project_id: &str, file_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM relationships WHERE project_id = ?1 AND source_file_id = ?2",
        params![project_id, file_id],
    )?;
    tx.execute(
        "DELETE FROM unresolved_references WHERE project_id = ?1 AND source_file_id = ?2",
        params![project_id, file_id],
    )?;
    tx.execute(
        "DELETE FROM symbols_fts WHERE symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
        params![file_id],
    )?;
    tx.execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])?;
    Ok(())
}

/// Task 5.2 (spec "Per-File Delete Removes Symbols and Converts Inbound
/// Edges to Unresolved"): removes a tracked file entirely — its own symbols,
/// its own outbound edges, and its `files` row — and converts every inbound
/// edge (a relationship from another file targeting one of this file's
/// symbols) into an `unresolved_references` row instead of silently
/// dropping it. `target_text` uses the target symbol's bare `name`, mirroring
/// how by-name resolution (`resolve_by_name`/`resolve_binding`) already
/// looks targets up.
pub fn delete_file(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    relative_path: &str,
) -> Result<()> {
    let file_id = file_id(project_id, relative_path);

    let mut stmt = tx.prepare(
        "SELECT r.id, r.source_symbol_id, r.source_file_id, r.kind, s.name
         FROM relationships r
         JOIN symbols s ON s.id = r.target_symbol_id
         WHERE r.project_id = ?1 AND s.file_id = ?2 AND r.source_file_id != ?2",
    )?;
    let inbound: Vec<(String, String, String, String, String)> = stmt
        .query_map(params![project_id, file_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let mut unresolved = Vec::with_capacity(inbound.len());
    for (rel_id, source_symbol_id, source_file_id, kind, target_name) in &inbound {
        tx.execute("DELETE FROM relationships WHERE id = ?1", params![rel_id])?;
        unresolved.push(UnresolvedReferenceInput {
            source_symbol_id: Some(source_symbol_id.clone()),
            source_file_id: source_file_id.clone(),
            relationship_kind: parse_relationship_kind(kind),
            target_text: target_name.clone(),
            context_json: None,
            candidate_count: 0,
            reason: "target symbol's file was deleted".to_string(),
            confidence: Confidence::Unresolved,
        });
    }
    persist_unresolved(tx, project_id, ts, &unresolved)?;

    delete_file_owned_rows(tx, project_id, &file_id)?;
    tx.execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
    Ok(())
}

/// Task 5.3 (design "Batch Atomicity" T2): applies one incremental batch's
/// data changes in the caller's transaction — every `deleted` path via
/// [`delete_file`], then every `files[i]`'s stale rows replaced by its fresh
/// ones via [`insert_file_and_symbols`]. `relationships`/`unresolved` cover
/// every file in `files` together in one call each, keeping `stable_id`'s
/// per-call ordinal (and therefore every row's id) identical to a full
/// `reindex` over the same rows (design "Stable Row Ids" invariant). Does
/// NOT write `index_state` — the caller stamps `pending_files`/
/// `last_verified_at` in the same transaction via [`mark_verified`] before
/// committing, so freshness metadata and data changes can never disagree.
#[allow(clippy::too_many_arguments)]
pub fn apply_incremental(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    files: &[FileInput],
    relationships: &[RelationshipInput],
    unresolved: &[UnresolvedReferenceInput],
    deleted: &[String],
) -> Result<usize> {
    for path in deleted {
        delete_file(tx, project_id, ts, path)?;
    }
    for file in files {
        delete_file_owned_rows(tx, project_id, &file_id(project_id, &file.relative_path))?;
    }

    let mut count = 0;
    for file in files {
        count += insert_file_and_symbols(tx, project_id, ts, file)?;
    }
    persist_relationships(tx, project_id, ts, relationships)?;
    persist_unresolved(tx, project_id, ts, unresolved)?;
    Ok(count)
}

/// Task 5.2 (`index_state` upsert): publishes the batch's pending count
/// (T1 of the Batch Atomicity sequence) so an interrupted run honestly
/// reports staleness until T2 commits.
pub fn set_pending_files(tx: &Transaction, project_id: &str, ts: &str, count: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO index_state(project_id, pending_files, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id) DO UPDATE SET
             pending_files = excluded.pending_files,
             updated_at = excluded.updated_at",
        params![project_id, count, ts],
    )?;
    Ok(())
}

/// Task 5.2/5.3 (`index_state` upsert, T2 of Batch Atomicity): clears the
/// pending count and stamps `last_verified_at`, meant to be called inside
/// the same transaction as the batch's data writes (never a separate one —
/// spec "Freshness Metadata Written Inside the Data Transaction").
pub fn mark_verified(tx: &Transaction, project_id: &str, ts: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO index_state(project_id, pending_files, last_verified_at, updated_at)
         VALUES (?1, 0, ?2, ?2)
         ON CONFLICT(project_id) DO UPDATE SET
             pending_files = 0,
             last_verified_at = excluded.last_verified_at,
             updated_at = excluded.updated_at",
        params![project_id, ts],
    )?;
    Ok(())
}

/// Count of tracked files, for the oversized-batch fallback check (task 5.4)
/// — cheap, avoids a full row read just to size a threshold comparison.
pub fn count_files(conn: &Connection, project_id: &str) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

/// `codekurve status` (task 7.1) and the stale-warning helper (task 7.2):
/// counts plus stored freshness metadata, read without a filesystem walk
/// (spec "Status Command Reports Pending Count and Last Verified Time").
/// `last_verified_at`/`pending_files` default to "never verified"/`0` when
/// `index_state` has no row yet (pre-Phase-3 project, first index in flight).
pub struct IndexStatus {
    pub files: usize,
    pub symbols: usize,
    pub relationships: usize,
    /// Relationships whose `confidence = 'unresolved'` (design's `status`
    /// example: "relationships: 9214 (1204 unresolved)") — a different count
    /// from the `unresolved_references` table, which holds zero-candidate
    /// references that never became a `relationships` row at all.
    pub relationships_unresolved: usize,
    pub pending_files: i64,
    pub last_verified_at: Option<String>,
}

pub fn index_status(conn: &Connection, project_id: &str) -> Result<IndexStatus> {
    let symbols: i64 = conn.query_row(
        "SELECT COUNT(*) FROM symbols WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    let relationships: i64 = conn.query_row(
        "SELECT COUNT(*) FROM relationships WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    let relationships_unresolved: i64 = conn.query_row(
        "SELECT COUNT(*) FROM relationships WHERE project_id = ?1 AND confidence = 'unresolved'",
        params![project_id],
        |row| row.get(0),
    )?;
    let (pending_files, last_verified_at) = conn
        .query_row(
            "SELECT pending_files, last_verified_at FROM index_state WHERE project_id = ?1",
            params![project_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .unwrap_or((0, None));
    Ok(IndexStatus {
        files: count_files(conn, project_id)?,
        symbols: symbols as usize,
        relationships: relationships as usize,
        relationships_unresolved: relationships_unresolved as usize,
        pending_files,
        last_verified_at,
    })
}

/// Every currently stored symbol id owned by one of `relative_paths` — the
/// "old" ids a changed/deleted file's symbols held before this batch,
/// needed as the `target_symbol_ids` input to `dependents_by_target_symbol`
/// (design "Dependent Re-Resolution Scope", trigger "B removed/renamed a
/// symbol"). `file_id` is deterministic, so no path->id join query is
/// needed to build the `file_id IN (...)` list.
pub fn symbol_ids_for_files(
    conn: &Connection,
    project_id: &str,
    relative_paths: &[String],
) -> Result<Vec<String>> {
    if relative_paths.is_empty() {
        return Ok(Vec::new());
    }
    let file_ids: Vec<String> = relative_paths
        .iter()
        .map(|p| file_id(project_id, p))
        .collect();
    let placeholders = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql =
        format!("SELECT id FROM symbols WHERE project_id = ? AND file_id IN ({placeholders})");
    let mut binds: Vec<&str> = vec![project_id];
    binds.extend(file_ids.iter().map(String::as_str));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter().copied()), |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every stored symbol id for a set of `(relative_path, qualified_name)`
/// pairs — resolves an `apply_batch` batch's edges that target a *baseline*
/// symbol (a file outside `B ∪ D`, not re-parsed this batch, so it has no
/// freshly computed id to look up locally). One query per pair: the pair
/// count is bounded by the batch's own edge count, not project size, so a
/// join over an IN-tuple list isn't worth the complexity here.
pub fn symbol_ids_by_qualified_names(
    conn: &Connection,
    project_id: &str,
    pairs: &[(String, String)],
) -> Result<HashMap<(String, String), String>> {
    let mut out = HashMap::new();
    if pairs.is_empty() {
        return Ok(out);
    }
    let mut stmt = conn.prepare(
        "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND f.relative_path = ?2 AND s.qualified_name = ?3",
    )?;
    for (file, qualified_name) in pairs {
        if let Ok(id) = stmt.query_row(params![project_id, file, qualified_name], |row| {
            row.get::<_, String>(0)
        }) {
            out.insert((file.clone(), qualified_name.clone()), id);
        }
    }
    Ok(out)
}

/// Per-file metadata read back for change detection (task 5.1's `detect`):
/// the stored size/hash/mtime to compare against the file on disk.
#[derive(Debug, Clone)]
pub struct StoredFileMeta {
    pub size_bytes: u64,
    pub content_hash: Option<String>,
    pub modified_ns: Option<i64>,
}

/// Every tracked file's change-detection metadata, keyed by relative path —
/// `detect`'s single read of "what does storage currently believe" before
/// walking the filesystem (design "Shared Change Detection Engine").
pub fn file_snapshot(
    conn: &Connection,
    project_id: &str,
) -> Result<HashMap<String, StoredFileMeta>> {
    let mut stmt = conn.prepare(
        "SELECT relative_path, size_bytes, content_hash, modified_ns
         FROM files WHERE project_id = ?1",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            StoredFileMeta {
                size_bytes: row.get::<_, i64>(1)? as u64,
                content_hash: row.get(2)?,
                modified_ns: row.get(3)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// Content-derived id; the ordinal only disambiguates rows whose entire
/// content tuple is identical (e.g. the same call twice on one line).
/// Design "Stable Row Ids": replaces deriving `rel`/`unr` ids from a
/// positional index, which churns under per-batch (incremental) persistence
/// since the same logical row lands at a different index per batch.
fn stable_id(prefix: &str, seen: &mut HashMap<String, u32>, parts: &[&str]) -> String {
    let base = parts.join("\u{1f}");
    let n = seen.entry(base.clone()).or_default();
    let id = hash_id(prefix, &format!("{base}\u{1f}{n}"));
    *n += 1;
    id
}

fn persist_relationships(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    relationships: &[RelationshipInput],
) -> Result<()> {
    let mut seen = HashMap::new();
    for rel in relationships {
        let target = rel
            .target_symbol_id
            .as_deref()
            .or(rel.target_external.as_deref())
            .unwrap_or("");
        let start_line = rel.start_line.map(|v| v.to_string()).unwrap_or_default();
        let start_column = rel.start_column.map(|v| v.to_string()).unwrap_or_default();
        let id = stable_id(
            "rel",
            &mut seen,
            &[
                project_id,
                &rel.source_symbol_id,
                rel.kind.as_str(),
                target,
                &start_line,
                &start_column,
            ],
        );
        tx.execute(
            "INSERT INTO relationships(id, project_id, source_symbol_id, target_symbol_id,
                 target_external, kind, provenance, confidence, source_file_id,
                 start_line, start_column, reason, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?13)",
            params![
                id,
                project_id,
                rel.source_symbol_id,
                rel.target_symbol_id,
                rel.target_external,
                rel.kind.as_str(),
                rel.provenance.as_str(),
                rel.confidence.as_str(),
                rel.source_file_id,
                rel.start_line,
                rel.start_column,
                rel.reason,
                ts,
            ],
        )?;
    }
    Ok(())
}

fn persist_unresolved(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    unresolved: &[UnresolvedReferenceInput],
) -> Result<()> {
    let mut seen = HashMap::new();
    for u in unresolved {
        let source_symbol = u.source_symbol_id.as_deref().unwrap_or("");
        let id = stable_id(
            "unr",
            &mut seen,
            &[
                project_id,
                &u.source_file_id,
                source_symbol,
                u.relationship_kind.as_str(),
                &u.target_text,
            ],
        );
        tx.execute(
            "INSERT INTO unresolved_references(id, project_id, source_symbol_id, source_file_id,
                 relationship_kind, target_text, context_json, candidate_count, reason,
                 confidence, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?11)",
            params![
                id,
                project_id,
                u.source_symbol_id,
                u.source_file_id,
                u.relationship_kind.as_str(),
                u.target_text,
                u.context_json,
                u.candidate_count,
                u.reason,
                u.confidence.as_str(),
                ts,
            ],
        )?;
    }
    Ok(())
}

/// A relationship edge as read back from storage, with both endpoints'
/// qualified names already joined — the `references`/`callers`/`callees`/
/// `implementations` display model (mirrors `StoredSymbol`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRelationship {
    pub source_symbol_id: String,
    pub source_qualified_name: String,
    pub target_symbol_id: Option<String>,
    pub target_qualified_name: Option<String>,
    pub target_external: Option<String>,
    pub kind: String,
    pub provenance: String,
    pub confidence: String,
    pub start_line: Option<u32>,
    pub start_column: Option<u32>,
    /// The source symbol's file (PR5, MCP §28.3 row shape: every query-tool
    /// row needs a `path`) — additive column, existing CLI JSON printing
    /// (`relationship_json`) selects fields by name and is unaffected.
    pub source_relative_path: String,
}

/// A bare-name match for CLI ambiguity handling (§27.4): a bare-name query
/// hitting more than one symbol must list every candidate's id + qualified
/// name, never silently pick one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolCandidate {
    pub id: String,
    pub qualified_name: String,
    pub kind: String,
    pub relative_path: String,
}

/// Relative confidence ordering for `--min-confidence` filtering — higher is
/// stricter, matching declaration order in §17.5 (Exact > High > Medium >
/// Low > Unresolved). A free fn over raw strings, not the `Confidence` enum,
/// because stored rows read `confidence` back as text (`pub(crate)`: reused
/// by `traverse.rs`'s BFS edge filtering).
pub(crate) fn confidence_rank(s: &str) -> u8 {
    match s {
        "exact" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0, // "unresolved" or unrecognized
    }
}

/// All relationships that target `symbol_id`, any kind — "who references
/// this symbol" (the `references` CLI command, PR5b).
pub fn references(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
) -> Result<Vec<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[],
        min_confidence,
    )
}

/// Call sites that call `symbol_id` (`kind = calls`, target = the callee) —
/// the `callers` CLI command.
pub fn callers(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
) -> Result<Vec<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Calls],
        min_confidence,
    )
}

/// Calls made *by* `symbol_id` (`kind = calls`, source = the caller) — the
/// `callees` CLI command.
pub fn callees(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
) -> Result<Vec<StoredRelationship>> {
    query_relationships(
        conn,
        "source_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Calls],
        min_confidence,
    )
}

/// Symbols that `implements` or `inherits`/`extends` `symbol_id` — the
/// `implementations` CLI command.
pub fn implementations(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
) -> Result<Vec<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Implements, RelationshipKind::Inherits],
        min_confidence,
    )
}

/// Shared query behind `references`/`callers`/`callees`/`implementations`:
/// one indexed SELECT (`idx_relationships_source_kind`/`_target_kind` covers
/// `column = ?` [`AND kind IN (...)`]) joined to both endpoints' symbol rows
/// for display, then an in-memory `min_confidence` filter (row counts per
/// symbol are small; no need for a second query or SQL rank `CASE`).
fn query_relationships(
    conn: &Connection,
    column: &'static str,
    project_id: &str,
    symbol_id: &str,
    kinds: &[RelationshipKind],
    min_confidence: Option<Confidence>,
) -> Result<Vec<StoredRelationship>> {
    let kind_filter = if kinds.is_empty() {
        String::new()
    } else {
        let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        format!(" AND r.kind IN ({placeholders})")
    };
    let sql = format!(
        "SELECT r.source_symbol_id, src.qualified_name, r.target_symbol_id, tgt.qualified_name,
                r.target_external, r.kind, r.provenance, r.confidence, r.start_line, r.start_column,
                src_file.relative_path
         FROM relationships r
         JOIN symbols src ON src.id = r.source_symbol_id
         JOIN files src_file ON src_file.id = src.file_id
         LEFT JOIN symbols tgt ON tgt.id = r.target_symbol_id
         WHERE r.project_id = ? AND r.{column} = ?{kind_filter}
         ORDER BY r.start_line"
    );
    let mut binds: Vec<&str> = vec![project_id, symbol_id];
    binds.extend(kinds.iter().map(|k| k.as_str()));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(binds.iter().copied()),
        map_relationship,
    )?;
    let mut result = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some(min) = min_confidence {
        let threshold = confidence_rank(min.as_str());
        result.retain(|r| confidence_rank(&r.confidence) >= threshold);
    }
    Ok(result)
}

fn map_relationship(row: &Row) -> rusqlite::Result<StoredRelationship> {
    Ok(StoredRelationship {
        source_symbol_id: row.get(0)?,
        source_qualified_name: row.get(1)?,
        target_symbol_id: row.get(2)?,
        target_qualified_name: row.get(3)?,
        target_external: row.get(4)?,
        kind: row.get(5)?,
        provenance: row.get(6)?,
        confidence: row.get(7)?,
        start_line: row.get::<_, Option<i64>>(8)?.map(|v| v as u32),
        start_column: row.get::<_, Option<i64>>(9)?.map(|v| v as u32),
        source_relative_path: row.get(10)?,
    })
}

/// All symbols named `name`, for CLI bare-name ambiguity handling (§27.4).
/// `find_by_name` already lists exact-name matches for the `symbol` command
/// but doesn't expose `id` — the graph query commands need it to resolve
/// `--symbol-name` into a concrete `--symbol-id` (or list candidates + exit
/// 6 when there's more than one).
pub fn find_candidates_by_name(
    conn: &Connection,
    project_id: &str,
    name: &str,
) -> Result<Vec<SymbolCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.qualified_name, s.kind, f.relative_path
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND s.name = ?2
         ORDER BY s.qualified_name",
    )?;
    let rows = stmt.query_map(params![project_id, name], |row| {
        Ok(SymbolCandidate {
            id: row.get(0)?,
            qualified_name: row.get(1)?,
            kind: row.get(2)?,
            relative_path: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Full-text search over symbol name/qualified-name/kind/path (§24.3).
pub fn search(
    conn: &Connection,
    project_id: &str,
    query: &str,
    limit: u32,
) -> Result<Vec<StoredSymbol>> {
    // ponytail: quote as a phrase to neutralize FTS5 operators; richer
    // tokenization/prefix search can come later.
    let fts_query = format!("\"{}\"", query.replace('"', "\"\""));
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?2
           AND s.id IN (SELECT symbol_id FROM symbols_fts WHERE symbols_fts MATCH ?1)
         ORDER BY s.name
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![fts_query, project_id, limit], map_stored)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Look up a project id by its canonical root path.
pub fn find_project(conn: &Connection, root_path: &str) -> Result<Option<String>> {
    let id = conn
        .query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            params![root_path],
            |row| row.get(0),
        )
        .ok();
    Ok(id)
}

/// Exact-name lookup for the `symbol` command.
pub fn find_by_name(conn: &Connection, project_id: &str, name: &str) -> Result<Vec<StoredSymbol>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND s.name = ?2
         ORDER BY f.relative_path, s.start_byte",
    )?;
    let rows = stmt.query_map(params![project_id, name], map_stored)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One symbol by its storage id (PR3, `get_symbol`'s data source): `None`
/// when the id doesn't resolve, rather than an error — the caller decides
/// whether that's a hard failure.
pub fn find_symbol_by_id(conn: &Connection, id: &str) -> Result<Option<StoredSymbol>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.id = ?1",
    )?;
    Ok(stmt.query_row(params![id], map_stored).optional()?)
}

/// Per-language file counts for `project_overview` (PR3 store addition) —
/// `files.language` is nullable (parse failures/unrecognized extensions),
/// grouped under `"unknown"` rather than dropped.
pub fn language_breakdown(conn: &Connection, project_id: &str) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(language, 'unknown') AS lang, COUNT(*)
         FROM files
         WHERE project_id = ?1
         GROUP BY lang
         ORDER BY COUNT(*) DESC, lang",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One stored symbol as needed by `codekurve-analysis::resolve::BaselineSymbol`
/// (design "Baseline for re-resolution", task 4.3). `codekurve-store` never
/// depends on `codekurve-analysis` — the caller (`codekurve/src/commands.rs`,
/// task 4.4) maps this into the analysis-side type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineSymbolRow {
    pub name: String,
    pub relative_path: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    /// Mirrors `SymbolTable::build`'s own `exports` fallback (resolve.rs):
    /// true when a persisted `kind = 'exports'` relationship targets this
    /// symbol, i.e. it was resolvable as an import binding at index time.
    pub exported: bool,
}

/// Every currently stored file path plus symbol row for `project_id` —
/// everything `resolve::ProjectBaseline` needs (task 4.3). Module stand-in
/// symbols (`kind = 'module'`, `commands.rs`'s `module_symbol`) are excluded:
/// they're a storage-only construct, never part of a fresh
/// `SymbolTable::build`'s input.
#[derive(Debug, Clone, Default)]
pub struct ResolutionSnapshot {
    pub files: Vec<String>,
    pub symbols: Vec<BaselineSymbolRow>,
}

/// Reads back the whole-project baseline `resolve::resolve_with` needs to
/// re-resolve an incremental batch without re-parsing every already-indexed
/// file (design "Baseline for re-resolution"). Unlike the dependent-set
/// queries below, this isn't scoped to a changed set — it's the full prior
/// index, cheap to read back (design: "Reading rows is far cheaper than
/// tree-sitter").
pub fn resolution_snapshot(conn: &Connection, project_id: &str) -> Result<ResolutionSnapshot> {
    let mut file_stmt = conn.prepare("SELECT relative_path FROM files WHERE project_id = ?1")?;
    let files = file_stmt
        .query_map(params![project_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut sym_stmt = conn.prepare(
        "SELECT s.name, f.relative_path, s.qualified_name, s.kind,
                EXISTS(SELECT 1 FROM relationships r
                       WHERE r.target_symbol_id = s.id AND r.kind = 'exports') AS exported
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND s.kind != 'module'",
    )?;
    let symbols = sym_stmt
        .query_map(params![project_id], |row| {
            let kind: String = row.get(3)?;
            Ok(BaselineSymbolRow {
                name: row.get(0)?,
                relative_path: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: parse_symbol_kind(&kind),
                exported: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(ResolutionSnapshot { files, symbols })
}

/// Dependent-set query 1 (design "Dependent Re-Resolution Scope", trigger
/// "B removed/renamed a symbol"): every file (outside `changed_files`, i.e.
/// `B`) with a relationship pointing at one of `target_symbol_ids` — a
/// symbol the incremental batch just changed or deleted. Bounded by
/// `idx_relationships_target_kind`: `target_symbol_id` is its leading
/// column, so an IN-list lookup stays a set of index seeks.
pub fn dependents_by_target_symbol(
    conn: &Connection,
    project_id: &str,
    target_symbol_ids: &[String],
    changed_files: &[String],
) -> Result<Vec<String>> {
    if target_symbol_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = target_symbol_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT f.relative_path
         FROM relationships r
         JOIN files f ON f.id = r.source_file_id
         WHERE r.project_id = ? AND r.target_symbol_id IN ({placeholders})"
    );
    let mut binds: Vec<&str> = vec![project_id];
    binds.extend(target_symbol_ids.iter().map(String::as_str));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter().copied()), |row| {
        row.get::<_, String>(0)
    })?;
    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    out.retain(|path| !changed_files.iter().any(|f| f == path));
    Ok(out)
}

/// Dependent-set query 2 (design "Dependent Re-Resolution Scope", trigger
/// "B added a symbol / new file others import"): every file with an
/// `unresolved_references` row whose `target_text` matches one of
/// `target_texts` (names/module specifiers the incremental batch might now
/// satisfy). Bounded by `idx_unresolved_project_target`
/// (`project_id, target_text`).
pub fn dependents_by_unresolved_target(
    conn: &Connection,
    project_id: &str,
    target_texts: &[String],
) -> Result<Vec<String>> {
    if target_texts.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = target_texts
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT f.relative_path
         FROM unresolved_references u
         JOIN files f ON f.id = u.source_file_id
         WHERE u.project_id = ? AND u.target_text IN ({placeholders})"
    );
    let mut binds: Vec<&str> = vec![project_id];
    binds.extend(target_texts.iter().map(String::as_str));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter().copied()), |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Stable BLAKE3 content hash of the config text.
pub fn config_hash(config_text: &str) -> String {
    blake3::hash(config_text.as_bytes()).to_hex().to_string()
}

/// BLAKE3 content hash of a file's bytes (Phase 3: `files.content_hash`,
/// the change-detection engine's confirm step after the mtime/size fast
/// path).
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn map_stored(row: &Row) -> rusqlite::Result<StoredSymbol> {
    Ok(StoredSymbol {
        id: row.get(0)?,
        name: row.get(1)?,
        qualified_name: row.get(2)?,
        kind: row.get(3)?,
        language: row.get(4)?,
        relative_path: row.get(5)?,
        span: SourceSpan {
            start_byte: row.get::<_, i64>(6)? as usize,
            end_byte: row.get::<_, i64>(7)? as usize,
            start_line: row.get::<_, i64>(8)? as usize,
            start_column: row.get::<_, i64>(9)? as usize,
            end_line: row.get::<_, i64>(10)? as usize,
            end_column: row.get::<_, i64>(11)? as usize,
        },
    })
}

/// Reverse of `SymbolKind::as_str` (codekurve-core), for reading `symbols.kind`
/// text columns back into the enum (`resolution_snapshot`). Every stored
/// value was written by `as_str` itself (`reindex`), so the fallback is
/// unreachable in practice — kept total rather than panicking on a read path.
fn parse_symbol_kind(s: &str) -> SymbolKind {
    match s {
        "namespace" => SymbolKind::Namespace,
        "class" => SymbolKind::Class,
        "interface" => SymbolKind::Interface,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "constructor" => SymbolKind::Constructor,
        "property" => SymbolKind::Property,
        "field" => SymbolKind::Field,
        "variable" => SymbolKind::Variable,
        "parameter" => SymbolKind::Parameter,
        "typealias" => SymbolKind::TypeAlias,
        "import" => SymbolKind::Import,
        "export" => SymbolKind::Export,
        _ => SymbolKind::Module,
    }
}

/// Reverse of `RelationshipKind::as_str` (codekurve-core), for reconstructing
/// an `UnresolvedReferenceInput` from a `relationships.kind` text column
/// (`delete_file`'s inbound-edge conversion). Every stored value was written
/// by `as_str` itself, so the fallback is unreachable in practice — kept
/// total rather than panicking on a read path, mirroring `parse_symbol_kind`.
fn parse_relationship_kind(s: &str) -> RelationshipKind {
    match s {
        "defines" => RelationshipKind::Defines,
        "contains" => RelationshipKind::Contains,
        "imports" => RelationshipKind::Imports,
        "exports" => RelationshipKind::Exports,
        "references" => RelationshipKind::References,
        "calls" => RelationshipKind::Calls,
        "constructs" => RelationshipKind::Constructs,
        "inherits" => RelationshipKind::Inherits,
        "implements" => RelationshipKind::Implements,
        "overrides" => RelationshipKind::Overrides,
        "usestype" => RelationshipKind::UsesType,
        "reads" => RelationshipKind::Reads,
        _ => RelationshipKind::Writes,
    }
}

fn hash_id(prefix: &str, input: &str) -> String {
    format!(
        "{prefix}-{}",
        &blake3::hash(input.as_bytes()).to_hex()[..32]
    )
}

/// Public (task 5.3): `apply_batch`'s T1/T2 timestamps need the same clock
/// `reindex` uses internally, from the binary crate.
pub fn now_ts() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use codekurve_core::{LanguageId, SymbolKind};

    fn symbol(name: &str, kind: SymbolKind, start_byte: usize) -> Symbol {
        Symbol {
            name: name.to_string(),
            qualified_name: format!("src/member.ts::{name}"),
            kind,
            language: LanguageId::TypeScript,
            span: SourceSpan {
                start_byte,
                end_byte: start_byte + 10,
                start_line: 1,
                start_column: 0,
                end_line: 2,
                end_column: 1,
            },
            parent: None,
            signature_fingerprint: String::new(),
        }
    }

    fn seed() -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("MemberService", SymbolKind::Class, 0),
                symbol("findMember", SymbolKind::Function, 100),
            ],
        }];
        let outcome = reindex(&mut conn, &project, &files, &[], &[]).unwrap();
        assert_eq!(outcome.files, 1);
        assert_eq!(outcome.symbols, 2);
        conn
    }

    fn project_id(conn: &Connection) -> String {
        conn.query_row("SELECT id FROM projects LIMIT 1", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn search_finds_symbol() {
        let conn = seed();
        let pid = project_id(&conn);
        let hits = search(&conn, &pid, "MemberService", 50).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "MemberService");
        assert_eq!(hits[0].kind, "class");
        assert_eq!(hits[0].relative_path, "src/member.ts");
    }

    #[test]
    fn find_by_name_exact() {
        let conn = seed();
        let pid = project_id(&conn);
        let hits = find_by_name(&conn, &pid, "findMember").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "function");
    }

    #[test]
    fn reindex_is_idempotent() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![symbol("MemberService", SymbolKind::Class, 0)],
        }];
        let outcome = reindex(&mut conn, &pid, &files, &[], &[]).unwrap();
        assert_eq!(outcome.symbols, 1);
        // Old symbols from the first generation are gone.
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1);
        let fts_total: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts_total, 1);
    }

    /// Spec scenario "Reindex after unrelated edit" (§16.3): a later edit
    /// that only shifts `start_byte` must not change an unaffected symbol's
    /// `symbol_key`.
    #[test]
    fn symbol_key_excludes_start_byte() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let before: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("MemberService", SymbolKind::Class, 500),
                symbol("findMember", SymbolKind::Function, 600),
            ],
        }];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();

        let after: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }

    /// Task 2.7 (symbol-index MODIFIED "Stable Symbol Key ... Uses BLAKE3"):
    /// `signature_fingerprint` is the 5th `symbol_key` component. A
    /// position-only reindex (blank-line-only edit, same pattern as
    /// `symbol_key_excludes_start_byte`) must not change the key; a
    /// signature edit (params/return type) must.
    #[test]
    fn symbol_key_changes_on_signature_edit() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let before: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'findMember'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Same signature (empty fingerprint), only position shifted.
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("MemberService", SymbolKind::Class, 0),
                symbol("findMember", SymbolKind::Function, 200),
            ],
        }];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();
        let unchanged: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'findMember'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, unchanged, "blank-line-only edit must not re-key");

        // Signature changed (e.g. a parameter added).
        let mut resigned = symbol("findMember", SymbolKind::Function, 200);
        resigned.signature_fingerprint = "\u{1f}(id: string)\u{1f}void".to_string();
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![symbol("MemberService", SymbolKind::Class, 0), resigned],
        }];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();
        let after: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'findMember'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(before, after, "signature edit must re-key");
    }

    /// PR3 task 3.3: `persist_relationships` (wired in PR2 with empty vecs)
    /// stores real same-file edges — the shape PR3's intra-file extraction
    /// produces once resolved to symbol ids by its caller.
    #[test]
    fn persists_and_reads_back_relationships() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let source_id: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let target_id: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'findMember'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("MemberService", SymbolKind::Class, 0),
                symbol("findMember", SymbolKind::Function, 100),
            ],
        }];
        let relationships = vec![RelationshipInput {
            source_symbol_id: source_id.clone(),
            target_symbol_id: Some(target_id.clone()),
            target_external: None,
            kind: RelationshipKind::Contains,
            provenance: Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: file_id,
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        }];
        reindex(&mut conn, &pid, &files, &relationships, &[]).unwrap();

        let (kind, target): (String, Option<String>) = conn
            .query_row(
                "SELECT kind, target_symbol_id FROM relationships WHERE source_symbol_id = ?1",
                params![source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "contains");
        assert_eq!(target.as_deref(), Some(target_id.as_str()));
    }

    /// PR3 task 3.3 (design "Stable Row Ids"): the same logical relationship
    /// rows must get identical ids whether persisted in one full-batch call
    /// or split across two per-file calls — the shape the incremental engine
    /// (PR5) will use, one `persist_relationships` call per changed file.
    #[test]
    fn relationship_ids_match_full_batch_vs_split_by_file() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![
            FileInput {
                relative_path: "src/a.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![
                    symbol("Foo", SymbolKind::Class, 0),
                    symbol("bar", SymbolKind::Function, 20),
                ],
            },
            FileInput {
                relative_path: "src/b.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![
                    symbol("Baz", SymbolKind::Class, 0),
                    symbol("qux", SymbolKind::Function, 20),
                ],
            },
        ];
        reindex(&mut conn, &project, &files, &[], &[]).unwrap();

        let sym_id = |path: &str, name: &str| -> String {
            conn.query_row(
                "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
                 WHERE f.relative_path = ?1 AND s.name = ?2",
                params![path, name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let fil_id = |path: &str| -> String {
            conn.query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let (foo, bar) = (sym_id("src/a.ts", "Foo"), sym_id("src/a.ts", "bar"));
        let (baz, qux) = (sym_id("src/b.ts", "Baz"), sym_id("src/b.ts", "qux"));
        let (file_a, file_b) = (fil_id("src/a.ts"), fil_id("src/b.ts"));

        fn rel(source: &str, target: &str, file: &str) -> RelationshipInput {
            RelationshipInput {
                source_symbol_id: source.to_string(),
                target_symbol_id: Some(target.to_string()),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                source_file_id: file.to_string(),
                start_line: Some(1),
                start_column: Some(0),
                reason: None,
            }
        }

        let ids_of = |conn: &mut Connection, batches: Vec<Vec<RelationshipInput>>| -> Vec<String> {
            let tx = conn.transaction().unwrap();
            tx.execute("DELETE FROM relationships", []).unwrap();
            for batch in &batches {
                persist_relationships(&tx, &project, "ts1", batch).unwrap();
            }
            let mut ids = tx
                .prepare("SELECT id FROM relationships")
                .unwrap()
                .query_map([], |r: &Row| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            tx.commit().unwrap();
            ids.sort();
            ids
        };

        let full_batch = vec![vec![rel(&foo, &bar, &file_a), rel(&baz, &qux, &file_b)]];
        let full_ids = ids_of(&mut conn, full_batch);

        let split_batches = vec![
            vec![rel(&foo, &bar, &file_a)],
            vec![rel(&baz, &qux, &file_b)],
        ];
        let split_ids = ids_of(&mut conn, split_batches);

        assert_eq!(full_ids, split_ids);
        assert_eq!(full_ids.len(), 2);
    }

    /// PR3 task 3.3, `unresolved_references` side of the same guarantee.
    #[test]
    fn unresolved_ids_match_full_batch_vs_split_by_file() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();

        fn unr(file: &str, target: &str) -> UnresolvedReferenceInput {
            UnresolvedReferenceInput {
                source_symbol_id: None,
                source_file_id: file.to_string(),
                relationship_kind: RelationshipKind::Calls,
                target_text: target.to_string(),
                context_json: None,
                candidate_count: 0,
                reason: "zero candidates".to_string(),
                confidence: Confidence::Low,
            }
        }

        let ids_of =
            |conn: &mut Connection, batches: Vec<Vec<UnresolvedReferenceInput>>| -> Vec<String> {
                let tx = conn.transaction().unwrap();
                tx.execute("DELETE FROM unresolved_references", []).unwrap();
                for batch in &batches {
                    persist_unresolved(&tx, &project, "ts1", batch).unwrap();
                }
                let mut ids = tx
                    .prepare("SELECT id FROM unresolved_references")
                    .unwrap()
                    .query_map([], |r: &Row| r.get::<_, String>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                tx.commit().unwrap();
                ids.sort();
                ids
            };

        let full_batch = vec![vec![unr("fil-a", "doStuff"), unr("fil-b", "doOther")]];
        let full_ids = ids_of(&mut conn, full_batch);

        let split_batches = vec![vec![unr("fil-a", "doStuff")], vec![unr("fil-b", "doOther")]];
        let split_ids = ids_of(&mut conn, split_batches);

        assert_eq!(full_ids, split_ids);
        assert_eq!(full_ids.len(), 2);
    }

    /// PR4b task 4b.6 / spec scenario "Atomic persistence on failure": a
    /// relationship row that violates the `source_symbol_id` foreign key
    /// mid-transaction must roll back the whole reindex, leaving the
    /// previous generation's rows untouched rather than a partial write.
    #[test]
    fn reindex_rolls_back_completely_on_relationship_error() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let symbols_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let files_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();

        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![symbol("MemberService", SymbolKind::Class, 0)],
        }];
        let bogus_relationships = vec![RelationshipInput {
            source_symbol_id: "sym-does-not-exist".to_string(),
            target_symbol_id: None,
            target_external: None,
            kind: RelationshipKind::Contains,
            provenance: Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: "fil-does-not-exist".to_string(),
            start_line: None,
            start_column: None,
            reason: None,
        }];

        let result = reindex(&mut conn, &pid, &files, &bogus_relationships, &[]);
        assert!(result.is_err());

        let symbols_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let files_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(symbols_after, symbols_before);
        assert_eq!(files_after, files_before);
    }

    /// The `IMemberService`/`MemberService`/`findMember`/
    /// `EligibilityController` fixture shared by PR5a's graph-query tests.
    fn graph_files() -> Vec<FileInput> {
        vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("IMemberService", SymbolKind::Interface, 0),
                symbol("MemberService", SymbolKind::Class, 50),
                symbol("findMember", SymbolKind::Function, 100),
                symbol("EligibilityController", SymbolKind::Class, 150),
            ],
        }]
    }

    /// Seeds `graph_files()` (no relationships yet) for PR5a's
    /// `references`/`callers`/`callees`/`implementations` tests (task 5a.4).
    fn seed_graph() -> (Connection, String, String, String, String, String) {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        reindex(&mut conn, &project, &graph_files(), &[], &[]).unwrap();

        let get = |n: &str| -> String {
            conn.query_row("SELECT id FROM symbols WHERE name = ?1", params![n], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let (interface, class, function, controller) = (
            get("IMemberService"),
            get("MemberService"),
            get("findMember"),
            get("EligibilityController"),
        );
        (conn, project, interface, class, function, controller)
    }

    /// PR5a task 5a.4 / spec scenario "Callers of a symbol": two call sites
    /// (one Exact, one Low-confidence) both come back via `callers`, with
    /// confidence/provenance intact — and via `references`/`callees` from
    /// the other side of the same edges.
    #[test]
    fn callers_of_a_symbol() {
        let (mut conn, pid, interface, class, function, controller) = seed_graph();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let relationships = vec![
            RelationshipInput {
                source_symbol_id: class.clone(),
                target_symbol_id: Some(interface.clone()),
                target_external: None,
                kind: RelationshipKind::Implements,
                provenance: Provenance::Extracted,
                confidence: Confidence::High,
                source_file_id: file_id.clone(),
                start_line: Some(1),
                start_column: Some(0),
                reason: None,
            },
            RelationshipInput {
                source_symbol_id: controller.clone(),
                target_symbol_id: Some(function.clone()),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                source_file_id: file_id.clone(),
                start_line: Some(5),
                start_column: Some(2),
                reason: None,
            },
            RelationshipInput {
                source_symbol_id: class.clone(),
                target_symbol_id: Some(function.clone()),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Heuristic,
                confidence: Confidence::Low,
                source_file_id: file_id,
                start_line: Some(9),
                start_column: Some(2),
                reason: Some("ambiguous receiver".to_string()),
            },
        ];
        reindex(&mut conn, &pid, &graph_files(), &relationships, &[]).unwrap();

        let calls_to_function = callers(&conn, &pid, &function, None).unwrap();
        assert_eq!(calls_to_function.len(), 2);
        assert!(calls_to_function.iter().all(|r| r.kind == "calls"));
        assert!(calls_to_function.iter().any(|r| r.confidence == "exact"));
        assert!(calls_to_function.iter().any(|r| r.confidence == "low"));

        let calls_from_controller = callees(&conn, &pid, &controller, None).unwrap();
        assert_eq!(calls_from_controller.len(), 1);
        assert_eq!(calls_from_controller[0].confidence, "exact");

        let implementers = implementations(&conn, &pid, &interface, None).unwrap();
        assert_eq!(implementers.len(), 1);
        assert_eq!(implementers[0].kind, "implements");
        assert_eq!(implementers[0].source_symbol_id, class);

        // `references` is kind-agnostic: the one edge targeting the
        // interface (Implements) comes back even though it's not a call.
        let refs_to_interface = references(&conn, &pid, &interface, None).unwrap();
        assert_eq!(refs_to_interface.len(), 1);
        assert_eq!(refs_to_interface[0].kind, "implements");
    }

    /// Spec scenario "Min-confidence filter": `--min-confidence high` keeps
    /// only Exact/High callers, dropping the Low-confidence one.
    #[test]
    fn min_confidence_filter() {
        let (mut conn, pid, _interface, class, function, controller) = seed_graph();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let relationships = vec![
            RelationshipInput {
                source_symbol_id: controller,
                target_symbol_id: Some(function.clone()),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                source_file_id: file_id.clone(),
                start_line: Some(5),
                start_column: Some(2),
                reason: None,
            },
            RelationshipInput {
                source_symbol_id: class,
                target_symbol_id: Some(function.clone()),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Heuristic,
                confidence: Confidence::Low,
                source_file_id: file_id,
                start_line: Some(9),
                start_column: Some(2),
                reason: Some("ambiguous receiver".to_string()),
            },
        ];
        reindex(&mut conn, &pid, &graph_files(), &relationships, &[]).unwrap();

        let filtered = callers(&conn, &pid, &function, Some(Confidence::High)).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].confidence, "exact");
    }

    /// PR1 task 1.5: BLAKE3-backed ids are stable (same input, same output
    /// across repeated calls) and correctly shaped (32 lowercase hex chars,
    /// prefixed) — `hash_id`/`config_hash`/`content_hash` all funnel through
    /// `blake3::hash`, so `file_id` is enough to pin the shared behavior.
    #[test]
    fn blake3_ids_are_stable() {
        let a = file_id("prj-1", "src/member.ts");
        let b = file_id("prj-1", "src/member.ts");
        assert_eq!(a, b);
        assert!(a.starts_with("fil-"));
        let hex = &a["fil-".len()..];
        assert_eq!(hex.len(), 32);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));

        // Different input must not collide with the fixture above.
        let c = file_id("prj-1", "src/other.ts");
        assert_ne!(a, c);
    }

    /// `config_hash`/`content_hash` are likewise stable and non-empty.
    #[test]
    fn config_and_content_hash_are_stable() {
        assert_eq!(config_hash("hello"), config_hash("hello"));
        assert_ne!(config_hash("hello"), config_hash("world"));
        assert_eq!(content_hash(b"hello"), content_hash(b"hello"));
        assert_ne!(content_hash(b"hello"), content_hash(b"world"));
        assert_eq!(content_hash(b"hello").len(), 64); // full BLAKE3 hex digest
    }

    /// Task 5a.3: bare-name lookups return every candidate with its id
    /// (needed to resolve `--symbol-name` before a graph query), never just
    /// the first match.
    #[test]
    fn find_candidates_by_name_lists_every_match() {
        let (conn, pid, interface, ..) = seed_graph();
        let hits = find_candidates_by_name(&conn, &pid, "IMemberService").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, interface);
        assert_eq!(hits[0].qualified_name, "src/member.ts::IMemberService");
    }

    /// Task 4.6 (design "Dependent Re-Resolution Scope", relationship-graph
    /// MODIFIED "Affected-Set Resolution for Incremental Batches"): `a.ts`
    /// exports `doWork`, `b.ts` calls it, `c.ts` is unrelated. When `a.ts`
    /// changes, `dependents_by_target_symbol` must surface `b.ts` and must
    /// NOT surface `c.ts`.
    #[test]
    fn dependents_by_target_symbol_finds_only_true_dependents() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![
            FileInput {
                relative_path: "src/a.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![symbol("doWork", SymbolKind::Function, 0)],
            },
            FileInput {
                relative_path: "src/b.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![symbol("run", SymbolKind::Function, 0)],
            },
            FileInput {
                relative_path: "src/c.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![symbol("unrelated", SymbolKind::Function, 0)],
            },
        ];
        reindex(&mut conn, &project, &files, &[], &[]).unwrap();

        let sym_id = |path: &str, name: &str| -> String {
            conn.query_row(
                "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
                 WHERE f.relative_path = ?1 AND s.name = ?2",
                params![path, name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let fil_id = |path: &str| -> String {
            conn.query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let do_work = sym_id("src/a.ts", "doWork");
        let run = sym_id("src/b.ts", "run");
        let file_b = fil_id("src/b.ts");

        let relationships = vec![RelationshipInput {
            source_symbol_id: run,
            target_symbol_id: Some(do_work.clone()),
            target_external: None,
            kind: RelationshipKind::Calls,
            provenance: Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: file_b,
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        }];
        reindex(&mut conn, &project, &files, &relationships, &[]).unwrap();

        let dependents =
            dependents_by_target_symbol(&conn, &project, &[do_work], &["src/a.ts".to_string()])
                .unwrap();
        assert_eq!(dependents, vec!["src/b.ts".to_string()]);
    }

    /// Task 4.6, `unresolved_references` side: `c.ts` has an unresolved
    /// `doStuff` reference; `dependents_by_unresolved_target` must surface it
    /// when `doStuff` is one of the newly-satisfiable target texts, and must
    /// NOT surface an unrelated `d.ts` with a different unresolved target.
    #[test]
    fn dependents_by_unresolved_target_finds_only_matching_targets() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![
            FileInput {
                relative_path: "src/c.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![],
            },
            FileInput {
                relative_path: "src/d.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "test-hash".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![],
            },
        ];
        reindex(&mut conn, &project, &files, &[], &[]).unwrap();
        let fil_id = |path: &str| -> String {
            conn.query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let unresolved = vec![
            UnresolvedReferenceInput {
                source_symbol_id: None,
                source_file_id: fil_id("src/c.ts"),
                relationship_kind: RelationshipKind::Calls,
                target_text: "doStuff".to_string(),
                context_json: None,
                candidate_count: 0,
                reason: "zero candidates".to_string(),
                confidence: Confidence::Unresolved,
            },
            UnresolvedReferenceInput {
                source_symbol_id: None,
                source_file_id: fil_id("src/d.ts"),
                relationship_kind: RelationshipKind::Calls,
                target_text: "doOther".to_string(),
                context_json: None,
                candidate_count: 0,
                reason: "zero candidates".to_string(),
                confidence: Confidence::Unresolved,
            },
        ];
        reindex(&mut conn, &project, &files, &[], &unresolved).unwrap();

        let dependents =
            dependents_by_unresolved_target(&conn, &project, &["doStuff".to_string()]).unwrap();
        assert_eq!(dependents, vec!["src/c.ts".to_string()]);
    }

    /// Task 4.3: `resolution_snapshot` reads back every file plus every
    /// non-module symbol, tagging `exported` from the persisted `exports`
    /// relationship — `MemberService`'s class-decl fallback path never
    /// stores an `exports` edge in this fixture, so `findMember` (explicitly
    /// exported below) is the one asserted true.
    #[test]
    fn resolution_snapshot_reads_files_and_symbols() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let source_id: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let target_id: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'findMember'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![
                symbol("MemberService", SymbolKind::Class, 0),
                symbol("findMember", SymbolKind::Function, 100),
            ],
        }];
        let relationships = vec![RelationshipInput {
            source_symbol_id: source_id,
            target_symbol_id: Some(target_id.clone()),
            target_external: None,
            kind: RelationshipKind::Exports,
            provenance: Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: file_id,
            start_line: None,
            start_column: None,
            reason: None,
        }];
        reindex(&mut conn, &pid, &files, &relationships, &[]).unwrap();

        let snapshot = resolution_snapshot(&conn, &pid).unwrap();
        assert_eq!(snapshot.files, vec!["src/member.ts".to_string()]);
        assert_eq!(snapshot.symbols.len(), 2);
        let find_member = snapshot
            .symbols
            .iter()
            .find(|s| s.name == "findMember")
            .unwrap();
        assert_eq!(find_member.kind, SymbolKind::Function);
        assert!(find_member.exported);
        let member_service = snapshot
            .symbols
            .iter()
            .find(|s| s.name == "MemberService")
            .unwrap();
        assert!(!member_service.exported);
    }

    /// Task 5.7 (design "Batch Atomicity"): mirrors
    /// `reindex_rolls_back_completely_on_relationship_error` at the
    /// incremental engine's own primitives — T1 (`set_pending_files`)
    /// followed by a T2 that fails on a bogus relationship must roll back
    /// `apply_incremental` in full, AND must leave `pending_files` exactly
    /// as T1 set it (`mark_verified` never ran, so it's not reset to 0).
    #[test]
    fn apply_incremental_rolls_back_and_leaves_pending_files_set() {
        let mut conn = seed();
        let pid = project_id(&conn);
        let symbols_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let files_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();

        // T1: a prior `detect()` found one pending file.
        {
            let tx = conn.transaction().unwrap();
            set_pending_files(&tx, &pid, "ts-t1", 1).unwrap();
            tx.commit().unwrap();
        }

        // T2: a legitimate file update alongside a relationship that
        // violates the `source_symbol_id` foreign key — the whole batch
        // transaction must roll back before `mark_verified` ever runs.
        let result = (|| -> Result<()> {
            let tx = conn.transaction()?;
            let files = vec![FileInput {
                relative_path: "src/member.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "new-hash".to_string(),
                modified_ns: 999,
                size_bytes: 42,
                symbols: vec![symbol("MemberService", SymbolKind::Class, 0)],
            }];
            let bogus = vec![RelationshipInput {
                source_symbol_id: "sym-does-not-exist".to_string(),
                target_symbol_id: None,
                target_external: None,
                kind: RelationshipKind::Contains,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                source_file_id: "fil-does-not-exist".to_string(),
                start_line: None,
                start_column: None,
                reason: None,
            }];
            apply_incremental(&tx, &pid, "ts-t2", &files, &bogus, &[], &[])?;
            mark_verified(&tx, &pid, "ts-t2")?;
            tx.commit()?;
            Ok(())
        })();
        assert!(result.is_err());

        let symbols_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let files_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(symbols_after, symbols_before, "previous generation intact");
        assert_eq!(files_after, files_before, "previous generation intact");

        let pending: i64 = conn
            .query_row(
                "SELECT pending_files FROM index_state WHERE project_id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1, "T1's pending count survives T2's rollback");
    }

    /// Task 5.2 (spec "Per-File Delete Removes Symbols and Converts Inbound
    /// Edges to Unresolved"): deleting `src/a.ts` removes its own symbols,
    /// and `src/b.ts`'s dangling call edge becomes an
    /// `unresolved_references` row rather than disappearing silently.
    #[test]
    fn delete_file_converts_inbound_edge_to_unresolved() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![
            FileInput {
                relative_path: "src/a.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "hash-a".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![symbol("doWork", SymbolKind::Function, 0)],
            },
            FileInput {
                relative_path: "src/b.ts".to_string(),
                language: "typescript".to_string(),
                content_hash: "hash-b".to_string(),
                modified_ns: 0,
                size_bytes: 10,
                symbols: vec![symbol("run", SymbolKind::Function, 0)],
            },
        ];
        reindex(&mut conn, &project, &files, &[], &[]).unwrap();

        let sym_id = |path: &str, name: &str| -> String {
            conn.query_row(
                "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
                 WHERE f.relative_path = ?1 AND s.name = ?2",
                params![path, name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let fil_id = |path: &str| -> String {
            conn.query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let do_work = sym_id("src/a.ts", "doWork");
        let run = sym_id("src/b.ts", "run");
        let file_b = fil_id("src/b.ts");

        let relationships = vec![RelationshipInput {
            source_symbol_id: run.clone(),
            target_symbol_id: Some(do_work.clone()),
            target_external: None,
            kind: RelationshipKind::Calls,
            provenance: Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: file_b,
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        }];
        reindex(&mut conn, &project, &files, &relationships, &[]).unwrap();

        {
            let tx = conn.transaction().unwrap();
            delete_file(&tx, &project, "ts-delete", "src/a.ts").unwrap();
            tx.commit().unwrap();
        }

        let a_files: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE relative_path = 'src/a.ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_files, 0);

        let remaining_calls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relationships WHERE source_symbol_id = ?1",
                params![run],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_calls, 0,
            "the dangling edge was removed, not left dangling"
        );

        let (target_text, kind): (String, String) = conn
            .query_row(
                "SELECT target_text, relationship_kind FROM unresolved_references
                 WHERE source_symbol_id = ?1",
                params![run],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(target_text, "doWork");
        assert_eq!(kind, "calls");
    }
}
