# MCP Server Specification

## Purpose

Expose the existing query layer (`graph-queries`, `symbol-index`, `incremental-index`) over MCP stdio so an agent client can call bounded, explicable tools instead of grepping the repo (plan §28 "Fase 4 — MCP"; proposal Intent/Scope).

## Requirements

### Requirement: Stdio Transport Only

`codekurve mcp` MUST serve the Model Context Protocol over stdio only. It MUST NOT open a network port, MUST NOT implement HTTP/SSE transport, and MUST NOT implement authentication, since stdio has no listening surface to authenticate (§28.1).

#### Scenario: Server starts on stdio

- GIVEN a client launches `codekurve mcp` as a subprocess
- WHEN the client sends an `initialize` request on stdin
- THEN the server responds on stdout over the same stdio channel, without binding any TCP/UDP port

### Requirement: stdout Carries JSON-RPC Only

While `codekurve mcp` is running, stdout MUST contain only JSON-RPC protocol frames. All logs, diagnostics, and error traces MUST be written to stderr, never to stdout, because any stray stdout write corrupts the protocol stream for the connected client.

#### Scenario: Logging goes to stderr

- GIVEN the server logs a diagnostic message while handling a tool call
- WHEN the message is emitted
- THEN it appears on stderr and stdout contains no bytes outside valid JSON-RPC frames

#### Scenario: A handler error does not leak to stdout

- GIVEN a tool handler encounters an internal error
- WHEN the server reports the failure
- THEN the failure is returned as a JSON-RPC error response (or logged to stderr), and stdout is never written to outside the JSON-RPC response itself

### Requirement: Single Project Root Per Server Instance

Each `codekurve mcp` process MUST resolve exactly one project root at startup, the same way the CLI resolves it, and MUST serve all tool calls against that single root for the process lifetime. Multi-repo/multi-root serving from one server instance is out of scope (proposal Confirmed Decision 1).

#### Scenario: Root resolved once at startup

- GIVEN `codekurve mcp` is started from a directory belonging to project P
- WHEN any tool is called during the session
- THEN every tool call is answered against project P's index, and the server exposes no parameter to switch project root mid-session

### Requirement: Tool Registry

The server MUST register the following tools, each with a JSON Schema for its input: `project_status`, `search_symbols`, `get_symbol`, `find_references`, `find_callers`, `find_callees`, `find_implementations`, `find_unresolved`, `trace_path`, `analyze_impact`, `project_overview`, `doctor`. `reindex` MUST be registered only when gated in (see the reindex gating requirement) (§28.2).

#### Scenario: Client lists tools

- GIVEN a real MCP client (e.g. Claude Code or Codex) connects to `codekurve mcp` with `reindex` disabled
- WHEN the client requests the tool list
- THEN it receives the twelve always-on tools with valid input schemas, and `reindex` is absent

### Requirement: find_unresolved Tool

`find_unresolved` MUST return the project's `unresolved_references` rows — references the analyzer recorded but deliberately declined to resolve into edges — each carrying at minimum the source symbol id and qualified name (both nullable for file-level references), the source file's path, the relationship kind, the target text, the recorded `reason`, the confidence, and the candidate count. It MUST accept optional `target_text` (matched exactly), `symbol_id`/`symbol_name`, `limit`, and `offset`; with no filter it MUST list the whole project's unresolved references, bounded by the same caps the other query tools use. A project with no unresolved references MUST return an empty, non-error result. The tool MUST NOT create, infer, or imply a relationship edge for any row it returns.

#### Scenario: Unresolvable external base type is explained

- GIVEN a C# class whose base list names a type defined outside the indexed project, recorded as an unresolved reference because base class vs interface is undeterminable
- WHEN `find_implementations` is called for that type and returns no rows, and `find_unresolved` is then called with the type's name as `target_text`
- THEN `find_unresolved` returns the recorded row with its `reason`, and no `Implements` edge exists for it

#### Scenario: Fully resolved project

- GIVEN an indexed project with zero `unresolved_references` rows
- WHEN `find_unresolved` is called with no filter
- THEN the response is an empty result set with `total: 0`, not an error

### Requirement: project_status Tool

`project_status` MUST return, without any input beyond the resolved root: project name, root path, index status, generation, last index timestamp, pending file count, parse error count, and schema version (§28.2).

#### Scenario: Status of a freshly indexed project

- GIVEN a project with a completed index run and 0 pending files
- WHEN `project_status` is called
- THEN the response includes project, root, index status, generation, last index timestamp, pending=0, parse error count, and schema version

### Requirement: search_symbols Tool Rejects Unsupported Filters

`search_symbols` MUST accept `query`, `kinds`, `languages`, `path_prefix`, and `limit`. If a filter value is not supported by the underlying store for the current project (e.g. a `kinds`/`languages` value the store cannot filter on), the tool MUST return an explicit error naming the unsupported filter; it MUST NOT silently ignore the filter and return unfiltered or partially filtered results (proposal Confirmed Decision 3).

#### Scenario: Supported filters narrow results

- GIVEN an indexed TypeScript project
- WHEN `search_symbols` is called with `query: "EligibilityService"`, `kinds: ["class", "interface"]`, `languages: ["typescript"]`
- THEN the response contains only matching symbols honoring all three filters, bounded by `limit`

#### Scenario: Unsupported filter is rejected, not ignored

