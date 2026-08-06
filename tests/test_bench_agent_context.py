#!/usr/bin/env python3
"""Stdlib tests for the agent-context benchmark runner."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("bench_agent_context", ROOT / "scripts/bench_agent_context.py")
assert SPEC and SPEC.loader
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)


class AgentContextBenchmarkTests(unittest.TestCase):
    def test_parse_jsonl_uses_real_usage_without_double_counting_cache(self) -> None:
        raw = (ROOT / "tests/fixtures/agent-context/codex-success.jsonl").read_text()
        parsed = bench.parse_jsonl(raw)
        self.assertEqual(parsed["input_tokens"], 120)
        self.assertEqual(parsed["output_tokens"], 30)
        self.assertEqual(parsed["total_tokens"], 150)
        self.assertEqual(parsed["cached_input_tokens"], 90)

    def test_parse_jsonl_fails_without_usage(self) -> None:
        with self.assertRaisesRegex(bench.Inconclusive, "input_tokens/output_tokens"):
            bench.parse_jsonl('{"type":"item.completed","item":{"type":"agent_message","text":"{}"}}\n')

    def test_deterministic_schedule_is_balanced_and_seeded(self) -> None:
        tasks = [{"id": "one", "corpus": "a"}, {"id": "two", "corpus": "b"}]
        first = bench.deterministic_schedule(tasks, 5, 7)
        self.assertEqual(first, bench.deterministic_schedule(tasks, 5, 7))
        self.assertEqual(len(first), 20)
        self.assertEqual(sum(item["arm"] == "with" for item in first), 10)
        self.assertNotEqual(first, bench.deterministic_schedule(tasks, 5, 8))

    def test_answer_validation_rejects_missing_and_forbidden_evidence(self) -> None:
        expected = {
            "required_evidence": [{"path": "a.cs", "symbol": "A", "relationship": "calls"}],
            "forbidden_evidence": [{"path": "b.cs", "symbol": "B", "relationship": "calls"}],
        }
        answer = {"answer": "ok", "evidence": [{"path": "a.cs", "symbol": "A", "relationship": "calls", "claim": "x"}]}
        self.assertEqual(bench.validate_answer(answer, expected), (True, None))
        answer["evidence"].append({"path": "b.cs", "symbol": "B", "relationship": "calls", "claim": "x"})
        self.assertFalse(bench.validate_answer(answer, expected)[0])

    def test_launch_isolation_allows_codekurve_only_in_with_arm(self) -> None:
        command = bench.codex_command("codex", Path("/repo"), Path("/schema"), Path("/bin"), "with", "prompt")
        bench.assert_launch_isolation(command, "with")
        command = bench.codex_command("codex", Path("/repo"), Path("/schema"), Path("/bin"), "without", "prompt")
        bench.assert_launch_isolation(command, "without")

    def test_content_snapshot_is_stable_and_ignores_generated_output(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            (root / "src").mkdir()
            (root / "src" / "Program.cs").write_text("class Program {}")
            baseline = bench.content_snapshot(root)
            (root / "obj").mkdir()
            (root / "obj" / "build.bin").write_bytes(b"generated")
            self.assertEqual(bench.content_snapshot(root), baseline)
            (root / "src" / "Program.cs").write_text("class ChangedProgram {}")
            self.assertNotEqual(bench.content_snapshot(root), baseline)

    def test_evaluation_requires_correctness_and_breadth(self) -> None:
        records = []
        for task in range(6):
            for arm, tokens in (("with", 70), ("without", 100)):
                records.append({"corpus": "c", "task": str(task), "arm": arm, "total_tokens": tokens, "correct": True})
        self.assertEqual(bench.summarize(records)["aggregate"]["verdict"], "ahorro_fuerte")
        records[0]["correct"] = False
        self.assertEqual(bench.summarize(records)["aggregate"]["verdict"], "inconcluso")

    @unittest.skipIf(
        sys.platform == "win32",
        "fake codex/codekurve fixtures are POSIX shebang scripts",
    )
    def test_integration_with_fake_codex(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            repo = root / "corpus"
            repo.mkdir()
            for command in (("git", "init"), ("git", "config", "user.email", "test@example.com"), ("git", "config", "user.name", "Test")):
                self.assertEqual(__import__("subprocess").run(command, cwd=repo).returncode, 0)
            (repo / "file.txt").write_text("fixture")
            self.assertEqual(__import__("subprocess").run(("git", "add", "."), cwd=repo).returncode, 0)
            self.assertEqual(__import__("subprocess").run(("git", "commit", "-m", "fixture"), cwd=repo).returncode, 0)
            sha = __import__("subprocess").check_output(("git", "rev-parse", "HEAD"), cwd=repo, text=True).strip()
            codekurve = root / "codekurve"
            codex = root / "codex"
            codekurve.write_text("#!/bin/sh\nexit 0\n")
            codex.write_text(
                "#!/usr/bin/env python3\n"
                "import json\n"
                "print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':json.dumps({'answer':'ok','evidence':[{'path':'file.txt','symbol':'F','relationship':'defines','claim':'fixture'}]})},'usage':{'input_tokens':10,'output_tokens':2}}))\n"
            )
            codekurve.chmod(0o755)
            codex.chmod(0o755)
            schema = root / "schema.json"
            schema.write_text("{}")
            lock = root / "lock.json"
            lock.write_text(json.dumps({"corpora": [{"id": "fixture", "commit": sha, "default_path": str(repo)}]}))
            tasks = root / "tasks.json"
            tasks.write_text(json.dumps({"tasks": [{"id": "task", "corpus": "fixture", "prompt": "fixture", "expected": {"required_evidence": [{"path": "file.txt", "symbol": "F", "relationship": "defines"}], "forbidden_evidence": []}}]}))
            output = root / "summary.json"
            status = bench.main if False else None  # exercise run() without global argv.
            args = __import__("argparse").Namespace(lock=lock, tasks=tasks, schema=schema, codex=str(codex), codekurve=codekurve, corpus=[], runs=1, seed=1, skip_prepare=False, debug_dir=None, output=output)
            result = bench.run(args)
            self.assertEqual(len(result["runs"]), 2)
            self.assertEqual(result["summary"]["aggregate"]["verdict"], "inconcluso")


if __name__ == "__main__":
    unittest.main()
