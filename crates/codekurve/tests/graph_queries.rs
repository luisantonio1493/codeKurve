//! End-to-end coverage for the graph-query CLI commands (PR5b). PR5b-1 wires
//! `references`/`callers`/`callees`/`implementations` and covers their two
//! symbol-resolution scenarios here; PR5b-2 extends this file with
//! `trace`/`impact`, the missing-index exit code, and the JSON envelope
//! shape (shared preamble/printer code, exercised once the full command set
//! lands). Fixture layout gives every scenario a controlled, deliberately
//! ambiguous symbol name:
//! - `src/a.ts::getEligibility` — same-file caller `callLocal` (Exact,
//!   extracted) + a cross-file ambiguous caller (Low, heuristic) via
//!   `src/c.ts::callAmbiguous`, which calls the bare (unimported)
//!   `getEligibility()` — 2 project-wide candidates (`a.ts`/`b.ts`), so
//!   `resolve()` emits one Low edge per candidate (§20.4).
//! - `src/b.ts::getEligibility` — the second same-named candidate, making a
//!   bare-name `--symbol-name getEligibility` lookup ambiguous (spec
//!   "Ambiguous name lookup").

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

const A_TS: &str = "export function getEligibility(): boolean {\n  return true;\n}\n\nexport function callLocal(): boolean {\n  return getEligibility();\n}\n";
const B_TS: &str = "export function getEligibility(): boolean {\n  return false;\n}\n";
const C_TS: &str = "export function callAmbiguous(): boolean {\n  return getEligibility();\n}\n";

fn seed_project(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.ts"), A_TS).unwrap();
    std::fs::write(root.join("src").join("b.ts"), B_TS).unwrap();
    std::fs::write(root.join("src").join("c.ts"), C_TS).unwrap();
    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();
}

/// Spec "Callers of a symbol" + "Min-confidence filter": qualified name
/// disambiguates to exactly `src/a.ts::getEligibility` (spec "Qualified name
/// disambiguates", exit 0), whose callers include one Exact and one Low edge
/// with visible confidence/provenance; `--min-confidence high` drops the Low
/// one.
#[test]
fn callers_qualified_name_returns_confidence_and_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    ck().arg("callers")
        .arg("--symbol-name")
        .arg("src/a.ts::getEligibility")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("exact")
                .and(predicate::str::contains("low"))
                .and(predicate::str::contains("extracted"))
                .and(predicate::str::contains("heuristic")),
        );

    ck().arg("callers")
        .arg("--symbol-name")
        .arg("src/a.ts::getEligibility")
        .arg("--min-confidence")
        .arg("high")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("exact").and(predicate::str::contains("low").not()));
}

/// Spec "Ambiguous name lookup": bare name `getEligibility` matches both
/// `a.ts` and `b.ts` -> exit 6, both candidates listed.
#[test]
fn bare_name_ambiguity_exits_6_with_both_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    ck().arg("callers")
        .arg("--symbol-name")
        .arg("getEligibility")
        .arg("--root")
        .arg(root)
        .assert()
        .code(6)
        .stderr(
            predicate::str::contains("src/a.ts::getEligibility")
                .and(predicate::str::contains("src/b.ts::getEligibility")),
        );
}
