#!/usr/bin/env python3

import importlib.util
import unittest
from pathlib import Path
from types import ModuleType


ROOT = Path(__file__).resolve().parents[2]


def load_publisher() -> ModuleType:
    script = ROOT / "scripts" / "publish-release.py"
    spec = importlib.util.spec_from_file_location("publish_release", script)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {script}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PublishReleaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.publisher = load_publisher()

    def test_prerelease_uses_base_changelog_when_exact_section_is_absent(self) -> None:
        release = self.publisher.parse_release_spec("v2.0.0-rc.1")
        changelog = "# Changelog\n\n## v2.0.0\n\n### Highlights\n\n- Rust release.\n"

        section = self.publisher.extract_changelog_section(changelog, release)

        self.assertIn("Rust release.", section)

    def test_prerelease_is_non_latest_and_requires_an_existing_tag(self) -> None:
        release = self.publisher.parse_release_spec("v2.0.0-rc.1")

        command = self.publisher.build_release_command(release, [], "notes.md")

        self.assertIn("--prerelease", command)
        self.assertIn("--latest=false", command)
        self.assertIn("--verify-tag", command)

    def test_stable_release_is_not_marked_prerelease(self) -> None:
        release = self.publisher.parse_release_spec("v2.0.0")

        command = self.publisher.build_release_command(release, [], "notes.md")

        self.assertNotIn("--prerelease", command)
        self.assertNotIn("--latest=false", command)

    def test_install_notes_pin_the_release_tag(self) -> None:
        release = self.publisher.parse_release_spec("v2.0.0-rc.1")

        notes = self.publisher.build_notes(release, "owner/repo", "notes")

        self.assertIn("sh -s -- --version v2.0.0-rc.1", notes)

    def test_invalid_release_tag_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            self.publisher.parse_release_spec("v2")


if __name__ == "__main__":
    unittest.main()
