use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn version_prints_semver_and_exits_zero() {
    Command::cargo_bin("codekurve")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout(contains("codekurve ").and(contains(env!("CARGO_PKG_VERSION"))));
}
