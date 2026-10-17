#!/usr/bin/env python3
"""Benchmark MCP query latency against an indexed project.

Measures what an agent actually waits for: one long-lived `codekurve mcp`
process (session already open) answering many `tools/call` requests over
stdio. Process start-up is excluded; each sample is one request/response
round trip. Reports median, p95 and max per tool, per docs/PERFORMANCE.md's
method (median + p95).

By default it generates and indexes a synthetic tier (see
`gen_bench_fixture.py`). `--root` benchmarks an existing, already-indexed
project instead; pass `--symbols` then, since the synthetic `BaseN` names
only exist in the fixture.

Stdlib only, matches the `scripts/check_licensing.py` convention.
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))
from bench import find_binary, percentile  # noqa: E402
from gen_bench_fixture import TIERS, generate  # noqa: E402


class McpClient:
    """Minimal newline-delimited JSON-RPC client for `codekurve mcp`."""

    def __init__(self, binary: Path, root: Path) -> None:
        self.proc = subprocess.Popen(
            [str(binary), "mcp", "--root", str(root)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self.request(
            "initialize",
            {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "codekurve-bench", "version": "0.0.0"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, message: dict) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        request_id = self.next_id
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        assert self.proc.stdout is not None
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise SystemExit("codekurve mcp closed stdout unexpectedly")
            message = json.loads(line)
            if message.get("id") == request_id:
                if "error" in message:
                    raise SystemExit(f"{method} failed: {message['error']}")
                return message["result"]

    def call(self, tool: str, arguments: dict) -> tuple[float, dict]:
        start = time.perf_counter()
        result = self.request("tools/call", {"name": tool, "arguments": arguments})
        elapsed = time.perf_counter() - start
        if result.get("isError"):
            raise SystemExit(f"{tool}({arguments}) returned an error: {result}")
        return elapsed, json.loads(result["content"][0]["text"])

    def close(self) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def prepare_fixture(binary: Path, tier: str) -> Path:
    root = REPO_ROOT / "fixtures" / "bench" / tier
    generate(tier, root)
    for command in (["init", str(root)], ["index", "--root", str(root)]):
        subprocess.run([str(binary), *command], check=True, capture_output=True, text=True)
    return root


def workload(symbols: list[str], calls: int, rng: random.Random) -> dict[str, list[dict]]:
    """Per-tool argument lists. `trace_path` aims a few hops down the
    synthetic `BaseN extends BaseN-1` chain so it finds a path; on a real
    project it simply reports `path_found: false`, which still exercises the
    full traversal."""
    picks = [rng.choice(symbols) for _ in range(calls)]

    def trace_args(name: str) -> dict:
        target = rng.choice(symbols)
        if name.startswith("Base") and name[4:].isdigit():
            target = f"Base{max(0, int(name[4:]) - 5)}"
        return {"symbol_name": name, "to": target}

    return {
        "codekurve_search_symbols": [{"query": name} for name in picks],
        "codekurve_find_callers": [{"symbol_name": name} for name in picks],
        "codekurve_analyze_impact": [{"symbol_name": name} for name in picks],
        "codekurve_trace_path": [trace_args(name) for name in picks],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--tier", choices=sorted(TIERS))
    source.add_argument("--root", type=Path, help="existing, already-indexed project")
    parser.add_argument("--symbols", nargs="+", help="symbol names to query (with --root)")
    parser.add_argument("--calls", type=int, default=50, help="samples per tool")
    parser.add_argument("--seed", type=int, default=1234)
    parser.add_argument("--keep", action="store_true", help="keep the generated fixture")
    args = parser.parse_args()

    binary = find_binary()
    rng = random.Random(args.seed)
    if args.root:
        if not args.symbols:
            parser.error("--root needs --symbols")
        root, symbols = args.root.resolve(), args.symbols
    else:
        root = prepare_fixture(binary, args.tier)
        count = TIERS[args.tier]
        # Skip the chain's first few links so `trace_path` always has a
        # target 5 hops away.
        symbols = [f"Base{n}" for n in range(min(10, count - 1), count)]

    client = McpClient(binary, root)
    try:
        _, status = client.call("codekurve_project_status", {})
        data = status["result"]
        print(
            f"project: files={data.get('files')} symbols={data.get('symbols')} "
            f"relationships={data.get('relationships')}"
        )
        # One untimed call per tool so SQLite's page cache is warm for all.
        plan = workload(symbols, args.calls, rng)
        for tool, arg_list in plan.items():
            client.call(tool, arg_list[0])
        for tool, arg_list in plan.items():
            samples = [client.call(tool, arguments)[0] * 1000 for arguments in arg_list]
            print(
                f"{tool:<28} n={len(samples):<4} "
                f"median={statistics.median(samples):7.2f}ms "
                f"p95={percentile(samples, 0.95):7.2f}ms max={max(samples):7.2f}ms"
            )
    finally:
        client.close()
        if args.tier and not args.keep:
            shutil.rmtree(root, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
