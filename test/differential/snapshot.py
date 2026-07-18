#!/usr/bin/env python3
"""Render a directory tree as a canonical, diffable manifest.

The differential harness compares what two git-sfs binaries leave on disk. That
comparison is only meaningful if the rendering is deterministic, so everything
that varies between two runs of the same scenario -- absolute workspace paths,
directory iteration order -- is normalized away here, and everything the
conformance contract freezes -- symlink targets, permission bits, content -- is
recorded exactly.

Functional core: `normalize`, `render_entry`, `render_manifest` are pure.
Imperative shell: `walk` is the only function that touches the filesystem.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
from dataclasses import dataclass
from pathlib import Path

# Symlink permission bits are not portable -- Linux reports 0o777, macOS 0o755 --
# and carry no meaning, so they are deliberately excluded from the manifest.
# File and directory modes ARE recorded: contract-spec 4.1 makes the write bits
# on cache objects load-bearing.


class Entry:
    """One filesystem entry, rendered as a single canonical manifest line.

    Each kind carries only the fields its own line needs, so there is no
    nullable field standing in for "not applicable to this kind".
    """

    def render(self) -> str:
        raise NotImplementedError


@dataclass(frozen=True)
class FileEntry(Entry):
    path: str
    mode: int
    size: int
    digest: str

    def render(self) -> str:
        return f"file {self.path} mode={self.mode:04o} size={self.size} sha256={self.digest}"


@dataclass(frozen=True)
class DirEntry(Entry):
    path: str
    mode: int

    def render(self) -> str:
        return f"dir  {self.path} mode={self.mode:04o}"


@dataclass(frozen=True)
class LinkEntry(Entry):
    path: str
    target: str

    def render(self) -> str:
        return f"link {self.path} target={self.target}"


def normalize(data: bytes, replacements: list[tuple[bytes, bytes]]) -> bytes:
    """Substitute run-specific byte sequences with stable placeholders.

    Applied to file content and symlink targets alike. Binary payloads are
    unaffected in practice because the patterns are absolute paths, so one
    uniform rule covers both without classifying files as text or binary.
    """
    for pattern, placeholder in replacements:
        data = data.replace(pattern, placeholder)
    return data


def render_manifest(entries: list) -> str:
    """Sort by path so the manifest does not depend on directory order."""
    lines = sorted(entry.render() for entry in entries)
    return "\n".join(lines) + "\n"


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _is_excluded(relative: str, excludes: list[str]) -> bool:
    parts = relative.split("/")
    return any(exclude in parts for exclude in excludes)


def walk(
    root: Path,
    replacements: list[tuple[bytes, bytes]],
    excludes: list[str],
) -> list[Entry]:
    """Collect every entry under `root`, normalized and content-hashed.

    Symlinks are never followed: their target string is itself part of the
    frozen contract (spec 3.1), so recording where they point matters more than
    recording what they point at -- and following them would also escape `root`.
    """
    entries: list[Entry] = []

    for current, dirnames, filenames in os.walk(root):
        current_path = Path(current)
        for name in sorted(dirnames) + sorted(filenames):
            absolute = current_path / name
            relative = absolute.relative_to(root).as_posix()
            if _is_excluded(relative, excludes):
                continue
            entries.append(_entry_for(absolute, relative, replacements))

        dirnames[:] = [d for d in dirnames if d not in excludes]

    return entries


def _entry_for(
    absolute: Path,
    relative: str,
    replacements: list[tuple[bytes, bytes]],
) -> Entry:
    if absolute.is_symlink():
        raw = os.readlink(absolute).encode()
        return LinkEntry(relative, normalize(raw, replacements).decode())

    mode = absolute.lstat().st_mode & 0o777
    if absolute.is_dir():
        return DirEntry(relative, mode)

    content = normalize(absolute.read_bytes(), replacements)
    return FileEntry(relative, mode, len(content), _digest(content))


def parse_replacement(spec: str) -> tuple[bytes, bytes]:
    """Parse a `PATH=PLACEHOLDER` pair.

    Kept generic rather than hardcoding the workspace layout, so the caller
    supplies the domain meaning of what varies between runs.
    """
    literal, _, placeholder = spec.partition("=")
    if not literal or not placeholder:
        raise argparse.ArgumentTypeError(f"expected PATH=PLACEHOLDER, got {spec!r}")
    return literal.encode(), placeholder.encode()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument(
        "--replace",
        type=parse_replacement,
        action="append",
        default=[],
        metavar="PATH=PLACEHOLDER",
        help="substitute a run-specific path with a stable placeholder",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        metavar="NAME",
        help="skip any path component with this name (e.g. .git)",
    )
    args = parser.parse_args()

    if not args.root.is_dir():
        sys.exit(f"snapshot: not a directory: {args.root}")

    # Longest patterns first, so a nested path is never shadowed by its parent.
    replacements = sorted(args.replace, key=lambda pair: len(pair[0]), reverse=True)
    sys.stdout.write(render_manifest(walk(args.root, replacements, args.exclude)))


if __name__ == "__main__":
    main()
