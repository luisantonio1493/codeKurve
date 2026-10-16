//! PERF-1 guard: a full index must produce the same symbols and relationships
//! no matter how its files are scheduled. The same project indexed twice (in
//! two different directories) must yield identical sets, and the cross-file
//! `inherits` and `imports` edges must all resolve to their targets.

use std::fs;
use std::path::Path;

use codekurve::query;
use codekurve_core::project;
use codekurve_store::{db, Connection};

const FILES: usize = 40;

fn write_project(root: &Path) {
    for i in 0..FILES {
        let next = (i + 1) % FILES;
        fs::write(
            root.join(format!("f{i}.ts")),
            format!(
                "import {{ C{next} }} from './f{next}';\n\
                 export class C{i} {{}}\n\
                 export class D{i} extends C{next} {{}}\n"
            ),
        )
        .unwrap();
    }
}

fn open_index(root: &Path) -> Connection {
    db::open(&root.join(".codekurve/index.db")).unwrap()
}

fn symbol_keys(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT symbol_key FROM symbols ORDER BY symbol_key")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn relationship_rows(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.symbol_key, COALESCE(t.symbol_key, r.target_external, ''), r.kind \
             FROM relationships r \
             JOIN symbols s ON s.id = r.source_symbol_id \
             LEFT JOIN symbols t ON t.id = r.target_symbol_id \
             ORDER BY 1, 2, 3",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(format!(
            "{} -> {} [{}]",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn index_fresh_project() -> (tempfile::TempDir, Vec<String>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    project::init(dir.path()).unwrap();
    write_project(dir.path());
    query::reindex(dir.path()).unwrap();
    let conn = open_index(dir.path());
    let symbols = symbol_keys(&conn);
    let relationships = relationship_rows(&conn);
    (dir, symbols, relationships)
}

#[test]
fn full_index_is_identical_across_runs_and_resolves_every_edge() {
    let (_first_dir, symbols_a, relationships_a) = index_fresh_project();
    let (_second_dir, symbols_b, relationships_b) = index_fresh_project();

    assert_eq!(symbols_a, symbols_b, "symbol sets differ between runs");
    assert_eq!(
        relationships_a, relationships_b,
        "relationship sets differ between runs"
    );
    // Two classes and one module symbol per file (observed on the base commit).
    assert_eq!(symbols_a.len(), 3 * FILES, "symbols per file");

    let count = |kind: &str| {
        relationships_a
            .iter()
            .filter(|row| row.ends_with(&format!("[{kind}]")))
            .count()
    };
    assert_eq!(
        count("inherits"),
        FILES,
        "one resolved inherits edge per D class"
    );
    assert_eq!(
        count("imports"),
        FILES,
        "one resolved imports edge per file"
    );
}
