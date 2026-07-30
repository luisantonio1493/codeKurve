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

**Experimental, pre-0.1.** Phases 0–4 (scaffold, vertical slice, TypeScript
graph, incremental watcher, MCP server) are complete and merged — indexing,
querying, and MCP behavior work today for TypeScript/JavaScript. Phase 5
(C# support) has all tasks implemented but failed independent verification
(missing visibility in CLI output, gaps in `using static`/alias-reference
test coverage) and is not yet merged — see `docs/ROADMAP.md` and
`openspec/changes/phase-5-csharp/verify-report.md`.

## Quickstart

```bash
codekurve init
codekurve index
codekurve search <query>
codekurve callers <symbol>
codekurve mcp
```

Full command surface and flags: `docs/ROADMAP.md`; CLI conventions: the plan
§27.1.

## Supported languages

CodeKurve indexes TypeScript, JavaScript, and C#. See the concise coverage
matrix and C# limitations in [docs/LANGUAGES.md](docs/LANGUAGES.md).

## MCP server

`codekurve mcp` serves the query layer over MCP stdio for agent clients
(Claude Code, Codex) instead of ad-hoc grepping. Full rules and client setup:
`docs/AGENT_USAGE.md`.

Quick start: `codekurve install claude-code` (or `cursor` / `codex-cli`) wires
the config automatically, or add this to `.mcp.json` by hand:

```json
{
  "mcpServers": {
    "codekurve": {
      "command": "/absolute/path/to/target/release/codekurve",
      "args": ["mcp", "--root", "/absolute/path/to/project"],
      "type": "stdio"
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
