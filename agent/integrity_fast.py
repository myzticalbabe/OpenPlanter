"""integrity_fast: skip re-hashing unchanged files.

Uses `op-fast fp` (parallel sha256 walk) as the fast path and falls
back to per-file `hashlib.sha256` when the binary is absent. Compares
against a `{rel: sha256}` baseline; only changed/new files need a
full re-read by the caller.
"""
from __future__ import annotations

import hashlib
from pathlib import Path

from . import op_fast


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fingerprint_dir(root: str | Path) -> dict[str, str]:
    fps = op_fast.fingerprint_dir(root)
    if fps is not None:
        return {d["rel"]: d["sha256"] for d in fps if "rel" in d and "sha256" in d}
    out: dict[str, str] = {}
    root_p = Path(root)
    for p in sorted(root_p.rglob("*")):
        if not p.is_file() or ".git" in p.parts:
            continue
        try:
            out[p.relative_to(root_p).as_posix()] = sha256_file(p)
        except OSError:
            continue
    return out


def changed_files(
    root: str | Path, baseline: dict[str, str]
) -> tuple[list[str], list[str], dict[str, str]]:
    """Return (changed_or_new, missing, current). Unchanged files cost
    one fast walk, not N full re-hashes by the caller."""
    current = fingerprint_dir(root)
    changed = sorted(rel for rel, digest in current.items()
                     if baseline.get(rel) != digest)
    missing = sorted(rel for rel in baseline if rel not in current)
    return changed, missing, current
