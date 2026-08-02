# Delta for Symbol Index

Phase 7 adds framework-role tags (`Controller`, `Route`, `Service`, `Repository`, `Component`, `Decorator`) on existing symbols — no new `SymbolKind` variant — plus an additive migration that stores them, per the proposal's § 17.2 role-tag list.

## ADDED Requirements

### Requirement: Framework-Role Tags Attach to Existing Symbols Without a New SymbolKind

A symbol MAY carry zero or more framework-role tags from the fixed set `Controller`, `Route`, `Service`, `Repository`, `Component`, `Decorator`, assigned by the `framework-awareness` recognition pass. Role tags MUST NOT introduce a new `SymbolKind` variant and MUST NOT change a symbol's `symbol_key` (role tags carry no bearing on identity).

#### Scenario: An Angular component symbol is role-tagged

- GIVEN a TypeScript class decorated `@Component({...})`
- WHEN the recognition pass runs
- THEN the class symbol carries the `Component` role tag, and its `symbol_key` is unchanged from before role tagging existed

#### Scenario: A symbol with no framework role carries no tag

- GIVEN a plain TypeScript utility function with no decorator
- WHEN indexed
- THEN the symbol carries zero framework-role tags

#### Scenario: A symbol can carry more than one role tag

- GIVEN a class that is both a controller and, per the catalogue's rules, independently recognized as a route handler owner
- WHEN role-tagged
- THEN both applicable tags are recorded on the same symbol without conflict

### Requirement: Schema Migration 0005 Adds Role-Tag Storage Without Wiping Data

Migration 0005 MUST add role-tag storage additively, without altering or wiping any existing table, and MUST bump `SCHEMA_VERSION` to 5. No pre-existing `symbol_key` MUST change as a result of this migration, since role tags do not participate in `symbol_key`.

(Previously: `SCHEMA_VERSION` was 4, with no framework-role-tag storage.)

#### Scenario: Migration applies to a populated index without wiping it

- GIVEN a project previously indexed under `SCHEMA_VERSION = 4`
- WHEN `codekurve index` first runs after upgrading
- THEN migration 0005 applies, existing rows survive, pre-0005 symbols carry zero role tags, and every pre-migration symbol id is unchanged

#### Scenario: Doctor reports post-migration schema version

- GIVEN a project indexed after migration 0005 applied
- WHEN `codekurve doctor` runs
- THEN it reports schema version 5

### Requirement: Role Tags Are Queryable

Framework-role tags MUST be retrievable through the existing symbol-query surface (e.g. `codekurve symbol`, MCP symbol lookups) alongside a symbol's other attributes, without requiring a separate dedicated command.

#### Scenario: A role-tagged symbol's tags appear in query output

- GIVEN an indexed symbol carrying the `Service` role tag
- WHEN it is queried via the existing symbol-lookup surface
- THEN its role tags are present in the returned symbol data
