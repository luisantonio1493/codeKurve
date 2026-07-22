//! CodeKurve CLI binary (composition root).

mod cli;
mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use cli::Args;

const USAGE: &str =
    "usage: codekurve <version|init|index|search|symbol|doctor> [args] [--root <path>]";

fn main() -> ExitCode {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let args = Args::parse(raw);

    let result = match command.as_str() {
        "version" => {
            println!("codekurve {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "init" => run_init(&args),
        "index" => commands::index(&args.root),
        "search" => match args.positional(0) {
            Some(query) => commands::search(&args.root, query),
            None => Err("usage: codekurve search <query> [--root <path>]".to_string()),
        },
        "symbol" => match args.positional(0) {
            Some(name) => commands::symbol(&args.root, name),
            None => Err("usage: codekurve symbol <name> [--root <path>]".to_string()),
        },
        "doctor" => commands::doctor(&args.root),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_init(args: &Args) -> Result<(), String> {
    // `init` takes an optional positional path; fall back to `--root`.
    let root = args
        .positional(0)
        .map_or_else(|| args.root.clone(), PathBuf::from);
    let file = codekurve_core::project::init(&root).map_err(|e| e.to_string())?;
    println!("initialized codekurve project: {}", file.display());
    Ok(())
}
