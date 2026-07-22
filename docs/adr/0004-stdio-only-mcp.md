# 0004. MCP transport is stdio-only

## Context

CodeKurve exposes its query surface to AI agents via MCP in addition to the
CLI. See CODEKURVE_MASTER_PLAN.md §28.1 (transport) and the tool list in
§28.2.

## Decision

The MCP server communicates exclusively over stdio. No HTTP transport is
implemented in the MVP.

## Alternatives

- **HTTP/SSE transport**: enables remote clients and multiple concurrent
  consumers, but opens a network port, requires authentication/authorization,
  and expands the security surface — directly against the local-first and
  no-network principles (§5.1, §5.8).
- **Unix domain socket / named pipe**: avoids a network port but adds
  platform-specific IPC code for no MVP benefit over stdio, which every MCP
  client already supports.

## Consequences

- Server lifecycle is controlled by the invoking client process (§28.1);
  no listener, no port, no auth code to write or audit.
- Multi-client/remote access is out of scope until a future ADR revisits
  this decision.
- Implementation (`codekurve-mcp` crate, tool handlers) lands in a later
  phase; see `docs/ROADMAP.md`.

## Status

Accepted (2026-07-22)