- GIVEN a store that cannot filter by a given `kinds`/`languages` value for the current project
- WHEN `search_symbols` is called with that unsupported filter value
- THEN the tool returns an explicit error identifying the unsupported filter, and does not return a result set computed by silently dropping the filter

### Requirement: get_symbol Reads Live Source and Flags Drift

`get_symbol` MUST read the requested source snippet from disk on every call, not from a cached/indexed copy of the file content. If the symbol's indexed span no longer matches the current file content (the file drifted since the last index run), the response MUST set an explicit stale flag on that result (proposal Confirmed Decision 4).

#### Scenario: Source reflects the current file

- GIVEN a symbol whose file was edited after the last index run, but the edit did not change the symbol's line span
- WHEN `get_symbol` is called with `include_source: true`
- THEN the returned source is read from the current file on disk, reflecting the edit

#### Scenario: Drifted span is flagged stale

- GIVEN a symbol's indexed line span no longer matches the current file content
- WHEN `get_symbol` is called for that symbol
- THEN the response sets an explicit stale flag for that result, distinct from the project-level stale warning

### Requirement: Query Tools Return the §28.3 Response Envelope

`find_references`, `find_callers`, `find_callees`, `find_implementations`, `trace_path`, `analyze_impact`, and `search_symbols` MUST return results as an envelope containing, per applicable row: source path, line range, confidence, and provenance; and at the result-set level: total count and a truncation flag. Result sets MUST be bounded by the same caps the CLI equivalents use (§28.3, proposal "Caps" decision).

#### Scenario: Bounded, explicable result

- GIVEN a symbol with more callers than the configured cap
- WHEN `find_callers` is called
- THEN the response returns at most the capped number of rows, each with path, line range, confidence, and provenance, plus a total count greater than the returned row count and `truncated: true`

#### Scenario: Small result is not marked truncated

- GIVEN a symbol with 2 callers, under the cap
- WHEN `find_callers` is called
- THEN the response returns both callers, `truncated: false`, and a total count of 2

### Requirement: Stale Warning Visible on Every Response

Every tool response MUST include a stale-index warning field reflecting whether the stored freshness metadata (`pending_files`) shows pending files, regardless of which tool was called. The server MUST NOT perform a filesystem walk to compute this; it MUST read the same stored freshness metadata the CLI staleness warning uses.

#### Scenario: Stale warning present with pending files

- GIVEN stored freshness metadata shows 3 pending files
- WHEN any tool is called
- THEN the response includes a stale-index warning indicating pending changes

#### Scenario: No stale warning when fresh

- GIVEN stored freshness metadata shows 0 pending files
- WHEN any tool is called
- THEN the response's stale-index warning field indicates no staleness

### Requirement: Missing or Stale Index Served Degraded, Never Auto-Indexed

If the index is missing or stale when the server starts or when a tool is called, the server MUST serve a degraded response with an appropriate warning rather than triggering an index run automatically. The server MUST NOT auto-index under any circumstance (proposal Confirmed Decision 2).

#### Scenario: No prior index

- GIVEN a project root with no completed `codekurve index` run
- WHEN a query tool is called
- THEN the server returns a response indicating the index is missing/empty, without starting an index run

#### Scenario: Stale index is served, not rebuilt

- GIVEN stored freshness metadata shows pending files
- WHEN a query tool is called
- THEN the server answers from the existing (stale) index data with the stale warning set, and does not trigger reindexing on its own

### Requirement: reindex Gated Off by Default

`reindex` MUST NOT be registered as an available tool unless `[mcp] allow_reindex = true` is set in project configuration. When disabled (the default), the tool list MUST NOT include `reindex` and calling a tool named `reindex` MUST fail as an unknown tool.

#### Scenario: reindex absent by default

- GIVEN a project with no `[mcp]` configuration
- WHEN the server starts and a client lists tools
- THEN `reindex` does not appear in the tool list

#### Scenario: reindex enabled via config

- GIVEN `[mcp] allow_reindex = true` in the project's config
- WHEN the server starts and a client lists tools
- THEN `reindex` appears in the tool list and, when called, triggers an index run

### Requirement: doctor Tool

`doctor` MUST expose the same diagnostic checks as the CLI `doctor` command (schema/version compatibility, index integrity, config validity) as tool output, without printing to stdout outside the JSON-RPC response.

#### Scenario: doctor reports a healthy project

- GIVEN a project with a valid, current-schema index
- WHEN `doctor` is called
- THEN the response reports each check as passing

### Requirement: AGENT_USAGE.md Documents the §28.4 Rules

`docs/AGENT_USAGE.md` MUST document the following eight rules and the client installation steps for connecting to `codekurve mcp` over stdio (§28.4):

1. Query CodeKurve before doing a broad exploration.
2. Use direct text search when looking for a literal string.
3. Verify current source before editing.
4. Do not trust low-confidence edges for critical changes.
5. Use `trace_path` for flows.
6. Use `analyze_impact` as a candidate list, not a guarantee.
7. After large changes, wait for the watcher or run reindex.
8. If a response says stale, read the current file.

#### Scenario: Docs present and complete

- GIVEN `docs/AGENT_USAGE.md` exists in the repository
- WHEN it is reviewed
- THEN it lists all eight rules above and documents how to configure a client (e.g. Claude Code, Codex) to launch `codekurve mcp` over stdio
