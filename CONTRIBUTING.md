# Contributing

## Dev commands (4 gates, must pass before every commit)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -- version
```

## Architecture decisions

Significant decisions are recorded as ADRs in `docs/adr/` (plan §42), each
with Context / Decision / Alternatives / Consequences / Status.

## Dependency policy

- No new dependency without justifying purpose, license, maintenance
  status, binary-size cost, and security impact, plus the alternative
  considered (plan §0.5, §12).
- No network-capable crates (HTTP clients, sockets) without an ADR — the
  application must not depend on network I/O (plan §29.4).
- Prefer crates.io, well-maintained, permissively licensed dependencies.

## Commit expectations

- Conventional commits (`feat:`, `fix:`, `chore:`, `docs:`, `ci:`, ...).
- Small, reviewable, single-purpose changes.
- The workspace must keep compiling and passing the 4 gates above at every
  commit (plan §0.4, §0.13).

## Licensing

Licensing has not been finalized. Do not redistribute.
