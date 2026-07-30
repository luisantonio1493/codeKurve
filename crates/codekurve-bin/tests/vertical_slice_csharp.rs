use assert_cmd::Command;
use predicates::prelude::*;

fn ck() -> Command {
    Command::cargo_bin("codekurve").unwrap()
}

#[test]
fn vertical_slice_csharp_init_index_search_symbol_callers_and_implementations() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("contracts.cs"),
        "namespace Acme.Contracts; public interface IGreeter { string Greet(string name); }",
    )
    .unwrap();
    std::fs::write(source.join("greeter.cs"), "using Acme.Contracts; namespace Acme.App; public class Greeter : IGreeter { public string Greet(string name) => name; }").unwrap();
    std::fs::write(source.join("program.cs"), "using Acme.Contracts; namespace Acme.App; public class Program { public string Run() { IGreeter greeter = new Greeter(); return greeter.Greet(\"world\"); } }").unwrap();
    ck().arg("init").arg(root.path()).assert().success();
    ck().arg("index")
        .arg("--root")
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("indexed 3 file(s)"));
    ck().arg("search")
        .arg("Greeter")
        .arg("--root")
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("class"));
    ck().arg("symbol")
        .arg("Greeter")
        .arg("--root")
        .arg(root.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("greeter.cs").and(predicate::str::contains("class Greeter")),
        );
    ck().arg("callers")
        .arg("--symbol-name")
        .arg("src/greeter.cs::Acme.App.Greeter.Greet")
        .arg("--root")
        .arg(root.path())
        .assert()
        .success();
    ck().arg("implementations")
        .arg("--symbol-name")
        .arg("src/contracts.cs::Acme.Contracts.IGreeter")
        .arg("--root")
        .arg(root.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Greeter"));
}
