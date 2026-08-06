#!/usr/bin/env python3
"""Compare Codex exploration with and without the CodeKurve MCP server.

The runner intentionally refuses to estimate tokens. A Codex JSONL stream
must contain real input and output token counts for every measured run.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import re
import shutil
import statistics
import subprocess
import sys
import time
from collections import defaultdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
BENCH_ROOT = REPO_ROOT / "benchmarks" / "agent-context"
DEFAULT_LOCK = BENCH_ROOT / "corpora.lock.json"
DEFAULT_TASKS = BENCH_ROOT / "tasks.json"
DEFAULT_SCHEMA = BENCH_ROOT / "answer.schema.json"
SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")
READ_FIELDS = ("files_read", "read_files", "read_paths")
SNAPSHOT_EXCLUDED_DIRS = {
    ".codegraph",
    ".codekurve",
    ".git",
    ".vs",
    "__pycache__",
    "bin",
    "node_modules",
    "obj",
    "target",
}
SNAPSHOT_EXCLUDED_FILES = {".DS_Store"}


class Inconclusive(RuntimeError):
    """The run cannot support a token-saving claim."""


def load_json(path: Path) -> dict[str, Any]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise Inconclusive(f"cannot read {path}: {error}") from error


def find_codekurve_binary() -> Path:
    for path in (REPO_ROOT / "target/release/codekurve", REPO_ROOT / "target/debug/codekurve"):
        if path.is_file():
            return path
    raise Inconclusive(
        "CodeKurve binary not found; build it first with "
        "`cargo build --release -p codekurve-bin`"
    )


def git_output(repo: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), *args], text=True, capture_output=True
    )
    if completed.returncode:
        raise Inconclusive(f"git -C {repo} {' '.join(args)} failed: {completed.stderr.strip()}")
    return completed.stdout.strip()


def content_snapshot(repo: Path) -> str:
    """Hash a source tree without VCS or generated output.

    This is local-only: it reads bytes to calculate a SHA-256 and never sends
    those bytes anywhere. A changed source/configuration file changes the
    corpus identity and stops the run before model calls begin.
    """
    paths = []
    for path in repo.rglob("*"):
        relative = path.relative_to(repo)
        if any(part in SNAPSHOT_EXCLUDED_DIRS for part in relative.parts):
            continue
        if path.name in SNAPSHOT_EXCLUDED_FILES:
            continue
        if path.is_symlink():
            raise Inconclusive(f"content snapshot refuses symlink: {relative}")
        if path.is_file():
            paths.append(path)
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(path.relative_to(repo).as_posix().encode("utf-8"))
        digest.update(b"\0")
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        digest.update(b"\0")
    return digest.hexdigest()


def resolve_corpora(lock: dict[str, Any], overrides: list[str]) -> dict[str, Path]:
    if any("=" not in item for item in overrides):
        raise Inconclusive("each --corpus override must use ID=PATH")
    override_paths = dict(item.split("=", 1) for item in overrides)
    resolved: dict[str, Path] = {}
    for corpus in lock["corpora"]:
        corpus_id = corpus["id"]
        raw_path = override_paths.get(corpus_id, corpus["default_path"])
        path = Path(raw_path).expanduser()
        if not path.is_absolute():
            path = REPO_ROOT / path
        path = path.resolve()
        if not path.is_dir():
            raise Inconclusive(f"corpus {corpus_id} is unavailable at {path}; benchmark never clones it")
        identity = corpus.get("identity", {"kind": "git_commit", "value": corpus.get("commit")})
        kind, expected = identity.get("kind"), identity.get("value")
        if kind == "git_commit":
            if not isinstance(expected, str) or not SHA_RE.fullmatch(expected):
                raise Inconclusive(f"corpus {corpus_id} has invalid pinned commit {expected!r}")
            head = git_output(path, "rev-parse", "HEAD")
            if not head.startswith(expected):
                raise Inconclusive(
                    f"corpus {corpus_id} is at {head}, expected pinned revision {expected}"
                )
        elif kind == "content_sha256":
            if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
                raise Inconclusive(f"corpus {corpus_id} has invalid content SHA-256 {expected!r}")
            actual = content_snapshot(path)
            if actual != expected:
                raise Inconclusive(
                    f"corpus {corpus_id} content changed ({actual}); expected snapshot {expected}"
                )
        else:
            raise Inconclusive(f"corpus {corpus_id} has unsupported identity kind {kind!r}")
        resolved[corpus_id] = path
    return resolved


def prepare_corpora(corpora: dict[str, Path], codekurve: Path) -> dict[str, float]:
    timings = {}
    for corpus_id, path in corpora.items():
        started = time.monotonic()
        completed = subprocess.run(
            [str(codekurve), "index", "--root", str(path)], text=True, capture_output=True
        )
        if completed.returncode:
            raise Inconclusive(
                f"preparation index failed for {corpus_id}: {completed.stderr.strip()}"
            )
        timings[corpus_id] = time.monotonic() - started
    return timings


def deterministic_schedule(tasks: list[dict[str, Any]], repetitions: int, seed: int) -> list[dict[str, Any]]:
    schedule = [
        {"corpus": task["corpus"], "task": task["id"], "arm": arm, "repeat": repeat}
        for task in tasks
        for arm in ("with", "without")
        for repeat in range(1, repetitions + 1)
    ]
    random.Random(seed).shuffle(schedule)
    return schedule


def codex_command(
    codex: str,
    corpus: Path,
    schema: Path,
    codekurve: Path,
    arm: str,
    prompt: str,
) -> list[str]:
    command = [
        codex,
        "exec",
        "--json",
        "--ephemeral",
        "--ignore-user-config",
        "--skip-git-repo-check",
        "--model",
        "gpt-5.6-sol",
        "--sandbox",
        "read-only",
        "--output-schema",
        str(schema),
        "--cd",
        str(corpus),
    ]
    if arm == "with":
        command.extend(
            [
                "-c",
                f"mcp_servers.codekurve.command={json.dumps(str(codekurve))}",
                "-c",
                "mcp_servers.codekurve.args="
                + json.dumps(["mcp", "--root", str(corpus)]),
            ]
        )
    command.append(prompt)
    return command


def assert_launch_isolation(command: list[str], arm: str) -> None:
    """Prove the invocation has no inherited MCP configuration."""
    if "--ignore-user-config" not in command:
        raise Inconclusive("benchmark command could inherit user MCP servers")
    codekurve_config = [part for part in command if part.startswith("mcp_servers.codekurve.")]
    if arm == "with" and len(codekurve_config) != 2:
        raise Inconclusive("with arm did not inject exactly the CodeKurve MCP server")
    if arm == "without" and codekurve_config:
        raise Inconclusive("without arm unexpectedly includes CodeKurve")


def _objects(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, dict):
        return [value, *sum((_objects(item) for item in value.values()), [])]
    if isinstance(value, list):
        return sum((_objects(item) for item in value), [])
    return []


def parse_jsonl(raw: str) -> dict[str, Any]:
    """Extract final answer, authoritative usage, and observed tool metadata."""
    events = []
    for line in raw.splitlines():
        if not line.strip():
            continue
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError as error:
            raise Inconclusive(f"Codex emitted invalid JSONL: {error}") from error

    final_answers: list[Any] = []
    usage_candidates: list[dict[str, Any]] = []
    tool_calls = 0
    file_reads: set[str] = set()
    context_windows: list[int] = []
    for event in events:
        for obj in _objects(event):
            if obj.get("type") in {"agent_message", "message"} and "text" in obj:
                final_answers.append(obj["text"])
            if obj.get("type") == "command_execution":
                tool_calls += 1
            for field in READ_FIELDS:
                values = obj.get(field)
                if isinstance(values, list) and all(isinstance(value, str) for value in values):
                    file_reads.update(values)
            usage = obj.get("usage")
            if isinstance(usage, dict) and {"input_tokens", "output_tokens"} <= usage.keys():
                usage_candidates.append(usage)
            window = obj.get("context_window")
            if isinstance(window, int):
                context_windows.append(window)

    if not usage_candidates:
        raise Inconclusive("Codex JSONL has no real input_tokens/output_tokens usage record")
    usage = usage_candidates[-1]
    if not all(isinstance(usage[key], int) and usage[key] >= 0 for key in ("input_tokens", "output_tokens")):
        raise Inconclusive("Codex token usage is malformed")

    answer: Any = None
    for candidate in reversed(final_answers):
        if isinstance(candidate, str):
            try:
                answer = json.loads(candidate)
                break
            except json.JSONDecodeError:
                continue
        if isinstance(candidate, dict):
            answer = candidate
            break
    if answer is None:
        raise Inconclusive("Codex JSONL contains no schema-shaped final answer")

    return {
        "answer": answer,
        "input_tokens": usage["input_tokens"],
        "output_tokens": usage["output_tokens"],
        # cached_input_tokens is deliberately not added: it would double-count
        # tokens already represented by input_tokens on supported Codex events.
        "cached_input_tokens": usage.get("cached_input_tokens"),
        "total_tokens": usage["input_tokens"] + usage["output_tokens"],
        "tool_calls": tool_calls,
        "file_reads": len(file_reads) if file_reads else None,
        "max_input_tokens_per_turn": usage["input_tokens"],
        "context_residual": max(context_windows) - usage["input_tokens"] if context_windows else None,
    }


def validate_answer(answer: Any, expected: dict[str, Any]) -> tuple[bool, str | None]:
    if not isinstance(answer, dict) or not isinstance(answer.get("answer"), str):
        return False, "final answer does not match the required schema"
    evidence = answer.get("evidence")
    if not isinstance(evidence, list) or not all(isinstance(item, dict) for item in evidence):
        return False, "evidence must be an array of objects"
    if any(
        not all(isinstance(item.get(field), str) for field in ("path", "symbol", "relationship", "claim"))
        for item in evidence
    ):
        return False, "each evidence item needs string path, symbol, relationship, and claim"

    def matches(item: dict[str, Any], requirement: dict[str, str]) -> bool:
        return all(item.get(key) == value for key, value in requirement.items())

    for requirement in expected.get("required_evidence", []):
        if not any(matches(item, requirement) for item in evidence):
            return False, f"missing required evidence {requirement}"
    for forbidden in expected.get("forbidden_evidence", []):
        if any(matches(item, forbidden) for item in evidence):
            return False, f"contains forbidden evidence {forbidden}"
    return True, None


def make_prompt(task: dict[str, Any]) -> str:
    return (
        "Answer this repository-exploration question. Use the available tools, verify every "
        "claim from the checked-out source, and return only the required JSON schema. "
        "Do not modify files.\n\nQuestion:\n"
        + task["prompt"]
    )


def median(values: list[float | int]) -> float | None:
    return float(statistics.median(values)) if values else None


def summarize(records: list[dict[str, Any]]) -> dict[str, Any]:
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for record in records:
        grouped[(record["corpus"], record["task"])].append(record)
    task_results = []
    improved = 0
    correct_with = correct_without = total_with = total_without = 0
    for (corpus, task), rows in sorted(grouped.items()):
        by_arm = {arm: [row for row in rows if row["arm"] == arm] for arm in ("with", "without")}
        medians = {arm: median([row["total_tokens"] for row in arm_rows]) for arm, arm_rows in by_arm.items()}
        if medians["with"] is None or medians["without"] is None:
            raise Inconclusive(f"missing an arm for {corpus}/{task}")
        reduction = (medians["without"] - medians["with"]) / medians["without"]
        if reduction > 0:
            improved += 1
        for row in by_arm["with"]:
            total_with += 1
            correct_with += int(row["correct"])
        for row in by_arm["without"]:
            total_without += 1
            correct_without += int(row["correct"])
        task_results.append(
            {
                "corpus": corpus,
                "task": task,
                "median_tokens": medians,
                "token_reduction": reduction,
                "with_correct": sum(row["correct"] for row in by_arm["with"]),
                "without_correct": sum(row["correct"] for row in by_arm["without"]),
                "secondary_medians": {
                    metric: {
                        arm: median(
                            [row[metric] for row in arm_rows if row.get(metric) is not None]
                        )
                        for arm, arm_rows in by_arm.items()
                    }
                    for metric in ("tool_calls", "file_reads", "wall_seconds", "max_input_tokens_per_turn", "context_residual")
                },
            }
        )
    aggregate_with = median([record["total_tokens"] for record in records if record["arm"] == "with"])
    aggregate_without = median([record["total_tokens"] for record in records if record["arm"] == "without"])
    aggregate_reduction = (aggregate_without - aggregate_with) / aggregate_without
    with_rate = correct_with / total_with
    without_rate = correct_without / total_without
    if aggregate_reduction > 0 and improved >= 4 and with_rate >= without_rate:
        verdict = "ahorro_demostrado"
        if aggregate_reduction >= 0.25 and improved >= 5:
            verdict = "ahorro_fuerte"
    else:
        verdict = "inconcluso"
    return {
        "task_results": task_results,
        "aggregate": {
            "median_tokens": {"with": aggregate_with, "without": aggregate_without},
            "token_reduction": aggregate_reduction,
            "correctness": {"with": with_rate, "without": without_rate},
            "tasks_improved": improved,
            "verdict": verdict,
        },
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    lock, task_manifest = load_json(args.lock), load_json(args.tasks)
    schema = args.schema.resolve()
    if not schema.is_file():
        raise Inconclusive(f"output schema is missing: {schema}")
    codekurve = args.codekurve.resolve() if args.codekurve else find_codekurve_binary()
    if not codekurve.is_file():
        raise Inconclusive(f"CodeKurve binary is missing: {codekurve}")
    corpora = resolve_corpora(lock, args.corpus)
    preparation = {} if args.skip_prepare else prepare_corpora(corpora, codekurve)
    tasks = task_manifest["tasks"]
    unknown_corpora = {task.get("corpus") for task in tasks} - corpora.keys()
    if unknown_corpora:
        raise Inconclusive(f"task manifest refers to unknown corpora: {sorted(unknown_corpora)}")
    schedule = deterministic_schedule(tasks, args.runs, args.seed)
    task_index = {(task["corpus"], task["id"]): task for task in tasks}
    debug_dir = args.debug_dir.resolve() if args.debug_dir else None
    if debug_dir and debug_dir.is_relative_to(REPO_ROOT):
        raise Inconclusive("--debug-dir must be outside this Git checkout")
    if debug_dir:
        debug_dir.mkdir(parents=True, exist_ok=True)

    records = []
    for position, item in enumerate(schedule, start=1):
        task = task_index[(item["corpus"], item["task"])]
        command = codex_command(args.codex, corpora[item["corpus"]], schema, codekurve, item["arm"], make_prompt(task))
        assert_launch_isolation(command, item["arm"])
        started = time.monotonic()
        completed = subprocess.run(command, text=True, capture_output=True)
        elapsed = time.monotonic() - started
        if debug_dir:
            (debug_dir / f"{position:03d}-{item['corpus']}-{item['task']}-{item['arm']}.jsonl").write_text(
                completed.stdout, encoding="utf-8"
            )
        if completed.returncode:
            raise Inconclusive(f"Codex failed for {item}: {completed.stderr.strip()}")
        parsed = parse_jsonl(completed.stdout)
        correct, reason = validate_answer(parsed.pop("answer"), task["expected"])
        records.append({**item, **parsed, "correct": correct, "incorrect_reason": reason, "wall_seconds": elapsed})

    return {
        "cohort": {
            "codex_cli": "0.146.1",
            "model": "gpt-5.6-sol",
            "runs_per_arm": args.runs,
            "seed": args.seed,
            "corpus_identities": {
                corpus["id"]: corpus.get("identity", {"kind": "git_commit", "value": corpus.get("commit")})
                for corpus in lock["corpora"]
            },
        },
        "prepared_seconds": preparation,
        "summary": summarize(records),
        "runs": records,
        "raw_logs_retained": bool(debug_dir),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    parser.add_argument("--tasks", type=Path, default=DEFAULT_TASKS)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--codekurve", type=Path)
    parser.add_argument("--corpus", action="append", default=[], metavar="ID=PATH")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--seed", type=int, default=20260805)
    parser.add_argument("--skip-prepare", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--debug-dir", type=Path, help="outside-repo directory for raw JSONL")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    try:
        result = run(args)
    except Inconclusive as error:
        print(f"INCONCLUSIVE: {error}", file=sys.stderr)
        return 2
    output = args.output or BENCH_ROOT / "results" / f"{datetime.now(UTC):%Y%m%dT%H%M%SZ}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result["summary"]["aggregate"], sort_keys=True))
    print(f"summary: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
