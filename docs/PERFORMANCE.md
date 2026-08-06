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

## Agent-context benchmark (Codex)

This is a separate benchmark for the product claim that CodeKurve lowers an
agent's exploration cost. It does **not** infer savings from fewer tool calls.
`scripts/bench_agent_context.py` runs identical repository questions through
Codex CLI 0.146.1 with `gpt-5.6-sol`, five times per arm:

- **with** uses only the CodeKurve MCP server injected on the command line;
- **without** uses no MCP server;
- both use `--ephemeral --ignore-user-config --sandbox read-only`, the same
  prompt, checkout, output schema, and standard Codex tools.

The runner checks that CodeKurve is injected in the `with` command only. It
indexes each local checkout before measurements and reports preparation time
separately. It never clones or downloads a corpus.

The versioned corpus lock and questions live in
`benchmarks/agent-context/`. The Angular checkout is pinned to
`66665aa669b3ab466bb5945572685f11cb08f439`. The local-only C# corpus,
`iungo-provider-api`, is pinned with a deterministic SHA-256 source-tree
snapshot instead of Git. The snapshot excludes VCS metadata and generated
output (`bin`, `obj`, `node_modules`, and local indexes), and changes to an
included file stop the benchmark before a model call. It is never cloned,
pushed, or uploaded to GitHub or another source repository.

Override either local location when necessary:

```sh
cargo build --release -p codekurve-bin
python3 scripts/bench_agent_context.py \
  --corpus csharp-iungo-provider-api=/path/to/iungo-provider-api
```

### Measurements and decision rule

Primary cost is the real Codex JSONL `input_tokens + output_tokens`; cached
input is reported but never added again. A run is inconclusive if Codex does
not emit those real usage fields. Answers must satisfy the schema and all
required structured evidence (path, symbol, relationship), while containing
no forbidden evidence.

Secondary measurements are tool calls, explicit file-read metadata when
Codex provides it, wall time, maximum input tokens per turn, and context
residual when Codex emits a context-window field. Missing secondary telemetry
is recorded as unavailable, never estimated. As CodeGraph notes, lower
processed tokens do not by themselves prove lower resident context.

- **Ahorro demostrado**: lower aggregate median tokens, at least 4 of 6 tasks
  improve, and CodeKurve does not reduce correctness.
- **Ahorro fuerte**: at least 25% lower aggregate median, at least 5 of 6
  tasks improve, and correctness is equal or better.
- Every other result is **inconcluso**.

Generated summaries contain aggregate metrics only and are ignored under
`benchmarks/agent-context/results/`. Raw JSONL is discarded by default. To
retain it for diagnosis, pass `--debug-dir` pointing outside this repository;
it can contain prompts and must never be committed.

No real cohort has been run yet, so this document makes no savings claim.

## Deferred decision

`[profile.release]` tuning (LTO, codegen-units, etc., plan §38) is not
configured in Phase 0; revisit once real workloads exist to measure
against.
