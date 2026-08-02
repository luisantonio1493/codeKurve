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

### `find_callers` empty ≠ "nothing calls this"

`find_callers`/`find_callees` only return `Calls` edges — a real invocation
expression (`foo()`). A route handler, DI-registered service, or Angular
component is typically *never* called directly by name — the framework
invokes it through a decorator, attribute, or delegate reference instead
(Angular `@Component`, .NET `[HttpGet]`, `app.MapGet("/x", Handler)`, `<Add
Scoped, Transient, Singleton><T>()`). CodeKurve models that as a separate
`Injects`/`RegisteredAs`/`HandlesRoute`/`Triggers` edge, `Heuristic`
provenance (`docs/FRAMEWORKS.md`) — not as `Calls`. Empty `find_callers` on a
symbol that looks like an entry point is a signal to run `find_references`
instead (it returns every relationship kind, including framework edges), not
a signal that the symbol is dead code.

## Connecting a client over stdio

`codekurve mcp` speaks MCP over stdio only (no network port, no auth
surface). Build the binary once, then point your client at it:

```bash
cargo build --release -p codekurve-bin
# binary lands at target/release/codekurve
```

Or run `codekurve install` with no arguments: it detects every MCP client
installed on the machine (`claude-code` / `cursor` / `codex-cli` / `copilot` /
`opencode`) and writes the configs below, backing up any existing file first.
Detection is filesystem probing only (`~/.claude`, `~/.cursor`, `$CODEX_HOME`
or `~/.codex`, VS Code's user dir, `~/.config/opencode` or `~/.opencode`);
CodeKurve never shells out to check.

In an interactive terminal this opens a checkbox picker: detected agents
start checked, undetected ones are shown greyed out and cannot be selected,
`space` toggles, `↵` installs the checked set, and `q`/`Esc` cancels without
writing anything.

**Agents: this picker will never block you.** It appears only when there is
no client argument, `--yes` was not passed, *and* stdin is a terminal — the
same conditions the old `[y/N]` prompt appeared under. Pass `--yes`, run with
a non-terminal stdin, or name a client (`codekurve install claude-code`) and
the non-interactive path runs unchanged: it prints the plan and writes the
configs with no prompt and no screen.

`codekurve install <client>` still targets a single client directly, with no
prompt.

`codekurve uninstall [<client>]` removes the `codekurve` entry from each
client config that has one, preserving every sibling entry (a config with no
`codekurve` entry is reported and skipped, not an error). It manages agent
configs only — the CLI binary itself is removed by `install.sh --uninstall`
(`install.ps1 -Uninstall` on Windows).

### Claude Code

Add to your MCP server config (e.g. `.mcp.json` in the project root, or via
`claude mcp add`):

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

### Cursor

Add to `.cursor/mcp.json` in the project root — Cursor's own config omits a
`type`/`transport` key entirely:

```json
{
  "mcpServers": {
    "codekurve": {
      "command": "/absolute/path/to/target/release/codekurve",
      "args": ["mcp", "--root", "/absolute/path/to/project"]
    }
  }
}
```

### Codex CLI

Codex has no project-scoped config — `codekurve install codex-cli` writes to
`$CODEX_HOME/config.toml`, falling back to `$HOME/.codex/config.toml`
(`%USERPROFILE%\.codex\config.toml` on Windows). Register a stdio server with
`command`/`args` only, no `type`/`transport` key:

```toml
[mcp_servers.codekurve]
command = "/absolute/path/to/target/release/codekurve"
args = ["mcp", "--root", "/absolute/path/to/project"]
```

### GitHub Copilot (VS Code)

Add to `.vscode/mcp.json` in the project root — VS Code's own config uses
`"servers"` (not `"mcpServers"`) and a `"type": "stdio"` key for local
servers:

```json
{
  "servers": {
    "codekurve": {
      "command": "/absolute/path/to/target/release/codekurve",
      "args": ["mcp", "--root", "/absolute/path/to/project"],
      "type": "stdio"
    }
  }
}
```

### OpenCode

Add to `opencode.json` in the project root. OpenCode's `McpLocalConfig`
schema differs from every other client here: `command` is a single array
holding the binary *and* its arguments (no separate `args` key), and
`type: "local"` is required:

```json
{
  "mcp": {
    "codekurve": {
      "type": "local",
      "command": ["/absolute/path/to/target/release/codekurve", "mcp", "--root", "/absolute/path/to/project"]
    }
  }
}
```

Every client will send `initialize`, then `tools/list`, then `tools/call`
for whichever tool it needs. `reindex` only shows up in `tools/list` when
`[mcp] allow_reindex = true` is set in the project's config (off by default).

## `codekurve tui` — for the human, not for you

`codekurve tui [--root <path>]` opens an interactive explorer over the same
index: a live search box, the hits on the left, and the selected symbol's
`references` plus (on `i`) its impact set on the right, with `↵` walking to a
related symbol and `Esc` walking back.

It reads through the identical `query::search` / `get_symbol` /
`relationships` / `impact` calls the MCP tools use, so it never shows
anything the tools would not — including the same stale-index warning and the
same "run `codekurve index` first" refusal (exit code 4).

**Agents should not run it.** It takes over the terminal (raw mode,
alternate screen) and only exits on a keypress; there is no `--json` and no
non-interactive mode. Use the MCP tools, or the plain CLI commands, for
anything scripted. Recommend it to a human who is exploring an unfamiliar
area of the codebase interactively.

## Tool registry

`project_status`, `search_symbols`, `get_symbol`, `find_references`,
`find_callers`, `find_callees`, `find_implementations`, `trace_path`,
`analyze_impact`, `project_overview`, `doctor`, and (gated) `reindex`. See
`openspec/changes/phase-4-mcp/specs/mcp-server/spec.md` for the full
per-tool contract and response envelope (§28.3: source paths, line ranges,
confidence, provenance, stale warning, total count — never unbounded
results).
