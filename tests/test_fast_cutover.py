"""Cutover proofs: the Python hot paths actually take the fast branches.

- tools: `WorkspaceTools` prefers `op-fast` (stub proves the branch,
  real tree proves parity with the old `rg`/walk rendering).
- patch: 200-case differential fuzz — Python `parse_agent_patch` and Rust
  `check_patch` agree on accept/reject for marker + add/delete classes.
  (Update-chunk structure stays Python-only by design.)
- wiki: `scan_wiki_fast` names/paths match `parse_index`.
- engine: a real two-subtask solve goes through `swarm_fast`
  (spy on the real `join_with_deadline`, no stubbed results).
"""
from __future__ import annotations

import random
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch as mock_patch

from conftest import _tc
from agent import op_fast
from agent.config import AgentConfig
from agent.engine import RLMEngine
from agent import engine as engine_module
from agent.model import ModelTurn, ScriptedModel
from agent.patching import PatchApplyError, parse_agent_patch
from agent.tools import WorkspaceTools
from agent.wiki_graph import parse_index, scan_wiki_fast


def _python_accepts(text: str) -> bool:
    try:
        parse_agent_patch(text)
        return True
    except PatchApplyError:
        return False


def _rust_accepts(text: str, tmp: Path) -> bool:
    p = tmp / "case.patch"
    p.write_text(text, encoding="utf-8")
    res = op_fast.patch_check(p)
    assert res is not None, "op-fast binary must be built for this test"
    return res[0]


class CutoverTests(unittest.TestCase):
    def test_tools_list_takes_op_fast_branch(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tools = WorkspaceTools(root=Path(tmpdir))
            import agent.tools as tools_module

            with mock_patch.object(
                tools_module._op_fast, "list_files", return_value=["sentinel.txt"]
            ) as m:
                out = tools.list_files()
            m.assert_called_once()
            self.assertIn("sentinel.txt", out)

    def test_tools_list_search_parity_real_tree(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            (root / "a.py").write_text("hello\n")
            (root / "sub").mkdir()
            (root / "sub" / "b.py").write_text("world\n")
            tools = WorkspaceTools(root=root)
            self.assertEqual(tools.list_files().splitlines(), ["a.py", "sub/b.py"])
            self.assertEqual(
                tools.search_files("hello").splitlines(), ["a.py:1:hello"]
            )

    def test_patch_fuzz_python_rust_agree(self) -> None:
        rng = random.Random(20261112)
        valid_add = "*** Begin Patch\n*** Add File: f{0}.txt\n+line{0}\n*** End Patch"
        valid_del = "*** Begin Patch\n*** Delete File: f{0}.txt\n*** End Patch"
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            for i in range(200):
                n = rng.randint(1, 3)
                ops: list[str] = []
                for _ in range(n):
                    k = rng.randint(0, 1)
                    if k == 0:
                        ops.append(f"*** Add File: f{rng.randint(0, 9)}.txt\n+data")
                    else:
                        ops.append(f"*** Delete File: f{rng.randint(0, 9)}.txt")
                text = "*** Begin Patch\n" + "\n".join(ops) + "\n*** End Patch"
                r = rng.random()
                if r < 0.2:
                    text = text.replace("*** Begin Patch\n", "", 1)
                elif r < 0.4:
                    text = text.rsplit("*** End Patch", 1)[0]
                elif r < 0.6 and "+data" in text:
                    text = text.replace("+data", "nodata", 1)
                py = _python_accepts(text)
                ru = _rust_accepts(text, tmp)
                self.assertEqual(
                    py, ru, f"case {i}: python={py} rust={ru}\n{text}"
                )

    def test_wiki_fast_parity(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            (root / "index.md").write_text(
                "### Campaign Finance\n"
                "| Foo Org | MA | [foo.md](foo.md) |\n"
                "| Bar Org | PA | [bar.md](bar.md) |\n",
                encoding="utf-8",
            )
            if op_fast.find_bin() is None:
                self.skipTest("op-fast binary not built")
            fast = scan_wiki_fast(root)
            self.assertIsNotNone(fast)
            slow = parse_index(root)
            self.assertEqual(
                sorted((e.name, e.rel_path) for e in fast or []),
                sorted((e.name, e.rel_path) for e in slow),
            )
            self.assertTrue(all(e.category == "campaign-finance" for e in fast or []))

    def test_engine_parallel_goes_through_swarm(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            cfg = AgentConfig(
                workspace=root, max_depth=2, max_steps_per_call=6,
                max_solve_seconds=300, recursive=True,
                acceptance_criteria=False,
            )
            tools = WorkspaceTools(root=root)
            done = ModelTurn(text="done", stop_reason="end_turn")
            model = ScriptedModel(
                scripted_turns=[
                    ModelTurn(tool_calls=[
                        _tc("subtask", objective="first"),
                        _tc("subtask", objective="second"),
                    ]),
                    done, done, done, done,
                ]
            )
            engine = RLMEngine(model=model, tools=tools, config=cfg)
            calls: list[int] = []
            real = engine_module._swarm_fast.join_with_deadline

            async def spy(coros, deadline_ms):
                calls.append(deadline_ms)
                return await real(coros, deadline_ms)

            with mock_patch.object(
                engine_module._swarm_fast, "join_with_deadline", spy
            ):
                result = engine.solve("two parallel subtasks")
            self.assertEqual(result, "done")
            self.assertTrue(calls, "parallel step must go through swarm_fast")
            self.assertTrue(all(ms > 0 for ms in calls))
