//! Task 7.1/7.2 end-to-end coverage: `codekurve status` and the stale-warning
//! stderr line on a query command.

use assert_cmd::Command;
use codekurve_store::{db, repo};
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

const SOURCE: &str = "export function greet(): string { return 'hi'; }\n";

/// Spec "Status after clean index": pending count 0, no stale warning.
#[test]
fn status_after_clean_index_reports_zero_pending_and_no_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/greet.ts"), SOURCE).unwrap();

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    ck().arg("status")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("pending files: 0")
                .and(predicate::str::contains("status: fresh")),
        );

    ck().arg("search")
        .arg("greet")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

/// Spec "Status with pending changes" + "Stale Index Warning on Stderr":
/// `status` reads `pending_files` from stored metadata without a filesystem
/// walk, and a query command (`symbol`) prints exactly one stderr warning
/// when it's nonzero, without touching stdout or the exit code. Writing
/// `index_state.pending_files` directly (rather than interrupting a real
/// batch mid-transaction, already covered by
/// `apply_incremental_rolls_back_and_leaves_pending_files_set` in
/// `codekurve-store`) isolates this test to the CLI-level read/warn path.
#[test]
fn status_and_query_warn_when_pending_changes_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/greet.ts"), SOURCE).unwrap();

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    let db_path = root.join(".codekurve").join("index.db");
    let conn = db::open(&db_path).unwrap();
    let pid = repo::find_project(&conn, &root.canonicalize().unwrap().to_string_lossy())
        .unwrap()
        .unwrap();
    conn.execute(
        "UPDATE index_state SET pending_files = 3 WHERE project_id = ?1",
        [&pid],
    )
    .unwrap();
    drop(conn);

    ck().arg("status")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("pending files: 3")
                .and(predicate::str::contains("status: stale")),
        );

    ck().arg("symbol")
        .arg("greet")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("greet"))
        .stderr(predicate::str::contains(
            "warning: index is stale (3 pending file(s)); run `codekurve index`",
        ));
}
