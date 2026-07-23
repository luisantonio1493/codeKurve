//! Repository operations for the Phase 1 vertical slice: register a project,
//! rebuild its index transactionally, and query symbols. See
//! CODEKURVE_MASTER_PLAN.md §24 (schema) and §Fase 1 (scope).

use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use codekurve_core::{SourceSpan, Symbol};
use rusqlite::{params, Connection, Row};

use crate::error::Result;

/// One discovered file plus the symbols extracted from it.
pub struct FileInput {
    pub relative_path: String,
    pub language: String,
    pub size_bytes: u64,
    pub symbols: Vec<Symbol>,
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
) -> Result<IndexOutcome> {
    let tx = conn.transaction()?;

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
        let file_id = hash_id("fil", &format!("{project_id}/{}", file.relative_path));
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
            // §16.3: excludes start_byte so unrelated edits shifting later
            // byte offsets don't change unaffected symbols' identity.
            let symbol_key = format!(
                "{}/{}/{}/{}",
                symbol.language.as_str(),
                file.relative_path,
                symbol.kind.as_str(),
                symbol.qualified_name,
            );
            let symbol_id = hash_id("sym", &format!("{file_id}/{symbol_key}"));
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

    tx.commit()?;
    Ok(outcome)
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
        let outcome = reindex(&mut conn, &project, &files).unwrap();
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
        let outcome = reindex(&mut conn, &pid, &files).unwrap();
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
        reindex(&mut conn, &pid, &files).unwrap();

        let after: String = conn
            .query_row(
                "SELECT symbol_key FROM symbols WHERE name = 'MemberService'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }
}
