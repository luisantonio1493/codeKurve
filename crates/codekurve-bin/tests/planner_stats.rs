//! `codekurve index` leaves query-planner statistics behind, including on a
//! no-change run against an index built before statistics existed. Without
//! them relationship lookups and FTS search scan the whole project (see
//! `codekurve_store::db::refresh_planner_stats`).

use assert_cmd::Command;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

fn stat_rows(db_path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'sqlite_stat1')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !exists {
        return 0;
    }
    conn.query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn index_writes_planner_stats_and_no_change_run_restores_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/a.ts"),
        "export function a(): number { return b(); }\nexport function b(): number { return 1; }\n",
    )
    .unwrap();
    let db_path = root.join(".codekurve").join("index.db");

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();
    assert!(stat_rows(&db_path) > 0, "first index should analyze");

    // Simulate an index written by a release that never ran ANALYZE.
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch("DROP TABLE sqlite_stat1;")
        .unwrap();
    assert_eq!(stat_rows(&db_path), 0);

    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicates::str::contains("no changes detected"));
    assert!(
        stat_rows(&db_path) > 0,
        "no-change index should restore stats"
    );
}
