# 0005. No network access, no telemetry

## Context

CodeKurve indexes and reads potentially sensitive source code locally. See
CODEKURVE_MASTER_PLAN.md §5.1 (local-first, non-negotiable) and §29.4
(network denial: the application must not depend on an HTTP client).

## Decision

CodeKurve makes no outbound network requests and collects no telemetry, in
any mode. No HTTP client crate is added to the workspace without a new ADR
explicitly superseding this one.

> **Scoped, not superseded, by ADR 0012**
> (`0012-update-via-install-script.md`): `codekurve update` and
> `codekurve uninstall --binary` spawn the published install script, which is
> what downloads or deletes the release binary. CodeKurve itself still has no
> HTTP client and makes no network call from Rust, and no analysis path
> (`index`, `watch`, `mcp`, `tui`, any query) reaches the network or spawns a
> subprocess. This ADR stands.

## Alternatives

- **Opt-in telemetry**: rejected — even opt-in telemetry requires a network
  client dependency and consent/config machinery the MVP does not need, and
  risks silent scope creep into a "phone home" default.
- **Update checks / crate registry pings**: rejected for the same reason;
  version checks are a manual, out-of-band concern (e.g. release notes),
  not runtime behavior.

## Consequences

- Dependency review must flag any new network-capable crate (e.g. `reqwest`,
  `hyper`, `ureq`) as a violation requiring an ADR before merge.
- No crash reporting, usage analytics, or remote logging in Phase 0/1.
- `docs/SECURITY_MODEL.md` and `CONTRIBUTING.md` restate this policy for
  contributors; this ADR is the binding record.

## Status

Accepted (2026-07-22)
