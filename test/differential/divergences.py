#!/usr/bin/env python3
"""Expected differences for a named candidate binary.

The state diff cannot tell an allowed compatibility difference from a
regression on its own, so this is where those allowed differences are declared.

A suppression list would be the obvious shape and is the wrong one. Ignoring a
known difference makes the harness silent about whether current actually fixed
anything, and a list of ignores only ever grows. Each declaration here does two
jobs instead:

  normalize   collapse the dimension that is *allowed* to differ, applied to
              both sides. Everything outside it still compares strictly, so an
              unrelated regression in the same scenario is still caught.
  occurred    assert the candidate-side change actually happened.

Declarations only apply when a candidate binary is named (`run.py --candidate`).
A self-check comparing one binary against itself must produce no divergence at
all, so nothing is normalized and nothing is asserted.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass


# A cache object's manifest line, e.g.
#   file files/sha256/ab/ab3f... mode=0444 size=15 sha256=...
CACHE_OBJECT_PREFIX = "file files/sha256/"
MODE_FIELD = re.compile(r"mode=\d{4}")


def mask_outcome(label: str) -> Callable[[str], str]:
    """Blank one `label=<status>` line, leaving every other outcome strict."""

    def normalize(body: str) -> str:
        lines = [
            f"{label}=<status>\n" if line.startswith(f"{label}=") else line
            for line in body.splitlines(keepends=True)
        ]
        return "".join(lines)

    return normalize


def outcome_became_zero(label: str) -> Callable[[str, str], bool]:
    """The candidate succeeds where the reference did not."""

    def occurred(reference: str, candidate: str) -> bool:
        return _outcome(reference, label) not in ("0", None) and (
            _outcome(candidate, label) == "0"
        )

    return occurred


def _outcome(body: str, label: str) -> str | None:
    for line in body.splitlines():
        name, _, status = line.partition("=")
        if name == label:
            return status
    return None


def mask_manifest_path(path: str) -> Callable[[str], str]:
    """Blank one manifest line by path, leaving neighboring state strict."""

    def normalize(body: str) -> str:
        lines = [
            _mask_manifest_line(line, path) for line in body.splitlines(keepends=True)
        ]
        return "".join(lines)

    return normalize


def manifest_path_changed(path: str) -> Callable[[str, str], bool]:
    """The candidate's manifest line for `path` differs from the reference."""

    def occurred(reference: str, candidate: str) -> bool:
        return _manifest_line(reference, path) != _manifest_line(candidate, path)

    return occurred


def _mask_manifest_line(line: str, path: str) -> str:
    for prefix in ("file", "dir ", "link"):
        marker = f"{prefix} {path} "
        if line.startswith(marker):
            return f"{marker}<manifest>\n"
    return line


def _manifest_line(body: str, path: str) -> str | None:
    for line in body.splitlines():
        if f" {path} " in line:
            return line
    return None


def mask_object_modes(body: str) -> str:
    """Blank the mode on cache-object lines only.

    Narrow on purpose: size and digest still compare strictly, and every other
    entry in the cache -- tmp/, locks/, directory modes -- is untouched. Masking
    the whole section would hide a genuine regression alongside the permitted
    difference.
    """
    lines = [
        MODE_FIELD.sub("mode=****", line)
        if line.startswith(CACHE_OBJECT_PREFIX)
        else line
        for line in body.splitlines(keepends=True)
    ]
    return "".join(lines)


def objects_became_read_only(reference: str, candidate: str) -> bool:
    """Every cache object is unwritable in the candidate, but not the reference."""
    return _has_writable_object(reference) and not _has_writable_object(candidate)


def _has_writable_object(body: str) -> bool:
    for line in body.splitlines():
        if not line.startswith(CACHE_OBJECT_PREFIX):
            continue
        found = MODE_FIELD.search(line)
        if found and int(found.group()[len("mode=") :], 8) & 0o222:
            return True
    return False


# The manifest sections run.py emits, in order. Declared here because this is
# the contract between the two files: a divergence names a section, so a
# renamed section must not silently stop matching. run.py asserts its own
# sections against this list.
SECTION_LABELS = (
    "scenario exit",
    "outcomes",
    "repo",
    "cache",
    "remote",
)


