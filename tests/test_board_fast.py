"""Board fast path: `board_fast` lease logic + WAL contention proof.

Decision recorded here: no sqlite dep in `op-fast` (keeps the Rust crate
offline-buildable). The pure rule (`claimable_ids` / `lease_expired`) is
unit-tested in Rust; this file proves the SQL layer above it: WAL mode,
busy timeout, and single BEGIN IMMEDIATE transactions hand each row to
exactly one claimant under thread contention.
"""
from __future__ import annotations

import sqlite3
import threading
from pathlib import Path

from agent import board_fast


def _make_store(path: Path, n: int = 10) -> None:
    cx = sqlite3.connect(str(path))
    cx.execute("PRAGMA journal_mode=WAL")
    cx.execute(
        "CREATE TABLE tasks (id INTEGER PRIMARY KEY, state TEXT, claimed_at INTEGER)"
    )
    cx.executemany(
        "INSERT INTO tasks (state, claimed_at) VALUES ('open', 0)",
        [()] * n,
    )
    cx.commit()
    cx.close()


def _claim_one(store: str, now: int) -> int | None:
    cx = board_fast.connect(store)
    assert cx is not None
    try:
        cx.execute("BEGIN IMMEDIATE")
        try:
            row = cx.execute(
                "SELECT id FROM tasks WHERE state='open' ORDER BY id LIMIT 1"
            ).fetchone()
            if row is None:
                cx.execute("COMMIT")
                return None
            task_id = row[0]
            cur = cx.execute(
                "UPDATE tasks SET state='claimed', claimed_at=? "
                "WHERE id=? AND state='open'",
                (now, task_id),
            )
            cx.execute("COMMIT")
            return task_id if cur.rowcount == 1 else None
        except Exception:
            try:
                cx.execute("ROLLBACK")
            except sqlite3.Error:
                pass
            raise
    finally:
        cx.close()


def test_claimable_ids_mirrors_rust_rule():
    assert board_fast.claimable_ids([1, 2, 3], {2}, {3}, 10) == [1]
    assert board_fast.claimable_ids([1, 2, 3], set(), set(), 2) == [1, 2]
    assert board_fast.lease_expired(100, 100 + 1800, 1800) is True
    assert board_fast.lease_expired(100, 100 + 1799, 1800) is False


def test_wal_contention_no_double_claim(tmp_path):
    store = str(tmp_path / "board.db")
    _make_store(tmp_path / "board.db", n=10)
    claimed: list[int] = []
    lock = threading.Lock()

    def worker():
        for _ in range(5):
            got = _claim_one(store, board_fast.now_epoch())
            if got is None:
                return
            with lock:
                claimed.append(got)

    threads = [threading.Thread(target=worker) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=60)

    assert sorted(claimed) == list(range(1, 11)), (
        f"each row claimed exactly once, got {sorted(claimed)}"
    )
