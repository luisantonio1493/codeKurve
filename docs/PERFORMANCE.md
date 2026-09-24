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

Peak memory is not yet measured. Query latency is, below.

## Query latency (measured 2026-09-24)

`scripts/bench_queries.py` measures what an agent waits for: one long-lived
`codekurve mcp` process answering `tools/call` requests over stdio, process
start-up excluded, 50 samples per tool after one untimed warm-up call.

```
cargo build --release -p codekurve-bin
python3 scripts/bench_queries.py --tier large --calls 50
python3 scripts/bench_queries.py --root <indexed project> --symbols Foo Bar   # a real project
```

Hardware: Linux cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, 15 GB RAM
(not the M1 above, so compare within a table, not across). Large tier:
10,000 files, 52,987 symbols, 39,809 relationships.

| Tool (large tier) | Before: median / p95 | After: median / p95 |
|---|---|---|
| `search_symbols` | 63.0 / 70.8 ms | 0.51 / 0.66 ms |
| `find_callers` | 44.5 / 49.5 ms | 0.33 / 0.46 ms |
| `analyze_impact` | 92.8 / 109.8 ms | 0.49 / 1.21 ms |
| `trace_path` | 107.5 / 114.8 ms | 0.55 / 0.95 ms |

"Before" grew linearly with project size (medium tier: 4-8 ms); "after" is
flat across tiers. Three causes, all fixed:

1. **No planner statistics.** The index never ran `ANALYZE`, so SQLite
   assumed `project_id` was selective (a database holds one project). It
   picked `(project_id, kind)` over `target_symbol_id` for relationship
   lookups and walked every symbol for FTS search. Index writes now refresh
   statistics (`codekurve_store::db::refresh_planner_stats`); a full
   `ANALYZE` costs ~60 ms on the large tier, a no-change `index` run adds it
   to an older database.
2. **Every tool call counted whole tables.** The stale-index warning called
   `index_status` (four `COUNT(*)`s, one a full scan) to read one value; it
   now reads `pending_files` alone.
3. **`trace`/`impact`/`export` loaded every relationship first.** They now
   fetch each visited node's edges on demand (`traverse::LazyAdjacency`), so
   cost follows the BFS caps rather than the project size.

Caveat: the synthetic graph is sparse (~4 edges per file). Real projects are
denser, which made "before" worse, not "after": lookups stay indexed.

## Cost of the syntax-depth guard (measured 2026-09-24)

Every file's syntax tree is walked once more (iteratively) to reject trees
deeper than `MAX_SYNTAX_DEPTH` before the recursive extractors run (see
`docs/SECURITY_MODEL.md`). Interleaved A/B on the large tier, same Linux
container as the query table, 6 cold runs each: 3.59 s median before,
3.81 s after (+6 %). Accepted: the alternative was a process abort on a
crafted file. Skipping the check for small files was rejected because tree
depth is not safely bounded by file size.

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
