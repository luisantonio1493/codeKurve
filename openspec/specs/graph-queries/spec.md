# Graph Queries Specification

## Purpose

Six hand-rolled CLI commands (no clap) that query the relationship graph built by `relationship-graph`: references, callers, callees, implementations, trace, impact (plan §26, §27, §50).

## Requirements

### Requirement: Six Graph Query Commands

The CLI MUST expose `references`, `callers`, `callees`, `implementations`, `trace`, and `impact`, each supporting `--json`, `--root`, `--limit`, `--offset`, and, where applicable, `--depth` and `--min-confidence` (§27.2). Each command's query logic MUST live in a reusable library function that returns structured data rather than printing directly; the CLI command MUST call that function and then format/print its result, and MUST NOT duplicate the query logic itself. Any consumer of the same library function (including the MCP server) MUST observe identical query results for identical inputs.

#### Scenario: Callers of a symbol

- GIVEN an indexed project where `MemberService.getEligibility` is called from two files
- WHEN `codekurve callers --symbol-id <id>` runs
- THEN the command returns both call sites with each edge's confidence and provenance

#### Scenario: Min-confidence filter

- GIVEN a symbol with both `Exact` and `Low` confidence callers
- WHEN `codekurve callers --symbol-id <id> --min-confidence high` runs
- THEN only `Exact`/`High` confidence callers are returned

#### Scenario: CLI output is unchanged after the extraction

- GIVEN the same project and query, run once before and once after the query logic is extracted into the shared library layer
- WHEN `codekurve callers --symbol-id <id>` runs both times (with and without `--json`)
- THEN stdout, formatting, and exit code are byte-for-byte identical between the two runs

#### Scenario: Library function reused by another consumer

- GIVEN the shared library function backing `callers` is called directly with the same symbol id and options as a CLI invocation
- WHEN both the CLI command and the direct library call run against the same index
- THEN they return the same structured result (same rows, confidence, provenance, truncation, total count)

### Requirement: Unresolved References Command

The CLI MUST expose `unresolved [<target-text>]`, supporting `--symbol-id`/`--symbol-name`, `--limit`, `--offset`, `--json`, and `--root`, listing the references the analyzer recorded but declined to resolve into edges, each with the `reason` it recorded. Unlike the six graph query commands it MUST NOT require a subject symbol: with no filter it lists the whole project, paginated. `<target-text>` MUST be matched exactly. Its query logic MUST live in the same reusable library layer, shared with the MCP `find_unresolved` tool. The command MUST NOT alter analyzer behaviour or synthesize an edge for any row it reports.

#### Scenario: Unresolved reference reports its reason

- GIVEN an indexed project containing a reference the analyzer could not resolve
- WHEN `codekurve unresolved <target-text>` runs
- THEN the matching row is printed with its source, target text, relationship kind, confidence, source path, and the recorded reason

#### Scenario: Nothing unresolved

- GIVEN an indexed project where every reference resolved
- WHEN `codekurve unresolved` runs
- THEN the command exits 0 with an empty result, not an error

### Requirement: Ambiguous Symbol Resolution Exits 6

If a query targets a symbol by name and multiple symbols match, the command MUST exit code 6, list every candidate with its qualified name, and MUST NOT silently pick the first match (§27.4).

#### Scenario: Ambiguous name lookup

- GIVEN two symbols both named `getEligibility` in different classes
- WHEN `codekurve references getEligibility` runs (no `--symbol-id`)
- THEN the command exits with code 6 and prints both candidates, instructing the user to pass `--symbol-id` or a qualified name

#### Scenario: Qualified name disambiguates

- GIVEN the same ambiguous name
- WHEN the command is run with the full qualified name instead
- THEN it resolves to exactly one symbol and exits 0

### Requirement: Missing Index Exits 4

Any of the six commands run against a project with no completed index run MUST exit code 4 with a message directing the user to run `codekurve index`.

#### Scenario: Query before first index

- GIVEN a project directory with no prior `codekurve index` run
- WHEN `codekurve trace --from A --to B` runs
- THEN the command exits code 4 and does not attempt a query

### Requirement: Trace Path Traversal

`trace` MUST perform bounded BFS between two symbols honoring `--depth`, `allowed_edge_types`, and `min_confidence`, and MUST set `truncated: true` with a `reason` when any limit (depth, nodes, edges, time budget) is hit (§26.4, §50.3).

#### Scenario: Path found within depth

- GIVEN symbols A and B connected by 2 calls edges
- WHEN `codekurve trace --from A --to B --depth 5` runs
- THEN the command returns the path with `truncated: false`

#### Scenario: Depth limit exceeded

- GIVEN the shortest path between A and B requires 6 hops
- WHEN `codekurve trace --from A --to B --depth 3` runs
- THEN no path is returned and the result reports `truncated: true`, `reason: max_depth`

### Requirement: Impact Analysis

`impact` MUST perform bounded reverse graph traversal from a symbol, labeling results "potential impact" (never guaranteed), grouping by file/module, and stating why each node is included (§26.5).

#### Scenario: Reverse dependency chain

- GIVEN `IMemberService` is implemented by `MemberService`, which is injected by `EligibilityController`
- WHEN `codekurve impact --symbol-id <IMemberService-id>` runs
- THEN the result includes both nodes with an explanation of the edge kind connecting each to the target

#### Scenario: Impact truncation

- GIVEN a symbol whose reverse graph exceeds the configured max-nodes limit
- WHEN `impact` runs
- THEN the result includes `truncated: true` and the partial result set, never a silently incomplete answer presented as complete

### Requirement: Versioned JSON Output

With `--json`, every command MUST emit the §27.5 envelope: `schema_version`, `project`, `result`, `warnings`, `truncated`.

#### Scenario: JSON envelope shape

- GIVEN any successful query with `--json`
- WHEN the command completes
- THEN stdout is a single JSON object containing all five envelope fields

### Requirement: Stale Index Warning on Stderr

If stored freshness metadata (`pending_files` from the `incremental-index` capability) shows one or more pending files when a query command runs, the command MUST print exactly one warning line to stderr noting the index may be stale, without performing a filesystem walk to check. Stdout content (including the `--json` envelope) and the command's exit code MUST be unaffected by this warning.

#### Scenario: Stale warning printed alongside normal output

- GIVEN 3 files are pending per stored freshness metadata
- WHEN `codekurve callers --symbol-id <id>` runs
- THEN stdout contains the normal query result (unaffected by staleness) and stderr contains exactly one line warning that the index has pending changes

#### Scenario: No warning when index is fresh

- GIVEN 0 files are pending per stored freshness metadata
- WHEN any of the six query commands runs
- THEN no staleness warning is printed to stderr

#### Scenario: Warning does not change exit codes

- GIVEN 3 files are pending and a query would otherwise exit 0
- WHEN the query command runs
- THEN it still exits 0; the staleness warning affects stderr only, never the exit code or stdout JSON structure
