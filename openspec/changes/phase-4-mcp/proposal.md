# Proposal: Phase 4 — MCP Server

## Intent

The index is only reachable from a human terminal. Agents (Claude Code, Codex) still grep and read whole files, which is exactly the cost CodeKurve exists to remove. Phase 4 (§28, "Fase 4 — MCP") exposes the existing queries over MCP stdio so an agent can ask "who calls this" instead of reading the repo — with bounded, explicable answers it can trust or discard.

## Scope

### In Scope
- `codekurve mcp` (stdio only) built on `rmcp` + `tokio`, registering the §28.2 tools.
- Tools: `project_status`, `search_symbols`, `get_symbol`, `find_references`, `find_callers`, `find_callees`, `find_implementations`, `trace_path`, `analyze_impact`, `project_overview`, `doctor`, `reindex` (off unless `[mcp] allow_reindex = true`).
- Extract the current command bodies into a reusable library layer returning data (not `println!`), shared by CLI and MCP.
- Response contract (§28.3): path, line range, confidence, provenance, total count, truncation flag, stale warning — all bounded by existing caps.
- stdout carries JSON-RPC only; all logs to stderr.
- JSON golden tests per tool; `docs/AGENT_USAGE.md` (§28.4, 8 rules) + client install docs.

### Out of Scope
- HTTP/SSE transport, auth, ports (§28.1 explicitly defers).
- MCP resources/prompts; only tools.
- New query semantics — no new graph capability, only exposure.
- Search filters not already supported by the store (`kinds`/`languages`/`path_prefix` shipped only where `repo` already supports them).

## Capabilities

### New Capabilities
- `mcp-server`: stdio transport, tool registry, schemas, response envelope, stdout discipline, reindex gating.

### Modified Capabilities
- `graph-queries`: query results become returnable data; CLI printing is a consumer, output unchanged.

## Key Decisions

| Question | Decision | Rationale |
|---|---|---|
| Where does tool logic live | Add `src/lib.rs` to the `codekurve` crate; `codekurve-mcp` depends on it | Reuses `paginate`/`resolve_symbol`/envelope instead of duplicating them in a new crate |
| MCP entrypoint | Subcommand `codekurve mcp` | One binary to configure in clients |
| Stale index | Warning field in every response, never a hard error | §28.3 + exit criterion "stale state visible" |
| `reindex` | Config-gated, default off | §28.2; a write tool behind an agent needs consent |
| Caps | Reuse CLI BFS/pagination caps | "No devolver 10,000 nodos" already solved |

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `crates/codekurve-mcp/src/` | New | Server, tool handlers, schemas |
| `crates/codekurve/src/lib.rs` | New | Public query layer |
| `crates/codekurve/src/commands.rs` | Modified | Print/return split; `search`/`symbol`/`doctor` gain data paths |
| `crates/codekurve-core/src/config.rs` | Modified | `[mcp]` section |
| `docs/AGENT_USAGE.md`, `README` | New/Modified | Agent rules, client setup |
| `Cargo.toml` | Modified | `rmcp`, `tokio`, `schemars` |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| A stray `println!` corrupts the protocol | High | Test asserting clean stdout; lint/grep gate in CI |
| `rmcp` API churn | Med | Pin exact version; thin adapter over the shared layer |
| Refactor regresses CLI output | Med | Existing CLI golden tests must pass untouched |
| Agents over-trust low-confidence edges | Med | `confidence` + `provenance` per row; `AGENT_USAGE.md` rule 4 |
| `tokio` enters a sync codebase | Low | Confined to the MCP crate; core stays sync |

## Rollback Plan

Revert the commits. The MCP crate is additive; the CLI-side split is behavior-preserving. Clients only lose an entry in their MCP config; the index is untouched.

## Dependencies

- `rmcp`, `tokio` (§ "Async y MCP"), `schemars` for tool input schemas.
- Phase 3 freshness metadata (already shipped) for the stale warning.

## Success Criteria

- [ ] At least one real client (Claude Code or Codex) connects and lists all enabled tools.
- [ ] Every tool call is reproducible and returns a valid JSON envelope.
- [ ] stdout contains only JSON-RPC; logs appear on stderr.
- [ ] Every result set is bounded and reports total count plus truncation.
- [ ] A stale index is visible in every tool response.
- [ ] `reindex` is absent from the tool list unless explicitly enabled.
- [ ] `docs/AGENT_USAGE.md` documents the 8 §28.4 rules and client installation.

## Confirmed Decisions

1. One MCP server per project root, resolved at startup like the CLI. Multi-repo is out of scope for Phase 4.
2. If the index is missing or stale at connect, the server serves degraded with warnings on every response; it never auto-indexes.
3. `search_symbols` filters the store cannot yet honor are rejected with an explicit error, not silently ignored.
4. `get_symbol` reads source from disk on every call, including when the file drifted from the index, and sets an explicit stale flag when spans no longer match.
