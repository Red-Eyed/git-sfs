#!/usr/bin/env python3
"""What git-sfs does when a cache object's mode and its content disagree.

contract-spec §4.1 calls the read-only bit "the single most dangerous invariant
in the contract", because it is not decoration: HasValid treats a stripped write
bit as *proof the bytes were hash-verified when written* and therefore never
re-hashes. Everything downstream inherits that trust.

The spec then names the environments where the bit can lie -- exFAT/FAT, some
FUSE and network mounts, SMB/NFS with unusual id mapping, Docker volume copies,
`rsync` without `-p`, archive extraction. Any of them can present unverified
bytes wearing a read-only bit.

Mounting exFAT in CI is not portable, so this harness injects the *state* such a
filesystem leaves behind rather than intercepting the chmod that produces it.
The three reachable combinations of (mode, content) are constructed directly and
each command's response is checked. What this deliberately cannot cover is a
chmod that silently fails at the moment of writing -- detecting that needs a
real interposer, and only matters once v2 ships the §7b "does the cache
filesystem preserve modes" probe. See README.

Usage:

    test/differential/mode_preservation.py --binary v1=./git-sfs
    test/differential/mode_preservation.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
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
from harness import Binary, Results, chmod_writable, parse_binary, prepare_workspace

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
    which is the case that matters, since §13.4 notes `--checksum` degrades to
    size+modtime on backends exposing no hash.
    """
    return b"\xde\xad\xbe\xef" * (size // 4) + b"\x00" * (size % 4)


def fixture(binary: Binary, root: Path, case: str) -> tuple[dict, str, Path]:
    """A fresh replicated workspace, since every case mutates cache state."""
    context = prepare_workspace(binary, root / case, SETUP)
    digest = tracked_object(context["repo"], TRACKED)
    return context, digest, object_path(context["cache"], digest)


def test_writable_but_intact_is_migrated(binary: Binary, root: Path, r: Results):
    """The legacy migration path §4.1 marks MUST: verify by hash, then protect.

    A cache written by an older version, or copied by something that dropped the
    mode, looks exactly like this. The bytes are fine; only the bit is missing.
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
    # verify takes the opposite line -- it refuses rather than migrating
    # (verify.go:245 raises ErrWrongCachePermissions). Which of the two is right
    # is policy, so it is recorded rather than asserted.
    context2, _, obj2 = fixture(binary, root, "writable-intact-verify")
    set_writable(obj2)
    verify = run(context2, ["verify", "--no-check-remote"])
    r.observe(
        "verify on a writable but intact object",
        f"exit={verify.returncode}",
    )


def test_writable_and_rotted_is_repaired(binary: Binary, root: Path, r: Results):
    """Replication as the repair source (rust-rewrite-plan §8).

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
    """The hazard §4.1 exists to name: bytes that lie while the mode vouches.

    This is what a mode-dropping filesystem, an `rsync` without `-p`, or an
    archive extraction can hand you. v1 trusts it permanently, so plain commands
    are OBSERVE. The explicit integrity paths are ASSERT -- whatever a version
    decides about trusting the bit, a command whose entire job is re-hashing
    must still catch this.
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
    """contract-spec §13.4: push overwrites a good replica with rotted bytes.

    Found by this harness. push admits an object on HasValid alone, which for a
    read-only file is the §4.1 mode bit and nothing else, and CopyToRemote omits
    --ignore-existing -- so the upload overwrites. The tier that exists to repair
    the other is destroyed by the damaged one, and the command exits 0.

    OBSERVE rather than ASSERT only because v1 is the binary under test today and
    the baseline has to stay green. It is enumerated in §13.4, so the Phase 0
    item "encode each §13 divergence as a positive assertion" is what flips this
    to an ASSERT of v2's behavior.
    """
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
    r.observe(
        "push with a protected rotted object",
        f"exit={push.returncode} good remote copy survived={survived}",
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
    root = Path(tempfile.mkdtemp(prefix="git-sfs-modes-"))
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
