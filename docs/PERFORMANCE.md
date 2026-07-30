# Performance

## Benchmark method

Measured on documented hardware/OS/storage, minimum 5 runs, report median
and p95, separate cold vs warm cache, no comparison to external tools
without a reproducible methodology (plan §33.4, §36).

## Fixtures

All three tiers are synthetic, seeded, and generated at runtime by
`scripts/gen_bench_fixture.py` (TS/C# files with a chain of inheritance and
cross-file calls, so discovery + extraction + resolution do real work). They
are never committed (`fixtures/bench/` is gitignored) — this keeps repo size
flat per the phase's "generate synthetically, don't mirror a real repo"
decision.

## Measured (2026-07-30)

Hardware: Apple M1 Pro, 16 GB RAM, macOS 26.5.2 (Darwin 25.5.0), APFS on
internal SSD. Rust 1.96.0 release build (`cargo build --release -p
codekurve-bin`), Python 3.14.5. 5 runs for small/medium, 3 runs for large
(see "Large tier is not run per-PR" below).

Reproduce with:

```
cargo build --release -p codekurve-bin
python3 scripts/bench.py --tier small --runs 5
python3 scripts/bench.py --tier medium --runs 5
python3 scripts/bench.py --tier large --runs 3
```

| Tier | Files | Cold median | Cold p95 | Warm median | Budget | Status |
|---|---|---|---|---|---|---|
| Small | 100 | 0.040s | 0.061s | 0.009s | < 1s | met |
| Medium | 1,000 | 0.283s | 0.295s | 0.014s | < 8s | met |
| Large | 10,000 | 3.440s | 3.899s | 0.075s | < 90s | met |

"Warm" is a second `codekurve index` run against the same `.codekurve/`
database with no file changes (incremental no-op path, spec "Index Skips
Files Classified Unchanged").

Peak-memory and search/callers p95 latency budgets from the table below are
not yet measured — no benchmark harness exists for those yet; tracked as a
follow-up, not required by this phase (only the index-time budget was in
scope here).

## Budgets (targets, plan §33)

| Fixture | Size | Cold index | Notes |
|---|---|---|---|
| Small | 100 files / 10k LOC | < 1s | search p95 < 25ms, callers p95 < 50ms |
| Medium | 1,000 files / 100k–250k LOC | < 8s | search p95 < 50ms, peak memory < 750MB |
| Large | 10,000 files / 1M+ LOC | < 90s | no OOM on 8GB machine |

Full detail: plan §33.

## Large tier is not run per-PR

The 10k-file tier is significantly slower to generate + index than the other
two. It is not part of routine CI; run it locally (as above) or on a
lower-frequency schedule instead, to avoid CI cost/flakiness from the
largest fixture (spec "Large tier does not run on every PR").

## Deferred decision

`[profile.release]` tuning (LTO, codegen-units, etc., plan §38) is not
configured in Phase 0; revisit once real workloads exist to measure
against.
