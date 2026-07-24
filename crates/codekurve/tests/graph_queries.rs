//! End-to-end coverage for the six graph-query CLI commands (PR5b, §26/§27).
//! Fixture layout gives every scenario a controlled, deliberately ambiguous
//! symbol name:
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

/// Spec "Query before first index": no prior `codekurve index` run -> exit
/// 4, no query attempted.
#[test]
fn query_before_index_exits_4() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    ck().arg("init").arg(root).assert().success();

    ck().arg("callers")
        .arg("--symbol-name")
        .arg("getEligibility")
        .arg("--root")
        .arg(root)
        .assert()
        .code(4);
}

/// Spec "JSON envelope shape": `--json` output is one JSON object with all
/// five §27.5 fields.
#[test]
fn json_output_has_all_envelope_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    let output = ck()
        .arg("callers")
        .arg("--symbol-name")
        .arg("src/a.ts::getEligibility")
        .arg("--json")
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let obj = envelope
        .as_object()
        .expect("envelope must be a JSON object");
    for field in [
        "schema_version",
        "project",
        "result",
        "warnings",
        "truncated",
    ] {
        assert!(obj.contains_key(field), "missing envelope field {field:?}");
    }
}

/// `trace`/`impact` share the same BFS-backed envelope and command
/// preamble; a smoke test that both run end to end without requiring a
/// specific reachable path (fixture has no multi-hop chain).
#[test]
fn trace_and_impact_run_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    ck().arg("trace")
        .arg("src/a.ts::getEligibility")
        .arg("--symbol-name")
        .arg("src/a.ts::callLocal")
        .arg("--root")
        .arg(root)
        .assert()
        .success();

    ck().arg("impact")
        .arg("--symbol-name")
        .arg("src/a.ts::getEligibility")
        .arg("--root")
        .arg(root)
        .assert()
        .success();
}
