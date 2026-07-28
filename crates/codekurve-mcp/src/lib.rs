//! Stdio-only MCP server surface exposing `codekurve::query` to MCP clients
//! (design "Server Bootstrap"). `#![deny]` below is the compiler-enforced
//! half of the stdout guarantee (design "stdout Discipline"): only
//! [`log`] may write to a stream, and it targets stderr, never stdout.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

mod server;
mod tools;

use std::path::Path;
use std::sync::Mutex;

use codekurve::query::Session;
use rmcp::ServiceExt;

pub(crate) use server::CodeKurve;

/// One `eprintln!` chokepoint (design "stdout Discipline" layer 1) — every
/// other line of this crate is forbidden from touching stdout/stderr
/// directly by the crate-level `#![deny]` above.
// ponytail: unused until a later PR needs ad-hoc logging beyond `tracing`;
// kept now so the chokepoint exists before any caller does.
#[allow(dead_code, clippy::print_stderr)]
pub(crate) fn log(msg: &str) {
    eprintln!("{msg}");
}

/// Entry point for `codekurve mcp` (design "Server Bootstrap"): resolves the
/// single project root once, then serves stdio JSON-RPC on a current-thread
/// tokio runtime until the client disconnects. `tokio` is confined to this
/// crate — `codekurve-core`/`-store`/`-analysis` stay sync.
pub fn run(root: &Path) -> Result<(), String> {
    let session = Session::open(root).map_err(|e| e.message)?;
    let allow_reindex = session.config().mcp.allow_reindex;

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(async {
            let handler = CodeKurve {
                session: Mutex::new(session),
                allow_reindex,
            };
            let service = handler
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| e.to_string())?;
            service.waiting().await.map_err(|e| e.to_string())?;
            Ok(())
        })
}
