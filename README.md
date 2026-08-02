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

**Experimental.** Phases 0–7 of `CODEKURVE_MASTER_PLAN.md` are complete,
verified, and archived under `openspec/changes/archive/`: scaffold, TypeScript
graph, incremental watcher, MCP server, C# support, enterprise hardening, and
Angular/.NET framework awareness. First tagged release: `v0.1.0`.

Fase 8 (real-repo pilot evaluation) is in progress — see `docs/ROADMAP.md` for
what's measured and what's still open.

## Installation

macOS/Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1 | iex
```

Both scripts install the latest release binary and add it to your PATH.
Re-run the same command to upgrade.

## Quickstart

```bash
codekurve init
codekurve index
codekurve search <query>
codekurve callers <symbol>
codekurve tui
codekurve mcp
```

Full command surface and flags: `docs/ROADMAP.md`; CLI conventions: the plan
§27.1.

## Interactive explorer

`codekurve tui [--root <path>]` opens a terminal UI over the same index the
CLI and the MCP server read — type to search, arrow through the hits, and
walk the graph without re-running a command per hop.

```text
┌ codekurve ────────────────────────────────────────────────────┐
│search: TodoItem█                                              │
└───────────────────────────────────────────────────────────────┘
┌ symbols (9) ────────┐┌ TodoItem (class) ───────────────────────┐
│  Title  property    ││Source/Models/TodoItem.cs:5              │
│> TodoItem  class    ││                                         │
│  User  property     ││references (4)                           │
│  UserId  property   ││ ← MinimalApi.Data.TodoDbContext  persist│
│                     ││ ← MinimalApi.TodoApi.CreateTodoItem  con│
└─────────────────────┘└─────────────────────────────────────────┘
 ↑↓ move  ↵ open  i impact  esc back  / search  q quit
```

| key | action |
|---|---|
| type / `Backspace` | edit the search box (results update live) |
| `↑` `↓` | move the selection in the focused list |
| `↵` | search box → relationship list; on a relationship row, open that symbol |
| `Tab` / `→` / `←` | move focus between the results and relationship lists |
| `Esc` / `Backspace` | go back one symbol; with an empty history, return to the search box |
| `i` | toggle the impact view (reached nodes and depth) for the selected symbol |
| `/` | jump back to the search box |
| `q` | quit (outside the search box, where `q` is just a character) |
| `Ctrl-C` | quit from anywhere |

The right panel lists **references**, not callers, so framework edges
(`Injects`, `HandlesRoute`, `RegisteredAs`, …) appear — see
`docs/AGENT_USAGE.md`. A stale index is shown as a warning line rather than
being silently ignored, and a project with no index refuses to open with the
same message the CLI gives.

Requires a real terminal; it is not part of the MCP surface, and no
non-interactive command's behaviour changed. Rationale and dependency
justification: [`docs/adr/0011-ratatui-tui.md`](docs/adr/0011-ratatui-tui.md).

## Supported languages

CodeKurve indexes TypeScript, JavaScript, and C#. See the concise coverage
matrix and C# limitations in [docs/LANGUAGES.md](docs/LANGUAGES.md).

## Framework awareness

CodeKurve recognizes Angular (`@Component`, `@Injectable`, DI, routes) and
.NET (attribute-driven controllers/Azure Functions, minimal APIs, DI
registration, EF Core) idioms as a separate heuristic pass downstream of
extraction. Every framework edge is marked `Heuristic` and never upgrades to
a resolved fact. Full catalogue, confidence semantics, and published
limitations: [docs/FRAMEWORKS.md](docs/FRAMEWORKS.md).

## MCP server

`codekurve mcp` serves the query layer over MCP stdio for agent clients
(Claude Code, Codex) instead of ad-hoc grepping. Full rules and client setup:
`docs/AGENT_USAGE.md`.

Quick start: `codekurve install` detects every MCP client installed on the
machine (`claude-code`, `cursor`, `codex-cli`, `copilot` (VS Code),
`opencode`) and configures the ones you pick. Detection is filesystem
probing only; CodeKurve never shells out.

In a terminal it opens a checkbox picker — detected agents start checked,
undetected ones are listed greyed out and cannot be selected (use
`codekurve install <client>` to force one):

```text
┌ codekurve install ──────────────────────────────────────┐
│ Detected agents:                                        │
│                                                         │
│ > [x] claude-code   .mcp.json                           │
│   [x] cursor        .cursor/mcp.json                    │
│   [ ] codex-cli     ~/.codex/config.toml (not detected) │
│   [ ] copilot       .vscode/mcp.json     (not detected) │
│   [x] opencode      opencode.json                       │
└─────────────────────────────────────────────────────────┘
 space toggle  ↵ install  q cancel
```

`space` toggles, `↵` installs the checked set, `q`/`Esc` cancels without
writing anything. Pass `--yes`, pipe stdin, or name a client and the picker
never appears — those paths print the plan and behave exactly as before.

To target one client instead, name it: `codekurve install <client>`.
`codekurve uninstall [<client>]` reverses it, removing only the `codekurve`
entry and leaving sibling servers intact (it does not touch the binary — use
`install.sh --uninstall` for that).

Or add this to `.mcp.json` by hand (`codekurve` on PATH after
[installing](#installation), or an absolute path to the binary):

```json
{
  "mcpServers": {
    "codekurve": {
      "command": "codekurve",
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

MIT. See [`LICENSE`](LICENSE) and `docs/LICENSING.md` for the rationale.
