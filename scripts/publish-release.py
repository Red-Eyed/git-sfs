#!/usr/bin/env python3
"""Creates a GitHub release with changelog notes and an install header."""

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


def extract_changelog_section(text: str, version: str) -> str:
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


def build_notes(version: str, repository: str, changelog_section: str) -> str:
    install_url = (
        f"https://github.com/{repository}/releases/download/{version}/install.sh"
    )
    return (
        f"## Install\n\n"
        f"```sh\n"
        f"curl -fsSL {install_url} | sh\n"
        f"```\n\n"
        f"---\n\n"
        f"{changelog_section}\n"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("version")  # e.g. vX.Y.Z
    parser.add_argument("repository")  # e.g. owner/repo
    args = parser.parse_args()

    changelog = Path("CHANGELOG.md").read_text()
    section = extract_changelog_section(changelog, args.version)

    if not section:
        print(
            f"error: no CHANGELOG.md section found for {args.version}", file=sys.stderr
        )
        sys.exit(1)

    notes = build_notes(args.version, args.repository, section)

    archives = sorted(Path("dist").glob("*.tar.gz"))

    with tempfile.NamedTemporaryFile(mode="w", suffix=".md", delete=False) as f:
        f.write(notes)
        notes_file = f.name

    try:
        subprocess.run(
            [
                "gh",
                "release",
                "create",
                args.version,
                *[str(p) for p in archives],
                "dist/SHA256SUMS",
                "scripts/install.sh",
                "--title",
                args.version,
                "--notes-file",
                notes_file,
            ],
            check=True,
        )
    finally:
        Path(notes_file).unlink(missing_ok=True)


if __name__ == "__main__":
    main()
