#!/usr/bin/env python3
"""Benchmark `codekurve index` against a synthetic fixture tier.

Generates the fixture via `gen_bench_fixture.py`, then runs `codekurve init`
+ `codekurve index` cold (fresh `.codekurve/`) and once more warm (same DB,
no file changes) `--runs` times, reporting median + p95 per
docs/PERFORMANCE.md's documented method.

Stdlib only, matches the `scripts/check_licensing.py` convention.
"""

from __future__ import annotations

import argparse
import shutil
import statistics
import subprocess
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys_path_helper = REPO_ROOT / "scripts"

import sys  # noqa: E402

sys.path.insert(0, str(sys_path_helper))
from gen_bench_fixture import TIERS, generate  # noqa: E402


def find_binary() -> Path:
    release = REPO_ROOT / "target" / "release" / "codekurve"
    debug = REPO_ROOT / "target" / "debug" / "codekurve"
    if release.exists():
        return release
    if debug.exists():
        return debug
    raise SystemExit(
        "codekurve binary not found; run `cargo build --release -p codekurve-bin` "
        "or `cargo build -p codekurve-bin` first"
    )


def run_index(binary: Path, root: Path) -> float:
    start = time.monotonic()
    subprocess.run(
        [str(binary), "index", "--root", str(root)],
        check=True,
        capture_output=True,
        text=True,
    )
    return time.monotonic() - start


def percentile(values: list[float], pct: float) -> float:
    values = sorted(values)
    idx = min(len(values) - 1, int(round(pct * (len(values) - 1))))
    return values[idx]


def bench_tier(tier: str, runs: int, keep: bool) -> dict:
    binary = find_binary()
    fixture_dir = REPO_ROOT / "fixtures" / "bench" / tier
    generate(tier, fixture_dir)

    cold_times = []
    for _ in range(runs):
        codekurve_dir = fixture_dir / ".codekurve"
        if codekurve_dir.exists():
            shutil.rmtree(codekurve_dir)
        subprocess.run(
            [str(binary), "init", "--root", str(fixture_dir)],
            check=True,
            capture_output=True,
            text=True,
        )
        cold_times.append(run_index(binary, fixture_dir))

    # Warm pass: same DB, no file changes, one more `index` invocation per run.
    warm_times = [run_index(binary, fixture_dir) for _ in range(runs)]

    if not keep:
        shutil.rmtree(fixture_dir, ignore_errors=True)

    return {
        "tier": tier,
        "files": TIERS[tier],
        "cold_median": statistics.median(cold_times),
        "cold_p95": percentile(cold_times, 0.95),
        "warm_median": statistics.median(warm_times),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tier", choices=sorted(TIERS), required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument(
        "--keep", action="store_true", help="keep the generated fixture dir after the run"
    )
    args = parser.parse_args()

    result = bench_tier(args.tier, args.runs, args.keep)
    print(
        f"tier={result['tier']} files={result['files']} "
        f"cold_median={result['cold_median']:.3f}s cold_p95={result['cold_p95']:.3f}s "
        f"warm_median={result['warm_median']:.3f}s"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
