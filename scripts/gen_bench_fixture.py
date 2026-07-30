#!/usr/bin/env python3
"""Generate a deterministic synthetic TS/C# tree for benchmarking `codekurve index`.

Fixtures are generated at runtime rather than committed (design PR3, D13):
`fixtures/bench/` is gitignored. Each tier is a fixed file count (100/1000/
10000) with a realistic-ish call/inherit density: every file exports a class
that extends a class from a "previous" file (chain of inheritance across the
tree) and a function that calls a function from another file, so discovery +
extraction + resolution all do real work, not just parse empty files.

Stdlib only, matches the `scripts/check_licensing.py` convention.
"""

from __future__ import annotations

import argparse
import random
import shutil
from pathlib import Path

TIERS = {"small": 100, "medium": 1_000, "large": 10_000}

# 70% TypeScript, 30% C# — matches the two languages this bench exercises.
TS_RATIO = 0.7

TS_TEMPLATE = """import {{ Base{base} }} from "./file{base:05d}";

export class Base{n} extends Base{base} {{
  value(): number {{
    return {n};
  }}
}}

export function helper{n}(): number {{
  return helper{callee}() + {n};
}}

function helper{callee}(): number {{
  return {callee};
}}
"""

TS_ROOT_TEMPLATE = """export class Base{n} {{
  value(): number {{
    return {n};
  }}
}}

export function helper{n}(): number {{
  return {n};
}}
"""

CS_TEMPLATE = """namespace Bench;

public class Base{n} : Base{base}
{{
    public int Value() => {n};

    public int Helper{n}() => Helper{callee}() + {n};

    private int Helper{callee}() => {callee};
}}
"""

CS_ROOT_TEMPLATE = """namespace Bench;

public class Base{n}
{{
    public int Value() => {n};

    public int Helper{n}() => {n};
}}
"""


def gen_file(n: int, is_ts: bool) -> str:
    if n == 0:
        return TS_ROOT_TEMPLATE.format(n=n) if is_ts else CS_ROOT_TEMPLATE.format(n=n)
    base = n - 1
    callee = max(0, n - 2)
    tmpl = TS_TEMPLATE if is_ts else CS_TEMPLATE
    return tmpl.format(n=n, base=base, callee=callee)


def generate(tier: str, out_dir: Path, seed: int = 1234) -> int:
    count = TIERS[tier]
    rng = random.Random(seed)

    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True)

    for n in range(count):
        is_ts = rng.random() < TS_RATIO
        ext = "ts" if is_ts else "cs"
        content = gen_file(n, is_ts)
        (out_dir / f"file{n:05d}.{ext}").write_text(content, encoding="utf-8")

    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tier", choices=sorted(TIERS), required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=1234)
    args = parser.parse_args()

    count = generate(args.tier, args.out, args.seed)
    print(f"generated {count} files for tier '{args.tier}' at {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