@dataclass(frozen=True)
class Divergence:
    id: str
    behavior: str
    scenario: str  # substring of the scenario name
    section: str  # manifest section label the divergence lives in
    statement: str
    normalize: Callable[[str], str]
    occurred: Callable[[str, str], bool]


DIVERGENCES = [
    Divergence(
        id="fresh-init-verifies-without-remote",
        behavior="remote verification",
        scenario="01-add-commit",
        section="outcomes",
        statement="a fresh current repo does not fail default verify because init no "
        "longer writes a missing rclone.conf reference",
        normalize=mask_outcome("verify_default"),
        occurred=outcome_became_zero("verify_default"),
    ),
    Divergence(
        id="init-default-config-omits-local-rclone-config-add",
        behavior="default config",
        scenario="01-add-commit",
        section="repo",
        statement="the default config template changes so a fresh repo does not "
        "point at an uncreated local rclone.conf",
        normalize=mask_manifest_path(".git-sfs/config.toml"),
        occurred=manifest_path_changed(".git-sfs/config.toml"),
    ),
    Divergence(
        id="init-default-config-omits-local-rclone-config-corrupt",
        behavior="default config",
        scenario="03-corrupt-cache",
        section="repo",
        statement="the default config template changes so a fresh repo does not "
        "point at an uncreated local rclone.conf",
        normalize=mask_manifest_path(".git-sfs/config.toml"),
        occurred=manifest_path_changed(".git-sfs/config.toml"),
    ),
    Divergence(
        id="init-default-config-omits-local-rclone-config-writable",
        behavior="default config",
        scenario="06-writable-cache",
        section="repo",
        statement="the default config template changes so a fresh repo does not "
        "point at an uncreated local rclone.conf",
        normalize=mask_manifest_path(".git-sfs/config.toml"),
        occurred=manifest_path_changed(".git-sfs/config.toml"),
    ),
    Divergence(
        id="verify-remote-denial-is-not-a-local-failure",
        behavior="remote verification",
        scenario="05-remote-fault",
        section="outcomes",
        statement="verify with remote checking reports the local tree result "
        "without collapsing a denied remote into missing objects",
        normalize=mask_outcome("verify_object_denied"),
        occurred=outcome_became_zero("verify_object_denied"),
    ),
    Divergence(
        id="verify-integrity-remote-denial-is-not-a-local-failure",
        behavior="remote verification",
        scenario="05-remote-fault",
        section="outcomes",
        statement="verify --with-integrity reports the local tree result "
        "without collapsing a denied remote into missing objects",
        normalize=mask_outcome("verify_integrity_denied"),
        occurred=outcome_became_zero("verify_integrity_denied"),
    ),
    Divergence(
        id="status-backend-denial-is-unknown-not-fatal",
        behavior="remote status",
        scenario="05-remote-fault",
        section="outcomes",
        statement="status represents remote lookup failure as unknown instead "
        "of treating it as a command failure",
        normalize=mask_outcome("status_backend_denied"),
        occurred=outcome_became_zero("status_backend_denied"),
    ),
    Divergence(
        id="writable-object-is-repaired-without-integrity-flag",
        behavior="cache integrity",
        scenario="06-writable-cache",
        section="outcomes",
        statement="verify hash-verifies and re-protects an intact writable "
        "object without requiring --with-integrity",
        normalize=mask_outcome("verify_presence"),
        occurred=outcome_became_zero("verify_presence"),
    ),
    Divergence(
        id="writable-object-is-repaired-not-failed",
        behavior="cache integrity",
        scenario="06-writable-cache",
        section="outcomes",
        statement="verify --with-integrity hash-verifies an intact but writable "
        "object and exits 0",
        normalize=mask_outcome("verify_integrity"),
        occurred=outcome_became_zero("verify_integrity"),
    ),
    Divergence(
        id="writable-object-is-reprotected",
        behavior="cache protection",
        scenario="06-writable-cache",
        section="cache",
        statement="the verified object is protected in place, so the write bits "
        "are gone afterwards",
        normalize=mask_object_modes,
        occurred=objects_became_read_only,
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
