# Agent Usage Guide

How an agent (or a human driving one) should use `codekurve mcp` instead of
grepping the repository cold. This is the reference the spec requirement
"AGENT_USAGE.md Documents the §28.4 Rules" points at.

## The eight rules (plan §28.4, verbatim)

Quoted as written in `CODEKURVE_MASTER_PLAN.md` §28.4 ("Guía para agentes"),
with an English gloss under each for readability.

> Crear `docs/AGENT_USAGE.md` con reglas:
>
> 1. Consultar CodeKurve antes de hacer una exploración amplia.
> 2. Usar búsqueda textual directa cuando se busca una cadena literal.
> 3. Verificar source actual antes de editar.
> 4. No confiar en edges low confidence para cambios críticos.
> 5. Usar `trace_path` para flujos.
> 6. Usar `impact` como candidato, no como garantía.
> 7. Después de cambios grandes, esperar watcher o ejecutar reindex.
> 8. Si la respuesta dice stale, leer el archivo actual.

1. **Consultar CodeKurve antes de hacer una exploración amplia.** Query
   CodeKurve (`search_symbols`, `find_references`, etc.) before doing a broad
   exploration — don't walk the whole tree by hand when a bounded query
   answers the same question.
2. **Usar búsqueda textual directa cuando se busca una cadena literal.** Use
   direct text search (`grep`/`ripgrep`) when looking for a literal string —
   CodeKurve indexes structure (symbols, relationships), not arbitrary text.
3. **Verificar source actual antes de editar.** Verify current source before
   editing — `get_symbol` reads disk on every call, but always re-check the
   file yourself before writing to it; the index can lag behind the working
   tree.
4. **No confiar en edges low confidence para cambios críticos.** Do not trust
   low-confidence edges for critical changes — check each row's `confidence`
   field before relying on it for anything you can't easily revert.
5. **Usar `trace_path` para flujos.** Use `trace_path` for flows — when you
   need "does A eventually reach B", trace it explicitly rather than
   following `find_callers`/`find_callees` by hand.
6. **Usar `impact` como candidato, no como garantía.** Treat `analyze_impact`
   as a candidate list, not a guarantee — it is a bounded reverse traversal,
   not proof that nothing outside the list is affected.
7. **Después de cambios grandes, esperar watcher o ejecutar reindex.** After
   large changes, wait for the watcher or run reindex — every tool response
   carries a stale warning driven by `pending_files`; don't keep querying a
   known-stale index for a large change without refreshing it first.
8. **Si la respuesta dice stale, leer el archivo actual.** If a response says
   stale, read the current file — a stale flag (project-level or per-symbol
   on `get_symbol`) means the index may not reflect what's on disk right now.

## Connecting a client over stdio

`codekurve mcp` speaks MCP over stdio only (no network port, no auth
surface). Build the binary once, then point your client at it:

```bash
cargo build --release -p codekurve-bin
# binary lands at target/release/codekurve
```

### Claude Code

Add to your MCP server config (e.g. `.mcp.json` in the project root, or via
`claude mcp add`):

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

### Codex

Codex's config format is equivalent — register a stdio server with the same
`command`/`args` pair:

```toml
[mcp_servers.codekurve]
command = "/absolute/path/to/target/release/codekurve"
args = ["mcp", "--root", "/absolute/path/to/project"]
```

Either client will send `initialize`, then `tools/list`, then `tools/call`
for whichever tool it needs. `reindex` only shows up in `tools/list` when
`[mcp] allow_reindex = true` is set in the project's config (off by default).

## Tool registry

`project_status`, `search_symbols`, `get_symbol`, `find_references`,
`find_callers`, `find_callees`, `find_implementations`, `trace_path`,
`analyze_impact`, `project_overview`, `doctor`, and (gated) `reindex`. See
`openspec/changes/phase-4-mcp/specs/mcp-server/spec.md` for the full
per-tool contract and response envelope (§28.3: source paths, line ranges,
confidence, provenance, stale warning, total count — never unbounded
results).
