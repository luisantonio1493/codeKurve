use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn init_creates_config_in_target_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    Command::cargo_bin("codekurve")
        .unwrap()
        .arg("init")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized codekurve project"));

    let config = root.join(".codekurve").join("config.toml");
    assert!(config.exists(), "config.toml should be created");

    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("version = 1"));
    assert!(text.contains("[project]"));
}

#[test]
fn init_twice_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    Command::cargo_bin("codekurve")
        .unwrap()
        .arg("init")
        .arg(root)
        .assert()
        .success();

    Command::cargo_bin("codekurve")
        .unwrap()
        .arg("init")
        .arg(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized"));
}
