# Design: Phase 4 — MCP Server

## Technical Approach

Two moves. First, the `codekurve` package grows a **library target** (`src/lib.rs`) that owns the existing modules; `main.rs` shrinks to `mod cli` + dispatch over `codekurve::commands::*`. Nothing moves directories, no new crate for query logic — the composition root simply becomes importable. Second, every command body splits into a **data function** (`query.rs`, returns structs, never prints) and a **printing wrapper** (`commands.rs`, byte-identical stdout). `codekurve-mcp` is a thin adapter: `rmcp` tool → `query::*` → JSON envelope. No new graph semantics, no second copy of `paginate`/`resolve_symbol`/`bfs_caps`/caps.

## Architecture Decisions

| Decision | Choice | Rejected | Rationale |
|---|---|---|---|
| Query layer location | `codekurve/src/lib.rs` + new `src/query.rs`; modules declared in the lib, `main.rs` consumes the lib | New `codekurve-query` crate; duplicate logic in `-mcp` | `crate::incremental::…` paths inside `commands.rs` keep resolving unchanged; the diff is a module-declaration move, not a code move |
| Print/return split | `query::foo(...) -> Result<FooData, CommandError>`; `commands::foo(...)` = call + print | `foo(json: bool, out: &mut dyn Write)` | A writer parameter keeps formatting in the shared path; MCP wants data, not text |
| Tool names | `codekurve_*` (§28.2 spelling) | Bare `project_status` (proposal shorthand) | Master plan is the contract clients see; reconcile the proposal's shorthand, not the plan |
| Envelope | Reuse §27.5 `envelope()`; new `total` key emitted **only** when `Some` | New MCP-specific envelope; always emit `total` | CLI passes `None` → existing JSON goldens are byte-identical; MCP gets §28.3's total count |
| Stale warning | `query::warnings(&Session) -> Vec<String>`, threaded into every envelope | Per-tool ad-hoc check | One source; CLI's `warn_if_stale` prints the same vec to stderr, MCP puts it in `warnings` |
| Missing index | Server starts; `Session::NotIndexed` → status/doctor answer degraded, query tools return a tool error carrying the warning | Refuse to start; auto-index | Confirmed decision 2 — never auto-index, never hard-fail the connection |
| Unsupported search filters | `kinds`/`languages`/`path_prefix` in the schema, rejected with `invalid params: filter not supported yet (supported: query, limit)` | Silently ignore; implement filters now | Confirmed decision 3; filters are out of scope, silence would lie to the agent |
| Concurrency | `std::sync::Mutex<Connection>`, current-thread tokio runtime, **no `await` while holding the lock** | `tokio::sync::Mutex`; connection pool | `rusqlite::Connection: Send + !Sync`; handlers are sync bodies, so a std mutex is enough and can't deadlock across await |
| `reindex` | Not registered at all unless `[mcp] allow_reindex = true` | Registered + runtime rejection | Success criterion: "absent from the tool list" |

## Interfaces

```rust
// codekurve/src/query.rs — the shared layer. No println! reaches this file.
pub enum Session {                       // resolved once, at server startup
    Indexed { root: PathBuf, config: Config, conn: Connection, project_id: String },
    NotIndexed { root: PathBuf, config: Config, reason: String },
}
impl Session {
    pub fn open(root: &Path) -> Result<Self, CommandError>; // config missing => Err (fatal)
    pub fn warnings(&self) -> Vec<String>;  // stale/pending/not-indexed, one wording everywhere
}

pub struct Page<T> { pub rows: Vec<T>, pub total: usize, pub truncated: bool }

pub fn status(s: &Session)   -> Result<StatusData, CommandError>;
pub fn overview(s: &Session) -> Result<OverviewData, CommandError>;
pub fn search(s: &Session, q: &SearchInput) -> Result<Page<SymbolHit>, CommandError>;
pub fn get_symbol(s: &Session, id: &str, ctx_lines: u32) -> Result<SymbolDetail, CommandError>;
pub fn relationships(s: &Session, kind: RelKind, a: &QueryArgs) -> Result<Page<StoredRelationship>, CommandError>;
pub fn trace(s: &Session, a: &QueryArgs, to: &str) -> Result<traverse::BfsOutcome, CommandError>;
pub fn impact(s: &Session, a: &QueryArgs)          -> Result<traverse::BfsOutcome, CommandError>;
pub fn doctor(s: &Session) -> DoctorReport;   // Vec<Check{name, ok, detail}>
pub fn envelope(project: &str, result: Value, warnings: Vec<String>,
                truncated: bool, total: Option<usize>) -> Value;
```

