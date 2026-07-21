#!/usr/bin/env python3
"""Downgrade as an invariant: v1 must still operate anything v2 has touched.

rust-rewrite-plan §7c. Users self-update *forward* and nothing in v1 goes back,
so if v2 ships a defect that touches data the exit path has to already exist --
writing one afterwards is too late, because the affected users are exactly the
ones who cannot afford to experiment.

The invariant is nearly free given decisions already made (no version floor,
identical cache/remote/lock layouts), and stating it as an invariant is what
makes it *testable* rather than incidental.

Handoff, not concurrency: one binary does a full workflow, the other takes over
the same repo, cache, and remote. That is a different question from the tree
diff (each binary in its own workspace) and from lock contention (two processes
at once), and neither of those can see it.

With two binaries both directions run, since the upgrade path has to work too.
With one it degrades to a self-check that still exercises every handoff step.

Usage:

    test/differential/downgrade.py --binary v1=./git-sfs
    test/differential/downgrade.py --binary v2=./target/release/git-sfs --binary v1=./git-sfs
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from cache_state import object_dir, object_path, sha256_of, tracked_object
from harness import Binary, Results, chmod_writable, parse_binary, prepare_workspace

HARNESS_DIR = Path(__file__).parent
SETUP = HARNESS_DIR / "replicated-setup.sh"

TRACKED = "data/blob.bin"

# A plausible v2-only cache addition (rust-rewrite-plan §7). Spec §4 calls it
# additive and migration-safe; this harness is where that stops being a claim.
TRASH_BATCH = "20260721T000000Z"


def as_binary(context: dict, binary: Binary) -> dict:
    """The same workspace, driven by a different binary."""
    return dict(context["env"]) | {"GIT_SFS": str(binary.path.resolve())}


def run(context: dict, binary: Binary, args: list[str]) -> subprocess.CompletedProcess:
    env = as_binary(context, binary)
    env.pop("RCLONE_FAULTS", None)
    return subprocess.run(
        [env["GIT_SFS"], "--quiet", *args],
        cwd=context["repo"],
        env=env,
        capture_output=True,
        text=True,
    )


def commit(context: dict, message: str) -> None:
    subprocess.run(
        ["git", "add", "-A"], cwd=context["repo"], check=True, capture_output=True
    )
    subprocess.run(
        ["git", "commit", "-qm", message],
        cwd=context["repo"],
        check=True,
        capture_output=True,
    )


def test_reads_foreign_state(writer: Binary, reader: Binary, root: Path, r: Results):
    """Everything the writer left behind must be legible to the reader.

    Covers the four artifacts §7c names: cache, symlinks, config, and remote.
    """
    print(f"\n[{writer.name} writes, {reader.name} reads] existing state")
    context = prepare_workspace(
        writer, root / f"read-{writer.name}-{reader.name}", SETUP
    )
    digest = tracked_object(context["repo"], TRACKED)

    for command in (
        ["verify", "--no-check-remote"],
        ["verify"],
        ["status"],
        ["remotes"],
        ["doctor"],
    ):
        completed = run(context, reader, command)
        r.check(
            completed.returncode == 0,
            f"{' '.join(command)}: succeeds against {writer.name}'s state",
        )

    r.check(
        sha256_of(object_path(context["cache"], digest)) == digest,
        "the cached object is untouched by the handoff",
    )


def test_pull_from_foreign_remote(
    writer: Binary, reader: Binary, root: Path, r: Results
):
    """The remote layout is shared across versions, not just across workspaces."""
    print(f"\n[{writer.name} pushes, {reader.name} pulls] remote layout")
    context = prepare_workspace(
        writer, root / f"pull-{writer.name}-{reader.name}", SETUP
    )
    digest = tracked_object(context["repo"], TRACKED)

    chmod_writable(context["cache"])
    shutil.rmtree(object_dir(context["cache"]), ignore_errors=True)

    completed = run(context, reader, ["pull"])
    r.check(completed.returncode == 0, f"pull: recovers from {writer.name}'s remote")
    restored = object_path(context["cache"], digest)
    r.check(
        restored.is_file() and sha256_of(restored) == digest,
        "the pulled object matches the hash naming it",
    )


def test_round_trip(writer: Binary, reader: Binary, root: Path, r: Results):
    """The strongest form: each binary consumes what the other just produced."""
    print(f"\n[{writer.name} then {reader.name} then {writer.name}] round trip")
    context = prepare_workspace(
        writer, root / f"trip-{writer.name}-{reader.name}", SETUP
    )

    payload = b"added by the second binary\n" * 64
    (context["repo"] / "data" / "second.bin").write_bytes(payload)
    added = run(context, reader, ["add", "data"])
    r.check(
        added.returncode == 0, f"{reader.name}: add succeeds in {writer.name}'s repo"
    )
    if added.returncode != 0:
        return
    commit(context, "second binary adds a file")

    pushed = run(context, reader, ["push"])
    r.check(
        pushed.returncode == 0,
        f"{reader.name}: push succeeds to {writer.name}'s remote",
    )

    digest = tracked_object(context["repo"], "data/second.bin")
    r.check(
        sha256_of(object_path(context["remote"], digest)) == digest,
        f"{reader.name}'s upload lands at the layout {writer.name} expects",
    )

    # Back to the first binary: it must accept the second's work, including
    # after losing the local copy and having to refetch it.
    chmod_writable(context["cache"])
    shutil.rmtree(object_dir(context["cache"]), ignore_errors=True)
    recovered = run(context, writer, ["pull"])
    r.check(
        recovered.returncode == 0,
        f"{writer.name}: pull recovers {reader.name}'s object",
    )
    r.check(
        run(context, writer, ["verify", "--no-check-remote"]).returncode == 0,
        f"{writer.name}: verify accepts the round-tripped tree",
    )


def test_trash_stays_ignorable(binary: Binary, root: Path, r: Results):
    """v2's `trash/` must be invisible to v1, or §7c is broken on arrival.

    Provable before v2 exists, because the claim is about *v1's* behavior:
    countOrphans walks only files/sha256 and PurgeTmp touches only tmp/. This
    plants the directory and checks that the most invasive walks leave it alone.
    """
    print(f"\n[{binary.name}] a v2-style trash/ directory in the cache")
    context = prepare_workspace(binary, root / f"trash-{binary.name}", SETUP)
    digest = tracked_object(context["repo"], TRACKED)

    evicted = context["cache"] / "trash" / TRASH_BATCH / digest[:2] / digest
    evicted.parent.mkdir(parents=True)
    payload = object_path(context["cache"], digest).read_bytes()
    evicted.write_bytes(payload)
    evicted.chmod(0o444)

    for command in (["verify"], ["status"], ["pull"], ["push"], ["verify", "--rehash"]):
        completed = run(context, binary, command)
        r.check(
            completed.returncode == 0,
            f"{' '.join(command)}: unaffected by a trash/ directory",
        )
    r.check(evicted.is_file(), "the trash object still exists")
    r.check(sha256_of(evicted) == digest, "the trash object is byte-identical")
    r.check(
        evicted.stat().st_mode & 0o222 == 0,
        "the trash object keeps its read-only bit, so a restore needs no re-hash",
    )


def test_unknown_config_keys_are_rejected(binary: Binary, root: Path, r: Results):
    """The constraint §7c does not currently state, pinned so v2 cannot trip it.

    config.go:188-245 rejects unknown sections, top-level fields, settings
    fields, and remote fields with ErrInvalidConfig. So **any** config key v2
    invents makes the config unreadable by v1 the moment it is written -- and
    config.toml is committed, so it reaches every clone. Recorded as an ASSERT
    of v1's actual behavior because that is what constrains v2's design.
    """
    print(f"\n[{binary.name}] unknown config keys")
    context = prepare_workspace(binary, root / f"config-{binary.name}", SETUP)
    config = context["repo"] / ".git-sfs" / "config.toml"
    original = config.read_text()

    variants = {
        "top-level field": original + "\nfuture_field = true\n",
        "section": original + "\n[future]\nenabled = true\n",
        "settings field": original.replace(
            "n_jobs = 0", "n_jobs = 0\nfuture_setting = 1"
        ),
        "remote field": original.replace(
            'backend = "local"', 'backend = "local"\nfuture_option = true'
        ),
    }
    for label, text in variants.items():
        config.write_text(text)
        r.check(
            run(context, binary, ["verify", "--no-check-remote"]).returncode != 0,
            f"an unknown {label} is rejected, so v2 must not add one",
        )

    config.write_text(original)
    r.check(
        run(context, binary, ["verify", "--no-check-remote"]).returncode == 0,
        "the unmodified config still works, so the checks above were not vacuous",
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=parse_binary,
        action="append",
        required=True,
        metavar="NAME=PATH",
    )
    args = parser.parse_args()

    results = Results()
    root = Path(tempfile.mkdtemp(prefix="git-sfs-downgrade-"))
    try:
        # Both directions when two binaries are given: downgrade is the one that
        # matters, but an upgrade that cannot read old state is equally fatal.
        pairs = [(a, b) for a in args.binary for b in args.binary if a.name != b.name]
        if not pairs:
            only = args.binary[0]
            pairs = [(only, only)]
        for writer, reader in pairs:
            test_reads_foreign_state(writer, reader, root, results)
            test_pull_from_foreign_remote(writer, reader, root, results)
            test_round_trip(writer, reader, root, results)
        for binary in args.binary:
            test_trash_stays_ignorable(binary, root, results)
            test_unknown_config_keys_are_rejected(binary, root, results)
    finally:
        chmod_writable(root)
        shutil.rmtree(root, ignore_errors=True)

    print(f"\n{results.asserts_passed} passed, {results.asserts_failed} failed")
    if results.asserts_failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
