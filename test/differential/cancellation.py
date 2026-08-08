#!/usr/bin/env python3
"""SIGINT driver: interrupt a transfer in flight and check what survives.

AGENTS.md makes cancellation a safety requirement, not a convenience --
"cancellation must leave state consistent: never publish a partial file, and
surface the interrupt as a clean cancellation, not a corrupt result." Nothing
else in the suite tests it, and rust-rewrite-plan §5.3 lists it first among the
checks most likely to be skipped and most likely to lose data.

This is a separate entry point rather than a scenario in run.py because an
interrupt lands at a point neither binary controls. The tree afterwards
legitimately differs between two runs of one binary, let alone between v1 and
v2, so there is nothing to diff. What is stable is the set of invariants that
must hold no matter where the interrupt landed -- so those are asserted
directly.

Usage:

    test/differential/cancellation.py --binary v1=./git-sfs
    test/differential/cancellation.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
"""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
from pathlib import Path

from cache_state import (
    held_locks,
    object_dir,
    object_path,
    sha256_of,
    stray_files,
    tracked_object,
    trusted_but_wrong,
)
from harness import (
    Binary,
    Results,
    chmod_writable,
    parse_binary,
    prepare_workspace,
    wait_for,
    workspace_root,
)

HARNESS_DIR = Path(__file__).parent
SETUP = HARNESS_DIR / "replicated-setup.sh"

# 128 = SIGINT's 130 by the shell's 128+signal convention. contract-spec §9 keeps
# this frozen even though the rest of the taxonomy is v2's to redesign.
SIGINT_EXIT = 130

# How long a stalled rclone holds its half-written object open. Only has to
# outlast the poll that spots the file plus the signal delivery.
STALL_SECONDS = 5.0
SYNC_TIMEOUT = 20.0
SHUTDOWN_TIMEOUT = 30.0

# Big enough that hashing and copying it take long enough to interrupt, small
# enough not to dominate the run. Built from a repeating block rather than read
# wholesale from /dev/urandom, which is the slow part at this size.
ADD_FIXTURE_BYTES = 128 * 1024 * 1024


