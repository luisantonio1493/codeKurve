//! A project containing constructs whose symbols share a storage key (see
//! `codekurve-analysis/tests/duplicate_keys.rs`) must index. Before, one such
//! duplicate aborted `codekurve index` with
//! `UNIQUE constraint failed: symbols.project_id, symbols.symbol_key`.

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

#[test]
fn project_with_duplicate_symbol_keys_indexes_and_reindexes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/ns.ts"),
        "export declare namespace A { interface Props { a: 1 } }\n\
         export declare namespace B { interface Props { b: 2 } }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/Req.cs"),
        "class Req : IMessage {\n\
           public static MessageDescriptor Descriptor { get { return null; } }\n\
           MessageDescriptor IMessage.Descriptor { get { return Descriptor; } }\n\
         }\n",
    )
    .unwrap();

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();
    ck().arg("search")
        .arg("Props")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("src/ns.ts"));

    // An edit takes the incremental path, which deletes and re-inserts the
    // file's symbols under the same keys.
    std::fs::write(
        root.join("src/ns.ts"),
        "export declare namespace A { interface Props { a: 1 } }\n\
         export declare namespace B { interface Props { b: 2 } }\n\
         export declare namespace C { interface Props { c: 3 } }\n",
    )
    .unwrap();
    ck().arg("index").arg("--root").arg(root).assert().success();
}