`commands::references` etc. become three lines: `let s = Session::open(root)?; let page = query::relationships(...)?; print_relationships(&page.rows)` (or `println!("{}", envelope(.., None))`). `SymbolHit` carries `id` — `repo::search`/`find_by_name` must select `s.id` into `StoredSymbol` (additive field; CLI prints name/kind/path, so text output is unchanged) or an agent can never chain `search_symbols` → `get_symbol`.

Two new store queries, nothing more: `repo::find_symbol_by_id` (`get_symbol`) and `repo::language_breakdown` (`project_overview`).

## `get_symbol` Staleness (confirmed decision 4)

Reuses `commands::snippet`'s bounds check, promoted to `query::source_slice` and made explicit rather than a text marker:

```
read file from disk (always, every call)
  missing            -> { source: None, stale: true, reason: "file_missing" }
  end_byte > len     -> { source: None, stale: true, reason: "span_out_of_range" }
  non-utf8 span      -> { source: None, stale: true, reason: "non_utf8" }
  ok                 -> { source: Some(text ± context_lines), stale: index_pending > 0 }
```

## Server Bootstrap

```rust
// crates/codekurve-mcp/src/lib.rs
#![deny(clippy::print_stdout, clippy::dbg_macro)]   // the stdout guarantee, enforced by the compiler

pub struct CodeKurve { session: Mutex<Session>, allow_reindex: bool }

pub fn run(root: &Path) -> Result<(), String> {     // called from `codekurve mcp`, sync
    let session = Session::open(root)?;             // single project root, resolved once (decision 1)
    let allow = session.config().mcp.allow_reindex;
    tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|e| e.to_string())?
        .block_on(async {
            let svc = CodeKurve { session: Mutex::new(session), allow_reindex: allow }
                .serve(rmcp::transport::stdio()).await?;
            svc.waiting().await
        })
}
```

Each tool body: `let s = self.session.lock().unwrap(); let data = query::x(&s, input)?; Ok(json(envelope(..., s.warnings(), ..., Some(total))))`. No `.await` inside. `main.rs` gains one arm, `"mcp" => codekurve_mcp::run(&args.root)`; `tokio` never enters `codekurve-core`/`-store`/`-analysis`.

ponytail: a long `reindex` blocks the single-threaded runtime — acceptable for one stdio client; upgrade path is a multi-thread runtime + `spawn_blocking` if a second client ever exists.

## Config

```toml
[mcp]
allow_reindex = false   # default; section absent in older configs parses fine
```

`Config { #[serde(default)] pub mcp: Mcp }`, `Mcp { #[serde(default)] pub allow_reindex: bool }` — same `#[serde(default)]` pattern as `[index.watch]`.

## stdout Discipline (High risk)

Three layers, cheapest first:

1. `#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]` on the `codekurve-mcp` crate and on `query.rs` (`#![deny]` at lib level with a `#[allow]` on `commands.rs`, which is the only module permitted to print). Logging in `-mcp` goes through one `fn log(msg)` = `eprintln!`, `#[allow]`-ed at that fn.
2. Integration test: spawn `codekurve mcp`, write `initialize` + `tools/list` + one tool call, assert **every** stdout line is valid JSON-RPC (`jsonrpc == "2.0"`) and that a deliberately noisy env (`RUST_LOG=trace`) does not change that.
3. Same test asserts `tools/list` omits `codekurve_reindex` by default and includes it under `allow_reindex = true`.

## Data Flow

```
client ──stdio JSON-RPC──▶ rmcp ──▶ tool handler ──▶ query::*  ──▶ repo/traverse ──▶ SQLite
                                        │                │
                                   envelope(+warnings, total, truncated)
                                        │
CLI: main ──▶ commands::* ──────────────┘   (same query::*, printing is the only difference)
```

## File Changes

| File | Action | Description |
|---|---|---|
| `crates/codekurve/src/lib.rs` | Create | Library target: `pub mod commands; pub mod query; mod cli-free modules (incremental, watch)` |
| `crates/codekurve/src/query.rs` | Create | Data-returning layer, `Session`, `Page`, `envelope`, shared helpers moved from `commands.rs` |
| `crates/codekurve/src/commands.rs` | Modify | Becomes printing wrappers; keeps every current stdout string verbatim |
| `crates/codekurve/src/main.rs` | Modify | Drops `mod commands/incremental/watch`, uses the lib; adds `"mcp"` arm + USAGE |
| `crates/codekurve-mcp/src/{lib,server,tools}.rs` | Create | `run()`, `CodeKurve` handler, 11–12 tool bodies + schemas |
| `crates/codekurve-mcp/Cargo.toml` | Modify | `rmcp` (pinned `=`), `tokio`, `schemars`, `serde_json`, `codekurve` |
| `crates/codekurve-store/src/repo.rs` | Modify | `StoredSymbol.id`; `find_symbol_by_id`; `language_breakdown` |
| `crates/codekurve-core/src/config.rs` | Modify | `[mcp] allow_reindex` |
| `Cargo.toml` (workspace) | Modify | Add `[workspace.dependencies]` for `rmcp`/`tokio`/`serde_json` |
| `docs/AGENT_USAGE.md`, `README.md` | Create/Modify | §28.4's 8 rules + Claude Code / Codex install snippets |

