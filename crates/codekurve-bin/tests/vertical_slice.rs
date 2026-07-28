use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

const SOURCE: &str = "export class MemberService {\n  find(id: string): string { return id; }\n}\n\nexport function createMemberService(): MemberService {\n  return new MemberService();\n}\n";

#[test]
fn vertical_slice_init_index_search_symbol() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("member.ts"), SOURCE).unwrap();

    ck().arg("init").arg(root).assert().success();

    // First run: no prior index_state/content_hash, so every file reads as
    // `Created` and the oversized-batch fallback (task 5.4) naturally takes
    // the full-reindex path (task 5.5's bootstrap case).
    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "indexed 1 file(s) changed, 0 deleted (full reindex)",
        ));

    ck().arg("search")
        .arg("MemberService")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("MemberService").and(predicate::str::contains("class")));

    ck().arg("symbol")
        .arg("MemberService")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("member.ts")
                .and(predicate::str::contains("class MemberService"))
                .and(predicate::str::contains("(live)")),
        );
}

#[test]
fn search_without_index_fails_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    ck().arg("init").arg(root).assert().success();

    ck().arg("search")
        .arg("Foo")
        .arg("--root")
        .arg(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("run `codekurve index`"));
}

#[test]
fn doctor_reports_fts5() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    ck().arg("init").arg(root).assert().success();

    ck().arg("doctor")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("[ok] fts5")
                .and(predicate::str::contains("[ok] schema: version 3")),
        );
}
