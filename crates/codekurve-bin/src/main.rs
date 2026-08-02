//! CodeKurve CLI binary (composition root).

mod cli;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use cli::Args;
use codekurve::{commands, install, update, watch};
use commands::CommandError;

const USAGE: &str =
    "usage: codekurve <version|init|index|watch|mcp|tui|status|search|symbol|doctor|\
references|callers|callees|implementations|unresolved|trace|impact|install|uninstall|update> \
[args] [--root <path>] [--debounce-ms <n>] [--client <name>] [--yes] [--binary]\n\
\x20      codekurve unresolved [<target-text>]  references the analyzer could not resolve, and why\n\
\x20      codekurve tui                   interactive code-graph explorer\n\
\x20      codekurve install [<client>]    configure every detected agent, or one by name\n\
\x20      codekurve uninstall [<client>]  remove codekurve from agent configs\n\
\x20      codekurve uninstall --binary    ...and delete the codekurve executable too\n\
\x20      codekurve update [--yes]        re-run the published install script to upgrade\n\
\x20      codekurve version | -v | --version";

fn main() -> ExitCode {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let args = Args::parse(raw);

    let result: Result<(), CommandError> = match command.as_str() {
        "version" | "-v" | "--version" => {
            println!("codekurve {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "init" => run_init(&args).map_err(CommandError::from),
        "index" => commands::index(&args.root).map_err(CommandError::from),
        "watch" => watch::run(&args.root, args.debounce_ms).map_err(CommandError::from),
        "mcp" => codekurve_mcp::run(&args.root).map_err(CommandError::from),
        "tui" => codekurve_tui::run_explorer(&args.root),
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
        "install" => {
            let client = args.positional(0).or(args.client.as_deref());
            // The checkbox picker is strictly an upgrade of the `[y/N]`
            // prompt, so it appears under exactly the conditions that prompt
            // appeared under: no client named, `--yes` absent, and a real
            // terminal on stdin. Scripted, piped and agent-driven installs
            // keep taking the identical non-interactive path.
            if client.is_none() && !args.yes && std::io::stdin().is_terminal() {
                codekurve_tui::run_picker(&args.root).map_err(CommandError::from)
            } else {
                install::run(&args.root, client, args.yes).map_err(CommandError::from)
            }
        }
        "uninstall" => {
            let client = args.positional(0).or(args.client.as_deref());
            install::uninstall(&args.root, client, args.yes, args.binary)
                .map_err(CommandError::from)
        }
        // The only two dispatch arms that can reach a subprocess, and both
        // require the user to type the command (ADR 0012). Nothing else —
        // index, watch, mcp, tui, any query — routes here.
        "update" => update::run(args.yes).map_err(CommandError::from),
        "references" => commands::references(&query_args(&args)),
        "callers" => commands::callers(&query_args(&args)),
        "callees" => commands::callees(&query_args(&args)),
        "implementations" => commands::implementations(&query_args(&args)),
        // Optional positional: `codekurve unresolved` alone lists the whole
        // project, `codekurve unresolved <target-text>` filters to one target.
        "unresolved" => commands::unresolved(&query_args(&args), args.positional(0)),
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
