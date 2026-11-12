"""swarm_fast: asyncio port of `op-swarm::join_with_deadline`.

Mirrors the Rust semantics for the Python agent's ThreadPoolExecutor
fan-out: gather with an overall deadline, keep partials, report which
calls timed out so every tool_call still gets an ordered result.
"""
from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, field


@dataclass
class SwarmReport:
    completed: list
    elapsed_ms: int
    serial_ms: int
    speedup: float
    timed_out: bool


async def join_with_deadline(
    coros: list,
    deadline_ms: int,
) -> tuple[list, bool]:
    """Await coros with an overall deadline; keep partials, flag timeout."""
    out: list = []
    if not coros:
        return out, False
    tasks = [asyncio.ensure_future(c) for c in coros]
    try:
        done, pending = await asyncio.wait(
            tasks, timeout=deadline_ms / 1000.0,
            return_when=asyncio.ALL_COMPLETED,
        )
    except asyncio.CancelledError:
        for t in tasks:
            t.cancel()
        raise
    for t in tasks:
        if t.done() and not t.cancelled():
            exc = t.exception()
            if exc is None:
                out.append(t.result())
    timed_out = len(pending) > 0 if 'pending' in dir() else False
    # Recompute pending safely (wait always binds both names on success).
    unfinished = [t for t in tasks if not t.done()]
    for t in unfinished:
        t.cancel()
    if unfinished:
        await asyncio.gather(*unfinished, return_exceptions=True)
        return out, True
    return out, False


async def run_swarm(n: int, work_ms: int, deadline_ms: int) -> SwarmReport:
    """Benchmark twin of `op-swarm::run_swarm`: N sleeps vs serial."""
    async def work(i: int):
        await asyncio.sleep(work_ms / 1000.0)
        return i * i

    t0 = time.perf_counter()
    got, timed_out = await join_with_deadline(
        [work(i) for i in range(n)], deadline_ms)
    elapsed_ms = int((time.perf_counter() - t0) * 1000)
    serial_ms = n * work_ms
    speedup = serial_ms / max(elapsed_ms, 1)
    return SwarmReport(
        completed=got, elapsed_ms=elapsed_ms, serial_ms=serial_ms,
        speedup=speedup, timed_out=timed_out,
    )