def start(
    context: dict, args: list[str], faults: str | None = None
) -> subprocess.Popen:
    env = dict(context["env"])
    if faults is not None:
        faults_file = context["work"] / "faults.jsonl"
        faults_file.write_text(faults + "\n")
        env["RCLONE_FAULTS"] = str(faults_file)
    else:
        env.pop("RCLONE_FAULTS", None)
    return subprocess.Popen(
        [env["GIT_SFS"], "--quiet", *args],
        cwd=context["repo"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def run_to_completion(context: dict, args: list[str]) -> subprocess.CompletedProcess:
    env = dict(context["env"])
    env.pop("RCLONE_FAULTS", None)
    return subprocess.run(
        [env["GIT_SFS"], "--quiet", *args],
        cwd=context["repo"],
        env=env,
        capture_output=True,
        text=True,
    )


def interrupt(process: subprocess.Popen) -> tuple[int, str, str]:
    """Send SIGINT to git-sfs alone and collect how it exited.

    Deliberately not signalling the process group. A real Ctrl-C would also hit
    the rclone child directly, which would mask whether git-sfs propagates
    cancellation to its own subprocess -- and propagation is the property under
    test (the child is spawned with exec.CommandContext for exactly this reason).
    """
    process.send_signal(signal.SIGINT)
    try:
        stdout, stderr = process.communicate(timeout=SHUTDOWN_TIMEOUT)
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        return -1, stdout, stderr
    return process.returncode, stdout, stderr


def assert_clean_cancellation(
    r: Results, label: str, status: int, stdout: str, stderr: str
) -> None:
    r.check(status == SIGINT_EXIT, f"{label}: exits {SIGINT_EXIT} (got {status})")
    # §9 freezes the stream and the "git-sfs: " prefix; the wording after it is
    # free, so the assertion stops there.
    r.check(
        stderr.startswith("git-sfs: ") or "\ngit-sfs: " in stderr,
        f"{label}: reports on stderr with the git-sfs prefix",
    )
    r.check(stdout.strip() == "", f"{label}: prints nothing to stdout")


def assert_no_trusted_corruption(
    r: Results, label: str, when: str, cache: Path
) -> None:
    """Checked at every observation point, not just after the interrupt.

    A regression can publish the bad object during *recovery* rather than during
    the cancelled run -- an unlink that stops happening turns the retry itself
    into the step that protects a partial file. Asserting only at the interrupt
    misses that entirely, which a mutant proved before this took a `when`.
    """
    wrong = trusted_but_wrong(cache)
    r.check(
        not wrong,
        f"{label}: no read-only object mismatches its hash {when} "
        f"({[p.name[:12] for p in wrong]})",
    )


def observe_residue(r: Results, label: str, cache: Path) -> None:
    strays = stray_files(cache)
    r.check(
        not strays,
        f"{label}: no temp files are left inside files/sha256 ({len(strays)})",
    )
    staged = list((cache / "tmp").glob("*")) if (cache / "tmp").is_dir() else []
    r.observe(f"{label}: files left in cache tmp/", str(len(staged)))
    r.observe(f"{label}: locks still held", str(held_locks(cache) or "none"))


def staged_cache_files(cache: Path) -> list[Path]:
    tmp = cache / "tmp"
    if not tmp.is_dir():
        return []
    return [path for path in tmp.rglob("*") if path.is_file()]


def test_pull_interrupted(binary: Binary, context: dict, r: Results) -> None:
    """The dangerous direction: pull must never stream into the final cache path."""
    print(f"\n[{binary.name}] SIGINT during pull, mid-download")
    cache = context["cache"]
    chmod_writable(cache)
    shutil.rmtree(object_dir(cache), ignore_errors=True)

    expected = tracked_object(context["repo"], "data/blob.bin")
    target = object_path(cache, expected)
    staged_pattern = (
        cache / "tmp" / "git-sfs-pull-*" / "files" / "sha256" / expected[:2] / expected
    )

    process = start(
        context, ["pull"], f'{{"subcommand": "copy", "stall": {STALL_SECONDS}}}'
    )
    if binary.name == "v1":
        caught = wait_for(target.exists, SYNC_TIMEOUT)
    else:
        caught = wait_for(
            lambda: bool(list(cache.glob(str(staged_pattern.relative_to(cache))))),
            SYNC_TIMEOUT,
        )
    r.check(caught, "pull: interrupt landed while the object was half-written")
    status, stdout, stderr = interrupt(process)

    assert_clean_cancellation(r, "pull", status, stdout, stderr)
    if binary.name == "v1":
        r.observe(
            "pull: final cache object after interrupted download",
            "present" if target.exists() else "absent",
        )
    else:
        r.check(
            not target.exists(),
            "pull: final cache object is absent after an interrupted staged download",
        )
    assert_no_trusted_corruption(r, "pull", "after the interrupt", cache)
    observe_residue(r, "pull", cache)

    # Recoverability is the point of a clean cancellation: an interrupted pull
    # must leave nothing that stops the next one from finishing the job. The
    # retry passes --ignore-existing (command.go:272), so it would skip the
    # half-written file outright were it not for the explicit unlink of partial
    # objects in pullMissingFiles (pull.go:79-87). That unlink is the mechanism
    # under test here; without it this assertion fails.
    completed = run_to_completion(context, ["pull"])
    r.check(completed.returncode == 0, "pull: re-running after the interrupt succeeds")
    r.check(
        target.is_file() and sha256_of(target) == expected,
        "pull: the recovered object matches its hash",
    )
    r.check(
        target.is_file() and target.stat().st_mode & 0o222 == 0,
        "pull: the recovered object is read-only (§4.1)",
    )
    assert_no_trusted_corruption(r, "pull", "after the recovery", cache)


def test_push_interrupted(binary: Binary, context: dict, r: Results) -> None:
    """A partial upload must live in remote tmp, never at the final object path."""
    print(f"\n[{binary.name}] SIGINT during push, mid-upload")
    cache, remote = context["cache"], context["remote"]
    shutil.rmtree(object_dir(remote), ignore_errors=True)

    expected = tracked_object(context["repo"], "data/blob.bin")
    landing = object_path(remote, expected)
    staged_pattern = remote / "tmp" / "*" / "files" / "sha256" / expected[:2] / expected

    process = start(
        context, ["push"], f'{{"subcommand": "copy", "stall": {STALL_SECONDS}}}'
    )
    if binary.name == "v1":
        caught = wait_for(landing.exists, SYNC_TIMEOUT)
    else:
        caught = wait_for(
            lambda: bool(list(remote.glob(str(staged_pattern.relative_to(remote))))),
            SYNC_TIMEOUT,
        )
    r.check(caught, "push: interrupt landed while the object was half-written")
    status, stdout, stderr = interrupt(process)

    assert_clean_cancellation(r, "push", status, stdout, stderr)
    assert_no_trusted_corruption(r, "push", "after the interrupt", cache)
    observe_residue(r, "push", cache)
    if binary.name == "v1":
        if landing.is_file():
            r.observe(
                "push: final remote object after the interrupt",
                "truncated" if sha256_of(landing) != expected else "complete",
            )
        else:
            r.observe("push: final remote object after the interrupt", "absent")
    else:
        r.check(
            not landing.exists(),
            "push: final remote object is absent after an interrupted staged upload",
        )
        staged = list(remote.glob(str(staged_pattern.relative_to(remote))))
        if staged:
            r.observe(
                "push: staged remote object after the interrupt",
                "truncated" if sha256_of(staged[0]) != expected else "complete",
            )

    completed = run_to_completion(context, ["push"])
    r.check(completed.returncode == 0, "push: re-running after the interrupt succeeds")
    if binary.name == "v1":
        if landing.is_file():
            r.observe(
                "push: remote object after the recovery push",
                "repaired" if sha256_of(landing) == expected else "STILL TRUNCATED",
            )
    else:
        r.check(
            landing.is_file() and sha256_of(landing) == expected,
            "push: recovery publishes a complete final remote object",
        )
    assert_no_trusted_corruption(r, "push", "after the recovery", cache)


def test_add_interrupted(binary: Binary, context: dict, r: Results) -> None:
    """The user's only copy is in play here, so absence is the failure to hunt.

    add removes the source and then creates the symlink (§13.1), so an interrupt
    in the wrong place leaves a path with neither. The assertion is written
    against the spec rather than against v1: the file must be readable
    afterwards, one way or the other.
    """
    print(f"\n[{binary.name}] SIGINT during add, mid-ingest")
    repo, cache = context["repo"], context["cache"]
    source = repo / "data" / "large.bin"
    write_fixture(source, ADD_FIXTURE_BYTES)
    expected = sha256_of(source)

    process = start(context, ["add", "data"])
    if binary.name == "v1":
        # v1 stages inside files/sha256.
        caught = wait_for(lambda: bool(stray_files(cache)), SYNC_TIMEOUT)
    else:
        # v2 stages local writes inside cache tmp/.
        caught = wait_for(lambda: bool(staged_cache_files(cache)), SYNC_TIMEOUT)
    status, stdout, stderr = interrupt(process)
    r.observe(
        "add: interrupt landed during the copy pass",
        "yes" if caught else "no (hash pass or later)",
    )

    assert_clean_cancellation(r, "add", status, stdout, stderr)
    assert_no_trusted_corruption(r, "add", "after the interrupt", cache)
    observe_residue(r, "add", cache)

    r.check(
        source.exists(),
        "add: the source path still resolves -- bytes were not left nowhere",
    )
    if source.exists():
        r.check(
            sha256_of(source) == expected,
            "add: the source still reads back as its original content",
        )

    completed = run_to_completion(context, ["add", "data"])
    r.check(completed.returncode == 0, "add: re-running after the interrupt succeeds")
    stored = object_path(cache, expected)
    r.check(stored.is_file(), "add: the object lands in the cache on the retry")
    assert_no_trusted_corruption(r, "add", "after the recovery", cache)


def write_fixture(path: Path, size: int) -> None:
    block = os.urandom(1024 * 1024)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as handle:
        for _ in range(size // len(block)):
            handle.write(block)


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
    root = workspace_root("git-sfs-cancel-")
    try:
        for binary in args.binary:
            context = prepare_workspace(binary, root, SETUP)
            test_pull_interrupted(binary, context, results)
            test_push_interrupted(binary, context, results)
            test_add_interrupted(binary, context, results)
    finally:
        chmod_writable(root)
        shutil.rmtree(root, ignore_errors=True)

    print(f"\n{results.asserts_passed} passed, {results.asserts_failed} failed")
    if results.asserts_failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
