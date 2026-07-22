//! CodeKurve CLI binary (composition root).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("version") => {
            println!("codekurve {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("init") => run_init(args.next()),
        _ => {
            eprintln!("usage: codekurve <version|init> [path]");
            ExitCode::from(2)
        }
    }
}

fn run_init(path: Option<String>) -> ExitCode {
    let root = path.map_or_else(|| PathBuf::from("."), PathBuf::from);
    match codekurve_core::project::init(&root) {
        Ok(file) => {
            println!("initialized codekurve project: {}", file.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
