//! End-to-end coverage for `codekurve export`: a real index, a real file on
//! disk, and the two rules the artifact has to keep — self-contained (no
//! external asset at all) and never silently clobbering an existing file.

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

const A_TS: &str = "export function target(): boolean {\n  return helper();\n}\n\nexport function helper(): boolean {\n  return true;\n}\n\nexport function caller(): boolean {\n  return target();\n}\n";

fn seed_project(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.ts"), A_TS).unwrap();
    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();
}

#[test]
fn export_writes_a_self_contained_file_and_refuses_to_clobber_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    let out = root.join("graph.html");

    ck().arg("export")
        .arg(&out)
        .arg("--symbol-name")
        .arg("src/a.ts::target")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("node(s)"));

    let html = std::fs::read_to_string(&out).unwrap();
    assert!(!html.is_empty());
    // The rule this test exists for: opening the file with the network cable
    // unplugged must render identically.
    assert!(!html.contains("http://"), "external reference in export");
    assert!(!html.contains("https://"), "external reference in export");
    // Bidirectional: `helper` is only reachable forward, `caller` only
    // backward. Both have to be in the picture.
    assert!(html.contains(">helper</text>"));
    assert!(html.contains(">caller</text>"));
    assert!(html.contains("Provenance and confidence"));

    // Second run over the same path refuses...
    ck().arg("export")
        .arg(&out)
        .arg("--symbol-name")
        .arg("src/a.ts::target")
        .arg("--root")
        .arg(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));

    // ...until `--yes`.
    ck().arg("export")
        .arg(&out)
        .arg("--symbol-name")
        .arg("src/a.ts::target")
        .arg("--root")
        .arg(root)
        .arg("--yes")
        .assert()
        .success();
}
