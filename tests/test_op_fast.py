"""op-fast wiring tests: Rust binary + Python shims agree."""
from __future__ import annotations

import asyncio
import json
import subprocess
from pathlib import Path

import pytest

from agent import board_fast, integrity_fast, op_fast, swarm_fast

BIN = op_fast.find_bin()


def test_binary_exists():
    assert BIN is not None and BIN.is_file(), "cargo build -p op-fast must run first"


def test_list_search_parity(tmp_path):
    (tmp_path / "a.py").write_text("hello\n")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "b.py").write_text("world\n")
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git" / "x.py").write_text("hidden\n")
    got = op_fast.list_files(tmp_path)
    assert got == ["a.py", "sub/b.py"]
    hits = op_fast.search_files(tmp_path, "HELLO")
    assert hits == ["a.py:1:hello"]


def test_fingerprint_matches_hashlib(tmp_path):
    (tmp_path / "a.json").write_text('{"k": 1}')
    fps = op_fast.fingerprint_dir(tmp_path)
    assert fps is not None and len(fps) == 1
    assert fps[0]["rel"] == "a.json"
    assert fps[0]["sha256"] == integrity_fast.sha256_file(tmp_path / "a.json")


def test_patch_check(tmp_path):
    good = tmp_path / "good.patch"
    good.write_text("*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch")
    ok, msg = op_fast.patch_check(good)
    assert ok and "1 ops" in msg
    bad = tmp_path / "bad.patch"
    bad.write_text("no markers")
    res = op_fast.patch_check(bad)
    assert res is not None and res[0] is False


def test_swarm_fast_partials_and_speedup():
    async def main():
        report = await swarm_fast.run_swarm(16, 50, 2000)
        assert len(report.completed) == 16
        assert report.speedup > 2.0
        got, timed_out = await swarm_fast.join_with_deadline(
            [asyncio.sleep(5)], deadline_ms=50)
        assert got == [] and timed_out is True
    asyncio.run(main())


def test_integrity_changed_detection(tmp_path):
    (tmp_path / "a.txt").write_text("one")
    changed, missing, current = integrity_fast.changed_files(tmp_path, {})
    assert changed == ["a.txt"] and missing == []
    changed2, missing2, _ = integrity_fast.changed_files(tmp_path, current)
    assert changed2 == [] and missing2 == []


def test_board_claim_logic():
    assert board_fast.claimable_ids([1, 2, 3], {2}, {3}, 10) == [1]
    assert board_fast.lease_expired(100, 100 + 1800, 1800) is True
    assert board_fast.lease_expired(100, 100 + 1799, 1800) is False
