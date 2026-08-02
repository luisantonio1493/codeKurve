//! `--help` is a successful request for information, not a usage error.
//! It regressed silently once: every spelling exited 2 and wrote to stderr,
//! so `install.sh`'s closing "Run: codekurve --help" landed the reader on a
//! red screen. These lock the contract.

use assert_cmd::Command;
use predicates::str::contains;

fn codekurve() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

#[test]
fn every_help_spelling_exits_zero_on_stdout() {
    for spelling in ["help", "-h", "--help"] {
        codekurve()
            .arg(spelling)
            .assert()
            .success()
            .stdout(contains("usage: codekurve"))
            .stderr(predicates::str::is_empty());
    }
}

#[test]
fn help_lists_every_command_group() {
    codekurve()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("indexing"))
        .stdout(contains("querying"))
        .stdout(contains("agents"))
        .stdout(contains("lifecycle"));
}

/// The other half of the contract: a genuinely wrong invocation still fails,
/// on stderr, so a script can tell the two apart.
#[test]
fn no_arguments_is_still_a_usage_error_on_stderr() {
    codekurve()
        .assert()
        .code(2)
        .stderr(contains("usage: codekurve"));
}

#[test]
fn unknown_command_is_still_a_usage_error() {
    codekurve().arg("definitely-not-a-command").assert().code(2);
}
