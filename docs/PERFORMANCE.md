# Performance

## Benchmark method

Measured on documented hardware/OS/storage, minimum 5 runs, report median
and p95, separate cold vs warm cache, no comparison to external tools
without a reproducible methodology (plan §33.4, §36).

## Baselines

**No benchmarks have been run yet.** Numbers below are budget targets from
the plan, not measured results — do not treat them as achieved (plan
§0.14).

## Budgets (targets, plan §33)

| Fixture | Size | Cold index | Notes |
|---|---|---|---|
| Small | 100 files / 10k LOC | < 1s | search p95 < 25ms, callers p95 < 50ms |
| Medium | 1,000 files / 100k–250k LOC | < 8s | search p95 < 50ms, peak memory < 750MB |
| Large | 10,000 files / 1M+ LOC | < 90s | no OOM on 8GB machine |

Full detail: plan §33.

## Deferred decision

`[profile.release]` tuning (LTO, codegen-units, etc., plan §38) is not
configured in Phase 0; revisit once real workloads exist to measure
against.
