//! `codekurve index` survives a file nested past the syntax-depth limit:
//! before the guard, a ~20 KB file of nested `[` aborted the process with a
//! stack overflow. The file is skipped with a warning, the rest of the
//! project is indexed, and the skipped file is not retried until it changes.

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

#[test]
fn deeply_nested_file_is_skipped_with_a_warning_and_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let depth = 100_000;
    std::fs::write(
        root.join("src/deep.ts"),
        format!(
            "export const x = {}{};\nexport function hiddenByDepth() {{}}\n",
            "[".repeat(depth),
            "]".repeat(depth)
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("src/ok.ts"),
        "export function stillIndexed() {}\n",
    )
    .unwrap();

    ck().arg("init").arg(root).assert().success();
    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: src/deep.ts not indexed: skipped: syntax nesting deeper than",
        ));

    ck().arg("search")
        .arg("stillIndexed")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("src/ok.ts"));
    ck().arg("search")
        .arg("hiddenByDepth")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::starts_with("no matches for"));

    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("no changes detected"))
        .stderr(predicate::str::is_empty());
}
