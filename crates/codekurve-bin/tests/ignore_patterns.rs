//! `[ignore] patterns` end-to-end: the default patterns keep vendored and
//! sensitive files out of the index even when the project has no
//! `.gitignore`, and a pattern added after the first index removes the files
//! it now matches on the next `index` run.

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

fn write(root: &std::path::Path, relative: &str, source: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}

/// Whether `search <name>` finds anything (its no-hit output echoes the
/// query, so a plain `contains(name)` can't tell hit from miss).
fn indexed(root: &std::path::Path, name: &str) -> bool {
    let output = ck()
        .arg("search")
        .arg(name)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    !String::from_utf8(output.stdout)
        .unwrap()
        .starts_with("no matches for")
}

#[test]
fn default_patterns_exclude_vendored_and_sensitive_files_without_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/app.ts", "export class AppService {}\n");
    write(root, "src/secrets.ts", "export class SecretHolder {}\n");
    write(
        root,
        "node_modules/lib/index.ts",
        "export class VendoredLib {}\n",
    );
    write(root, "dist/app.ts", "export class BuiltArtifact {}\n");

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    assert!(indexed(root, "AppService"));
    for excluded in ["SecretHolder", "VendoredLib", "BuiltArtifact"] {
        assert!(!indexed(root, excluded), "{excluded} should not be indexed");
    }
}

#[test]
fn pattern_added_after_index_removes_matching_files_on_reindex() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/app.ts", "export class AppService {}\n");
    write(root, "legacy/old.ts", "export class LegacyThing {}\n");

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();
    assert!(indexed(root, "LegacyThing"));

    let config_path = root.join(".codekurve").join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("patterns = [\n"));
    let config = config.replacen("patterns = [\n", "patterns = [\n    \"legacy/**\",\n", 1);
    std::fs::write(&config_path, config).unwrap();

    ck().arg("index").arg("--root").arg(root).assert().success();

    assert!(!indexed(root, "LegacyThing"));
    assert!(indexed(root, "AppService"));
}

#[test]
fn invalid_pattern_fails_index_with_a_clear_message() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/app.ts", "export class AppService {}\n");

    ck().arg("init").arg(root).assert().success();
    let config_path = root.join(".codekurve").join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    let config = config.replacen("patterns = [\n", "patterns = [\n    \"!src/**\",\n", 1);
    std::fs::write(&config_path, config).unwrap();

    ck().arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid ignore.patterns entry"));
}
