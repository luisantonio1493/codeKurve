# MCP

The MCP server is implemented (`codekurve mcp`, stdio). This document
describes its tools and the response contract.

## Setup

`stdio` transport only for the MVP: no open ports, no auth surface,
lifecycle controlled by the client (plan §28.1).

## Tools

`codekurve_project_status`, `codekurve_search_symbols`,
`codekurve_get_symbol`, `codekurve_find_references`,
`codekurve_find_callers`, `codekurve_find_callees`,
`codekurve_find_implementations`, `codekurve_find_unresolved`,
`codekurve_find_routes`,
`codekurve_trace_path`, `codekurve_analyze_impact`,
`codekurve_project_overview`, `codekurve_doctor`, `codekurve_reindex`
(disabled by default). Full input schemas: plan §28.2.

`codekurve_find_unresolved` is not in plan §28.2 — it surfaces the
`unresolved_references` rows the analyzer already writes (references it
deliberately declined to resolve into edges, each with a `reason`), which
were previously only visible as `project_status`'s
`relationships_unresolved` count. CLI equivalent: `codekurve unresolved
[<target-text>]`.

## Response contract (`schema_version` 2)

Every response is one JSON envelope:

| field | meaning |
| --- | --- |
| `schema_version` | `2` |
| `project` | project name |
| `result` | the tool's data |
| `truncated` | `true` when more results exist than were returned |
| `truncated_reason` | present on `trace_path`/`analyze_impact` when truncated: `max_depth`, `max_nodes`, `max_edges` or `max_duration` |
| `total` | paginated tools only: rows returned in this page |
| `warnings` | present only when non-empty (e.g. a stale-index notice) |

**Absent means none.** Fields whose value would be `null`, and an empty
`warnings`, are omitted, not sent as `null`/`[]`.

`trace_path` returns `result.path` (only the nodes on the found path, start
first), `result.path_found` and `result.reached_count` (how many nodes the
search visited) — never the whole visited frontier. `analyze_impact` rows
carry `qualified_name`, so a node can be read without a follow-up
`get_symbol`. `analyze_impact` takes optional `limit` and `offset` over its
reached rows (BFS order); without them every reached row is returned, as
before. `total` is always the full reached count, and `truncated` is set when
a `limit`/`offset` window leaves reached rows out. `trace_path` has no paging:
its answer is one path. `codekurve_search_symbols` sets `truncated` when
more matches exist than `limit` (which is capped at `queries.max_limit`).
`get_symbol` omits `source_truncated` unless the snippet was cut at
`queries.max_snippet_bytes`.

Derivable or constant fields are not repeated. A symbol's file is the part
of its `qualified_name` before the first `::` (the whole name for a
file-level symbol), and its short name is the last segment, so rows do not
carry `path` or `name`. Symbol rows (`search_symbols`, `get_symbol`) are
always tree-sitter provenance and high confidence, so they carry neither
field; relationship, graph and unresolved rows keep `confidence`/`provenance`
because those vary per edge.

The CLI's `--json` output keeps `schema_version` 1 and is unchanged.

## Schemas / response design

Responses must be compact, structured, explainable, bounded, and include
source paths, line ranges, confidence, provenance, staleness warnings, and
total counts — never unbounded result sets (plan §28.3).

## Agent guidance

Rules for agents consuming CodeKurve via MCP (query before broad
exploration, verify current source before editing, don't trust low-
confidence edges for critical changes, treat `impact` as a candidate not a
guarantee, reindex after large changes): plan §28.4.
