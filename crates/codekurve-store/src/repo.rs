//! Repository operations for the Phase 1 vertical slice: register a project,
//! rebuild its index transactionally, and query symbols. See
//! CODEKURVE_MASTER_PLAN.md §24 (schema) and §Fase 1 (scope).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use codekurve_core::{
    Confidence, FrameworkRole, LanguageId, Provenance, RelationshipKind, SourceSpan, Symbol,
    SymbolKind, Visibility,
};
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
    /// Phase 5 PR1 (design "repo.rs"): `'default'`/`false`/`false` for every
    /// pre-Phase-5 row and every non-C# symbol.
    pub visibility: String,
    pub is_partial: bool,
    pub is_record: bool,
    /// Phase 7 PR1 (design "Model and Storage Changes"): parsed back from
    /// the comma-joined `symbols.roles` column via `parse_roles`. Empty for
    /// every pre-Phase-7 row.
    pub roles: Vec<FrameworkRole>,
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
///
/// Phase 5 PR1 (design "symbol_key and signature_fingerprint"): `partial_ordinal`
/// is a 6th, optional component that disambiguates multiple C# `partial`
/// fragments of the same type in one file. `None` MUST leave the hashed
/// input byte-for-byte identical to the pre-Phase-5 five-component
/// `format!` — pinned by `symbol_key_none_matches_pre_migration_golden_hash`.
pub fn symbol_key(
    language: &str,
    relative_path: &str,
    kind: &str,
    qualified_name: &str,
    signature_fingerprint: &str,
    partial_ordinal: Option<u32>,
) -> String {
    let mut input = format!(
        "{language}\u{1f}{relative_path}\u{1f}{kind}\u{1f}{qualified_name}\u{1f}{signature_fingerprint}"
    );
    if let Some(ordinal) = partial_ordinal {
        input.push('\u{1f}');
        input.push_str(&ordinal.to_string());
    }
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Deterministic symbol storage id from a precomputed `file_id`/`symbol_key`
/// — see `file_id`'s doc comment.
pub fn symbol_id(file_id: &str, symbol_key: &str) -> String {
    hash_id("sym", &format!("{file_id}/{symbol_key}"))
}

/// `execute` through the connection's prepared-statement cache: bulk writes
/// (one symbol/relationship/FTS row at a time) used to re-parse the same SQL
/// for every row.
fn exec(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
    conn.prepare_cached(sql)?.execute(params)
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

/// The `codekurve` binary version that last fully indexed this project, or
/// `None` for a never-indexed project or one migrated from a schema before
/// `analyzer_version` existed. Read by `commands::index`/`watch::reconcile`
/// to decide whether a version bump (new extraction logic, same source
/// files) needs a forced full reindex — file content alone can't signal
/// that (§ analyzer-version staleness).
pub fn analyzer_version(conn: &Connection, project_id: &str) -> Result<Option<String>> {
    let version = conn.query_row(
        "SELECT analyzer_version FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    Ok(version)
}

/// Stamps the current build's version onto a project after it has been
/// fully (re)indexed — see [`analyzer_version`].
pub fn set_analyzer_version(conn: &Connection, project_id: &str, version: &str) -> Result<()> {
    conn.execute(
        "UPDATE projects SET analyzer_version = ?1 WHERE id = ?2",
        params![version, project_id],
    )?;
    Ok(())
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
    exec(
        &tx,
        "DELETE FROM relationships WHERE project_id = ?1",
        params![project_id],
    )?;
    exec(
        &tx,
        "DELETE FROM unresolved_references WHERE project_id = ?1",
        params![project_id],
    )?;
    exec(
        &tx,
        "DELETE FROM symbols_fts WHERE symbol_id IN
             (SELECT id FROM symbols WHERE project_id = ?1)",
        params![project_id],
    )?;
    exec(
        &tx,
        "DELETE FROM symbols WHERE project_id = ?1",
        params![project_id],
    )?;
    exec(
        &tx,
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
    exec(
        tx,
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
    // Same-named blocks (`namespace A.B { } namespace A.B { }`) share a key;
    // keep the first so one such file cannot fail the whole index.
    let mut seen_keys = std::collections::HashSet::new();
    for symbol in &file.symbols {
        let symbol_key = symbol_key(
            symbol.language.as_str(),
            &file.relative_path,
            symbol.kind.as_str(),
            &symbol.qualified_name,
            &symbol.signature_fingerprint,
            symbol.partial_ordinal,
        );
        if !seen_keys.insert(symbol_key.clone()) {
            continue;
        }
        let symbol_id = symbol_id(&file_id, &symbol_key);
        let qualified = &symbol.qualified_name;
        let mut roles = symbol.roles.clone();
        roles.sort();
        roles.dedup();
        let roles_text = roles
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(",");
        exec(
            tx,
            "INSERT INTO symbols(id, project_id, file_id, symbol_key, name, qualified_name,
                 kind, language, start_byte, end_byte, start_line, start_column,
                 end_line, end_column, provenance, confidence, visibility, is_partial,
                 is_record, roles, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 'tree-sitter', 'high', ?15, ?16, ?17, ?18, 1, ?19, ?19)",
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
                symbol.visibility.as_str(),
                symbol.is_partial,
                symbol.is_record,
                roles_text,
                ts,
            ],
        )?;
        exec(
            tx,
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
    delete_file_owned_relationships(tx, project_id, file_id)?;
    delete_file_owned_symbols(tx, file_id)?;
    Ok(())
}

/// First half of [`delete_file_owned_rows`]: this file's own outbound
/// `relationships`/`unresolved_references` rows. Split out so a multi-file
/// batch (`apply_incremental`) can clear every file's own edges *before*
/// deleting any file's symbols — otherwise, since `foreign_keys = ON` checks
/// are immediate (not deferred), deleting file A's symbols while file B
/// (later in the same batch) still owns an edge targeting one of them trips
/// `FOREIGN KEY constraint failed`, even though B's own edge is about to be
/// cleared and rebuilt a few statements later in the same transaction.
fn delete_file_owned_relationships(
    tx: &Transaction,
    project_id: &str,
    file_id: &str,
) -> Result<()> {
    exec(
        tx,
        "DELETE FROM relationships WHERE project_id = ?1 AND source_file_id = ?2",
        params![project_id, file_id],
    )?;
    exec(
        tx,
        "DELETE FROM unresolved_references WHERE project_id = ?1 AND source_file_id = ?2",
        params![project_id, file_id],
    )?;
    Ok(())
}

/// Second half of [`delete_file_owned_rows`]: this file's own symbols (+ FTS
/// shadow). Must only run, across a whole batch, after every batch file's
/// [`delete_file_owned_relationships`] has already run — see that function's
/// doc comment.
fn delete_file_owned_symbols(tx: &Transaction, file_id: &str) -> Result<()> {
    exec(
        tx,
        "DELETE FROM symbols_fts WHERE symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
        params![file_id],
    )?;
    exec(
        tx,
        "DELETE FROM symbols WHERE file_id = ?1",
        params![file_id],
    )?;
    Ok(())
}

/// [`delete_file_owned_symbols`] for a whole batch. `symbols_fts` can only be
/// matched by its unindexed `symbol_id`, so every FTS delete scans the whole
/// table: one statement per [`ID_CHUNK`] files instead of one per file turns
/// N full scans into one (measured 72 ms -> 5 ms for 50 files of a 5.5k
/// symbol index).
fn delete_symbols_of_files(tx: &Transaction, file_ids: &[String]) -> Result<()> {
    for chunk in file_ids.chunks(ID_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        exec(
            tx,
            &format!(
                "DELETE FROM symbols_fts WHERE symbol_id IN
                     (SELECT id FROM symbols WHERE file_id IN ({placeholders}))"
            ),
            rusqlite::params_from_iter(chunk.iter()),
        )?;
        exec(
            tx,
            &format!("DELETE FROM symbols WHERE file_id IN ({placeholders})"),
            rusqlite::params_from_iter(chunk.iter()),
        )?;
    }
    Ok(())
}

// Test-only count of the DELETE statements `park_inbound_edges` runs.
#[cfg(test)]
thread_local! {
    static PARK_DELETE_STATEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Every relationship from ANOTHER file into `file_id`'s symbols becomes an
/// `unresolved_references` row (target text = the symbol's bare name), so
/// the symbols can be deleted without a dangling foreign key and without
/// silently dropping the reference.
/// An edge from one file into a symbol of another, read before that symbol
/// is deleted (the name is gone afterwards).
struct InboundEdge {
    rel_id: String,
    source_symbol_id: String,
    source_file_id: String,
    kind: String,
    target_symbol_id: String,
    target_name: String,
}

fn collect_inbound_edges(
    tx: &Transaction,
    project_id: &str,
    file_ids: &[String],
) -> Result<Vec<InboundEdge>> {
    let mut edges = Vec::new();
    for file_id in file_ids {
        let mut stmt = tx.prepare_cached(
            "SELECT r.id, r.source_symbol_id, r.source_file_id, r.kind, r.target_symbol_id, s.name
             FROM relationships r
             JOIN symbols s ON s.id = r.target_symbol_id
             WHERE r.project_id = ?1 AND s.file_id = ?2 AND r.source_file_id != ?2",
        )?;
        let rows = stmt.query_map(params![project_id, file_id], |row| {
            Ok(InboundEdge {
                rel_id: row.get(0)?,
                source_symbol_id: row.get(1)?,
                source_file_id: row.get(2)?,
                kind: row.get(3)?,
                target_symbol_id: row.get(4)?,
                target_name: row.get(5)?,
            })
        })?;
        for row in rows {
            edges.push(row?);
        }
    }
    Ok(edges)
}

/// Replace each edge with an `unresolved_references` row (target text = the
/// target symbol's bare name). One unresolved row per edge, persisted in a
/// single call: ids are `stable_id`s keyed by (file, symbol, kind, target
/// name), so edges from one outside file into same-named symbols of
/// different files (`x.Name`) share a key and rely on the ordinal that only
/// one call's `seen` map can hand out. An identical reference may already be
/// parked from an earlier pass (e.g. an "ambiguous" row for the same name);
/// that one is kept.
fn park_inbound_edges(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    edges: &[InboundEdge],
    reason: &str,
) -> Result<()> {
    // One DELETE per ID_CHUNK edges: the parked rows are the same set, and
    // only the parked rows' ids are needed, so statement count is the only
    // thing that changes.
    for chunk in edges.chunks(ID_CHUNK) {
        #[cfg(test)]
        PARK_DELETE_STATEMENTS.with(|c| c.set(c.get() + 1));
        let placeholders = vec!["?"; chunk.len()].join(",");
        exec(
            tx,
            &format!("DELETE FROM relationships WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(chunk.iter().map(|edge| edge.rel_id.as_str())),
        )?;
    }
    let mut unresolved = Vec::with_capacity(edges.len());
    for edge in edges {
        unresolved.push(UnresolvedReferenceInput {
            source_symbol_id: Some(edge.source_symbol_id.clone()),
            source_file_id: edge.source_file_id.clone(),
            relationship_kind: parse_relationship_kind(&edge.kind),
            target_text: edge.target_name.clone(),
            context_json: None,
            candidate_count: 0,
            reason: reason.to_string(),
            confidence: Confidence::Unresolved,
        });
    }
    persist_unresolved_rows(tx, project_id, ts, &unresolved, true)
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
    let inbound = collect_inbound_edges(tx, project_id, std::slice::from_ref(&file_id))?;
    park_inbound_edges(
        tx,
        project_id,
        ts,
        &inbound,
        "target symbol's file was deleted",
    )?;

    delete_file_owned_rows(tx, project_id, &file_id)?;
    exec(tx, "DELETE FROM files WHERE id = ?1", params![file_id])?;
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
    // Two full passes over `files`, not one interleaved pass: every batch
    // file's own edges must be gone before any batch file's symbols are
    // deleted, or an as-yet-unprocessed file's still-present edge to an
    // already-deleted symbol trips the immediate foreign-key check (see
    // `delete_file_owned_relationships`'s doc comment).
    for file in files {
        delete_file_owned_relationships(tx, project_id, &file_id(project_id, &file.relative_path))?;
    }
    // An edge still pointing into a batch file's symbols comes from a file
    // outside the batch (a dependent of a dependent, or one that could not be
    // re-read). The batch's symbols are deleted and re-inserted, almost
    // always with the same ids, so those edges stay valid: remember them,
    // let the foreign-key check wait for the commit, and afterwards park as
    // unresolved only the edges whose target symbol is really gone.
    let batch_file_ids: Vec<String> = files
        .iter()
        .map(|file| file_id(project_id, &file.relative_path))
        .collect();
    let inbound = collect_inbound_edges(tx, project_id, &batch_file_ids)?;
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
    delete_symbols_of_files(tx, &batch_file_ids)?;

    let mut count = 0;
    for file in files {
        count += insert_file_and_symbols(tx, project_id, ts, file)?;
    }
    let mut gone = Vec::new();
    for edge in inbound {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM symbols WHERE id = ?1)",
            params![edge.target_symbol_id],
            |row| row.get(0),
        )?;
        if !exists {
            gone.push(edge);
        }
    }
    park_inbound_edges(
        tx,
        project_id,
        ts,
        &gone,
        "target symbol was removed; this file was not re-analyzed",
    )?;
    persist_relationships(tx, project_id, ts, relationships)?;
    persist_unresolved(tx, project_id, ts, unresolved)?;
    Ok(count)
}

/// Task 5.2 (`index_state` upsert): publishes the batch's pending count
/// (T1 of the Batch Atomicity sequence) so an interrupted run honestly
/// reports staleness until T2 commits.
pub fn set_pending_files(tx: &Transaction, project_id: &str, ts: &str, count: i64) -> Result<()> {
    exec(
        tx,
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
    exec(
        tx,
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

/// Just the stored pending-file count (`0` when `index_state` has no row).
/// Every MCP tool call reads it for its stale warning, so it must stay a
/// single-row lookup; [`index_status`]'s full-table counts cost tens of
/// milliseconds per call on a 10k-file project.
pub fn pending_files(conn: &Connection, project_id: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT pending_files FROM index_state WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
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
    strings_in_chunks(
        conn,
        project_id,
        "SELECT id FROM symbols WHERE project_id = ? AND file_id",
        &file_ids,
    )
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
    let mut stmt = conn.prepare_cached(
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
    let mut stmt = conn.prepare_cached(
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

/// Refresh the stored mtime of files whose bytes did not change, so the
/// size+mtime fast path in change detection applies to them again.
pub fn touch_files(conn: &Connection, project_id: &str, files: &[(&str, i64)]) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "UPDATE files SET modified_ns = ?1 WHERE project_id = ?2 AND relative_path = ?3",
    )?;
    for (path, modified_ns) in files {
        stmt.execute(params![modified_ns, project_id, path])?;
    }
    Ok(())
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
        exec(
            tx,
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
    persist_unresolved_rows(tx, project_id, ts, unresolved, false)
}

/// `or_ignore` skips a row whose id already exists instead of failing.
fn persist_unresolved_rows(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    unresolved: &[UnresolvedReferenceInput],
    or_ignore: bool,
) -> Result<()> {
    let insert = if or_ignore {
        "INSERT OR IGNORE INTO"
    } else {
        "INSERT INTO"
    };
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
        exec(
            tx,
            &format!(
                "{insert} unresolved_references(id, project_id, source_symbol_id, source_file_id,
                 relationship_kind, target_text, context_json, candidate_count, reason,
                 confidence, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?11)"
            ),
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
    /// Why this edge was classified the way it was (e.g. `PROPERTY_TYPE_REASON`
    /// distinguishing a nav-property's declared type from a call-site usage,
    /// or `BASE_LIST_REASON` for an inheritance entry) — `None` for the
    /// common case (a real call/construct/read), same additive shape as
    /// `source_relative_path` above.
    pub reason: Option<String>,
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
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[],
        min_confidence,
        page,
    )
}

/// Call sites that call `symbol_id` (`kind = calls`, target = the callee) —
/// the `callers` CLI command.
pub fn callers(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Calls],
        min_confidence,
        page,
    )
}

/// Calls made *by* `symbol_id` (`kind = calls`, source = the caller) — the
/// `callees` CLI command.
pub fn callees(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    query_relationships(
        conn,
        "source_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Calls],
        min_confidence,
        page,
    )
}

/// Symbols that `implements` or `inherits`/`extends` `symbol_id` — the
/// `implementations` CLI command.
pub fn implementations(
    conn: &Connection,
    project_id: &str,
    symbol_id: &str,
    min_confidence: Option<Confidence>,
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    query_relationships(
        conn,
        "target_symbol_id",
        project_id,
        symbol_id,
        &[RelationshipKind::Implements, RelationshipKind::Inherits],
        min_confidence,
        page,
    )
}

/// Window of a result list, pushed into SQL as `LIMIT`/`OFFSET` so rows
/// outside the page are never materialized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageRequest {
    /// `None` returns every row after `offset`.
    pub limit: Option<usize>,
    pub offset: usize,
}

impl PageRequest {
    /// No windowing: every matching row.
    pub const ALL: PageRequest = PageRequest {
        limit: None,
        offset: 0,
    };
}

/// One page of rows plus the number of rows that match the query before
/// paging (after every filter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paged<T> {
    pub rows: Vec<T>,
    pub total: usize,
}

/// Run `rows_sql` (which must end in `LIMIT ? OFFSET ?`) for one page and
/// `count_sql` for the total. The count is skipped when the page already
/// proves it: the first page came back short of its limit.
fn fetch_page<T>(
    conn: &Connection,
    count_sql: &str,
    rows_sql: &str,
    binds: Vec<rusqlite::types::Value>,
    page: PageRequest,
    map: fn(&Row) -> rusqlite::Result<T>,
) -> Result<Paged<T>> {
    use rusqlite::types::Value;
    let to_sql_int = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    let mut page_binds = binds.clone();
    page_binds.push(Value::Integer(page.limit.map_or(-1, to_sql_int)));
    page_binds.push(Value::Integer(to_sql_int(page.offset)));

    let mut stmt = conn.prepare_cached(rows_sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(page_binds), map)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let complete = page.offset == 0 && page.limit.is_none_or(|limit| rows.len() < limit);
    let total = if complete {
        rows.len()
    } else {
        let mut stmt = conn.prepare_cached(count_sql)?;
        stmt.query_row(rusqlite::params_from_iter(binds), |row| {
            row.get::<_, i64>(0)
        })? as usize
    };
    Ok(Paged { rows, total })
}

/// `AND r.confidence IN (...)` keeping every rank at or above `min` — the
/// SQL form of `confidence_rank(row) >= confidence_rank(min)`. Empty when
/// `min` ranks 0 (every row, unrecognized values included, passes).
fn confidence_filter(min: Option<Confidence>) -> String {
    let threshold = min.map_or(0, |m| confidence_rank(m.as_str()));
    let kept: Vec<String> = ["exact", "high", "medium", "low"]
        .into_iter()
        .filter(|name| confidence_rank(name) >= threshold)
        .map(|name| format!("'{name}'"))
        .collect();
    if threshold == 0 {
        String::new()
    } else {
        format!(" AND r.confidence IN ({})", kept.join(","))
    }
}

/// Columns every relationship listing returns (see [`map_relationship`]).
const RELATIONSHIP_COLUMNS: &str =
    "r.source_symbol_id, src.qualified_name, r.target_symbol_id, tgt.qualified_name,
     r.target_external, r.kind, r.provenance, r.confidence, r.start_line, r.start_column,
     src_file.relative_path, r.reason";

/// Shared query behind `references`/`callers`/`callees`/`implementations`:
/// one indexed SELECT (`idx_relationships_source_kind`/`_target_kind` covers
/// `column = ?` [`AND kind IN (...)`]) joined to both endpoints' symbol rows
/// for display. `min_confidence` and the page window are applied in SQL; the
/// ordering ends in `rowid` so pages never overlap or skip rows on ties.
fn query_relationships(
    conn: &Connection,
    column: &'static str,
    project_id: &str,
    symbol_id: &str,
    kinds: &[RelationshipKind],
    min_confidence: Option<Confidence>,
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    let kind_filter = if kinds.is_empty() {
        String::new()
    } else {
        let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        format!(" AND r.kind IN ({placeholders})")
    };
    let confidence_filter = confidence_filter(min_confidence);
    let joins = "FROM relationships r
         JOIN symbols src ON src.id = r.source_symbol_id
         JOIN files src_file ON src_file.id = src.file_id
         LEFT JOIN symbols tgt ON tgt.id = r.target_symbol_id";
    let filter =
        format!("WHERE r.project_id = ? AND r.{column} = ?{kind_filter}{confidence_filter}");
    let rows_sql = format!(
        "SELECT {RELATIONSHIP_COLUMNS} {joins} {filter}
         ORDER BY r.start_line, r.rowid LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) {joins} {filter}");

    let mut binds = vec![
        rusqlite::types::Value::from(project_id.to_string()),
        rusqlite::types::Value::from(symbol_id.to_string()),
    ];
    binds.extend(
        kinds
            .iter()
            .map(|k| rusqlite::types::Value::from(k.as_str().to_string())),
    );
    fetch_page(conn, &count_sql, &rows_sql, binds, page, map_relationship)
}

/// One `unresolved_references` row read back for display — the query side of
/// what [`persist_unresolved`] writes, joined to the source symbol's
/// qualified name and the source file's relative path exactly like
/// [`query_relationships`] does (mirrors [`StoredRelationship`]).
///
/// `source_symbol_id`/`source_qualified_name` are both optional: the column
/// is nullable (file-level edges have no owning symbol), so the symbol join
/// is a LEFT JOIN. No `start_line`/`start_column`/`provenance` — the table
/// stores none of the three (§24.2), and adding them would need a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredUnresolved {
    pub source_symbol_id: Option<String>,
    pub source_qualified_name: Option<String>,
    pub source_relative_path: String,
    pub relationship_kind: String,
    pub target_text: String,
    pub reason: String,
    pub confidence: String,
    pub candidate_count: u32,
}

/// Every reference the analyzer recorded but deliberately refused to turn
/// into an edge (§27.4 "never silently pick first", ADR 0007) — until now
/// only visible as `index_status`'s count. `reason` is the whole point: it
/// says *why* resolution stopped ("base list entry not found in project;
/// class vs interface undeterminable", "zero candidates", …).
///
/// `target_text` filtering is exact, not `LIKE`: `idx_unresolved_project_target`
/// is `(project_id, target_text)`, so equality stays a bounded index seek
/// while a substring predicate would force a full scan of a table that runs
/// to hundreds of rows on a real project. `source_symbol_id` has no index of
/// its own; it's an additional narrowing filter, applied in the same SELECT
/// rather than in memory.
pub fn unresolved(
    conn: &Connection,
    project_id: &str,
    target_text: Option<&str>,
    source_symbol_id: Option<&str>,
    page: PageRequest,
) -> Result<Paged<StoredUnresolved>> {
    use rusqlite::types::Value;
    let mut filters = String::new();
    let mut binds = vec![Value::from(project_id.to_string())];
    if let Some(target_text) = target_text {
        filters.push_str(" AND u.target_text = ?");
        binds.push(Value::from(target_text.to_string()));
    }
    if let Some(source_symbol_id) = source_symbol_id {
        filters.push_str(" AND u.source_symbol_id = ?");
        binds.push(Value::from(source_symbol_id.to_string()));
    }
    let joins = "FROM unresolved_references u
         JOIN files f ON f.id = u.source_file_id
         LEFT JOIN symbols src ON src.id = u.source_symbol_id";
    let rows_sql = format!(
        "SELECT u.source_symbol_id, src.qualified_name, f.relative_path, u.relationship_kind,
                u.target_text, u.reason, u.confidence, u.candidate_count
         {joins}
         WHERE u.project_id = ?{filters}
         ORDER BY f.relative_path, u.target_text, u.rowid LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) {joins} WHERE u.project_id = ?{filters}");
    fetch_page(conn, &count_sql, &rows_sql, binds, page, map_unresolved)
}

fn map_unresolved(row: &Row) -> rusqlite::Result<StoredUnresolved> {
    Ok(StoredUnresolved {
        source_symbol_id: row.get(0)?,
        source_qualified_name: row.get(1)?,
        source_relative_path: row.get(2)?,
        relationship_kind: row.get(3)?,
        target_text: row.get(4)?,
        reason: row.get(5)?,
        confidence: row.get(6)?,
        candidate_count: row.get::<_, i64>(7)? as u32,
    })
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
        reason: row.get(11)?,
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
    let mut stmt = conn.prepare_cached(
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
    // The query is quoted as one phrase to neutralize FTS5 operators; the
    // trailing `*` makes its last token a prefix (`RegisterMan` finds
    // `RegisterManager` without a table scan) and the column filter keeps
    // `kind`/`relative_path` out of it (`class` must not match every class).
    let fts_query = format!(
        "{{name qualified_name}}: \"{}\"*",
        query.replace('"', "\"\"")
    );
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column,
                s.visibility, s.is_partial, s.is_record, s.roles
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?2
           AND s.id IN (SELECT symbol_id FROM symbols_fts WHERE symbols_fts MATCH ?1)
         ORDER BY LOWER(s.name) = LOWER(?4) DESC,
                  SUBSTR(LOWER(s.name), 1, LENGTH(?4)) = LOWER(?4) DESC,
                  s.name, s.qualified_name, s.id
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![fts_query, project_id, limit, query], map_stored)?;
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if !rows.is_empty() {
        return Ok(rows);
    }
    search_partial(conn, project_id, query, limit)
}

/// Fallback for agents that guessed a longer PascalCase name than the source
/// uses (`SubmitReferral` -> `Submit`). It only runs after exact FTS misses.
fn search_partial(
    conn: &Connection,
    project_id: &str,
    query: &str,
    limit: u32,
) -> Result<Vec<StoredSymbol>> {
    let terms = partial_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let where_terms = std::iter::repeat_n(
        "(LOWER(s.name) LIKE ? OR LOWER(s.qualified_name) LIKE ?)",
        terms.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column,
                s.visibility, s.is_partial, s.is_record, s.roles
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ? AND ({where_terms})
         ORDER BY CASE WHEN LOWER(s.name) LIKE ? THEN 0 ELSE 1 END, s.name
         LIMIT ?"
    );
    let mut binds = vec![project_id.to_owned()];
    for term in &terms {
        let pattern = format!("%{term}%");
        binds.push(pattern.clone());
        binds.push(pattern);
    }
    binds.push(format!("{}%", terms[0]));
    binds.push(limit.to_string());
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter()), map_stored)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn partial_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in query.chars() {
        if !ch.is_alphanumeric() {
            push_partial_term(&mut terms, &mut current);
        } else {
            if ch.is_uppercase() && !current.is_empty() {
                push_partial_term(&mut terms, &mut current);
            }
            current.extend(ch.to_lowercase());
        }
    }
    push_partial_term(&mut terms, &mut current);
    terms
        .into_iter()
        .filter(|term| term.len() >= 3)
        .take(3)
        .collect()
}

fn push_partial_term(terms: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() && !terms.contains(current) {
        terms.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

/// Escape `LIKE` metacharacters so user text matches literally (pair with
/// `ESCAPE '\'`).
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
/// Framework route bindings, optionally narrowed by verb/path text.
pub fn routes(
    conn: &Connection,
    project_id: &str,
    route_query: Option<&str>,
    page: PageRequest,
) -> Result<Paged<StoredRelationship>> {
    let pattern = format!(
        "%{}%",
        escape_like(&route_query.unwrap_or("").replace('/', "").to_lowercase())
    );
    let joins = "FROM relationships r
         JOIN symbols src ON src.id = r.source_symbol_id
         JOIN files src_file ON src_file.id = src.file_id
         LEFT JOIN symbols tgt ON tgt.id = r.target_symbol_id";
    let filter = "WHERE r.project_id = ? AND r.kind = 'handlesroute'
           AND LOWER(REPLACE(COALESCE(r.target_external, ''), '/', '')) LIKE ? ESCAPE '\\'";
    let rows_sql = format!(
        "SELECT {RELATIONSHIP_COLUMNS} {joins} {filter}
         ORDER BY r.target_external, src.qualified_name, r.rowid LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) {joins} {filter}");
    let binds = vec![
        rusqlite::types::Value::from(project_id.to_string()),
        rusqlite::types::Value::from(pattern),
    ];
    fetch_page(conn, &count_sql, &rows_sql, binds, page, map_relationship)
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
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column,
                s.visibility, s.is_partial, s.is_record, s.roles
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
/// whether that's a hard failure. Scoped to `project_id`: an id from another
/// project sharing the same database file does not resolve.
pub fn find_symbol_by_id(
    conn: &Connection,
    project_id: &str,
    id: &str,
) -> Result<Option<StoredSymbol>> {
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column,
                s.visibility, s.is_partial, s.is_record, s.roles
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.id = ?1 AND s.project_id = ?2",
    )?;
    Ok(stmt
        .query_row(params![id, project_id], map_stored)
        .optional()?)
}

/// Ids per `IN (...)` list — well under SQLite's bound-variable limit.
const ID_CHUNK: usize = 500;

/// Distinct first-column strings of `sql_before_list` + `IN (<chunk>)` run
/// once per [`ID_CHUNK`] of the deduplicated `values`, with `project_id`
/// bound first. Keeps long lists under SQLite's bound-variable limit;
/// results are merged across chunks, sorted.
fn strings_in_chunks(
    conn: &Connection,
    project_id: &str,
    sql_before_list: &str,
    values: &[String],
) -> Result<Vec<String>> {
    let unique: std::collections::BTreeSet<&str> = values.iter().map(String::as_str).collect();
    let unique: Vec<&str> = unique.into_iter().collect();
    let mut found = std::collections::BTreeSet::new();
    for chunk in unique.chunks(ID_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare_cached(&format!("{sql_before_list} IN ({placeholders})"))?;
        let binds = std::iter::once(project_id).chain(chunk.iter().copied());
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            found.insert(row?);
        }
    }
    Ok(found.into_iter().collect())
}

/// Many symbols by storage id in one chunked lookup per [`ID_CHUNK`] ids
/// (replaces a `find_symbol_by_id` per node when rendering BFS results).
/// Duplicate ids collapse; ids unknown to `project_id` are simply absent.
pub fn find_symbols_by_ids(
    conn: &Connection,
    project_id: &str,
    ids: &[&str],
) -> Result<HashMap<String, StoredSymbol>> {
    let mut unique: Vec<&str> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();

    let mut found = HashMap::with_capacity(unique.len().min(ID_CHUNK));
    for chunk in unique.chunks(ID_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT s.id, s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                    s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column,
                    s.visibility, s.is_partial, s.is_record, s.roles
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             WHERE s.project_id = ? AND s.id IN ({placeholders})"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let binds = std::iter::once(project_id).chain(chunk.iter().copied());
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), map_stored)?;
        for row in rows {
            let symbol = row?;
            found.insert(symbol.id.clone(), symbol);
        }
    }
    Ok(found)
}

/// Per-language file counts for `project_overview` (PR3 store addition) —
/// `files.language` is nullable (parse failures/unrecognized extensions),
/// grouped under `"unknown"` rather than dropped.
pub fn language_breakdown(conn: &Connection, project_id: &str) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare_cached(
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
    pub language: LanguageId,
    /// Mirrors `SymbolTable::build_with`'s own `exports` rules (resolve.rs):
    /// true when a persisted `kind = 'exports'` relationship targets this
    /// symbol, or it is a top-level TypeScript class/function/interface
    /// (plain `export` declarations leave no edge) — i.e. it was resolvable
    /// as an import binding at index time.
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
    let mut file_stmt =
        conn.prepare_cached("SELECT relative_path FROM files WHERE project_id = ?1")?;
    let files = file_stmt
        .query_map(params![project_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut sym_stmt = conn.prepare_cached(
        "SELECT s.name, f.relative_path, s.qualified_name, s.kind, s.language,
                EXISTS(SELECT 1 FROM relationships r
                       WHERE r.target_symbol_id = s.id AND r.kind = 'exports') AS exported
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND s.kind != 'module'",
    )?;
    let symbols = sym_stmt
        .query_map(params![project_id], |row| {
            let kind: String = row.get(3)?;
            let language: String = row.get(4)?;
            let qualified_name: String = row.get(2)?;
            let kind = parse_symbol_kind(&kind);
            let language = parse_language(&language);
            // `export class Foo {}` / `export function f() {}` leave no
            // `exports` edge (extract.rs), so the persisted edge alone would
            // make every plain declaration export look un-exported on an
            // incremental pass. Mirror the clean build's fallback
            // (`SymbolTable::build_with`): a top-level TypeScript
            // class/function/interface — qualified name `<path>::<Name>`,
            // no member separator — counts as exported.
            let top_level_declaration = language == LanguageId::TypeScript
                && matches!(
                    kind,
                    SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
                )
                && qualified_name
                    .split_once("::")
                    .is_some_and(|(_, local)| !local.contains('.'));
            Ok(BaselineSymbolRow {
                name: row.get(0)?,
                relative_path: row.get(1)?,
                qualified_name,
                kind,
                language,
                exported: row.get::<_, i64>(5)? != 0 || top_level_declaration,
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
    let mut out = strings_in_chunks(
        conn,
        project_id,
        "SELECT DISTINCT f.relative_path
         FROM relationships r
         JOIN files f ON f.id = r.source_file_id
         WHERE r.project_id = ? AND r.target_symbol_id",
        target_symbol_ids,
    )?;
    let changed: std::collections::HashSet<&str> =
        changed_files.iter().map(String::as_str).collect();
    out.retain(|path| !changed.contains(path.as_str()));
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
    strings_in_chunks(
        conn,
        project_id,
        "SELECT DISTINCT f.relative_path
         FROM unresolved_references u
         JOIN files f ON f.id = u.source_file_id
         WHERE u.project_id = ? AND u.target_text",
        target_texts,
    )
}

/// `(relative_path, name)` of every namespace symbol the stored index holds
/// for `relative_paths` — what those files declared before a batch replaces
/// them.
pub fn namespace_declarations_for_files(
    conn: &Connection,
    project_id: &str,
    relative_paths: &[String],
) -> Result<std::collections::HashSet<(String, String)>> {
    let file_ids: Vec<String> = relative_paths
        .iter()
        .map(|p| file_id(project_id, p))
        .collect();
    let rows = strings_in_chunks(
        conn,
        project_id,
        "SELECT f.relative_path || char(31) || s.name
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ? AND s.kind = 'namespace' AND s.file_id",
        &file_ids,
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.split_once('\u{1f}')
                .map(|(path, name)| (path.to_string(), name.to_string()))
        })
        .collect())
}

/// Dependent-set query 3: files (outside `changed_files`) with an `imports`
/// edge into a namespace symbol named like one of `namespace_names`. A batch
/// file newly declaring `namespace N` adds a namespace symbol that every
/// `using N;` importer must now link to as well (C# `using` imports the namespace, not
/// a file), though none of those importers targets a symbol the batch changed.
pub fn dependents_by_namespace(
    conn: &Connection,
    project_id: &str,
    namespace_names: &[String],
    changed_files: &[String],
) -> Result<Vec<String>> {
    let mut out = strings_in_chunks(
        conn,
        project_id,
        "SELECT DISTINCT f.relative_path
         FROM relationships r
         JOIN symbols t ON t.id = r.target_symbol_id AND t.kind = 'namespace'
         JOIN files f ON f.id = r.source_file_id
         WHERE r.project_id = ? AND r.kind = 'imports' AND t.name",
        namespace_names,
    )?;
    let changed: std::collections::HashSet<&str> =
        changed_files.iter().map(String::as_str).collect();
    out.retain(|path| !changed.contains(path.as_str()));
    Ok(out)
}

/// Files holding a relative import that did not resolve to any project file
/// (`reason = 'module not found in project'`) and whose specifier ends in one
/// of `stems` — i.e. imports a newly created module may now satisfy, even
/// when the import names no symbol (`import * as M from './m'`, `import
/// './m'`). Bare package specifiers (`react`) never match: they have no `/`.
pub fn dependents_by_unresolved_module(
    conn: &Connection,
    project_id: &str,
    stems: &[String],
) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for chunk in stems.chunks(100) {
        let clauses = chunk
            .iter()
            .map(|_| "(u.target_text LIKE ? ESCAPE '\\' OR u.target_text LIKE ? ESCAPE '\\')")
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT DISTINCT f.relative_path
             FROM unresolved_references u
             JOIN files f ON f.id = u.source_file_id
             WHERE u.project_id = ? AND u.reason = 'module not found in project'
               AND u.target_text LIKE '%/%' AND ({clauses})"
        );
        let mut binds: Vec<String> = vec![project_id.to_string()];
        for stem in chunk {
            let escaped = escape_like(stem);
            binds.push(format!("%/{escaped}"));
            binds.push(format!("%/{escaped}.%"));
        }
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            found.push(row?);
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// The `content_hash` a file had when it was last indexed; `None` if the
/// file is not indexed or no hash was stored. Readers of live source compare
/// it with [`content_hash`] of the current bytes: a mismatch means the stored
/// spans no longer describe the file, even though the file may still be
/// long enough to slice.
pub fn indexed_content_hash(
    conn: &Connection,
    project_id: &str,
    relative_path: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT content_hash FROM files WHERE project_id = ?1 AND relative_path = ?2",
            params![project_id, relative_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
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
        visibility: row.get(12)?,
        is_partial: row.get(13)?,
        is_record: row.get(14)?,
        roles: parse_roles(&row.get::<_, String>(15)?),
    })
}

/// Reverse of `Visibility::as_str` (codekurve-core), for reading
/// `symbols.visibility` text back into the enum. Every stored value was
/// written by `as_str` itself, so the fallback is unreachable in practice —
/// kept total rather than panicking on a read path, mirroring
/// `parse_symbol_kind`.
#[allow(dead_code)] // wired into a read path once C# resolution needs it (PR3+)
fn parse_visibility(s: &str) -> Visibility {
    match s {
        "public" => Visibility::Public,
        "protected" => Visibility::Protected,
        "internal" => Visibility::Internal,
        "private" => Visibility::Private,
        "protectedinternal" => Visibility::ProtectedInternal,
        "privateprotected" => Visibility::PrivateProtected,
        _ => Visibility::Default,
    }
}

/// Reverse of the comma-joined `symbols.roles` column written by
/// `insert_file_and_symbols` (each token itself `FrameworkRole::as_str`),
/// for reading it back into `StoredSymbol.roles` (Phase 7 PR1, mirrors
/// `parse_visibility`). Empty string → empty `Vec`. An unrecognized token is
/// skipped rather than erroring — every stored value was written by
/// `as_str` itself, so this is unreachable in practice.
fn parse_roles(s: &str) -> Vec<FrameworkRole> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',')
        .filter_map(|token| match token {
            "controller" => Some(FrameworkRole::Controller),
            "route" => Some(FrameworkRole::Route),
            "service" => Some(FrameworkRole::Service),
            "repository" => Some(FrameworkRole::Repository),
            "component" => Some(FrameworkRole::Component),
            "decorator" => Some(FrameworkRole::Decorator),
            _ => None,
        })
        .collect()
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

/// Reverse of `LanguageId::as_str`, kept total for persisted rows from older
/// database versions or malformed external databases.
fn parse_language(s: &str) -> LanguageId {
    match s {
        "javascript" => LanguageId::JavaScript,
        "csharp" => LanguageId::CSharp,
        _ => LanguageId::TypeScript,
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
    use codekurve_core::{FrameworkRole, LanguageId, SymbolKind};

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
            visibility: Visibility::Default,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
            roles: Vec::new(),
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
    fn route_query_treats_like_wildcards_literally() {
        let (mut conn, pid, _interface, class, _function, _controller) = seed_graph();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let route = |path: &str| RelationshipInput {
            source_symbol_id: class.clone(),
            target_symbol_id: None,
            target_external: Some(path.to_string()),
            kind: RelationshipKind::HandlesRoute,
            provenance: Provenance::Extracted,
            confidence: Confidence::High,
            source_file_id: file_id.clone(),
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        };
        let relationships = vec![route("GET /a_b"), route("GET /axb"), route("GET /100%")];
        reindex(&mut conn, &pid, &graph_files(), &relationships, &[]).unwrap();

        let hits = |q: &str| {
            routes(&conn, &pid, Some(q), PageRequest::ALL)
                .unwrap()
                .total
        };
        assert_eq!(hits("a_b"), 1, "`_` must not match any character");
        assert_eq!(hits("%"), 1, "`%` must only match a literal percent sign");
        assert_eq!(hits("a"), 2);
    }

    #[test]
    fn pending_files_matches_index_status() {
        let conn = seed();
        let pid = project_id(&conn);
        conn.execute(
            "UPDATE index_state SET pending_files = 3 WHERE project_id = ?1",
            params![pid],
        )
        .unwrap();

        assert_eq!(pending_files(&conn, &pid).unwrap(), 3);
        assert_eq!(
            pending_files(&conn, &pid).unwrap(),
            index_status(&conn, &pid).unwrap().pending_files
        );
        assert_eq!(pending_files(&conn, "unknown-project").unwrap(), 0);
    }

    #[test]
    fn find_symbol_by_id_is_scoped_to_the_project() {
        let conn = seed();
        let pid = project_id(&conn);
        let id = search(&conn, &pid, "MemberService", 50).unwrap()[0]
            .id
            .clone();

        assert!(find_symbol_by_id(&conn, &pid, &id).unwrap().is_some());
        assert!(find_symbol_by_id(&conn, "other-project", &id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn find_symbols_by_ids_batches_dedupes_and_scopes_to_the_project() {
        let conn = seed();
        let pid = project_id(&conn);
        let id = search(&conn, &pid, "MemberService", 50).unwrap()[0]
            .id
            .clone();

        // Past SQLite's bound-variable limit: only chunking keeps this working.
        let mut ids: Vec<&str> = (0..40_000).map(|_| "missing-id").collect();
        ids.push(&id);
        ids.push(&id);

        let found = find_symbols_by_ids(&conn, &pid, &ids).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[&id].name, "MemberService");
        assert!(find_symbols_by_ids(&conn, "other-project", &ids)
            .unwrap()
            .is_empty());
        assert!(find_symbols_by_ids(&conn, &pid, &[]).unwrap().is_empty());
    }

    #[test]
    fn find_by_name_exact() {
        let conn = seed();
        let pid = project_id(&conn);
        let hits = find_by_name(&conn, &pid, "findMember").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "function");
    }

    /// Phase 7 PR1 task 1.10: `roles` sorted + deduped round-trips through
    /// `insert_file_and_symbols` → `parse_roles` for a symbol carrying 2+
    /// roles, written out of order with a duplicate.
    #[test]
    fn roles_round_trip_sorted_and_deduped() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let mut sym = symbol("WidgetService", SymbolKind::Class, 0);
        sym.roles = vec![
            FrameworkRole::Service,
            FrameworkRole::Component,
            FrameworkRole::Service,
        ];
        let files = vec![FileInput {
            relative_path: "src/widget.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![sym],
        }];
        reindex(&mut conn, &project, &files, &[], &[]).unwrap();

        let hits = find_by_name(&conn, &project, "WidgetService").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].roles,
            vec![FrameworkRole::Service, FrameworkRole::Component],
            "roles sorted (derive order) and deduped on write"
        );

        let roles_text: String = conn
            .query_row(
                "SELECT roles FROM symbols WHERE name = 'WidgetService'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(roles_text, "service,component");
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

    /// `namespace A.B { } namespace A.B { }` in one C# file yields two symbols
    /// with the same `symbol_key`. They must not fail the whole index.
    #[test]
    fn reindex_collapses_duplicate_symbol_keys_within_one_file() {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let first = symbol("Shared", SymbolKind::Class, 0);
        let second = symbol("Shared", SymbolKind::Class, 100);
        let files = vec![FileInput {
            relative_path: "src/shared.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: vec![first, second],
        }];

        let outcome = reindex(&mut conn, &project, &files, &[], &[]).unwrap();
        assert_eq!(outcome.symbols, 1);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let fts_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(fts_rows, 1);
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

    /// Phase 5 PR1 task 1.8 / symbol-index "Stable Symbol Key ... Disambiguates
    /// Partial Fragments", scenario "Non-partial key is byte-identical to the
    /// pre-Phase-5 hash": `partial_ordinal: None` must hash exactly the
    /// pre-Phase-5 five-component tuple. This hash was captured before the
    /// sixth parameter was added and must never change.
    #[test]
    fn symbol_key_none_matches_pre_migration_golden_hash() {
        let key = symbol_key(
            "typescript",
            "src/member.ts",
            "class",
            "src/member.ts::MemberService",
            "",
            None,
        );
        assert_eq!(
            key,
            "8be6b5b411dfdc32b2948f30e91dab5251ad60480b1cd3cddf250f5c1d4470af"
        );
    }

    /// Phase 5 PR1 task 1.9: `partial_ordinal` must actually participate in
    /// the hash, and `None` must differ from every `Some(_)` variant.
    #[test]
    fn symbol_key_partial_ordinal_disambiguates() {
        let base = (
            "csharp",
            "src/Widget.cs",
            "class",
            "src/Widget.cs::Widget",
            "",
        );
        let none = symbol_key(base.0, base.1, base.2, base.3, base.4, None);
        let some_0 = symbol_key(base.0, base.1, base.2, base.3, base.4, Some(0));
        let some_1 = symbol_key(base.0, base.1, base.2, base.3, base.4, Some(1));
        assert_ne!(none, some_0);
        assert_ne!(none, some_1);
        assert_ne!(some_0, some_1);
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
            exec(&tx, "DELETE FROM relationships", []).unwrap();
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
                exec(&tx, "DELETE FROM unresolved_references", []).unwrap();
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

        let calls_to_function = callers(&conn, &pid, &function, None, PageRequest::ALL)
            .unwrap()
            .rows;
        assert_eq!(calls_to_function.len(), 2);
        assert!(calls_to_function.iter().all(|r| r.kind == "calls"));
        assert!(calls_to_function.iter().any(|r| r.confidence == "exact"));
        assert!(calls_to_function.iter().any(|r| r.confidence == "low"));

        let calls_from_controller = callees(&conn, &pid, &controller, None, PageRequest::ALL)
            .unwrap()
            .rows;
        assert_eq!(calls_from_controller.len(), 1);
        assert_eq!(calls_from_controller[0].confidence, "exact");

        let implementers = implementations(&conn, &pid, &interface, None, PageRequest::ALL)
            .unwrap()
            .rows;
        assert_eq!(implementers.len(), 1);
        assert_eq!(implementers[0].kind, "implements");
        assert_eq!(implementers[0].source_symbol_id, class);

        // `references` is kind-agnostic: the one edge targeting the
        // interface (Implements) comes back even though it's not a call.
        let refs_to_interface = references(&conn, &pid, &interface, None, PageRequest::ALL)
            .unwrap()
            .rows;
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

        let filtered = callers(
            &conn,
            &pid,
            &function,
            Some(Confidence::High),
            PageRequest::ALL,
        )
        .unwrap()
        .rows;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].confidence, "exact");
    }

    /// QRY-10: lookups keyed by a long list (a big batch's symbols, targets
    /// or names) are chunked, so they never exceed SQLite's bound-variable
    /// limit, and still find the real entries among the bogus ones.
    #[test]
    fn long_in_lists_are_chunked() {
        let (conn, pid) = seed_unresolved();
        let bogus =
            |prefix: &str| -> Vec<String> { (0..40_000).map(|i| format!("{prefix}{i}")).collect() };

        let mut paths = bogus("missing/");
        paths.push("src/member.ts".to_string());
        assert!(!symbol_ids_for_files(&conn, &pid, &paths)
            .unwrap()
            .is_empty());

        assert!(
            dependents_by_target_symbol(&conn, &pid, &bogus("sym-"), &[])
                .unwrap()
                .is_empty()
        );

        let mut texts = bogus("Nope");
        texts.push("ILogger".to_string());
        assert_eq!(
            dependents_by_unresolved_target(&conn, &pid, &texts).unwrap(),
            vec!["src/member.ts".to_string()]
        );
    }

    fn file_with(path: &str, names: &[&str]) -> FileInput {
        FileInput {
            relative_path: path.to_string(),
            language: "typescript".to_string(),
            content_hash: format!("hash-{path}"),
            modified_ns: 0,
            size_bytes: 10,
            symbols: names
                .iter()
                .enumerate()
                .map(|(i, n)| symbol(n, SymbolKind::Function, i * 100))
                .collect(),
        }
    }

    /// QRY-5: the symbol kind is not searchable text, so a query that equals
    /// a kind ("function", "class") no longer returns every symbol of it.
    #[test]
    fn search_does_not_match_the_kind_column() {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        reindex(&mut conn, &pid, &[file_with("a.ts", &["walrus"])], &[], &[]).unwrap();

        assert!(search(&conn, &pid, "function", 50).unwrap().is_empty());
        assert_eq!(search(&conn, &pid, "walrus", 50).unwrap().len(), 1);
    }

    /// QRY-5: an exact name match ranks ahead of symbols that only match
    /// through their qualified name (here: `src/zed.ts::Apple`).
    #[test]
    fn search_ranks_exact_name_matches_first() {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let mut zed_file = file_with("src/zed.ts", &["Apple"]);
        zed_file.symbols[0].qualified_name = "src/zed.ts::Apple".to_string();
        let mut other = file_with("src/other.ts", &["Zed"]);
        other.symbols[0].qualified_name = "src/other.ts::Zed".to_string();
        reindex(&mut conn, &pid, &[zed_file, other], &[], &[]).unwrap();

        let names: Vec<String> = search(&conn, &pid, "zed", 50)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["Zed", "Apple"]);
    }

    /// A file outside the batch with edges into same-named symbols of two
    /// batch files (C# `x.Name`) has both edges parked as unresolved rows;
    /// their ids must not collide across the per-file conversions.
    fn inbound_fixture() -> (Connection, String, String, String) {
        let named = |dir: &str, name: &str| {
            let mut f = file_with(&format!("{dir}.ts"), &[name]);
            f.symbols[0].qualified_name = format!("{dir}.ts::{name}");
            f
        };
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = [named("x", "run"), named("a", "Name"), named("b", "Name")];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();
        let sym = |path: &str| -> String {
            conn.query_row(
                "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
                 WHERE f.relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let fid = |path: &str| -> String {
            conn.query_row(
                "SELECT id FROM files WHERE relative_path = ?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap()
        };
        let (run, a_name, b_name) = (sym("x.ts"), sym("a.ts"), sym("b.ts"));
        let file_x = fid("x.ts");
        let reads = |target: &str| RelationshipInput {
            source_symbol_id: run.clone(),
            target_symbol_id: Some(target.to_string()),
            target_external: None,
            kind: RelationshipKind::Reads,
            provenance: Provenance::Heuristic,
            confidence: Confidence::Low,
            source_file_id: file_x.clone(),
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        };
        reindex(
            &mut conn,
            &pid,
            &files,
            &[reads(&a_name), reads(&b_name)],
            &[],
        )
        .unwrap();
        (conn, pid, run, file_x)
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn named_file(dir: &str, name: &str) -> FileInput {
        let mut f = file_with(&format!("{dir}.ts"), &[name]);
        f.symbols[0].qualified_name = format!("{dir}.ts::{name}");
        f
    }

    /// `x.ts`'s `run` reads three symbols of `t.ts`; deleting `t.ts` parks
    /// all three edges as unresolved rows (PERF-7 (b) fixture).
    fn parked_edges_fixture() -> (Connection, String) {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let mut target = file_with("t.ts", &["Alpha", "Beta", "Gamma"]);
        for sym in &mut target.symbols {
            sym.qualified_name = format!("t.ts::{}", sym.name);
        }
        let mut caller = file_with("x.ts", &["run"]);
        caller.symbols[0].qualified_name = "x.ts::run".to_string();
        let files = [caller, target];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();

        let symbol_id = |conn: &Connection, path: &str, name: &str| -> String {
            conn.query_row(
                "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
                 WHERE f.relative_path = ?1 AND s.name = ?2",
                params![path, name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let run = symbol_id(&conn, "x.ts", "run");
        let file_x: String = conn
            .query_row(
                "SELECT id FROM files WHERE relative_path = 'x.ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let edges: Vec<RelationshipInput> = ["Alpha", "Beta", "Gamma"]
            .iter()
            .map(|name| RelationshipInput {
                source_symbol_id: run.clone(),
                target_symbol_id: Some(symbol_id(&conn, "t.ts", name)),
                target_external: None,
                kind: RelationshipKind::Reads,
                provenance: Provenance::Heuristic,
                confidence: Confidence::Low,
                source_file_id: file_x.clone(),
                start_line: Some(1),
                start_column: Some(0),
                reason: None,
            })
            .collect();
        reindex(&mut conn, &pid, &files, &edges, &[]).unwrap();
        (conn, pid)
    }

    /// PERF-7 (b) equivalence pin: deleting a file with inbound edges leaves
    /// exactly one parked unresolved row per edge (target text, reason, kind,
    /// confidence) and no relationship pointing at the deleted file.
    #[test]
    fn delete_file_parks_each_inbound_edge_with_its_reason() {
        let (mut conn, pid) = parked_edges_fixture();
        let tx = conn.transaction().unwrap();
        delete_file(&tx, &pid, &now_ts(), "t.ts").unwrap();
        tx.commit().unwrap();

        assert_eq!(count(&conn, "relationships"), 0);
        let mut stmt = conn
            .prepare(
                "SELECT target_text, reason, relationship_kind, confidence, candidate_count
                 FROM unresolved_references ORDER BY target_text",
            )
            .unwrap();
        let rows: Vec<(String, String, String, String, i64)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let parked = "target symbol's file was deleted";
        let row = |target: &str| {
            (
                target.to_string(),
                parked.to_string(),
                "reads".to_string(),
                "unresolved".to_string(),
                0,
            )
        };
        assert_eq!(rows, vec![row("Alpha"), row("Beta"), row("Gamma")]);
    }

    /// PERF-7 (b): three inbound edges are parked by one DELETE statement,
    /// not one statement per edge.
    #[test]
    fn parking_inbound_edges_deletes_them_in_one_statement() {
        let (mut conn, pid) = parked_edges_fixture();
        PARK_DELETE_STATEMENTS.with(|c| c.set(0));
        let tx = conn.transaction().unwrap();
        delete_file(&tx, &pid, &now_ts(), "t.ts").unwrap();
        tx.commit().unwrap();

        assert_eq!(PARK_DELETE_STATEMENTS.with(|c| c.get()), 1);
    }

    /// A batch file's symbols are deleted and re-inserted with the same ids;
    /// an outside file's edge into one that survives must stay an edge (it
    /// used to be demoted to unresolved, so an incremental index drifted from
    /// a clean rebuild: 1,509 edges lost on the real C# project).
    #[test]
    fn inbound_edges_into_surviving_symbols_of_batch_files_are_kept() {
        let (mut conn, pid, _run, _x) = inbound_fixture();
        let tx = conn.transaction().unwrap();
        let batch = [named_file("a", "Name"), named_file("b", "Name")];
        apply_incremental(&tx, &pid, &now_ts(), &batch, &[], &[], &[]).unwrap();
        tx.commit().unwrap();

        assert_eq!(count(&conn, "relationships"), 2);
        assert_eq!(count(&conn, "unresolved_references"), 0);
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relationships r
                 WHERE NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = r.target_symbol_id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0);
    }

    /// Edges into symbols the batch removed are parked as unresolved. Two
    /// edges from one outside file into same-named symbols of two batch files
    /// share an unresolved-id key; their ids must not collide.
    #[test]
    fn inbound_edges_into_removed_same_named_symbols_do_not_collide() {
        let (mut conn, pid, _run, _x) = inbound_fixture();
        let tx = conn.transaction().unwrap();
        let batch = [named_file("a", "Other"), named_file("b", "Other")];
        apply_incremental(&tx, &pid, &now_ts(), &batch, &[], &[], &[]).unwrap();
        tx.commit().unwrap();

        assert_eq!(count(&conn, "relationships"), 0);
        assert_eq!(count(&conn, "unresolved_references"), 2);
    }

    #[test]
    fn inbound_edge_matching_an_existing_unresolved_row_does_not_collide() {
        let (mut conn, pid, run, file_x) = inbound_fixture();
        // The same logical reference is already parked as unresolved.
        let tx = conn.transaction().unwrap();
        persist_unresolved(
            &tx,
            &pid,
            "ts0",
            &[UnresolvedReferenceInput {
                source_symbol_id: Some(run),
                source_file_id: file_x,
                relationship_kind: RelationshipKind::Reads,
                target_text: "Name".to_string(),
                context_json: None,
                candidate_count: 9,
                reason: "ambiguous: 9 candidates".to_string(),
                confidence: Confidence::Unresolved,
            }],
        )
        .unwrap();
        tx.commit().unwrap();

        // `a` loses `Name`: x's edge into it is parked next to the old row.
        let tx = conn.transaction().unwrap();
        apply_incremental(
            &tx,
            &pid,
            &now_ts(),
            &[named_file("a", "Other")],
            &[],
            &[],
            &[],
        )
        .unwrap();
        tx.commit().unwrap();

        // `x` still reads b.Name.
        assert_eq!(count(&conn, "relationships"), 1);
        assert!(count(&conn, "unresolved_references") >= 1);
    }

    #[test]
    fn namespace_declarations_and_importers_are_found_by_namespace_name() {
        let (mut conn, pid, _run, file_x) = inbound_fixture();
        // a.ts declares a namespace symbol `NS`; x.ts imports it.
        let files = || {
            let mut a = file_with("a.ts", &["Name"]);
            a.symbols[0].qualified_name = "a.ts::Name".to_string();
            a.symbols.push(symbol("NS", SymbolKind::Namespace, 500));
            a.symbols[1].qualified_name = "a.ts::NS".to_string();
            [a, named_file("b", "Name"), named_file("x", "run")]
        };
        reindex(&mut conn, &pid, &files(), &[], &[]).unwrap();
        let sym = |name: &str| -> String {
            conn.query_row(
                "SELECT id FROM symbols WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .unwrap()
        };
        let edge = RelationshipInput {
            source_symbol_id: sym("run"),
            target_symbol_id: Some(sym("NS")),
            target_external: None,
            kind: RelationshipKind::Imports,
            provenance: Provenance::Heuristic,
            confidence: Confidence::Low,
            source_file_id: file_x,
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        };
        reindex(&mut conn, &pid, &files(), &[edge], &[]).unwrap();

        let declared = namespace_declarations_for_files(
            &conn,
            &pid,
            &["a.ts".to_string(), "b.ts".to_string()],
        )
        .unwrap();
        assert_eq!(
            declared,
            std::collections::HashSet::from([("a.ts".to_string(), "NS".to_string())])
        );
        let importers = dependents_by_namespace(&conn, &pid, &["NS".to_string()], &[]).unwrap();
        assert_eq!(importers, vec!["x.ts".to_string()]);
        assert!(
            dependents_by_namespace(&conn, &pid, &["NS".to_string()], &["x.ts".to_string()])
                .unwrap()
                .is_empty()
        );
    }

    /// QRY-5 follow-up: equal names order by qualified name, never by insertion
    /// order, so a limited page is the same for a fresh and an incrementally
    /// built index.
    #[test]
    fn search_orders_equal_names_independently_of_index_order() {
        let save = |dir: &str| {
            let mut f = file_with(&format!("{dir}/save.ts"), &["Save"]);
            f.symbols[0].qualified_name = format!("{dir}/save.ts::Save");
            f
        };
        let names = |conn: &Connection, pid: &str| -> Vec<String> {
            search(conn, pid, "Save", 50)
                .unwrap()
                .into_iter()
                .map(|s| s.qualified_name)
                .collect()
        };

        let mut fresh = db::open_in_memory().unwrap();
        let pid = upsert_project(&fresh, "demo", "/tmp/demo", "hash").unwrap();
        reindex(&mut fresh, &pid, &[save("a"), save("b")], &[], &[]).unwrap();

        let mut grown = db::open_in_memory().unwrap();
        let pid2 = upsert_project(&grown, "demo", "/tmp/demo", "hash").unwrap();
        reindex(&mut grown, &pid2, &[save("b")], &[], &[]).unwrap();
        let tx = grown.transaction().unwrap();
        apply_incremental(&tx, &pid2, &now_ts(), &[save("a")], &[], &[], &[]).unwrap();
        tx.commit().unwrap();

        assert_eq!(
            names(&fresh, &pid),
            vec!["a/save.ts::Save", "b/save.ts::Save"]
        );
        assert_eq!(names(&grown, &pid2), names(&fresh, &pid));
    }

    /// QRY-5 follow-up (real C# project): a symbol whose own name starts with
    /// the query ranks ahead of symbols that only match through their file path.
    #[test]
    fn search_ranks_own_name_prefix_before_path_matches() {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let mut in_path = file_with("src/RegisterManager.ts", &["Apple"]);
        in_path.symbols[0].qualified_name = "src/RegisterManager.ts::Apple".to_string();
        let mut own = file_with("src/other.ts", &["RegisterManager"]);
        own.symbols[0].qualified_name = "src/other.ts::RegisterManager".to_string();
        reindex(&mut conn, &pid, &[in_path, own], &[], &[]).unwrap();

        let names: Vec<String> = search(&conn, &pid, "RegisterMan", 50)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["RegisterManager", "Apple"]);
    }

    /// QRY-5: a name prefix finds longer names through the FTS index.
    #[test]
    fn search_matches_name_prefixes() {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        reindex(
            &mut conn,
            &pid,
            &[file_with("a.ts", &["RegisterManager", "Other"])],
            &[],
            &[],
        )
        .unwrap();

        let hits = search(&conn, &pid, "RegisterMan", 50).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "RegisterManager");
    }

    /// QRY-8: the batched FTS/symbol delete in `apply_incremental` replaces
    /// exactly the batch files' rows and leaves every other file's search
    /// entries alone.
    #[test]
    fn apply_incremental_replaces_fts_rows_for_the_whole_batch_only() {
        let file = |path: &str, name: &str| FileInput {
            relative_path: path.to_string(),
            language: "typescript".to_string(),
            content_hash: format!("hash-{name}"),
            modified_ns: 0,
            size_bytes: 10,
            symbols: vec![symbol(name, SymbolKind::Function, 0)],
        };
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let before = [
            file("a.ts", "quokka"),
            file("b.ts", "emu"),
            file("c.ts", "tapir"),
        ];
        reindex(&mut conn, &pid, &before, &[], &[]).unwrap();

        let tx = conn.transaction().unwrap();
        let batch = [file("a.ts", "walrus"), file("b.ts", "ibex")];
        apply_incremental(&tx, &pid, &now_ts(), &batch, &[], &[], &[]).unwrap();
        tx.commit().unwrap();

        let names = |q: &str| search(&conn, &pid, q, 50).unwrap().len();
        assert_eq!(names("quokka"), 0);
        assert_eq!(names("emu"), 0);
        assert_eq!(names("walrus"), 1);
        assert_eq!(names("ibex"), 1);
        assert_eq!(names("tapir"), 1);
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("symbols_fts"), count("symbols"));
        assert_eq!(count("symbols"), 3);
    }

    /// QRY-4: paging and the `min_confidence` filter run in SQL, and `total`
    /// counts the filtered rows (not the page, not the unfiltered set).
    #[test]
    fn relationship_pages_are_cut_in_sql_and_total_counts_filtered_rows() {
        let (mut conn, pid, interface, class, function, controller) = seed_graph();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let call = |source: &str, confidence: Confidence, line: u32| RelationshipInput {
            source_symbol_id: source.to_string(),
            target_symbol_id: Some(function.clone()),
            target_external: None,
            kind: RelationshipKind::Calls,
            provenance: Provenance::Extracted,
            confidence,
            source_file_id: file_id.clone(),
            start_line: Some(line),
            start_column: Some(0),
            reason: None,
        };
        let relationships = vec![
            call(&controller, Confidence::Exact, 5),
            call(&interface, Confidence::High, 7),
            call(&class, Confidence::Low, 9),
        ];
        reindex(&mut conn, &pid, &graph_files(), &relationships, &[]).unwrap();

        let page = |min: Option<Confidence>, limit: Option<usize>, offset: usize| {
            let p = callers(&conn, &pid, &function, min, PageRequest { limit, offset }).unwrap();
            (
                p.rows.iter().map(|r| r.start_line).collect::<Vec<_>>(),
                p.total,
            )
        };
        assert_eq!(page(None, None, 0), (vec![Some(5), Some(7), Some(9)], 3));
        assert_eq!(page(None, Some(1), 1), (vec![Some(7)], 3));
        assert_eq!(page(Some(Confidence::High), Some(1), 0), (vec![Some(5)], 2));
        assert_eq!(page(Some(Confidence::High), Some(5), 1), (vec![Some(7)], 2));
        assert_eq!(page(Some(Confidence::High), Some(5), 9), (vec![], 2));
    }

    #[test]
    fn unresolved_pages_are_cut_in_sql_with_the_filtered_total() {
        let (conn, pid) = seed_unresolved();
        let page = |target: Option<&str>, limit: Option<usize>, offset: usize| {
            let p = unresolved(&conn, &pid, target, None, PageRequest { limit, offset }).unwrap();
            (
                p.rows
                    .into_iter()
                    .map(|r| r.target_text)
                    .collect::<Vec<_>>(),
                p.total,
            )
        };
        assert_eq!(
            page(None, Some(2), 1),
            (
                vec![
                    "ILogger".to_string(),
                    "IOpenApiDocumentTransformer".to_string()
                ],
                3
            )
        );
        assert_eq!(page(None, Some(2), 3), (vec![], 3));
        assert_eq!(
            page(Some("ILogger"), Some(2), 0),
            (vec!["ILogger".to_string()], 1)
        );
    }

    #[test]
    fn route_pages_are_cut_in_sql_with_the_filtered_total() {
        let (mut conn, pid, _interface, class, _function, _controller) = seed_graph();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let route = |path: &str| RelationshipInput {
            source_symbol_id: class.clone(),
            target_symbol_id: None,
            target_external: Some(path.to_string()),
            kind: RelationshipKind::HandlesRoute,
            provenance: Provenance::Extracted,
            confidence: Confidence::High,
            source_file_id: file_id.clone(),
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        };
        let relationships = vec![route("GET /a"), route("GET /b"), route("GET /c")];
        reindex(&mut conn, &pid, &graph_files(), &relationships, &[]).unwrap();

        let page = |query: Option<&str>, limit: Option<usize>, offset: usize| {
            let p = routes(&conn, &pid, query, PageRequest { limit, offset }).unwrap();
            (
                p.rows
                    .into_iter()
                    .filter_map(|r| r.target_external)
                    .collect::<Vec<_>>(),
                p.total,
            )
        };
        assert_eq!(
            page(None, Some(2), 1),
            (vec!["GET /b".to_string(), "GET /c".to_string()], 3)
        );
        assert_eq!(page(Some("b"), Some(2), 0), (vec!["GET /b".to_string()], 1));
        assert_eq!(page(None, Some(2), 3), (vec![], 3));
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

    /// Seeds `seed()`'s project with three unresolved rows: two different
    /// targets owned by `MemberService`, and one file-level row (no source
    /// symbol) for a third target — enough to exercise every filter
    /// combination `unresolved` supports.
    fn seed_unresolved() -> (Connection, String) {
        let mut conn = seed();
        let pid = project_id(&conn);
        let file = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        let owner: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'MemberService'",
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
                symbol("MemberService", SymbolKind::Class, 0),
                symbol("findMember", SymbolKind::Function, 100),
            ],
        }];
        let row = |source: Option<&str>, target: &str, reason: &str| UnresolvedReferenceInput {
            source_symbol_id: source.map(str::to_string),
            source_file_id: file.clone(),
            relationship_kind: RelationshipKind::UsesType,
            target_text: target.to_string(),
            context_json: None,
            candidate_count: 0,
            reason: reason.to_string(),
            confidence: Confidence::Unresolved,
        };
        let unresolved_rows = vec![
            row(
                Some(&owner),
                "IOpenApiDocumentTransformer",
                "base list entry not found in project; class vs interface undeterminable",
            ),
            row(Some(&owner), "ILogger", "zero candidates"),
            row(None, "ExternalThing", "zero candidates"),
        ];
        reindex(&mut conn, &pid, &files, &[], &unresolved_rows).unwrap();
        (conn, pid)
    }

    /// The whole point of the query: an exact `target_text` filter returns
    /// the row *with the reason the analyzer recorded*, which is otherwise
    /// only ever surfaced as `index_status`'s bare count.
    #[test]
    fn unresolved_filters_by_target_text() {
        let (conn, pid) = seed_unresolved();

        let rows = unresolved(
            &conn,
            &pid,
            Some("IOpenApiDocumentTransformer"),
            None,
            PageRequest::ALL,
        )
        .unwrap()
        .rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target_text, "IOpenApiDocumentTransformer");
        assert_eq!(
            rows[0].reason,
            "base list entry not found in project; class vs interface undeterminable"
        );
        assert_eq!(rows[0].relationship_kind, "usestype");
        assert_eq!(rows[0].confidence, "unresolved");
        assert_eq!(rows[0].source_relative_path, "src/member.ts");
        assert_eq!(
            rows[0].source_qualified_name.as_deref(),
            Some("src/member.ts::MemberService"),
            "the source symbol's qualified name is joined in, not just its id"
        );

        // Exact match, never substring — a prefix must not match.
        assert!(
            unresolved(&conn, &pid, Some("IOpenApi"), None, PageRequest::ALL)
                .unwrap()
                .rows
                .is_empty()
        );
    }

    /// `source_symbol_id` narrows to the rows one symbol owns; the
    /// file-level row (null `source_symbol_id`) is excluded by it, but still
    /// listed — with a `None` qualified name — when no filter is given.
    #[test]
    fn unresolved_filters_by_source_symbol() {
        let (conn, pid) = seed_unresolved();
        let owner: String = conn
            .query_row(
                "SELECT id FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let mut targets: Vec<String> =
            unresolved(&conn, &pid, None, Some(&owner), PageRequest::ALL)
                .unwrap()
                .rows
                .into_iter()
                .map(|u| u.target_text)
                .collect();
        targets.sort();
        assert_eq!(targets, vec!["ILogger", "IOpenApiDocumentTransformer"]);

        let all = unresolved(&conn, &pid, None, None, PageRequest::ALL)
            .unwrap()
            .rows;
        assert_eq!(all.len(), 3);
        let file_level = all
            .iter()
            .find(|u| u.target_text == "ExternalThing")
            .unwrap();
        assert!(file_level.source_symbol_id.is_none());
        assert!(file_level.source_qualified_name.is_none());
        assert_eq!(file_level.source_relative_path, "src/member.ts");

        // Both filters compose in the same SELECT.
        let both = unresolved(&conn, &pid, Some("ILogger"), Some(&owner), PageRequest::ALL)
            .unwrap()
            .rows;
        assert_eq!(both.len(), 1);
        assert!(unresolved(
            &conn,
            &pid,
            Some("ExternalThing"),
            Some(&owner),
            PageRequest::ALL
        )
        .unwrap()
        .rows
        .is_empty());
    }

    /// A project with nothing unresolved is an empty result, never an error —
    /// same contract the relationship queries give.
    #[test]
    fn unresolved_empty_project_returns_no_rows() {
        let conn = seed();
        let pid = project_id(&conn);
        assert!(unresolved(&conn, &pid, None, None, PageRequest::ALL)
            .unwrap()
            .rows
            .is_empty());
        assert!(
            unresolved(&conn, &pid, Some("Anything"), None, PageRequest::ALL)
                .unwrap()
                .rows
                .is_empty()
        );
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
        // No `exports` edge targets it, but a top-level TypeScript class is
        // exported by the clean build's declaration fallback, and the
        // snapshot must agree or incremental passes lose its importers.
        assert!(member_service.exported);
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

    /// Regression (Fase 8 pilot, real-repo incremental edit crash): when an
    /// incremental batch re-submits BOTH a file whose symbol is targeted by
    /// another file's edge AND that dependent file itself (design's
    /// "Dependent Re-Resolution Scope" D-set correctly pulls the dependent
    /// in), `apply_incremental` must not delete the target's symbols before
    /// the dependent's own edge referencing them is gone too — `foreign_keys
    /// = ON` checks are immediate, so doing it file-by-file in one
    /// interleaved pass trips `FOREIGN KEY constraint failed` depending on
    /// slice order, even though the edge is about to be rebuilt identically
    /// a few statements later in the same transaction.
    #[test]
    fn apply_incremental_reindexing_a_target_and_its_dependent_together_does_not_violate_fk() {
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

        // Incremental batch re-submits BOTH files (a.ts is "the edit", b.ts
        // is the D-set dependent pulled in because its edge targets a.ts's
        // symbol) — same content, so the same deterministic symbol/file ids
        // come back out, but the delete-then-reinsert sequence must not trip
        // over the still-live inbound edge partway through.
        let tx = conn.transaction().unwrap();
        apply_incremental(&tx, &project, "ts-incr", &files, &relationships, &[], &[])
            .expect("must not violate the foreign-key constraint mid-batch");
        tx.commit().unwrap();

        let remaining_calls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relationships WHERE kind = 'calls'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_calls, 1,
            "the edge survives the round-trip intact"
        );
    }

    /// A 300-function call chain: enough rows that the planner's choice
    /// reflects statistics rather than "any index will do".
    fn seed_call_chain() -> (Connection, String) {
        let mut conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 42,
            symbols: (0..300)
                .map(|n| symbol(&format!("f{n}"), SymbolKind::Function, n * 10))
                .collect(),
        }];
        reindex(&mut conn, &pid, &files, &[], &[]).unwrap();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let id = |n: usize| -> String {
            conn.query_row(
                "SELECT id FROM symbols WHERE name = ?1",
                params![format!("f{n}")],
                |r| r.get(0),
            )
            .unwrap()
        };
        let relationships: Vec<_> = (1..300)
            .map(|n| RelationshipInput {
                source_symbol_id: id(n),
                target_symbol_id: Some(id(n - 1)),
                target_external: None,
                kind: RelationshipKind::Calls,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                source_file_id: file_id.clone(),
                start_line: Some(n as u32),
                start_column: Some(0),
                reason: None,
            })
            .collect();
        reindex(&mut conn, &pid, &files, &relationships, &[]).unwrap();
        (conn, pid)
    }

    fn query_plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(3)).unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n")
    }

    /// Without statistics SQLite takes `(project_id, kind)` for a
    /// by-target relationship lookup (every edge of that kind in the project)
    /// and walks every project symbol for FTS search; with them it seeks by
    /// target / by FTS hit. The queries mirror `query_relationships` and
    /// `search` (the `ORDER BY s.name` is what lures the unanalyzed planner
    /// onto `(project_id, name)`).
    #[test]
    fn planner_stats_make_lookups_use_selective_indexes() {
        let (conn, pid) = seed_call_chain();
        let callers_sql = format!(
            "SELECT r.source_symbol_id FROM relationships r
             WHERE r.project_id = '{pid}' AND r.target_symbol_id = 'x' AND r.kind IN ('calls')"
        );
        let search_sql = format!(
            "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.project_id = '{pid}'
               AND s.id IN (SELECT symbol_id FROM symbols_fts WHERE symbols_fts MATCH '\"f7\"')
             ORDER BY s.name LIMIT 50"
        );

        assert!(query_plan(&conn, &callers_sql).contains("idx_relationships_project_kind"));
        assert!(query_plan(&conn, &search_sql).contains("idx_symbols_project_name"));

        db::refresh_planner_stats(&conn).unwrap();

        let callers_plan = query_plan(&conn, &callers_sql);
        assert!(
            callers_plan.contains("idx_relationships_target_kind"),
            "{callers_plan}"
        );
        let search_plan = query_plan(&conn, &search_sql);
        assert!(
            search_plan.contains("sqlite_autoindex_symbols_1 (id=?)"),
            "{search_plan}"
        );

        // Second call takes the `PRAGMA optimize` branch and keeps the plan.
        db::refresh_planner_stats(&conn).unwrap();
        assert!(query_plan(&conn, &callers_sql).contains("idx_relationships_target_kind"));
    }

    #[test]
    fn pending_files_reads_index_state_and_defaults_to_zero() {
        let conn = db::open_in_memory().unwrap();
        let pid = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        assert_eq!(pending_files(&conn, &pid).unwrap(), 0);

        let mut conn = conn;
        let tx = conn.transaction().unwrap();
        set_pending_files(&tx, &pid, &now_ts(), 3).unwrap();
        tx.commit().unwrap();
        assert_eq!(pending_files(&conn, &pid).unwrap(), 3);
        assert_eq!(index_status(&conn, &pid).unwrap().pending_files, 3);
    }
}
