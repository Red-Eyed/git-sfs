#!/usr/bin/env python3
"""What git-sfs does when a cache object's mode and its content disagree.

The read-only bit is not decoration: ordinary commands trust it as proof that
the bytes were hash-verified when written and therefore do not re-hash on every
access. Everything downstream inherits that trust.

The spec then names the environments where the bit can lie -- exFAT/FAT, some
FUSE and network mounts, SMB/NFS with unusual id mapping, Docker volume copies,
`rsync` without `-p`, archive extraction. Any of them can present unverified
bytes wearing a read-only bit.

Mounting exFAT in CI is not portable, so this harness injects the *state* such a
filesystem leaves behind rather than intercepting the chmod that produces it.
The three reachable combinations of (mode, content) are constructed directly and
each command's response is checked. What this deliberately cannot cover is a
chmod that silently fails at the moment of writing -- detecting that needs a
real interposer.

Usage:

    test/differential/mode_preservation.py --binary current=./target/release/git-sfs
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

from cache_state import (
    is_protected,
    object_path,
    overwrite,
    set_writable,
    sha256_of,
    tracked_object,
    trusted_but_wrong,
)
from harness import (
    Binary,
    Results,
    chmod_writable,
    parse_binary,
    prepare_workspace,
    workspace_root,
)

HARNESS_DIR = Path(__file__).parent
SETUP = HARNESS_DIR / "replicated-setup.sh"

TRACKED = "data/blob.bin"


def run(context: dict, args: list[str]) -> subprocess.CompletedProcess:
    env = dict(context["env"])
    env.pop("RCLONE_FAULTS", None)
    return subprocess.run(
        [env["GIT_SFS"], "--quiet", *args],
        cwd=context["repo"],
        env=env,
        capture_output=True,
        text=True,
    )


def rotted(size: int) -> bytes:
    """Same-length bytes that cannot hash to the original.

    Length is preserved so the corruption is invisible to any size comparison --
    which is the case that matters when a backend exposes size but not hashes.
    """
    return b"\xde\xad\xbe\xef" * (size // 4) + b"\x00" * (size % 4)


def fixture(binary: Binary, root: Path, case: str) -> tuple[dict, str, Path]:
    """A fresh replicated workspace, since every case mutates cache state."""
    context = prepare_workspace(binary, root / case, SETUP)
    digest = tracked_object(context["repo"], TRACKED)
    return context, digest, object_path(context["cache"], digest)


def test_writable_but_intact_is_migrated(binary: Binary, root: Path, r: Results):
    """A writable but intact cache object is verified once, then protected.

    A cache copied by something that dropped the mode looks exactly like this.
    The bytes are fine; only the bit is missing.
    """
    print(f"\n[{binary.name}] cache object writable, content intact")
    context, digest, obj = fixture(binary, root, "writable-intact")
    set_writable(obj)

    completed = run(context, ["pull"])
    r.check(completed.returncode == 0, "pull: succeeds against a writable object")
    r.check(
        is_protected(obj),
        "pull: re-protects the object in place rather than leaving it writable",
    )
    r.check(
        sha256_of(obj) == digest, "pull: the object still matches its hash afterwards"
    )
    context2, _, obj2 = fixture(binary, root, "writable-intact-verify")
    set_writable(obj2)
    verify = run(context2, ["verify", "--no-check-remote"])
    r.check(
        verify.returncode == 0,
        "verify: repairs a writable but intact object",
    )


def test_writable_and_rotted_is_repaired(binary: Binary, root: Path, r: Results):
    """Replication is the repair source for writable corrupted objects.

    The write bit is what makes this recoverable at all: it forces a re-hash,
    the re-hash fails, the object counts as missing, and pull re-fetches it. One
    copy means rot is fatal; two means it is repairable.
    """
    print(f"\n[{binary.name}] cache object writable, content rotted")
    context, digest, obj = fixture(binary, root, "writable-rotted")
    overwrite(obj, rotted(obj.stat().st_size))
    set_writable(obj)

    completed = run(context, ["pull"])
    r.check(
        completed.returncode == 0, "pull: succeeds against a rotted writable object"
    )
    r.check(sha256_of(obj) == digest, "pull: repairs the rotted object from the remote")
    r.check(is_protected(obj), "pull: the repaired object is protected again")
    r.check(
        not trusted_but_wrong(context["cache"]),
        "pull: leaves no read-only object mismatching its hash",
    )


def test_protected_but_rotted_is_trusted(binary: Binary, root: Path, r: Results):
    """The dangerous state: bytes that lie while the mode vouches.

    This is what a mode-dropping filesystem, an `rsync` without `-p`, or an
    archive extraction can hand you. Ordinary commands trust protected objects;
    explicit integrity paths must still catch the rot.
    """
    print(f"\n[{binary.name}] cache object protected, content rotted")
    context, digest, obj = fixture(binary, root, "protected-rotted")
    overwrite(obj, rotted(obj.stat().st_size))

    r.check(
        is_protected(obj) and sha256_of(obj) != digest,
        "fixture: the object is read-only and does not match its hash",
    )

    pull = run(context, ["pull"])
    repaired = sha256_of(obj) == digest
    r.observe(
        "pull against a protected rotted object",
        f"exit={pull.returncode} repaired={repaired}",
    )
    verify = run(context, ["verify", "--no-check-remote"])
    r.observe("verify (no integrity flags)", f"exit={verify.returncode}")

    integrity = run(context, ["verify", "--no-check-remote", "--with-integrity"])
    r.check(
        integrity.returncode != 0,
        "verify --with-integrity: detects the rot the mode bit hid",
    )
    rehash = run(context, ["verify", "--rehash"])
    r.check(rehash.returncode != 0, "verify --rehash: detects the rot the mode bit hid")


def test_push_replicates_rot(binary: Binary, root: Path, r: Results):
    """A protected rotted object must not overwrite a good remote replica."""
    print(f"\n[{binary.name}] push with a protected rotted object")
    context, digest, obj = fixture(binary, root, "push-rotted")
    remote_obj = object_path(context["remote"], digest)
    r.check(
        sha256_of(remote_obj) == digest,
        "fixture: the remote copy is good before the push",
    )
    overwrite(obj, rotted(obj.stat().st_size))

    push = run(context, ["push"])
    survived = remote_obj.is_file() and sha256_of(remote_obj) == digest
    r.check(
        push.returncode == 0 and survived,
        "push: a protected rotted local object must not overwrite a good remote copy",
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
    root = workspace_root("git-sfs-modes-")
    try:
        for binary in args.binary:
            test_writable_but_intact_is_migrated(binary, root, results)
            test_writable_and_rotted_is_repaired(binary, root, results)
            test_protected_but_rotted_is_trusted(binary, root, results)
            test_push_replicates_rot(binary, root, results)
    finally:
        chmod_writable(root)
        shutil.rmtree(root, ignore_errors=True)

    print(f"\n{results.asserts_passed} passed, {results.asserts_failed} failed")
    if results.asserts_failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
