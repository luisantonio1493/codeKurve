# Delta for Graph Queries

Phase 3 adds a one-line stderr staleness warning to the six query commands when the stored freshness metadata shows pending files, without changing stdout content, JSON envelope shape, or exit codes (proposal: "stale warning on stderr; stdout/exit codes unchanged").

## ADDED Requirements

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
