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
**no server and no hosted UI** — nothing listens on a port, nothing runs in
the background for you to browse. No embeddings, no vector database, no
LLM-built graph, no code execution or modification. Full non-goal list: §7.

(`codekurve export` writes a *file*, the same way `--json` and the release
SBOM do. It is an output format, not a service:
[`docs/adr/0013-html-subgraph-export.md`](docs/adr/0013-html-subgraph-export.md).)

## Status

**Experimental.** All phases of `CODEKURVE_MASTER_PLAN.md` (0–8) are complete:
scaffold, TypeScript graph, incremental watcher, MCP server, C# support,
enterprise hardening, Angular/.NET framework awareness, and a real-repository
pilot. Phases 0–7 are archived under `openspec/changes/archive/`.

Phase 8 piloted CodeKurve on three real repositories — including a 350-file
production ASP.NET solution — measured every metric the plan asks for, found
and fixed three defects no fixture had caught, and concluded **continue**.
Evidence and honest limitations: [`docs/PILOT_REPORT.md`](docs/PILOT_REPORT.md).

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

Or, once installed, upgrade from the CLI:

```bash
codekurve update          # add --yes for non-interactive use
```

`codekurve update` is exactly equivalent to re-running the install script
above: it prints the command it is about to run, asks for confirmation, then
spawns it. CodeKurve has no HTTP client and makes no network request of its
own — the script does the download. Without a terminal to confirm at, it
refuses unless you pass `--yes`. See
[`docs/adr/0012-update-via-install-script.md`](docs/adr/0012-update-via-install-script.md),
which is honest about the supply-chain cost.

To remove the binary: `codekurve uninstall --binary`, or
`install.sh --uninstall` (`install.ps1 -Uninstall` on Windows) directly.

## Quickstart

```bash
codekurve init
codekurve index
codekurve search <query>
codekurve callers <symbol>
codekurve unresolved [<target-text>]
codekurve tui
codekurve mcp
codekurve update
```

Full command surface and flags: `docs/ROADMAP.md`; CLI conventions: the plan
§27.1.

`codekurve unresolved` answers the question the other queries can't:
CodeKurve never invents an edge it cannot determine (an external base type, a
name with zero candidates), and this command — plus the
`codekurve_find_unresolved` MCP tool — shows those references and the reason
each one stopped. Reach for it when `callers`/`implementations` come back
empty for a symbol that obviously has relationships; see
`docs/AGENT_USAGE.md`.

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

## HTML subgraph export

```bash
codekurve export graph.html --symbol-name SmartDBContext
codekurve export graph.html --symbol-name UserController --depth 3 --yes
```

Writes **one self-contained HTML file** picturing a symbol's neighbourhood in
both directions at once — what it reaches *and* what reaches it — then stops.
Open it with `file://`; there is no server, no daemon and no network. All CSS,
JS and geometry are inlined, so it renders identically with the cable
unplugged, and it survives being emailed or attached to a ticket.

- Layout is **radial by BFS depth**, computed in Rust: the focus symbol at the
  centre, one ring per hop. Distance therefore *means* hops, and the same
  index always produces byte-identical output.
- **Dashed edges are `Heuristic`-provenance** — framework inferences (Angular
  DI, ASP.NET routing, EF Core, …), never parsed facts. Solid edges are
  `Extracted`/`Resolved`. Colour is the relationship kind, opacity is
  confidence, and the legend says all three.
- Hover a node for its full qualified name and path; click it to keep only its
  incident edges lit.
- The same BFS caps `impact`/`trace` respect apply. If one fired, the file says
  so in a banner — a picture that silently omits nodes is worse than one that
  admits it.

Flags: `--symbol-name <name>` or `--symbol-id <id>`, `--depth N` (default 2),
`--min-confidence <level>`, `--root <path>`. An existing output path is
refused unless `--yes` is passed.

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
entry and leaving sibling servers intact. The distinction is worth being
explicit about:

- `codekurve uninstall` — agent configs only. The binary is left alone.
- `codekurve uninstall --binary` — agent configs **and** the executable. This
  spawns `install.sh --uninstall` (`install.ps1 -Uninstall` on Windows) and
  follows the same rules as `codekurve update`: it prints the exact command,
  confirms, and refuses without a terminal unless `--yes` is passed.

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
analyzed code. CodeKurve has no HTTP client dependency at all: indexing,
querying, watching and MCP make zero network requests and spawn zero
subprocesses. The single scoped exception is `codekurve update` /
`codekurve uninstall --binary`, which you must type yourself and confirm, and
which spawns the published install script rather than downloading anything
from Rust — see
[`docs/adr/0012-update-via-install-script.md`](docs/adr/0012-update-via-install-script.md).
Full model: `docs/SECURITY_MODEL.md` (plan §5.8, §29).

## Limitations

Single repository, no dynamic-dispatch resolution guarantees, no semantic
analysis beyond what's listed in §6/§7 of the plan for v0.1.

## Licensing

MIT. See [`LICENSE`](LICENSE) and `docs/LICENSING.md` for the rationale.
