"""op_fast: Python shim over the `op-fast` Rust binary.

Prefers the compiled Rust path (target/debug / target/release /
PATH), falls back to returning None so callers keep their
rg-then-os.walk chain. Never raises for a missing binary.
"""
from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

_BIN_NAMES = ("op-fast", "op-fast.exe")


def find_bin() -> Path | None:
    here = Path(__file__).resolve()
    # agent/ -> openplanter/ -> openplanter-desktop/target/{release,debug}
    # Release first: the debug binary is unoptimized (measured ~6x slower
    # on fingerprint workloads) and must never win when both exist.
    for base in (here.parent.parent / "openplanter-desktop" / "target",):
        for profile in ("release", "debug"):
            for name in _BIN_NAMES:
                cand = base / profile / name
                if cand.is_file():
                    return cand
    found = shutil.which("op-fast")
    return Path(found) if found else None


def available() -> bool:
    return find_bin() is not None


def _run(args: list[str], timeout: int = 45) -> subprocess.CompletedProcess[str] | None:
    exe = find_bin()
    if exe is None:
        return None
    try:
        return subprocess.run(
            [str(exe), *args],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None


def list_files(root: str | Path, glob: str | None = None) -> list[str] | None:
    proc = _run([ "list", str(root), *(["--glob", glob] if glob else []) ])
    if proc is None or proc.returncode != 0:
        return None
    return [ln for ln in (proc.stdout or "").splitlines() if ln.strip()]


def search_files(
    root: str | Path, query: str, glob: str | None = None, max_hits: int = 200
) -> list[str] | None:
    proc = _run(
        ["search", str(root), query, *(["--glob", glob] if glob else []),
         "--max", str(max_hits)]
    )
    if proc is None or proc.returncode != 0:
        return None
    return [ln for ln in (proc.stdout or "").splitlines() if ln.strip()]


def fingerprint_dir(root: str | Path) -> list[dict] | None:
    proc = _run(["fp", str(root)], timeout=120)
    if proc is None or proc.returncode != 0:
        return None
    out = []
    for ln in (proc.stdout or "").splitlines():
        ln = ln.strip()
        if not ln:
            continue
        try:
            out.append(json.loads(ln))
        except json.JSONDecodeError:
            continue
    return out


def wiki_scan(root: str | Path) -> list[dict] | None:
    proc = _run(["wiki-scan", str(root)])
    if proc is None or proc.returncode != 0:
        return None
    out = []
    for ln in (proc.stdout or "").splitlines():
        ln = ln.strip()
        if not ln:
            continue
        try:
            out.append(json.loads(ln))
        except json.JSONDecodeError:
            continue
    return out


def patch_check(patch_file: str | Path) -> tuple[bool, str] | None:
    proc = _run(["patch-check", str(patch_file)])
    if proc is None:
        return None
    if proc.returncode == 0:
        return (True, (proc.stdout or "").strip())
    return (False, (proc.stderr or proc.stdout or "").strip())
