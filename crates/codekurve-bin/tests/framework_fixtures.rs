//! PR7 (tasks.md 7.8-7.10, 7.14): full CLI round-trip (`init` -> `index` ->
//! `search` -> `symbol` -> `trace`/`impact`) over the real `fixtures/angular/`
//! and `fixtures/dotnet/` trees, copied into a scratch project root. Proves
//! recognized roles surface through `symbol` and that framework edges are
//! traversable end to end through the stored graph, not just present in the
//! in-memory `FileAnalysis` (which `angular_graph.rs`/`dotnet_graph.rs`
//! already cover).

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn repo_fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Task 7.10: `index` -> trace a route -> component -> injected-service path
/// on the Angular fixture; `symbol` surfaces every recognized role.
#[test]
fn angular_fixture_end_to_end_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    copy_tree(&repo_fixture("fixtures/angular/src"), &root.join("src"));

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    ck().arg("search")
        .arg("InvoiceComponent")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("InvoiceComponent"));

    ck().arg("symbol")
        .arg("InvoiceComponent")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("roles=").and(predicate::str::contains("component")));

    ck().arg("symbol")
        .arg("InvoiceApiRepository")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("roles=")
                .and(predicate::str::contains("service"))
                .and(predicate::str::contains("repository")),
        );

    // Route (the `routes` array variable) -> InvoiceComponent (HandlesRoute).
    ck().arg("trace")
        .arg("InvoiceComponent")
        .arg("--symbol-name")
        .arg("routes")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("handlesroute"));

    // InvoiceComponent -> InvoiceApiRepository (Injects).
    ck().arg("trace")
        .arg("InvoiceApiRepository")
        .arg("--symbol-name")
        .arg("InvoiceComponent")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("injects"));

    ck().arg("impact")
        .arg("--symbol-name")
        .arg("routes")
        .arg("--root")
        .arg(root)
        .assert()
        .success();
}

/// Task 7.8: the .NET controller variant — route -> DI registration ->
/// data layer, traceable through `trace`/`symbol`.
#[test]
fn dotnet_controller_fixture_end_to_end_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    copy_tree(
        &repo_fixture("fixtures/dotnet/controller"),
        &root.join("src"),
    );

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    ck().arg("symbol")
        .arg("InvoiceController")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("roles=").and(predicate::str::contains("controller")));

    ck().arg("symbol")
        .arg("AppDbContext")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("AppDbContext"));

    // `ConfigureServices` registers `IInvoiceRepository` -> `InvoiceRepository`
    // (RegisteredAs).
    ck().arg("trace")
        .arg("src/InvoiceRepository.cs::Acme.Invoicing.Data.InvoiceRepository")
        .arg("--symbol-name")
        .arg("ConfigureServices")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("registeredas"));

    // `AppDbContext` -> `Invoice` (PersistsTo, the data-layer end of the
    // chain).
    ck().arg("trace")
        .arg("Invoice")
        .arg("--symbol-name")
        .arg("AppDbContext")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("persiststo"));

    ck().arg("impact")
        .arg("--symbol-name")
        .arg("GetById")
        .arg("--root")
        .arg(root)
        .assert()
        .success();
}

/// Task 7.9: the .NET minimal-API variant traces the same shape as the
/// controller variant, via `Program`'s `MapGet` handler edge instead of an
/// `[HttpGet]` attribute.
#[test]
fn dotnet_minimal_api_fixture_end_to_end_cli_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    copy_tree(
        &repo_fixture("fixtures/dotnet/minimal-api"),
        &root.join("src"),
    );

    ck().arg("init").arg(root).assert().success();
    ck().arg("index").arg("--root").arg(root).assert().success();

    // `Program`'s synthetic entry point -> `GetInvoice` handler
    // (HandlesRoute).
    ck().arg("trace")
        .arg("GetInvoice")
        .arg("--symbol-name")
        .arg("Main")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("handlesroute"));

    ck().arg("trace")
        .arg("Invoice")
        .arg("--symbol-name")
        .arg("AppDbContext")
        .arg("--root")
        .arg(root)
        .assert()
        .success()
        .stdout(predicate::str::contains("persiststo"));
}
