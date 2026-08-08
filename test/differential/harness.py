#!/usr/bin/env python3
"""Pieces shared by the three differential entry points.

`run.py` diffs trees, `lock_contention.py` races two processes, and
`cancellation.py` interrupts one mid-transfer -- but all three name binaries the
same way, build workspaces the same way, and wait on the filesystem the same
way. This module holds that common ground so the entry points hold only what is
distinct about them.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

HARNESS_DIR = Path(__file__).parent
REPO_ROOT = HARNESS_DIR.parent.parent

POLL_SECONDS = 0.05


@dataclass(frozen=True)
class Binary:
    name: str
    path: Path


def parse_binary(spec: str) -> Binary:
    name, _, path = spec.partition("=")
    if not name or not path:
        raise argparse.ArgumentTypeError(f"expected NAME=PATH, got {spec!r}")
    resolved = Path(path)
    if not os.access(resolved, os.X_OK):
        raise argparse.ArgumentTypeError(f"not executable: {path}")
    return Binary(name, resolved)


def workspace_root(prefix: str) -> Path:
    """Create a repo-local scratch root instead of relying on system temp."""
    scratch_parent = REPO_ROOT / ".cache" / "differential"
    scratch_parent.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix=prefix, dir=scratch_parent))


@dataclass
class Results:
    """Tally that separates frozen mechanism from observed v1 policy.

    ASSERT is contract: any conforming binary must satisfy it, so a failure
    fails the run. OBSERVE records behavior v2 is *allowed or required* to
    diverge from -- asserting it would mean inverting the test later, which is
    how a harness teaches people to ignore it.
    """

    asserts_passed: int = 0
    asserts_failed: int = 0

    def check(self, condition: bool, description: str) -> None:
        if condition:
            self.asserts_passed += 1
            print(f"  ASSERT ok   {description}")
        else:
            self.asserts_failed += 1
            print(f"  ASSERT FAIL {description}")

    def observe(self, description: str, value: str) -> None:
        print(f"  OBSERVE     {description}: {value}")


def wait_for(predicate, timeout: float) -> bool:
    """Poll until predicate holds, returning whether it did before the timeout."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(POLL_SECONDS)
    return False


def scenario_env(binary: Binary, work: Path, outcomes: Path) -> dict:
    """The environment a scenario script expects, per lib.sh's header.

    The fake rclone goes on PATH unconditionally: entry points that drive one
    process at a time need it for fault injection, and the argv log it writes is
    harmless when unread.
    """
    return os.environ | {
        "GIT_SFS": str(binary.path.resolve()),
        "HARNESS_DIR": str(HARNESS_DIR),
        "WORK": str(work),
        "REPO": str(work / "repo"),
        "CACHE": str(work / "cache"),
        "REMOTE": str(work / "remote"),
        "OUTCOMES": str(outcomes),
        "GIT_TERMINAL_PROMPT": "0",
        "PATH": f"{HARNESS_DIR / 'fake-rclone'}:{os.environ['PATH']}",
        "RCLONE_ARGV_LOG": str(work / "rclone-argv.log"),
    }


def prepare_workspace(binary: Binary, root: Path, setup: Path) -> dict:
    """Run a setup script in a fresh workspace and return its paths and env.

    Setup failure exits rather than returning: every caller builds a fixture it
    then makes assertions about, and assertions against a hollow fixture are
    worse than no assertions at all.
    """
    work = root / binary.name
    repo, cache, remote = work / "repo", work / "cache", work / "remote"
    for directory in (cache, remote):
        directory.mkdir(parents=True)
    outcomes = work / "outcomes.txt"
    outcomes.touch()

    env = scenario_env(binary, work, outcomes)
    script = f'set -uo pipefail; . "{HARNESS_DIR}/lib.sh"; . "{setup}"'
    completed = subprocess.run(
        ["bash", "-c", script], env=env, capture_output=True, text=True
    )
    if completed.returncode != 0:
        sys.exit(
            f"harness: setup {setup.name} failed for {binary.name}:\n{completed.stderr}"
        )
    return {"work": work, "repo": repo, "cache": cache, "remote": remote, "env": env}


def chmod_writable(root: Path) -> None:
    """Restore write bits so a cache full of read-only objects can be removed."""
    for path in root.rglob("*"):
        if path.is_file() and not path.is_symlink():
            path.chmod(0o644)
