#!/usr/bin/env python3
"""Creates a GitHub release with changelog notes and an install header."""

import argparse
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


RELEASE_TAG = re.compile(
    r"^v(?P<base>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))"
    r"(?P<prerelease>-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


@dataclass(frozen=True)
class ReleaseSpec:
    tag: str
    base_tag: str
    prerelease: bool


def parse_release_spec(tag: str) -> ReleaseSpec:
    match = RELEASE_TAG.fullmatch(tag)
    if match is None:
        raise ValueError(f"invalid release tag: {tag}")
    return ReleaseSpec(
        tag=tag,
        base_tag=f"v{match.group('base')}",
        prerelease=match.group("prerelease") is not None,
    )


def extract_changelog_section(text: str, spec: ReleaseSpec) -> str:
    section = extract_named_section(text, spec.tag)
    if section or not spec.prerelease:
        return section
    return extract_named_section(text, spec.base_tag)


def extract_named_section(text: str, version: str) -> str:
    section_lines: list[str] = []
    in_section = False

    for line in text.splitlines():
        if line == f"## {version}":
            in_section = True
            continue
        if in_section:
            if line == "---" or line.startswith("## v"):
                break
            section_lines.append(line)

    return "\n".join(section_lines).strip()


def build_notes(spec: ReleaseSpec, repository: str, changelog_section: str) -> str:
    install_url = (
        f"https://github.com/{repository}/releases/download/{spec.tag}/install.sh"
    )
    return (
        f"## Install\n\n"
        f"```sh\n"
        f"curl -fsSL {install_url} | sh -s -- --version {spec.tag}\n"
        f"```\n\n"
        f"---\n\n"
        f"{changelog_section}\n"
    )


def build_release_command(
    spec: ReleaseSpec, archives: list[Path], notes_file: str
) -> list[str]:
    command = [
        "gh",
        "release",
        "create",
        spec.tag,
        *[str(path) for path in archives],
        "dist/SHA256SUMS",
        "scripts/install.sh",
        "--verify-tag",
        "--title",
        spec.tag,
        "--notes-file",
        notes_file,
    ]
    if spec.prerelease:
        command.extend(["--prerelease", "--latest=false"])
    return command


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("version")  # e.g. vX.Y.Z
    parser.add_argument("repository")  # e.g. owner/repo
    args = parser.parse_args()

    try:
        spec = parse_release_spec(args.version)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(2)

    changelog = Path("CHANGELOG.md").read_text()
    section = extract_changelog_section(changelog, spec)

    if not section:
        print(
            f"error: no CHANGELOG.md section found for {spec.tag} or {spec.base_tag}",
            file=sys.stderr,
        )
        sys.exit(1)

    notes = build_notes(spec, args.repository, section)

    archives = sorted(Path("dist").glob("*.tar.gz"))

    with tempfile.NamedTemporaryFile(mode="w", suffix=".md", delete=False) as f:
        f.write(notes)
        notes_file = f.name

    try:
        subprocess.run(build_release_command(spec, archives, notes_file), check=True)
    finally:
        Path(notes_file).unlink(missing_ok=True)


if __name__ == "__main__":
    main()
