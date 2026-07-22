# Security Model

Full source: plan §29.

## Threat model

Indexing secrets, escaping the project root, symlink/path traversal,
oversized or malformed files, malicious grammars, excessive CPU/RAM,
database corruption, log injection, oversized MCP output, compromised
dependencies, accidental execution of repository code, and reading
sensitive generated artifacts (plan §29.1).

## Controls

Never execute analyzed code, package-manager scripts, or repository
config; never shell out to `npm`/`dotnet`/`cargo`; canonicalize paths;
symlinks off by default; max file size and total file count; timeouts and
cancellation; memory budgets; **no network**; structured logs with content
redaction; dependency audit and SBOM; release checksums (plan §29.2).

## No-network

The application does not depend on an HTTP client. See
`docs/ARCHITECTURE.md` and plan §29.4.

## Ignored files

Respects `.gitignore` plus configurable exclusions; sensitive-file patterns
(`.env`, `*.pem`, `*.key`, `credentials*`, ...) are excluded by default but
the policy is configurable, not hardcoded (plan §29.3).

## Storage

SQLite is local, no source-code duplication beyond what's required (plan
§0.8, §24).

## Update process

No auto-update in Phase 0; release checksums and future binary signing are
tracked in plan §29.2, not yet implemented.
