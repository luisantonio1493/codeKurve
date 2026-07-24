//! Repository operations for the Phase 1 vertical slice: register a project,
//! rebuild its index transactionally, and query symbols. See
//! CODEKURVE_MASTER_PLAN.md §24 (schema) and §Fase 1 (scope).

use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use codekurve_core::{Confidence, Provenance, RelationshipKind, SourceSpan, Symbol};
use rusqlite::{params, Connection, Row, Transaction};

use crate::error::Result;

/// One discovered file plus the symbols extracted from it.
pub struct FileInput {
    pub relative_path: String,
    pub language: String,
    pub size_bytes: u64,
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

/// A symbol as read back from storage (display model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSymbol {
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

/// §16.3: excludes `start_byte` so unrelated edits shifting later byte
/// offsets don't change unaffected symbols' identity.
pub fn symbol_key(language: &str, relative_path: &str, kind: &str, qualified_name: &str) -> String {
    format!("{language}/{relative_path}/{kind}/{qualified_name}")
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
        let file_id = file_id(project_id, &file.relative_path);
        tx.execute(
            "INSERT INTO files(id, project_id, relative_path, language, size_bytes,
                 parse_status, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'ok', 1, ?6, ?6)",
            params![
                file_id,
                project_id,
                file.relative_path,
                file.language,
                file.size_bytes as i64,
                ts,
            ],
        )?;
        outcome.files += 1;

        for symbol in &file.symbols {
            let symbol_key = symbol_key(
                symbol.language.as_str(),
                &file.relative_path,
                symbol.kind.as_str(),
                &symbol.qualified_name,
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
            outcome.symbols += 1;
        }
    }

    persist_relationships(&tx, project_id, &ts, relationships)?;
    persist_unresolved(&tx, project_id, &ts, unresolved)?;

    tx.commit()?;
    Ok(outcome)
}

fn persist_relationships(
    tx: &Transaction,
    project_id: &str,
    ts: &str,
    relationships: &[RelationshipInput],
) -> Result<()> {
    for (i, rel) in relationships.iter().enumerate() {
        let id = hash_id("rel", &format!("{project_id}/{}/{i}", rel.source_symbol_id));
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
    for (i, u) in unresolved.iter().enumerate() {
        let id = hash_id("unr", &format!("{project_id}/{}/{i}", u.source_file_id));
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
                r.target_external, r.kind, r.provenance, r.confidence, r.start_line, r.start_column
         FROM relationships r
         JOIN symbols src ON src.id = r.source_symbol_id
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
        "SELECT s.name, s.qualified_name, s.kind, s.language, f.relative_path,
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
        "SELECT s.name, s.qualified_name, s.kind, s.language, f.relative_path,
                s.start_byte, s.end_byte, s.start_line, s.start_column, s.end_line, s.end_column
         FROM symbols s
         JOIN files f ON f.id = s.file_id
         WHERE s.project_id = ?1 AND s.name = ?2
         ORDER BY f.relative_path, s.start_byte",
    )?;
    let rows = stmt.query_map(params![project_id, name], map_stored)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Stable content hash of the config text. ponytail: DefaultHasher is a
/// placeholder; BLAKE3 replaces it when hashing lands in Phase 3.
pub fn config_hash(config_text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config_text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn map_stored(row: &Row) -> rusqlite::Result<StoredSymbol> {
    Ok(StoredSymbol {
        name: row.get(0)?,
        qualified_name: row.get(1)?,
        kind: row.get(2)?,
        language: row.get(3)?,
        relative_path: row.get(4)?,
        span: SourceSpan {
            start_byte: row.get::<_, i64>(5)? as usize,
            end_byte: row.get::<_, i64>(6)? as usize,
            start_line: row.get::<_, i64>(7)? as usize,
            start_column: row.get::<_, i64>(8)? as usize,
            end_line: row.get::<_, i64>(9)? as usize,
            end_column: row.get::<_, i64>(10)? as usize,
        },
    })
}

fn hash_id(prefix: &str, input: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{prefix}-{:016x}", hasher.finish())
}

fn now_ts() -> String {
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
        }
    }

    fn seed() -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![FileInput {
            relative_path: "src/member.ts".to_string(),
            language: "typescript".to_string(),
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
}