## `rmcp` Risk (Med) — Pin and Verify

`rmcp`'s macro surface (`#[tool_router]`, `#[tool]`, `ServerHandler::get_info`) has churned across 0.x. Implementation **must**, before writing tools: (1) `cargo add rmcp --features server,transport-io` then rewrite the entry to an exact `=X.Y.Z` pin; (2) verify the handler shape against `docs.rs/rmcp/X.Y.Z` and `modelcontextprotocol/rust-sdk` `examples/servers/src/std_io.rs` for that same tag; (3) land a one-tool walking skeleton (`codekurve_project_status`) and a real client handshake **before** the remaining tools. Fallback if the macros differ: implement `ServerHandler::list_tools`/`call_tool` by hand over a `&[ToolSpec]` table — the adapter is thin by design, so a manual dispatch costs one match arm per tool.

## CLI Regression Risk (Med)

The split is behavior-preserving by construction: printing strings are moved, never rewritten, and `envelope()` emits `total` only when `Some`. Guard: the existing `crates/codekurve/tests/*` golden suite must pass **with zero edits** — any diff to an existing CLI test is a design violation, not a test update.

## Testing Strategy

| Layer | What | Approach |
|---|---|---|
| Unit | `Session::open` on missing config (fatal) / missing DB (degraded) / stale index | tempdir + in-memory DB |
| Unit | `warnings()` wording is identical for CLI stderr and MCP `warnings` | Assert the same `Vec<String>` feeds both |
| Unit | `source_slice` stale reasons: missing file, truncated file, non-utf8 | tempdir fixtures |
| Unit | `envelope(.., None)` == today's exact JSON bytes | Compare against the current golden string |
| Integration | JSON golden per tool (§28.3 fields: path, line range, confidence, provenance, total, truncated, warnings) | Fixture project, snapshot each `call_tool` result |
| Integration | stdout is JSON-RPC only; `reindex` hidden by default, present when enabled | Spawned-process test (layer 2/3 above) |
| Integration | Unsupported `kinds`/`languages`/`path_prefix` → explicit error, never silent | One case per filter |
| Regression | Every existing CLI test passes unmodified | Run the suite untouched |
| Manual | Claude Code (or Codex) connects and lists tools | Recorded in the PR description |

## Threat Matrix

N/A — no routing, shell command, VCS/PR automation, or executable-file classification. Rows: documentation-like paths, git selection, commit state, push state, PR commands — all N/A, no git or subprocess is invoked. The boundaries this design does own are covered above and in the test table: stdio protocol integrity (stdout discipline, 3 layers), untrusted `symbol_id`/`context_lines` input (parameterized SQL, caps, no path from the client — paths come from the index and are joined to the resolved root), and the single write tool (`reindex`, config-gated off, unregistered when off).

## Migration / Rollout

No data migration; schema untouched. `[mcp]` is additive with a safe default. Rollback = revert; clients drop one config entry, the index is untouched.

## Open Questions

- [ ] `rmcp` exact version and macro shape — resolved at implementation via the walking skeleton, not before.
- [ ] `project_overview` content beyond counts + language breakdown (entry points? hot symbols?) — shipping counts only until an agent-usage need appears.
- [ ] `search_symbols` filters are rejected in Phase 4; if agents hit this constantly, the fix is `repo::search` gaining SQL predicates, not the MCP layer filtering post-hoc.

## Acceptance Criteria

- [ ] `codekurve mcp` serves stdio; a real client lists exactly the enabled tools.
- [ ] Every tool returns `envelope` with `warnings`, `truncated`, and `total`; result rows carry path, line range, confidence, provenance.
- [ ] stdout contains only JSON-RPC under a spawned-process test; all logs on stderr.
- [ ] A stale or missing index appears in `warnings` on every response; the server never auto-indexes.
- [ ] `codekurve_reindex` is absent from `tools/list` unless `[mcp] allow_reindex = true`.
- [ ] Unsupported search filters fail with an explicit error naming the supported ones.
- [ ] The full pre-existing CLI test suite passes with zero edits.
- [ ] `docs/AGENT_USAGE.md` carries §28.4's 8 rules plus client install instructions.
