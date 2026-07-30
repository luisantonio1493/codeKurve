//! Phase 3 tasks 7.4/7.5 (design "Testing Strategy"): black-box, CLI-driven
//! integration tests that open the resulting SQLite database directly
//! (`codekurve-store` is a regular dependency of the `codekurve` binary, so
//! it's available here too) to compare stored rows rather than stdout.

use std::path::Path;

use assert_cmd::Command;
use codekurve_store::{db, repo, Connection};
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

/// `N` numbered files (`f0.ts`..`f{n-1}.ts`), each exporting a
/// uniquely-named function, plus `caller.ts` calling `f5`'s function —
/// enough tracked files that a 1-3 file mutation batch stays under the
/// default 25% `full_reindex_threshold_pct` and exercises the real
/// incremental path (task 5.4), not its full-reindex fallback.
fn write_numbered_fixture(root: &Path, n: usize) {
    std::fs::create_dir_all(root).unwrap();
    for i in 0..n {
        std::fs::write(
            root.join(format!("f{i}.ts")),
            format!("export function fn{i}() {{ return {i}; }}\n"),
        )
        .unwrap();
    }
    std::fs::write(root.join("caller.ts"), "function run() { return fn5(); }\n").unwrap();
}

/// Every comparable row of one table as `"col|col|..."` strings, `NULL`
/// normalized to `""`, excluding the run-specific `generation`/
/// `created_at`/`updated_at` columns so an incremental run and a fresh full
/// reindex of the identical final tree can be compared byte-for-byte
/// (design "Stable Row Ids" invariant: the golden test this pins down).
fn table_rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut rows: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.sort();
    rows
}

struct Snapshot {
    files: Vec<String>,
    symbols: Vec<String>,
    relationships: Vec<String>,
    unresolved: Vec<String>,
}

fn snapshot(db_path: &Path, project_id: &str) -> Snapshot {
    let conn = db::open(db_path).unwrap();
    Snapshot {
        files: table_rows(
            &conn,
            &format!(
                "SELECT id || '|' || relative_path || '|' || language || '|' || size_bytes \
                 || '|' || parse_status || '|' || COALESCE(parse_error,'') \
                 || '|' || COALESCE(content_hash,'') || '|' || COALESCE(modified_ns,'') \
                 FROM files WHERE project_id = '{project_id}'"
            ),
        ),
        symbols: table_rows(
            &conn,
            &format!(
                "SELECT id || '|' || file_id || '|' || symbol_key || '|' || name \
                 || '|' || qualified_name || '|' || kind || '|' || language \
                 || '|' || start_byte || '|' || end_byte || '|' || start_line \
                 || '|' || start_column || '|' || end_line || '|' || end_column \
                 || '|' || provenance || '|' || confidence || '|' || is_exported \
                 || '|' || visibility || '|' || is_partial || '|' || is_record \
                 FROM symbols WHERE project_id = '{project_id}'"
            ),
        ),
        relationships: table_rows(
            &conn,
            &format!(
                "SELECT id || '|' || source_symbol_id || '|' || COALESCE(target_symbol_id,'') \
                 || '|' || COALESCE(target_external,'') || '|' || kind || '|' || provenance \
                 || '|' || confidence || '|' || source_file_id || '|' || COALESCE(start_line,'') \
                 || '|' || COALESCE(start_column,'') || '|' || COALESCE(reason,'') \
                 FROM relationships WHERE project_id = '{project_id}'"
            ),
        ),
        unresolved: table_rows(
            &conn,
            &format!(
                "SELECT id || '|' || COALESCE(source_symbol_id,'') || '|' || source_file_id \
                 || '|' || relationship_kind || '|' || target_text \
                 || '|' || COALESCE(context_json,'') || '|' || candidate_count \
                 || '|' || reason || '|' || confidence \
                 FROM unresolved_references WHERE project_id = '{project_id}'"
            ),
        ),
    }
}

