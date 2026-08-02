#!/usr/bin/env python3
"""Fail CI if the repo's licensing declaration is missing or inconsistent.

CodeKurve is MIT-licensed (see LICENSE and docs/LICENSING.md). Until
2026-08-02 this script enforced the *opposite* invariant — that licensing
stay undecided (plan §40) — and failed on any license artifact at all. That
guard did its job through the undecided period; once MIT was deliberately
chosen it became a false alarm, so it is inverted here rather than deleted:
the failure mode worth guarding is no longer "a license appeared by
accident" but "the declaration drifted out of sync."

Checks:

1. A LICENSE file exists at the repo root and is non-empty.
2. The root `Cargo.toml`'s `[workspace.package]` declares a `license` key.
3. That declared SPDX id actually matches the LICENSE file's own text — a
   Cargo.toml saying MIT over an Apache-2.0 LICENSE is worse than either
   alone.
4. Every workspace member crate declares a license, whether inherited
   (`license.workspace = true`) or its own. A new crate added without one
   would otherwise ship undeclared.

Stdlib only, runs identically on Ubuntu/Windows/macOS.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"

# `license = "MIT"` / `license.workspace = true` / `license-file = "..."`.
LICENSE_KEY_RE = re.compile(r"^\s*license(-file)?\s*(\.\s*workspace\s*)?=")
WORKSPACE_LICENSE_RE = re.compile(r'^\s*license\s*=\s*"([^"]+)"')

# Distinctive phrases from each license body, used to confirm the LICENSE
# file really is what Cargo.toml claims. Only the licenses this project
# could plausibly carry are listed; an SPDX id absent here is reported as
# unverifiable rather than silently accepted.
LICENSE_BODY_MARKERS = {
    "MIT": "Permission is hereby granted, free of charge",
    "Apache-2.0": "Licensed under the Apache License, Version 2.0",
    "BSD-2-Clause": "Redistribution and use in source and binary forms",
    "BSD-3-Clause": "Redistribution and use in source and binary forms",
    "ISC": "Permission to use, copy, modify, and/or distribute this software",
}


def read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (UnicodeDecodeError, OSError):
        return None


def check_license_file(root: Path) -> tuple[list[str], str | None]:
    """LICENSE exists and is non-empty. Returns (problems, license_text)."""
    path = root / "LICENSE"
    if not path.is_file():
        return ["LICENSE file is missing from the repo root"], None
    text = read_text(path)
    if text is None:
        return ["LICENSE exists but could not be read as UTF-8"], None
    if not text.strip():
        return ["LICENSE exists but is empty"], None
    return [], text


def check_workspace_license(root: Path) -> tuple[list[str], str | None]:
    """`[workspace.package]` declares a license. Returns (problems, spdx)."""
    path = root / "Cargo.toml"
    text = read_text(path)
    if text is None:
        return ["root Cargo.toml could not be read"], None

    in_workspace_package = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        if stripped.startswith("["):
            in_workspace_package = stripped == "[workspace.package]"
            continue
        if in_workspace_package:
            match = WORKSPACE_LICENSE_RE.match(stripped)
            if match:
                return [], match.group(1)

    return [
        'root Cargo.toml [workspace.package] has no license key (expected e.g. license = "MIT")'
    ], None


def check_declaration_matches_file(spdx: str | None, license_text: str | None) -> list[str]:
    if spdx is None or license_text is None:
        return []  # Already reported by an earlier check; don't pile on.
    marker = LICENSE_BODY_MARKERS.get(spdx)
    if marker is None:
        return [
            f'Cargo.toml declares license = "{spdx}", which this check cannot verify '
            f"against LICENSE's text (known: {', '.join(sorted(LICENSE_BODY_MARKERS))}). "
            "Add its body marker to LICENSE_BODY_MARKERS if the id is intentional."
        ]
    if marker not in license_text:
        return [
            f'Cargo.toml declares license = "{spdx}" but LICENSE does not read like '
            f'{spdx} (expected to contain "{marker}")'
        ]
    return []


def check_member_crates(crates_dir: Path, root: Path) -> list[str]:
    if not crates_dir.is_dir():
        return [f"{crates_dir.relative_to(root)}/ directory is missing"]

    problems = []
    for manifest in sorted(crates_dir.glob("*/Cargo.toml")):
        text = read_text(manifest)
        if text is None:
            problems.append(f"{manifest.relative_to(root)}: could not be read")
            continue
        declares = any(
            LICENSE_KEY_RE.match(line.strip())
            for line in text.splitlines()
            if not line.strip().startswith("#")
        )
        if not declares:
            problems.append(
                f"{manifest.relative_to(root)}: no license key "
                "(add `license.workspace = true`)"
            )
    return problems


def main() -> int:
    file_problems, license_text = check_license_file(REPO_ROOT)
    workspace_problems, spdx = check_workspace_license(REPO_ROOT)

    checks = [
        ("LICENSE file", file_problems),
        ("workspace license declaration", workspace_problems),
        (
            "declaration matches LICENSE text",
            check_declaration_matches_file(spdx, license_text),
        ),
        ("per-crate license declaration", check_member_crates(CRATES_DIR, REPO_ROOT)),
    ]

    failures = [(label, problems) for label, problems in checks if problems]

    if failures:
        print("licensing check FAILED — licensing declaration is missing or inconsistent:")
        for label, problems in failures:
            print(f"\n  {label}:")
            for problem in problems:
                print(f"    - {problem}")
        return 1

    print(f"licensing check passed — {spdx}, declared consistently across the workspace.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
