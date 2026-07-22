#!/usr/bin/env python3
"""Fail CI if any OSS licensing artifact appears in the repo.

CodeKurve's licensing status is intentionally undecided (see plan §40 and
docs/LICENSING.md). This script enforces that no license has been added by
accident: no `license`/`license-file` key in any Cargo.toml, no
`SPDX-License-Identifier` header, no OSS license badge/declaration in
README/docs, and no LICENSE/COPYING file or LICENSES/ directory. Prose that
merely discusses licensing as pending (e.g. "Licensing has not been
finalized") must NOT trigger a failure — only actual license artifacts do.

Stdlib only, runs identically on Ubuntu/Windows/macOS.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
EXCLUDED_DIRS = {".git", "target"}
# This script itself references the SPDX marker string and license names as
# part of implementing the checks below — exclude it from its own scan.
SELF_PATH = Path(__file__).resolve()

CARGO_LICENSE_KEY_RE = re.compile(r"^\s*license(-file)?\s*=")
SPDX_MARKER = "SPDX-License-Identifier"
LICENSE_BADGE_RE = re.compile(r"shields\.io.*licen[cs]e", re.IGNORECASE)
LICENSE_NAME_RE = re.compile(r"\b(MIT|Apache(?:-2\.0)?|GPL(?:v[23])?|BSD(?:-[23]-Clause)?)\b")
LICENSE_DECLARATION_TRIGGER_RE = re.compile(
    r"licensed under|released under|distributed under|license\s*:", re.IGNORECASE
)
LICENSE_FILE_NAME_RE = re.compile(r"^(license|copying)($|[._-])", re.IGNORECASE)


def iter_files(root: Path):
    for path in root.rglob("*"):
        if any(part in EXCLUDED_DIRS for part in path.relative_to(root).parts):
            continue
        if path.resolve() == SELF_PATH:
            continue
        yield path


def read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (UnicodeDecodeError, OSError):
        return None


def check_cargo_license_keys(root: Path) -> list[str]:
    hits = []
    for path in iter_files(root):
        if path.name != "Cargo.toml":
            continue
        text = read_text(path)
        if text is None:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if CARGO_LICENSE_KEY_RE.match(stripped):
                hits.append(f"{path.relative_to(root)}:{lineno}: {stripped}")
    return hits


def check_spdx_headers(root: Path) -> list[str]:
    hits = []
    for path in iter_files(root):
        if not path.is_file():
            continue
        text = read_text(path)
        if text is None or SPDX_MARKER not in text:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            if SPDX_MARKER in line:
                hits.append(f"{path.relative_to(root)}:{lineno}: {line.strip()}")
    return hits


def _doc_md_files(root: Path):
    candidates = list(root.glob("README*.md")) + list(root.glob("CONTRIBUTING*.md"))
    docs_dir = root / "docs"
    if docs_dir.is_dir():
        candidates += list(docs_dir.rglob("*.md"))
    return candidates


def check_license_badges_or_declarations(root: Path) -> list[str]:
    hits = []
    for path in _doc_md_files(root):
        text = read_text(path)
        if text is None:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            if LICENSE_BADGE_RE.search(line):
                hits.append(f"{path.relative_to(root)}:{lineno}: license badge: {line.strip()}")
                continue
            if LICENSE_DECLARATION_TRIGGER_RE.search(line) and LICENSE_NAME_RE.search(line):
                hits.append(
                    f"{path.relative_to(root)}:{lineno}: license declaration: {line.strip()}"
                )
    return hits


def check_license_files(root: Path) -> list[str]:
    hits = []
    for path in iter_files(root):
        rel = path.relative_to(root)
        if path.is_dir() and path.name.upper() == "LICENSES":
            hits.append(f"{rel}/ (LICENSES directory)")
        elif path.is_file() and LICENSE_FILE_NAME_RE.match(path.name):
            hits.append(str(rel))
    return hits


def main() -> int:
    checks = [
        ("Cargo.toml license/license-file key", check_cargo_license_keys),
        ("SPDX-License-Identifier header", check_spdx_headers),
        ("license badge or declaration in README/docs", check_license_badges_or_declarations),
        ("LICENSE/COPYING file or LICENSES/ directory", check_license_files),
    ]

    all_hits: list[tuple[str, list[str]]] = []
    for label, check in checks:
        hits = check(REPO_ROOT)
        if hits:
            all_hits.append((label, hits))

    if all_hits:
        print("licensing check FAILED — licensing must remain undecided (plan §40):")
        for label, hits in all_hits:
            print(f"\n  {label}:")
            for hit in hits:
                print(f"    - {hit}")
        return 1

    print("licensing check passed — no licensing artifacts found.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
