# 0013. HTML subgraph export is an output format, not a web UI

## Context

`codekurve impact` and `codekurve trace` answer real questions and print flat
text. For a bounded neighbourhood — "where does `SmartDBContext` sit in this
solution?" — a list of 60 indented lines is the wrong shape for the answer.
The relationships are two-dimensional; the terminal is not. The TUI (ADR 0011)
helps a human walk the graph one hop at a time, but it cannot be pasted into a
ticket, attached to a design review, or read by someone who does not have the
repo checked out.

One promise stands in the way, and it is advertised in two places:

- `README.md`, "What it is NOT": "No cloud, **no web UI**, no embeddings …"
- `CODEKURVE_MASTER_PLAN.md` §7 (non-goals): "interfaz web".

Read literally, "no web UI" could be taken to forbid emitting HTML at all.
That reading is wrong, and leaving it ambiguous is worse than either
interpretation. The non-goal exists to keep out **servers and browsing
applications** — the scope that drags in hosting, accounts, collaboration,
cloud sync, a desktop shell, and an update surface. It was never about output
formats: CodeKurve already emits `--json` envelopes, an SPDX SBOM, and a
`NOTICE` file, and nobody has ever argued those violate a non-goal.

## Decision

Add `codekurve export <output.html>`: a command that writes **one file** and
exits. It is another output format alongside `--json`, and it is explicitly
**not** a web UI.

The distinction, stated so it can be checked rather than argued:

| A web UI (still a non-goal) | `codekurve export` (this ADR) |
|---|---|
| A process that keeps running | Runs, writes a file, exits |
| Listens on a port / serves requests | Binds nothing |
| Has a URL, a session, users | Has a file path |
| Fetches data at view time | Data is baked in at write time |
| Needs the tool installed to view | Renders on any browser, offline, forever |
| Ships an update/hosting surface | Ships a file you can delete |

Concretely, and enforced by tests:

- **The output is genuinely self-contained.** All CSS, all JS, and the SVG are
  inlined. No CDN, no external font, no remote image, no `fetch`, no
  `xmlns="http://…"` — literally not one `http://` or `https://` string in the
  file. Opening it with the network cable unplugged renders identically. A CLI
  end-to-end test greps the generated file for external references, and a unit
  test does the same on the rendered string.
- **No new Rust dependency, and no new runtime dependency.** The HTML is built
  with ordinary `write!` calls into a `String` — no template engine — and the
  page loads no library.
- **No new traversal.** The neighbourhood is the existing
  `codekurve-store::traverse::bfs` run twice, once over the forward adjacency
  and once over the reverse one (`load_adjacency`'s `reverse` flag), then
  merged. The same caps `impact`/`trace` respect (`commands::bfs_caps`) apply,
  and the same `--min-confidence` filter.
- **Nothing existing changes.** The analyzer, the schema, and every existing
  command's behaviour are untouched; `export` is additive dispatch plus one new
  module.
- **Truncation is admitted in the artifact.** If a BFS cap fired, the HTML
  itself carries a banner saying so, not only stderr. A picture that silently
  omits nodes is more dangerous than a list that does, because it looks
  complete.
- **An existing output path is refused unless `--yes` is passed.** An export
  must not silently clobber a file.

Two visual decisions are load-bearing rather than cosmetic:

- **Radial layout by BFS depth, computed in Rust** — focus at the centre, one
  ring per hop, angles distributed evenly within a ring. Not a force-directed
  simulation. This keeps the file self-contained without inlining ~100–280 KB
  of a JS layout library; it is deterministic, so the same index produces
  byte-identical output that is diffable and testable; and the radial distance
  *means* something (hops from the focus), whereas the distance a force layout
  settles on means nothing at all.
- **`Heuristic`-provenance edges are dashed; `Extracted`/`Resolved` edges are
  solid**, and the legend says which is which. This is the project's core
  differentiator (`docs/FRAMEWORKS.md`, ADR 0007): a framework-inferred
  `RegisteredAs` must never look like a parsed fact. A viewer must be able to
  tell at a glance which edges are inferences. Confidence is carried as stroke
  opacity plus the exact value in the edge tooltip — an encoding chosen not to
  imply more certainty than the stored data holds.

This ADR **scopes** the "no web UI" non-goal; it does not reverse it.
CodeKurve still serves nothing, hosts nothing, and opens no socket — ADR 0005
("no network, no telemetry, in any mode") is untouched, and this command adds
no HTTP client and no subprocess. `README.md`'s wording is corrected from "no
web UI" to "no server and no hosted UI" so the promise says what it always
meant.

## Alternatives

- **Leave it at `impact --json` and let people bring their own renderer**:
  rejected. It pushes the interesting half — knowing that a dashed edge means
  "inferred, not parsed" — onto every consumer, and every consumer will get it
  wrong differently. Provenance styling is exactly the thing CodeKurve knows
  and a generic graph viewer does not.
- **Emit DOT/GraphML and let Graphviz or Gephi draw it**: rejected as the
  primary path. It requires the reader to install a tool, and both formats lose
  the styling contract above unless the consumer reimplements it. It remains
  the obvious future addition if someone asks for it, and would be a small
  change to the same `(nodes, edges, focus) -> String` seam.
- **Inline a real graph library (d3-force, cytoscape) and simulate the
  layout**: rejected. It adds 100–280 KB to every exported file, makes the
  output non-deterministic (so it cannot be byte-compared in a test or
  usefully diffed), and buys a layout whose geometry carries less information
  than the radial one it would replace.
- **A `codekurve serve` that renders the graph live in a browser**: rejected —
  *this* is the non-goal. It would be a real server, with a port, a session, a
  security surface, and an appetite for auth and sharing.
- **Only the blast radius (reverse BFS, like `impact`)**: rejected. Half the
  value of a picture is seeing what the symbol depends on next to what depends
  on it; a one-directional picture explains consequences without explaining
  place.

## Consequences

- The "no web UI" line in `README.md` and plan §7 must from now on be read as
  "no server or hosted UI". Anyone proposing a long-running process, a port, or
  a hosted view is still proposing a non-goal and needs an ADR that supersedes
  this one.
- The self-contained rule is a **standing constraint on this file**, not a
  one-time property. Any future edit that adds a CDN script, a web font, an
  external image, or a `fetch` breaks the promise this ADR makes. The
  external-reference tests exist to fail loudly when that happens; deleting
  them requires superseding this ADR.
- Rendering stays a pure `(nodes, edges, focus) -> String` function
  (`crates/codekurve/src/export.rs`), so it is tested without a database and
  can back a second output format later without another traversal.
- The picture is a BFS *tree* per direction: `Reached::via` records only the
  edge that first reached a node, so a node reachable several ways shows one
  edge per direction rather than all of them. This matches what `impact` and
  `trace` already report; it is a readability choice, and it is documented in
  the code rather than left for a reader to discover from a missing line.
- Very large neighbourhoods are handled by truncation, not by pan/zoom
  machinery. `--depth 4` on a hub symbol will hit `max_nodes` and say so. If
  that becomes the common case, the fix is a filter (`--min-confidence`, a
  kind filter), not a bigger viewer.

## Status

Accepted (2026-08-02)
