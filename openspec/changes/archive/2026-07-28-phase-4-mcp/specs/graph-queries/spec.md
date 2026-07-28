# Delta for Graph Queries

Phase 4 extracts each query command's body into a reusable data-returning function in `codekurve`'s public library layer, so both the CLI and the new MCP server call the same logic. The CLI commands become a thin printing consumer of that shared layer; CLI stdout, exit codes, and JSON envelope shape are unchanged (proposal: "CLI printing becomes a consumer of the same data layer, CLI output itself unchanged").

## MODIFIED Requirements

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
