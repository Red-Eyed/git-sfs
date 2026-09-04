#!/usr/bin/env python3
"""Generate llms.txt from git-sfs documentation.

Reads docs/ and README.md, produces a single self-contained reference with no
external links so an agent can work entirely from the embedded document.

Writes to:
  llms.txt               -- repo root (web/GitHub discovery)

Run via: just gen-llms-txt
"""

from pathlib import Path
import re

REPO_ROOT = Path(__file__).parent.parent
DOCS_DIR = REPO_ROOT / "docs"

OUTPUTS = [
    REPO_ROOT / "llms.txt",
]


def extract_section(text: str, heading: str, level: int = 2) -> str:
    """Return the body of a markdown section (exclusive of the heading line).

    Stops at the next heading of the same or higher level.
    Ignores heading-like lines inside fenced code blocks.
    Returns an empty string if the heading is not found.
    """
    prefix = "#" * level + " "
    lines = text.splitlines()
    start = None
    for i, line in enumerate(lines):
        if line.rstrip() == prefix + heading:
            start = i + 1
            break
    if start is None:
        return ""
    stop_re = re.compile(r"^#{1," + str(level) + r"} ")
    end = len(lines)
    in_fence = False
    for i in range(start, len(lines)):
        if lines[i].startswith("```"):
            in_fence = not in_fence
        if not in_fence and stop_re.match(lines[i]):
            end = i
            break
    return "\n".join(lines[start:end]).strip()


def read_doc(name: str) -> str:
    return (DOCS_DIR / name).read_text()


def bump_headings(text: str, by: int = 1) -> str:
    """Increase all markdown heading levels by `by` (e.g. ## → ###)."""

    def _bump(m: re.Match) -> str:
        return "#" * by + m.group(0)

    return re.sub(r"^(#+) ", _bump, text, flags=re.MULTILINE)


def build() -> str:
    readme = (REPO_ROOT / "README.md").read_text()
    concepts = read_doc("concepts.md")
    commands_doc = read_doc("commands.md")
    config_doc = read_doc("configuration.md")
    remotes_doc = read_doc("remotes.md")
    workflows_doc = read_doc("workflows.md")
    safety_doc = read_doc("safety.md")

    quick_start = extract_section(readme, "Quick start")

    # Strip leading preamble from corporate-environments table, keep only the table.
    corp_env_raw = extract_section(commands_doc, "Corporate environments", level=3)
    corp_env_lines = corp_env_raw.splitlines()
    table_start = next(
        (i for i, line in enumerate(corp_env_lines) if line.startswith("|")), 0
    )
    env_table = "\n".join(corp_env_lines[table_start:])

    commands_table = """\
| Command | Synopsis | Description |
|---------|----------|-------------|
| `init` | `git-sfs init [--force]` | Initialize `.git-sfs/config.toml` |
| `setup` | `git-sfs setup` | Bind local cache and restore symlinks (run after clone) |
| `add` | `git-sfs add <path>...` | Hash files and replace them with symlinks |
| `mv` | `git-sfs mv <src> <dst>` | Move a symlink, rewriting its relative target |
| `import` | `git-sfs import [--move] [-L] <src> <dst>` | Import external files into tracking |
| `verify` | `git-sfs verify [-r NAME] [path]` | Check integrity; exits non-zero on failure |
| `status` | `git-sfs status [-r NAME] [--json] [path]` | Show sizes and cache/remote presence |
| `remotes` | `git-sfs remotes [--json]` | List configured remotes |
| `push` | `git-sfs push [-r NAME] [--skip-missing] [path]` | Upload cached files to remote |
| `pull` | `git-sfs pull [-r NAME] [--verify] [path]` | Download missing files from remote |
| `doctor` | `git-sfs doctor [-r NAME]` | Diagnose configuration and remote problems |
| `self update` | `git-sfs self update [--pre]` | Update git-sfs and rclone; optionally include git-sfs prereleases |
| `llms-txt` | `git-sfs llms-txt` | Print this document |"""

    sections = [
        "# git-sfs",
        (
            "> CLI for storing large file bytes outside Git while Git tracks symlinks.\n"
            "> Primary use case: ML datasets, model checkpoints, and any large files\n"
            "> that must be versioned, shared across machines, and reproduced exactly.\n"
            ">\n"
            "> No LFS server. No database. No pointer files.\n"
            "> Git commits normal symlinks; bytes live in a local content-addressed\n"
            "> cache and sync to any rclone remote."
        ),
        "## Agent tips",
        (
            "- Run `git-sfs llms-txt` to print this document from the installed binary.\n"
            "- Run `git-sfs doctor` to diagnose configuration and connectivity without touching data.\n"
            "- Run `git-sfs status` to inspect tracked files and sizes without downloading.\n"
            "- Run `git-sfs verify` in CI to fail fast when files are missing or corrupt.\n"
            "- Large file bytes are never in Git. They live in the local cache and on rclone remotes.\n"
            "- After `git clone`, always run `git-sfs setup` then `git-sfs pull` to materialize files."
        ),
        "## Quick start",
        quick_start,
        "## Commands",
        commands_table,
        "## Global flags",
        (
            "```\n"
            "-j, --jobs N    max parallel workers; 0 = auto (overrides config n_jobs)\n"
            "--verbose       debug output to stderr\n"
            "--quiet         silence progress output\n"
            "--version       print release version\n"
            "```"
        ),
        "## Cache path resolution",
        (
            "```\n"
            "1. init/setup --cache binding\n"
            "2. .git-sfs/cache symlink (written by git-sfs setup)\n"
            "```"
        ),
        "## Environment variables",
        env_table,
        "## Concepts",
        bump_headings(concepts, by=1),
        "## Configuration reference",
        bump_headings(config_doc, by=1),
        "## Remotes",
        bump_headings(remotes_doc, by=1),
        "## Workflows",
        bump_headings(workflows_doc, by=1),
        "## Command reference",
        bump_headings(commands_doc, by=1),
        "## Safety",
        bump_headings(safety_doc, by=1),
    ]

    return "\n\n".join(s.strip() for s in sections) + "\n"


def main() -> None:
    content = build()
    for path in OUTPUTS:
        path.write_text(content)
        print(f"wrote {path.relative_to(REPO_ROOT)}")


if __name__ == "__main__":
    main()
