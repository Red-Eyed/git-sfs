#!/usr/bin/env python3
"""Queries over a git-sfs cache tree.

Separate from harness.py on purpose: that module knows about binaries,
workspaces, and polling and would work for any CLI, while everything here
encodes the git-sfs cache layout. Both invariant-asserting entry points need
these, and neither should be reaching into the other's internals.
"""

from __future__ import annotations

import hashlib
import os
import re
from pathlib import Path

OBJECT_NAME = re.compile(r"^[0-9a-f]{64}$")


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def object_dir(root: Path) -> Path:
    """The content-addressed store inside a cache or remote root."""
    return root / "files" / "sha256"


def object_path(root: Path, digest: str) -> Path:
    return object_dir(root) / digest[:2] / digest


def tracked_object(repo: Path, relative: str) -> str:
    """The hash a worktree symlink points at, read from its target."""
    return os.readlink(repo / relative).rsplit("/", 1)[-1]


def published_objects(root: Path) -> list[Path]:
    """Every file sitting at a content-addressed path under root."""
    objects = object_dir(root)
    if not objects.is_dir():
        return []
    return [
        path
        for path in sorted(objects.rglob("*"))
        if path.is_file() and OBJECT_NAME.match(path.name)
    ]


def is_protected(path: Path) -> bool:
    """Whether write bits are stripped, marking the object as verified."""
    return path.is_file() and path.stat().st_mode & 0o222 == 0


def trusted_but_wrong(root: Path) -> list[Path]:
    """Objects that are read-only yet do not hash to their own name.

    This is the dangerous state: ordinary commands read the stripped write bit
    as proof the bytes were verified when written, so they do not re-hash on
    every access.
    """
    return [
        path
        for path in published_objects(root)
        if is_protected(path) and sha256_of(path) != path.name
    ]


def stray_files(root: Path) -> list[Path]:
    """Non-object files inside the object store."""
    objects = object_dir(root)
    if not objects.is_dir():
        return []
    return [
        path
        for path in sorted(objects.rglob("*"))
        if path.is_file() and not OBJECT_NAME.match(path.name)
    ]


def held_locks(root: Path) -> list[str]:
    locks = root / "locks"
    if not locks.is_dir():
        return []
    return sorted(path.name for path in locks.iterdir())


def set_writable(path: Path) -> None:
    """Put write bits back, as a filesystem that does not preserve them would."""
    path.chmod(path.stat().st_mode | 0o200)


def set_protected(path: Path) -> None:
    path.chmod(path.stat().st_mode & ~0o222)


def overwrite(path: Path, payload: bytes) -> None:
    """Replace an object's bytes in place, restoring its original mode.

    Rot does not ask permission, so the read-only bit is lifted and put back
    rather than left off -- the point is to produce bytes that disagree with the
    name while the mode still claims they were verified.
    """
    mode = path.stat().st_mode
    path.chmod(mode | 0o200)
    path.write_bytes(payload)
    path.chmod(mode)
