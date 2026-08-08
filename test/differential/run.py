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
from dataclasses import dataclass
from pathlib import Path

import divergences
import snapshot
from harness import Binary, parse_binary, workspace_root

HARNESS_DIR = Path(__file__).parent
SCENARIO_DIR = HARNESS_DIR / "scenarios"

# .git holds commit hashes and index timestamps that differ between two runs of
# the same scenario. The worktree symlinks it tracks are captured directly, so
# excluding it costs no coverage of anything the contract freezes.
WORKTREE_EXCLUDED = [".git", ".git-sfs/README.md"]
REMOTE_EXCLUDED = ["tmp"]


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


def execute(scenario: Scenario, binary: Binary, work: Path) -> list[tuple[str, str]]:
    """Run one scenario in a fresh workspace and return its manifest sections.

    Sections stay separate rather than pre-joined so a declared divergence can
    normalize the one it names (see divergences.py) without re-parsing text the
    driver just assembled.
    """
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
        ("repo", _manifest(repo, replacements, WORKTREE_EXCLUDED)),
        ("cache", _manifest(cache, replacements, WORKTREE_EXCLUDED)),
        ("remote", _manifest(remote, replacements, REMOTE_EXCLUDED)),
    ]
    assert [label for label, _ in sections] == list(divergences.SECTION_LABELS), (
        "manifest sections drifted from divergences.SECTION_LABELS"
    )
    return sections


def compare(
    base: Binary,
    other: Binary,
    reports: dict[str, list[tuple[str, str]]],
    applicable: list[divergences.Divergence],
) -> tuple[str, list[str]]:
    """Diff two runs, and report on every divergence declared for them.

    Returns the unified diff (empty when they agree) and one status line per
    declared divergence. A declared divergence that did not happen is a failure
    in its own right: it means v2 kept the behavior 13 says to fix.
    """
    base_sections = divergences.normalize_sections(reports[base.name], applicable)
    other_sections = divergences.normalize_sections(reports[other.name], applicable)
    delta = diff_reports(
        base, other, compose_report(base_sections), compose_report(other_sections)
    )

    notes = []
    for divergence in applicable:
        happened = divergence.occurred(
            divergences.section_body(reports[base.name], divergence.section),
            divergences.section_body(reports[other.name], divergence.section),
        )
        mark = "confirmed" if happened else "MISSING"
        notes.append(
            f"     divergence {divergence.spec} {divergence.id}: {mark}"
            f" -- {divergence.statement}"
        )
    return delta, notes


def _unmet_precondition(work: Path) -> str:
    """Text of any precondition a scenario reported as failed, else empty."""
    sentinel = work / "precondition-failed"
    return sentinel.read_text() if sentinel.is_file() else ""


def _manifest(
    root: Path, replacements: list[tuple[bytes, bytes]], excludes: list[str]
) -> str:
    if not root.is_dir():
        return "(absent)\n"
    return snapshot.render_manifest(snapshot.walk(root, replacements, excludes))


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
    parser.add_argument(
        "--candidate",
        metavar="NAME",
        help="binary expected to implement the contract-spec 13 fixes; enables "
        "the declared divergences in divergences.py",
    )
    args = parser.parse_args()

    if len(args.binary) < 2:
        sys.exit("run: need at least two --binary arguments to compare")

    names = {binary.name for binary in args.binary}
    if args.candidate and args.candidate not in names:
        sys.exit(f"run: --candidate {args.candidate!r} is not one of {sorted(names)}")

    scenarios = discover_scenarios(args.scenario)
    if not scenarios:
        sys.exit("run: no scenarios matched")

    # Validated against every scenario, not just the selected ones, so running a
    # single scenario cannot mask a declaration that has gone stale.
    problems = divergences.validate([s.name for s in discover_scenarios(None)])
    if problems:
        sys.exit("run: bad divergence declaration:\n  " + "\n  ".join(problems))

    root = workspace_root("git-sfs-differential-")
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
                # Declarations describe how v2 differs from v1, so they apply
                # only to a pair where one side is the named candidate. A
                # self-check must show no divergence at all.
                applicable = (
                    divergences.for_scenario(scenario.name)
                    if other.name == args.candidate
                    else []
                )
                delta, notes = compare(base, other, reports, applicable)
                missing = [note for note in notes if "MISSING" in note]
                if delta or missing:
                    failures += 1
                    print(f"FAIL {scenario.name}: {base.name} vs {other.name}")
                    print(delta, end="")
                else:
                    print(f"ok   {scenario.name}: {base.name} == {other.name}")
                for note in notes:
                    print(note)
    finally:
        if args.keep:
            print(f"workspaces retained in {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)

    if failures:
        sys.exit(f"{failures} divergence(s)")


if __name__ == "__main__":
    main()
