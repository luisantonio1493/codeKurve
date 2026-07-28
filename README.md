# CodeKurve

A local-first tool that indexes a repository's code structure (symbols,
relationships, call graphs) into SQLite and exposes fast queries through a
CLI and an MCP server. See `CODEKURVE_MASTER_PLAN.md` for the full plan.

## What it is

CodeKurve answers structural questions about a codebase — where a symbol is
defined, who calls it, what implements an interface, what the blast radius
of a change might be — without a human or an agent re-deriving that
structure by grepping the tree each time (§1).

## What it is NOT

Not a compiler, not a Language Server, not `ripgrep`, not a VCS. No cloud,
no web UI, no embeddings, no vector database, no LLM-built graph, no code
execution or modification. Full non-goal list: §7.

## Status

**Experimental, pre-0.1, Phase 0 (governance and scaffold).** No indexing,
querying, or MCP behavior exists yet — see `docs/ROADMAP.md`.

## Quickstart (target shape, not yet implemented)

The intended short-term workflow is `codekurve init`, `index`, `search`,
`callers`, `mcp` (§4.1). Actual command surface and flags land per
`docs/ROADMAP.md`; CLI conventions are defined in the plan §27.1.

## Supported languages (v0.1 target)

TypeScript/JavaScript first, C# second (§1, §6).

## MCP server

`codekurve mcp` serves the query layer over MCP stdio for agent clients
(Claude Code, Codex) instead of ad-hoc grepping. Full rules and client setup:
`docs/AGENT_USAGE.md`.

Quick start (Claude Code, `.mcp.json`):

```json
{
  "mcpServers": {
    "codekurve": {
      "command": "/absolute/path/to/target/release/codekurve",
      "args": ["mcp", "--root", "/absolute/path/to/project"],
      "transport": "stdio"
    }
  }
}
```

## Security promise

Local-only, no network, no telemetry, respects `.gitignore`, never executes
analyzed code. Full model: `docs/SECURITY_MODEL.md` (plan §5.8, §29).

## Limitations

Single repository, no dynamic-dispatch resolution guarantees, no semantic
analysis beyond what's listed in §6/§7 of the plan for v0.1.

## Licensing

Licensing has not been finalized. Do not redistribute. See
`docs/LICENSING.md`.
