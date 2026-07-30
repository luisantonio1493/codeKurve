# Release Packaging Specification

## Overview

Defines how Codekurve releases are audited, benchmarked, packaged, and installed for an internal pilot: dependency/license posture is checked in CI, performance is measured against reproducible synthetic fixtures (not aspirational targets), release artifacts are built for a defined platform matrix with SBOM/checksums, and distribution stays inside CI workflow artifacts (no public publish step in this phase). `codekurve install` wires the MCP server into supported local clients without manual JSON editing.

## Requirements

### Requirement: Dependency and License Audit Gates CI

CI MUST run `cargo-deny check` (advisories, licenses, bans) as a required job, replacing the current "Deferred" placeholder. Any advisory or license exception MUST be recorded in `deny.toml` with a written justification per exception; blanket/unjustified ignores MUST NOT be used. CI MUST fail if `cargo-deny check` fails and no matching justified exception exists.

#### Scenario: Clean dependency tree passes CI

- GIVEN a commit whose dependency tree has no unaddressed advisories and no disallowed licenses
- WHEN CI runs
- THEN the `cargo-deny` job passes

#### Scenario: New disallowed license fails CI without a justified exception

- GIVEN a commit that introduces a dependency under a license not in the allow-list
- WHEN CI runs and no matching exception exists in `deny.toml`
- THEN the `cargo-deny` job fails, blocking merge

#### Scenario: Justified exception is honored

- GIVEN a `deny.toml` entry allow-listing a specific advisory ID with a written justification
- WHEN CI runs against a dependency tree that trips only that advisory
- THEN the `cargo-deny` job passes

### Requirement: Benchmarks Run Against Reproducible Synthetic Fixture Tiers

`docs/PERFORMANCE.md` MUST report numbers measured against three synthetic fixture tiers (100, 1,000, and 10,000 files, generated at runtime rather than checked in as a real pilot repo mirror) using the documented methodology, together with the exact command needed to reproduce each measurement. Reported numbers MUST reflect actual measured results even when they miss a previously documented target; a missed target MUST NOT block this phase and MUST NOT be silently retuned to match reality — it is recorded as a follow-up.

#### Scenario: All three tiers are measured and documented

- GIVEN the benchmark runner and the 100/1k/10k synthetic fixture generators
- WHEN the documented benchmark command is run for each tier
- THEN `docs/PERFORMANCE.md` records a measured number for each of the three tiers plus the exact reproduction command

#### Scenario: A measured number misses its documented target

- GIVEN the 10k-file tier's measured indexing time exceeds the previously documented target in `docs/PERFORMANCE.md`
- WHEN the benchmark report is written
- THEN the miss is reported as-is (not hidden or retuned), and the phase's success criteria still treat this as acceptable, with optimization tracked as a separate follow-up change

#### Scenario: Large tier does not run on every PR

- GIVEN the 10k-file tier is significantly slower than the 100/1k tiers
- WHEN CI runs on a routine pull request
- THEN the 10k tier is not required per-PR (run locally or on a lower-frequency schedule instead), avoiding CI cost/flakiness from the largest fixture

### Requirement: Release Workflow Produces a Verifiable, CI-Internal Artifact Set

A CI release workflow MUST build binaries for macOS x64, macOS aarch64, Linux x64, and Windows x64, and MUST produce, per run: the binaries, a CycloneDX SBOM, a third-party license/NOTICE report, and a SHA-256 checksum file covering every produced binary. All artifacts MUST be published only as CI workflow artifacts (e.g., GitHub Actions artifacts); the workflow MUST NOT publish to any public release channel, public download URL, or package registry, and MUST NOT perform binary signing or notarization.

#### Scenario: A release run produces the full artifact set for all four platforms

- GIVEN a triggered release workflow run
- WHEN the workflow completes successfully
- THEN for each of the four platforms a binary exists, and the run also produces one SBOM, one license/NOTICE report, and one checksum file listing a SHA-256 digest for every binary

#### Scenario: Checksums validate the binaries

- GIVEN the checksum file from a release run
- WHEN each binary is hashed and compared against its listed digest
- THEN every digest matches

#### Scenario: No public distribution occurs

- GIVEN a completed release workflow run
- WHEN the run's outputs are inspected
- THEN no artifact is published to a public GitHub Release, public URL, or package registry — only CI workflow artifacts exist

### Requirement: `codekurve install` Auto-Wires Supported MCP Clients

`codekurve install <client>` MUST support exactly three clients: Claude Code, Cursor, and Codex CLI. For a supported client, it MUST locate that client's local MCP configuration file, back up the existing file (if present) before making any change, and rewrite/insert the Codekurve MCP server entry so the client can start the server without manual `.mcp.json` editing. For an unrecognized or unsupported client name, it MUST fail loudly with the exact list of supported clients and manual configuration instructions, and MUST make no filesystem changes.

#### Scenario: Installing into a client with no prior config

- GIVEN Claude Code is installed locally with no existing Codekurve MCP entry
- WHEN `codekurve install claude-code` runs
- THEN the client's config file is created/updated with a valid Codekurve MCP server entry, and no backup file is needed since nothing existed to overwrite

#### Scenario: Installing into a client with an existing config backs it up first

- GIVEN Cursor already has an MCP config file with unrelated entries
- WHEN `codekurve install cursor` runs
- THEN a backup of the original file is written before any modification, and the original file is then updated to include the Codekurve MCP server entry alongside the pre-existing unrelated entries

#### Scenario: Rollback is possible without git

- GIVEN `codekurve install codex-cli` has run and altered the Codex CLI config
- WHEN a user wants to undo the change on a machine with no git repository for that config
- THEN the backup file written during install can be used to restore the prior configuration

#### Scenario: Unsupported client fails loudly with manual instructions

- GIVEN a client name that is not one of Claude Code, Cursor, or Codex CLI
- WHEN `codekurve install <unsupported-client>` runs
- THEN the command exits with a non-zero status, an error naming the exact three supported clients, and manual configuration instructions, and no config file anywhere is modified
