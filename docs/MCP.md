# MCP

**Not implemented yet — this is Phase 4 (plan §43), not Phase 0.** This
document records the intended design for future reference.

## Setup

`stdio` transport only for the MVP: no open ports, no auth surface,
lifecycle controlled by the client (plan §28.1).

## Tools

`codekurve_project_status`, `codekurve_search_symbols`,
`codekurve_get_symbol`, `codekurve_find_references`,
`codekurve_find_callers`, `codekurve_find_callees`,
`codekurve_find_implementations`, `codekurve_find_unresolved`,
`codekurve_trace_path`, `codekurve_analyze_impact`,
`codekurve_project_overview`, `codekurve_doctor`, `codekurve_reindex`
(disabled by default). Full input schemas: plan §28.2.

`codekurve_find_unresolved` is not in plan §28.2 — it surfaces the
`unresolved_references` rows the analyzer already writes (references it
deliberately declined to resolve into edges, each with a `reason`), which
were previously only visible as `project_status`'s
`relationships_unresolved` count. CLI equivalent: `codekurve unresolved
[<target-text>]`.

## Schemas / response design

Responses must be compact, structured, explainable, bounded, and include
source paths, line ranges, confidence, provenance, staleness warnings, and
total counts — never unbounded result sets (plan §28.3).

## Agent guidance

Rules for agents consuming CodeKurve via MCP (query before broad
exploration, verify current source before editing, don't trust low-
confidence edges for critical changes, treat `impact` as a candidate not a
guarantee, reindex after large changes): plan §28.4.
