//! CodeKurve CLI binary (composition root).

mod cli;

use std::path::PathBuf;
use std::process::ExitCode;

use cli::Args;
use codekurve::{commands, watch};
use commands::CommandError;

const USAGE: &str = "usage: codekurve <version|init|index|watch|mcp|status|search|symbol|doctor|\
references|callers|callees|implementations|trace|impact> [args] [--root <path>] [--debounce-ms <n>]";

fn main() -> ExitCode {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let args = Args::parse(raw);

    let result: Result<(), CommandError> = match command.as_str() {
        "version" => {
            println!("codekurve {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "init" => run_init(&args).map_err(CommandError::from),
        "index" => commands::index(&args.root).map_err(CommandError::from),
        "watch" => watch::run(&args.root, args.debounce_ms).map_err(CommandError::from),
        "mcp" => codekurve_mcp::run(&args.root).map_err(CommandError::from),
        "status" => commands::status(&args.root, args.json).map_err(CommandError::from),
        "search" => match args.positional(0) {
            Some(query) => commands::search(&args.root, query).map_err(CommandError::from),
            None => Err(usage_error(
                "usage: codekurve search <query> [--root <path>]",
            )),
        },
        "symbol" => match args.positional(0) {
            Some(name) => commands::symbol(&args.root, name).map_err(CommandError::from),
            None => Err(usage_error(
                "usage: codekurve symbol <name> [--root <path>]",
            )),
        },
        "doctor" => commands::doctor(&args.root).map_err(CommandError::from),
        "references" => commands::references(&query_args(&args)),
        "callers" => commands::callers(&query_args(&args)),
        "callees" => commands::callees(&query_args(&args)),
        "implementations" => commands::implementations(&query_args(&args)),
        "impact" => commands::impact(&query_args(&args)),
        "trace" => match args.positional(0) {
            Some(to) => commands::trace(&query_args(&args), to),
            None => Err(usage_error(
                "usage: codekurve trace <to> [--symbol-id <id>|--symbol-name <name>] [--root <path>]",
            )),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", e.message);
            ExitCode::from(e.code)
        }
    }
}

/// A CLI usage mistake (missing required positional) — same code-1 bucket as
/// every other pre-PR5b error, not a graph-query-specific failure.
fn usage_error(message: &str) -> CommandError {
    CommandError::from(message.to_string())
}

/// Translates the raw string flags in `cli::Args` into the six graph-query
/// commands' shared, typed [`commands::QueryArgs`].
fn query_args(args: &Args) -> commands::QueryArgs<'_> {
    commands::QueryArgs {
        root: &args.root,
        symbol_id: args.symbol_id.as_deref(),
        symbol_name: args.symbol_name.as_deref(),
        min_confidence: args.min_confidence.as_deref(),
        depth: args.depth,
        limit: args.limit,
        offset: args.offset,
        json: args.json,
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
