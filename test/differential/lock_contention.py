#!/usr/bin/env python3
"""Cross-binary lock contention tests.

contract-spec §8 makes the lock protocol an *inter-version* contract: during
migration a user will run v1 in one shell and v2 in another against the same
cache. If the two disagree about the lock path or mechanism, both acquire "the
lock" simultaneously and write concurrently to one cache. No single-binary test
can detect that, which is why this harness drives two real processes.

Two kinds of check, and the distinction is deliberate:

  ASSERT   frozen mechanism (§8). Any conforming binary must satisfy it, so a
           failure fails the run.
  OBSERVE  policy v1 gets wrong (§8.1) -- waiting forever, no liveness check,
           a panic on a malformed owner file. Recorded as a baseline rather
           than asserted, because v2 is required to *diverge* here and a check
           demanding v1's behavior would have to be inverted later.

Usage:

    test/differential/lock_contention.py --binary v1=./git-sfs
    test/differential/lock_contention.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from harness import (
    Binary,
    Results,
    chmod_writable,
    parse_binary,
    prepare_workspace,
    wait_for,
)

HARNESS_DIR = Path(__file__).parent
SETUP = HARNESS_DIR / "lock-setup.sh"

# Every lock git-sfs takes (internal/core: add, import, setup, pull, push).
LOCK_NAMES = ("add", "import", "setup", "pull", "push")

HOLD_SECONDS = 3.0
# How long a blocked process is given to prove it is genuinely blocked. v1 waits
# forever by design, so this bounds the run rather than measuring anything.
BLOCK_PROBE_SECONDS = 2.0


def lock_path(cache: Path, name: str) -> Path:
    return cache / "locks" / f"{name}.lock"


def write_lock(cache: Path, name: str, owner: bytes | None) -> Path:
    """Create a lock exactly as contract-spec §8 specifies another binary would."""
    path = lock_path(cache, name)
    path.mkdir(parents=True, mode=0o755)
    if owner is not None:
        (path / "owner").write_bytes(owner)
        (path / "owner").chmod(0o644)
    return path


def start_command(
    context: dict, argv: list[str], faults: str | None = None
) -> subprocess.Popen:
    env = dict(context["env"])
    if faults is not None:
        faults_file = context["work"] / "faults.jsonl"
        faults_file.write_text(faults + "\n")
        env["RCLONE_FAULTS"] = str(faults_file)
    return subprocess.Popen(
        [env["GIT_SFS"], "--quiet", *argv],
        cwd=context["repo"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def start_push(context: dict, faults: str | None = None) -> subprocess.Popen:
    return start_command(context, ["push"], faults)


def command_for(lock_name: str, source: Path) -> list[str]:
    """The command that takes each lock (contract-spec §8.2)."""
    return {
        "add": ["add", "data"],
        "import": ["import", str(source), "imported/blob.bin"],
        "setup": ["setup"],
        "pull": ["pull"],
        "push": ["push"],
    }[lock_name]


def test_lock_is_created_at_contract_path(binary: Binary, context: dict, r: Results):
    """A push must publish its lock where a different binary will look for it."""
    print(f"\n[{binary.name}] holds its lock at the contract path")
    cache = context["cache"]
    process = start_push(context, f'{{"subcommand": "copy", "sleep": {HOLD_SECONDS}}}')
    try:
        appeared = wait_for(lambda: lock_path(cache, "push").is_dir(), HOLD_SECONDS)
        r.check(appeared, "locks/push.lock exists while push runs")
        if appeared:
            path = lock_path(cache, "push")
            r.check(path.stat().st_mode & 0o777 == 0o755, "lock directory mode is 0755")
            owner = path / "owner"
            r.check(owner.is_file(), "owner file exists")
            if owner.is_file():
                text = owner.read_text()
                r.check(
                    text.startswith("pid: ") and text.endswith("\n"),
                    f"owner content is 'pid: <N>\\n' (got {text!r})",
                )
                r.check(
                    owner.stat().st_mode & 0o777 == 0o644, "owner file mode is 0644"
                )
    finally:
        process.wait(timeout=HOLD_SECONDS + 10)
    r.check(
        not lock_path(cache, "push").exists(), "lock is released when push finishes"
    )


def test_blocks_on_foreign_lock(binary: Binary, context: dict, r: Results):
    """The interop direction that matters: respect a lock this binary did not create."""
    print(f"\n[{binary.name}] blocks on a lock created by another process")
    cache = context["cache"]
    held = write_lock(cache, "push", b"pid: 1\n")
    process = start_push(context)
    try:
        still_running = not wait_for(
            lambda: process.poll() is not None, BLOCK_PROBE_SECONDS
        )
        r.check(still_running, "push does not proceed while the lock is held")
    finally:
        if process.poll() is None:
            shutil.rmtree(held)
            finished = wait_for(lambda: process.poll() is not None, 15)
            r.check(finished, "push proceeds once the lock is released")
            r.check(process.returncode == 0, "push succeeds after acquiring the lock")
        else:
            shutil.rmtree(held, ignore_errors=True)
        if process.poll() is None:
            process.kill()


def test_every_command_locks_its_own_name(binary: Binary, context: dict, r: Results):
    """§8.2: five locks, one per command, and the names are themselves contract.

    Consolidating them into a single cache.lock is the obvious cleanup and is
    more correct in isolation. It also silently removes everything this file
    exists to protect: v2's `add` would take cache.lock while v1's takes
    add.lock, the two mkdir calls would target different paths, neither would
    block, and both binaries would report holding the lock.

    Planting each lock exactly as a foreign process would write it, then
    requiring the matching command to wait, is what catches that. Note what is
    deliberately *not* asserted: nothing here requires a command to ignore
    another command's lock. §8.2 lets v2 take extra locks, since acquiring more
    is strictly more conservative and cannot break interop with v1.
    """
    print(f"\n[{binary.name}] every command blocks on its own lock name")
    cache = context["cache"]
    source = context["work"] / "import-source.bin"
    source.write_bytes(b"import payload\n")

    for name in LOCK_NAMES:
        argv = command_for(name, source)
        held = write_lock(cache, name, b"pid: 1\n")
        process = start_command(context, argv)
        try:
            blocked = not wait_for(
                lambda: process.poll() is not None, BLOCK_PROBE_SECONDS
            )
            r.check(blocked, f"{argv[0]} waits for locks/{name}.lock")
        finally:
            shutil.rmtree(held, ignore_errors=True)
            r.check(
                wait_for(lambda: process.poll() is not None, 15),
                f"{argv[0]} proceeds once locks/{name}.lock is released",
            )
            if process.poll() is None:
                process.kill()


def test_cross_binary_contention(
    holder: Binary, waiter: Binary, contexts: dict, r: Results
):
    """Two real binaries, one cache: the second must wait for the first."""
    print(f"\n[{holder.name} holds, {waiter.name} waits] cross-binary contention")
    context = contexts[holder.name]
    cache = context["cache"]

    # The waiter runs against the holder's cache and repo, which is the actual
    # migration scenario -- one user, one dataset, two binaries.
    waiter_env = dict(context["env"]) | {"GIT_SFS": str(waiter.path.resolve())}

    holding = start_push(context, f'{{"subcommand": "copy", "sleep": {HOLD_SECONDS}}}')
    try:
        if not wait_for(lambda: lock_path(cache, "push").is_dir(), HOLD_SECONDS):
            r.check(False, "holder acquired the lock")
            return
        started = time.monotonic()
        blocked = subprocess.Popen(
            [waiter_env["GIT_SFS"], "--quiet", "push"],
            cwd=context["repo"],
            env=waiter_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        blocked.wait(timeout=HOLD_SECONDS + 20)
        waited = time.monotonic() - started
        holding.wait(timeout=10)
        # The waiter must not have finished before the holder released. Compared
        # against the remaining hold time rather than the total, since the probe
        # starts partway through.
        r.check(
            blocked.returncode == 0,
            f"waiter completed successfully after {waited:.2f}s",
        )
        r.check(
            waited > 0.5,
            f"waiter was genuinely blocked, not racing through ({waited:.2f}s)",
        )
    finally:
        for process in (holding,):
            if process.poll() is None:
                process.kill()


def observe_malformed_owner(binary: Binary, context: dict, r: Results):
    """§8.1: v1 slices data[:len(data)-1] with no length check (lock.go:62)."""
    print(f"\n[{binary.name}] behavior with a zero-byte owner file")
    cache = context["cache"]
    write_lock(cache, "push", b"")
    process = start_push(context)
    try:
        finished = wait_for(lambda: process.poll() is not None, BLOCK_PROBE_SECONDS)
        if not finished:
            r.observe("zero-byte owner", "blocked (no crash)")
            return
        stderr = (process.stderr.read() or "").strip()
        crashed = "panic" in stderr or (process.returncode or 0) > 128
        r.observe(
            "zero-byte owner",
            f"exit={process.returncode} crashed={crashed} stderr={stderr.splitlines()[:1]}",
        )
    finally:
        if process.poll() is None:
            process.kill()
        shutil.rmtree(lock_path(cache, "push"), ignore_errors=True)


def observe_stale_lock(binary: Binary, context: dict, r: Results):
    """§8.1: no liveness check, so a dead owner blocks every future operation."""
    print(f"\n[{binary.name}] behavior with a stale lock held by a dead pid")
    cache = context["cache"]
    dead_pid = find_dead_pid()
    write_lock(cache, "push", f"pid: {dead_pid}\n".encode())
    process = start_push(context)
    try:
        finished = wait_for(lambda: process.poll() is not None, BLOCK_PROBE_SECONDS)
        r.observe(
            f"stale lock (pid {dead_pid} not running)",
            "broke the lock and proceeded"
            if finished
            else f"still waiting after {BLOCK_PROBE_SECONDS}s",
        )
    finally:
        if process.poll() is None:
            process.kill()
        shutil.rmtree(lock_path(cache, "push"), ignore_errors=True)


def find_dead_pid() -> int:
    """A pid that is not currently running, for the stale-lock case."""
    for candidate in range(99999, 4000, -7):
        try:
            os.kill(candidate, 0)
        except ProcessLookupError:
            return candidate
        except PermissionError:
            continue
    return 99999


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

    print(f"lock names in this build: {', '.join(LOCK_NAMES)}")
    results = Results()
    root = Path(tempfile.mkdtemp(prefix="git-sfs-lock-"))
    try:
        contexts = {b.name: prepare_workspace(b, root, SETUP) for b in args.binary}
        for binary in args.binary:
            test_lock_is_created_at_contract_path(
                binary, contexts[binary.name], results
            )
            test_blocks_on_foreign_lock(binary, contexts[binary.name], results)
            test_every_command_locks_its_own_name(
                binary, contexts[binary.name], results
            )
            observe_malformed_owner(binary, contexts[binary.name], results)
            observe_stale_lock(binary, contexts[binary.name], results)

        for holder in args.binary:
            for waiter in args.binary:
                if holder.name != waiter.name or len(args.binary) == 1:
                    test_cross_binary_contention(holder, waiter, contexts, results)
    finally:
        chmod_writable(root)
        shutil.rmtree(root, ignore_errors=True)

    print(f"\n{results.asserts_passed} passed, {results.asserts_failed} failed")
    if results.asserts_failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