/// Task 7.4: index a project, mutate it (update + delete + create, all in
/// one batch under the oversized-batch threshold so the real incremental
/// path runs), capture the incrementally-updated database, then delete the
/// database and reindex the identical final tree from scratch (a full
/// reindex, since nothing is tracked yet). Both runs share the same root
/// path, hence the same deterministic `project_id`/`file_id`/`symbol_id`s
/// (design "BLAKE3 Substitution") — every stored row must match exactly.
#[test]
fn incremental_result_matches_full_reindex_of_identical_final_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_numbered_fixture(root, 12); // f0..f11 + caller.ts = 13 tracked files

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    // Mutate: update f0, delete f1, create new.ts. 3 changes / 13 tracked =
    // ~23% < the default 25% full_reindex_threshold_pct, so this batch
    // stays on the incremental path (task 5.4's fallback does NOT trigger).
    std::fs::write(
        root.join("f0.ts"),
        "export function fn0() { return 100; }\n",
    )
    .unwrap();
    std::fs::remove_file(root.join("f1.ts")).unwrap();
    std::fs::write(
        root.join("new.ts"),
        "export function fnNew() { return 99; }\n",
    )
    .unwrap();

    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicates::str::contains("full reindex").not());

    let db_path = root.join(".codekurve").join("index.db");
    let conn = db::open(&db_path).unwrap();
    let pid = repo::find_project(&conn, &root.canonicalize().unwrap().to_string_lossy())
        .unwrap()
        .unwrap();
    drop(conn);

    let incremental = snapshot(&db_path, &pid);

    std::fs::remove_file(&db_path).unwrap();
    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicates::str::contains("full reindex"));

    let full = snapshot(&db_path, &pid);

    assert_eq!(incremental.files, full.files, "files table diverged");
    assert_eq!(incremental.symbols, full.symbols, "symbols table diverged");
    assert_eq!(
        incremental.relationships, full.relationships,
        "relationships table diverged"
    );
    assert_eq!(
        incremental.unresolved, full.unresolved,
        "unresolved_references table diverged"
    );
}

/// Task 7.5: deleting a file whose function is called from another tracked
/// file converts that inbound `calls` relationship into an
/// `unresolved_references` row instead of silently dropping it (spec
/// "Inbound edges to the deleted file become unresolved"). Enough padding
/// files keep the single deletion under the oversized-batch threshold so
/// the real incremental delete cascade (`repo::delete_file`) runs, not the
/// full-reindex fallback.
#[test]
fn deleting_cross_file_callee_converts_inbound_edge_to_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_numbered_fixture(root, 12); // f0..f11 + caller.ts = 13 tracked; f5 is the callee

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    let db_path = root.join(".codekurve").join("index.db");
    let conn = db::open(&db_path).unwrap();
    let pid = repo::find_project(&conn, &root.canonicalize().unwrap().to_string_lossy())
        .unwrap()
        .unwrap();

    let calls_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM relationships WHERE project_id = ?1 AND kind = 'calls'",
            [&pid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        calls_before, 1,
        "caller.ts -> f5::fn5 must resolve before the delete"
    );
    drop(conn);

    // 1 deletion / 13 tracked ~= 7.7% < 25% threshold: stays incremental.
    std::fs::remove_file(root.join("f5.ts")).unwrap();
    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicates::str::contains("full reindex").not());

    let conn = db::open(&db_path).unwrap();
    let calls_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM relationships WHERE project_id = ?1 AND kind = 'calls'",
            [&pid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        calls_after, 0,
        "the resolved calls edge to the deleted file's symbol must be removed"
    );

    let unresolved: Vec<String> = conn
        .prepare(
            "SELECT target_text FROM unresolved_references \
             WHERE project_id = ?1 AND relationship_kind = 'calls'",
        )
        .unwrap()
        .query_map([&pid], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        unresolved,
        vec!["fn5".to_string()],
        "the dropped call must reappear as an unresolved_references row, never silently dropped"
    );
}
