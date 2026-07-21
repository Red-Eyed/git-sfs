#!/usr/bin/env python3
"""Differences between v1 and v2 that are fixes, declared before they happen.

rust-rewrite-plan §5.1: "An unenumerated divergence is a regression. An
enumerated one is a fix. The difference is written down in advance, never
adjudicated after a red run." The tree diff and the argv diff cannot tell the
two apart on their own -- both are just differences -- so this is where the
writing down happens.

A suppression list would be the obvious shape and is the wrong one. Ignoring a
known difference makes the harness silent about whether v2 actually fixed
anything, and a list of ignores only ever grows. Each declaration here does two
jobs instead:

  normalize   collapse the dimension that is *allowed* to differ, applied to
              both sides. Everything outside it still compares strictly, so an
              unrelated regression in the same scenario is still caught.
  occurred    assert the divergence *did* happen. A v2 that quietly kept v1's
              behavior fails, which a suppression list can never detect.

Declarations only apply when a candidate binary is named (`run.py --candidate`).
A self-check comparing one binary against itself must produce no divergence at
all, so nothing is normalized and nothing is asserted.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass


def collapse_repeats(body: str) -> str:
    """Collapse runs of consecutive identical lines to a single line.

    v1's retryLoop reissues the *same* argv on failure, so a retried call is a
    run of identical adjacent lines. Collapsing the run compares what was
    attempted while staying silent about how many times.

    The narrowness matters: this only hides a change in the *count* of adjacent
    duplicates, and only in the scenario that declares it. A command genuinely
    issued twice in a row that becomes once would also be hidden, which is the
    accepted cost.
    """
    lines = body.splitlines(keepends=True)
    kept = [
        line
        for index, line in enumerate(lines)
        if index == 0 or line != lines[index - 1]
    ]
    return "".join(kept)


def fewer_lines(reference: str, candidate: str) -> bool:
    return len(candidate.splitlines()) < len(reference.splitlines())


# The manifest sections run.py emits, in order. Declared here because this is
# the contract between the two files: a divergence names a section, so a
# renamed section must not silently stop matching. run.py asserts its own
# sections against this list.
SECTION_LABELS = (
    "scenario exit",
    "outcomes",
    "rclone argv",
    "repo",
    "cache",
    "remote",
)


@dataclass(frozen=True)
class Divergence:
    id: str
    spec: str
    scenario: str  # substring of the scenario name
    section: str  # manifest section label the divergence lives in
    statement: str
    normalize: Callable[[str], str]
    occurred: Callable[[str, str], bool]


DIVERGENCES = [
    Divergence(
        id="retry-only-transient",
        spec="§13.4",
        scenario="05-remote-fault",
        section="rclone argv",
        statement="a permanent 403 is issued once, not retried retry_max times",
        normalize=collapse_repeats,
        occurred=fewer_lines,
    ),
]


def validate(scenario_names: list[str]) -> list[str]:
    """Problems with the declarations themselves, as a list of descriptions.

    A declaration naming a scenario or section that does not exist is inert: it
    would never normalize anything and never assert anything, while reading as
    though the divergence were handled.
    """
    problems = []
    for divergence in DIVERGENCES:
        if not any(divergence.scenario in name for name in scenario_names):
            problems.append(
                f"{divergence.id}: no scenario matches {divergence.scenario!r}"
            )
        if divergence.section not in SECTION_LABELS:
            problems.append(f"{divergence.id}: unknown section {divergence.section!r}")
    return problems


def for_scenario(name: str) -> list[Divergence]:
    return [d for d in DIVERGENCES if d.scenario in name]


def normalize_sections(
    sections: list[tuple[str, str]], applicable: list[Divergence]
) -> list[tuple[str, str]]:
    """Apply each divergence's normalization to the section it names."""
    by_section: dict[str, list[Divergence]] = {}
    for divergence in applicable:
        by_section.setdefault(divergence.section, []).append(divergence)

    normalized = []
    for label, body in sections:
        for divergence in by_section.get(label, []):
            body = divergence.normalize(body)
        normalized.append((label, body))
    return normalized


def section_body(sections: list[tuple[str, str]], label: str) -> str:
    for name, body in sections:
        if name == label:
            return body
    return ""
