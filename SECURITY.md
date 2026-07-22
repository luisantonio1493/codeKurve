# Security

## Posture

CodeKurve is designed local-first and network-free (plan §5.8, §29):

- No network access, no telemetry, no external service calls.
- Never executes analyzed code, package-manager scripts, or repository
  config as code.
- Respects `.gitignore` plus configurable additional exclusions.
- Does not follow symlinks by default; cannot escape the project root.
- Enforces file-size and total-file limits, timeouts, and memory budgets.

Full threat model and controls: `docs/SECURITY_MODEL.md`.

## Reporting a vulnerability

This project is not yet public. Report issues internally to the project
maintainer rather than opening a public issue. A public disclosure process
will be documented once the project and its licensing status (see
`docs/LICENSING.md`) are finalized.
