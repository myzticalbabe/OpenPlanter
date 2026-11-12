"""board_fast: low-contention SQLite claim helper.

Mirrors `op-fast::claimable_ids` / `lease_expired` (pure logic,
unit-tested in Rust) at the SQL layer: WAL mode, busy timeout, and a
single BEGIN IMMEDIATE transaction per claim so parallel lanes don't
serialize on process startup. Falls back cleanly when the store is
absent — the caller keeps its existing LaneMessageBoard path.
"""
from __future__ import annotations

import sqlite3
import time

LEASE_SECS = 30 * 60


def connect(store: str, timeout_s: float = 30.0) -> sqlite3.Connection | None:
    try:
        cx = sqlite3.connect(store, timeout=timeout_s, isolation_level=None)
    except sqlite3.Error:
        return None
    try:
        cx.execute("PRAGMA journal_mode=WAL")
        cx.execute(f"PRAGMA busy_timeout={int(timeout_s * 1000)}")
    except sqlite3.Error:
        pass
    return cx


def claimable_ids(
    ordered: list[int], live: set[int], retired: set[int], limit: int
) -> list[int]:
    """Python mirror of `op-fast::claimable_ids` (oldest-first, skip live
    leases and retired rows, take up to limit)."""
    out = [i for i in ordered if i not in live and i not in retired]
    return out[:limit]


def lease_expired(claimed_at: int, now_epoch: int, lease_secs: int = LEASE_SECS) -> bool:
    return now_epoch >= claimed_at + lease_secs


def now_epoch() -> int:
    return int(time.time())
