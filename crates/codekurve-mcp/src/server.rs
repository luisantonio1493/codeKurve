//! [`CodeKurve`] — the `rmcp` [`ServerHandler`]. The tool bodies live in
//! [`crate::tools`]; this module only wires `get_info` and the
//! `#[tool_handler]` glue rmcp's macros need (design "Server Bootstrap").

use std::any::Any;
use std::sync::{Arc, Mutex, MutexGuard};

use codekurve::query::Session;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool_handler, ErrorData as McpError, RoleServer, ServerHandler};

/// The one tool `codekurve_reindex` gates on (spec "reindex Gated Off by
/// Default") — `#[tool_router]` in `tools.rs` registers it unconditionally
/// (a compile-time list); the runtime gate lives entirely in this module's
/// `list_tools`/`call_tool` overrides below.
const REINDEX_TOOL_NAME: &str = "codekurve_reindex";

/// One project root's session plus the reindex gate (design "Concurrency"):
/// `std::sync::Mutex`, never held across an `.await`. Tool bodies never lock
/// it directly; they go through [`CodeKurve::blocking`].
pub struct CodeKurve {
    pub(crate) session: Arc<Mutex<Session>>,
    pub(crate) allow_reindex: bool,
}

impl CodeKurve {
    pub(crate) fn new(session: Session, allow_reindex: bool) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            allow_reindex,
        }
    }

    /// Runs one tool body against the locked session on tokio's blocking
    /// pool. Every tool body is sync SQLite/BFS work (seconds, for
    /// `codekurve_reindex`), and `lib.rs` serves on a *current-thread*
    /// runtime: run inline, a slow body would stall the only thread, so the
    /// server could neither read the next message nor honour a cancellation
    /// until it finished.
    ///
    /// A panicking body becomes an `internal_error` response. Left to rmcp,
    /// the spawned request task would die silently and the client would
    /// wait forever for a response that is never sent.
    pub(crate) async fn blocking<F>(&self, body: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&mut Session) -> Result<CallToolResult, McpError> + Send + 'static,
    {
        let session = Arc::clone(&self.session);
        let joined = tokio::task::spawn_blocking(move || {
            let mut guard = lock_session(&session)?;
            body(&mut guard)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(e) if e.is_panic() => Err(McpError::internal_error(
                format!("tool panicked: {}", panic_message(e.into_panic())),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

/// Locks the session, recovering from a poisoned lock. A body that panicked
/// while holding the guard may have left the session half-updated (e.g.
/// mid-`reindex`), so recovery reopens it from disk rather than trusting it
/// (`into_inner` alone would). Until a reopen succeeds the lock stays
/// poisoned and each call retries; with a plain `lock().unwrap()`, one
/// panic made every later tool call panic too.
fn lock_session(session: &Mutex<Session>) -> Result<MutexGuard<'_, Session>, McpError> {
    match session.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            let root = guard.root().to_path_buf();
            *guard = Session::open(&root).map_err(|e| {
                McpError::internal_error(
                    format!(
                        "session unusable after an earlier tool panic: {}",
                        e.message
                    ),
                    None,
                )
            })?;
            session.clear_poison();
            Ok(guard)
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[tool_handler]
impl ServerHandler for CodeKurve {
    /// Overrides `#[tool_handler]`'s generated `list_tools` (task 6.4): same
    /// full list `tools::CodeKurve::tool_router()` builds, minus
    /// `codekurve_reindex` when `allow_reindex` is off.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = Self::tool_router().list_all();
        if !self.allow_reindex {
            tools.retain(|tool| tool.name != REINDEX_TOOL_NAME);
        }
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    /// Overrides `#[tool_handler]`'s generated `call_tool` (task 6.4): a
    /// disabled `codekurve_reindex` fails exactly like calling any other
    /// unregistered tool name (`METHOD_NOT_FOUND`), not a distinct
    /// "forbidden" shape — spec requires it fail "as an unknown tool".
    /// Every other tool name delegates unchanged to the generated dispatch.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if request.name == REINDEX_TOOL_NAME && !self.allow_reindex {
            return Err(McpError::method_not_found::<CallToolRequestMethod>());
        }
        let tcc = ToolCallContext::new(self, request, context);
        Self::tool_router().call(tcc).await
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "CodeKurve exposes the project's code graph over MCP. Query before broad \
                 exploration; see docs/AGENT_USAGE.md for the full agent usage rules.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn server() -> (tempfile::TempDir, CodeKurve) {
        let tmp = tempfile::tempdir().unwrap();
        codekurve_core::project::init(tmp.path()).unwrap();
        let session = Session::open(tmp.path()).unwrap();
        (tmp, CodeKurve::new(session, false))
    }

    fn runtime() -> tokio::runtime::Runtime {
        // Same flavour `lib.rs::run` serves on.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn ok(_: &mut Session) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![]))
    }

    /// A panicking body answers with an error instead of never answering,
    /// and the next call gets a freshly reopened session instead of
    /// panicking on the poisoned lock.
    #[test]
    fn panicking_body_returns_error_and_next_call_recovers() {
        let (_tmp, server) = server();
        let rt = runtime();

        let err = rt
            .block_on(server.blocking(|_| -> Result<CallToolResult, McpError> { panic!("boom") }))
            .unwrap_err();
        assert!(
            err.message.contains("tool panicked: boom"),
            "{}",
            err.message
        );
        assert!(server.session.is_poisoned());

        rt.block_on(server.blocking(ok)).unwrap();
        assert!(!server.session.is_poisoned());
    }

    /// While a body runs, the runtime's only thread stays free for other
    /// tasks (in production: rmcp's read loop, pings, cancellations). The
    /// body waits for a signal that only another task on the runtime can
    /// send; run inline, it would hold the thread and time out.
    #[test]
    fn running_body_does_not_stall_the_runtime() {
        let (_tmp, server) = server();
        let server = Arc::new(server);
        let rt = runtime();

        rt.block_on(async {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let slow = {
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    server
                        .blocking(move |_| {
                            started_tx.send(()).unwrap();
                            release_rx
                                .recv_timeout(Duration::from_secs(10))
                                .expect("runtime thread was blocked by the tool body");
                            Ok(CallToolResult::success(vec![]))
                        })
                        .await
                })
            };
            started_rx.await.unwrap();
            release_tx.send(()).unwrap();
            slow.await.unwrap().unwrap();
        });
    }
}
