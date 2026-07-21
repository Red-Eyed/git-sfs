#!/usr/bin/env python3
"""Differential harness: run one scenario against several git-sfs binaries and
diff what each left behind.

The comparison artifact is filesystem state plus per-command success/failure --
the two surfaces the conformance contract still freezes once human output, the
exit-code taxonomy, and the status --json schema are unfrozen (contract-spec 12).

Usage:

    test/differential/run.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
    test/differential/run.py --binary a=./git-sfs --binary b=./git-sfs   # self-check

Exits non-zero when any pair of binaries diverges on any scenario.
"""

from __future__ import annotations

import argparse
import difflib
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

import snapshot
from harness import Binary, parse_binary

HARNESS_DIR = Path(__file__).parent
SCENARIO_DIR = HARNESS_DIR / "scenarios"

# .git holds commit hashes and index timestamps that differ between two runs of
# the same scenario. The worktree symlinks it tracks are captured directly, so
# excluding it costs no coverage of anything the contract freezes.
EXCLUDED = [".git"]


@dataclass(frozen=True)
class Scenario:
    name: str
    path: Path


def discover_scenarios(pattern: str | None) -> list[Scenario]:
    scenarios = [
        Scenario(path.stem, path) for path in sorted(SCENARIO_DIR.glob("*.sh"))
    ]
    if pattern is None:
        return scenarios
    return [s for s in scenarios if pattern in s.name]


def compose_report(sections: list[tuple[str, str]]) -> str:
    """Join labelled sections into one manifest, pure and order-preserving."""
    return "".join(f"=== {label} ===\n{body}" for label, body in sections)


def diff_reports(left: Binary, right: Binary, left_body: str, right_body: str) -> str:
    """Unified diff of two reports; empty string when they agree."""
    if left_body == right_body:
        return ""
    lines = difflib.unified_diff(
        left_body.splitlines(keepends=True),
        right_body.splitlines(keepends=True),
        fromfile=left.name,
        tofile=right.name,
    )
    return "".join(lines)


def _replacements(work: Path) -> list[tuple[bytes, bytes]]:
    """Every path that varies between runs reduces to the workspace root.

    The resolved form is registered too: `.git-sfs/cache` stores a canonicalized
    absolute target (spec 2), which on macOS differs from the path we created
    (/var/folders/... vs /private/var/folders/...).
    """
    literals = {str(work), str(work.resolve())}
    return [(literal.encode(), b"{WORK}") for literal in literals]


def execute(scenario: Scenario, binary: Binary, work: Path) -> str:
    """Run one scenario in a fresh workspace and return its manifest."""
    repo = work / "repo"
    cache = work / "cache"
    remote = work / "remote"
    outcomes = work / "outcomes.txt"
    for directory in (cache, remote):
        directory.mkdir(parents=True)
    outcomes.touch()

    env = os.environ | {
        "GIT_SFS": str(binary.path.resolve()),
        "HARNESS_DIR": str(HARNESS_DIR),
        "WORK": str(work),
        "REPO": str(repo),
        "CACHE": str(cache),
        "REMOTE": str(remote),
        "OUTCOMES": str(outcomes),
        "GIT_TERMINAL_PROMPT": "0",
        # Keep commit metadata identical across binaries; .git is excluded from
        # the manifest anyway, but a deterministic run is easier to debug.
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    script = f'set -uo pipefail; . "{HARNESS_DIR}/lib.sh"; . "{scenario.path}"'
    completed = subprocess.run(
        ["bash", "-c", script],
        env=env,
        capture_output=True,
        text=True,
    )

    replacements = sorted(
        _replacements(work), key=lambda pair: len(pair[0]), reverse=True
    )
    sections = [
        ("scenario exit", f"{completed.returncode}\n"),
        ("outcomes", outcomes.read_text()),
        # Remote behavior has no tree to diff, but every remote operation is an
        # rclone invocation -- so the argv stream is its equivalent artifact
        # (rust-rewrite-plan 5.2b). Absent unless a scenario opts into the fake.
        ("rclone argv", _argv_log(work / "rclone-argv.log", replacements)),
        ("repo", _manifest(repo, replacements)),
        ("cache", _manifest(cache, replacements)),
        ("remote", _manifest(remote, replacements)),
    ]
    return compose_report(sections)


def _unmet_precondition(work: Path) -> str:
    """Text of any precondition a scenario reported as failed, else empty."""
    sentinel = work / "precondition-failed"
    return sentinel.read_text() if sentinel.is_file() else ""


def _argv_log(path: Path, replacements: list[tuple[bytes, bytes]]) -> str:
    if not path.is_file():
        return "(not recorded)\n"
    return snapshot.normalize(path.read_bytes(), replacements).decode()


def _manifest(root: Path, replacements: list[tuple[bytes, bytes]]) -> str:
    if not root.is_dir():
        return "(absent)\n"
    return snapshot.render_manifest(snapshot.walk(root, replacements, EXCLUDED))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=parse_binary,
        action="append",
        required=True,
        metavar="NAME=PATH",
        help="a binary under test; pass at least twice",
    )
    parser.add_argument("--scenario", help="only run scenarios matching this substring")
    parser.add_argument(
        "--keep", action="store_true", help="retain workspaces for inspection"
    )
    args = parser.parse_args()

    if len(args.binary) < 2:
        sys.exit("run: need at least two --binary arguments to compare")

    scenarios = discover_scenarios(args.scenario)
    if not scenarios:
        sys.exit("run: no scenarios matched")

    root = Path(tempfile.mkdtemp(prefix="git-sfs-differential-"))
    failures = 0
    try:
        for scenario in scenarios:
            reports = {}
            for binary in args.binary:
                work = root / scenario.name / binary.name
                work.mkdir(parents=True)
                reports[binary.name] = execute(scenario, binary, work)

            base = args.binary[0]
            # Agreement is not correctness: a broken fixture fails identically
            # for every binary and would otherwise read as green. Scenarios are
            # written to complete cleanly, so an unmet precondition is a harness
            # fault regardless of whether the binaries agreed.
            unmet = _unmet_precondition(root / scenario.name / base.name)
            if unmet:
                failures += 1
                print(f"ERROR {scenario.name}: precondition failed under {base.name}")
                print(f"       {unmet.strip()}")
                continue

            for other in args.binary[1:]:
                delta = diff_reports(
                    base, other, reports[base.name], reports[other.name]
                )
                if delta:
                    failures += 1
                    print(f"FAIL {scenario.name}: {base.name} vs {other.name}")
                    print(delta)
                else:
                    print(f"ok   {scenario.name}: {base.name} == {other.name}")
    finally:
        if args.keep:
            print(f"workspaces retained in {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)

    if failures:
        sys.exit(f"{failures} divergence(s)")


if __name__ == "__main__":
    main()
