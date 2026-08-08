#!/usr/bin/env python3
"""Performance baselines, captured from the binary rather than from packages.

The classic regression for a tool of this shape is parallel work turning into
I/O contention. At TB scale a 2x slowdown is a serious user-facing regression,
so the benchmark compares binaries measured in the same run.

Everything here drives the CLI. Only the command surface is comparable across
implementations.

**Absolute numbers do not gate anything.** A millisecond count from one laptop
says nothing about another machine, so the gate is the *ratio* between two
binaries measured side by side in a single run:

    benchmark.py --binary old=./git-sfs --binary new=./target/release/git-sfs

Committed baselines are reference material -- they record what the shape of the
workload cost on a named machine, so a later run on that machine can be
sanity-checked. They are not the acceptance criterion.

Two tiers, because the costs are different and workflow scenarios use tiny
fixtures:

  count       many small objects; per-object overhead, locks, syscalls, walks
  throughput  one large object; the hashing hot path where the SHA-NI claim lives
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

from cache_state import object_dir
from harness import (
    Binary,
    chmod_writable,
    parse_binary,
    prepare_workspace,
    workspace_root,
)

HARNESS_DIR = Path(__file__).parent
SETUP = HARNESS_DIR / "bench-setup.sh"

DEFAULT_FILES = 1000
DEFAULT_FILE_BYTES = 1024
DEFAULT_LARGE_BYTES = 256 * 1024 * 1024
DEFAULT_REPETITIONS = 3

# Above the 1.08x self-check noise floor, below the contention regression 9b
# exists to catch. Re-measure with --binary a=X --binary b=X on the gating
# machine before trusting it; the floor is machine-specific.
DEFAULT_MAX_RATIO = 1.25

# Ordered for reporting; also the set a regression gate would consult.
OPERATIONS = ("add", "push", "pull", "verify", "rehash", "add_large")


class CommandFailed(RuntimeError):
    pass


def measure(context: dict, args: list[str]) -> float:
    """Wall time for one command, failing loudly rather than timing an error path.

    --quiet throughout: progress rendering is real work but it is also the part
    current replaces wholesale , so including it would
    measure the renderer rather than the operation.
    """
    env = dict(context["env"])
    started = time.perf_counter()
    completed = subprocess.run(
        [env["GIT_SFS"], "--quiet", *args],
        cwd=context["repo"],
        env=env,
        capture_output=True,
        text=True,
    )
    elapsed = time.perf_counter() - started
    if completed.returncode != 0:
        raise CommandFailed(
            f"{' '.join(args)} exited {completed.returncode}: {completed.stderr.strip()[:300]}"
        )
    return elapsed


def write_payload(directory: Path, count: int, size: int) -> None:
    """Distinct contents per file, so dedup does not collapse the workload.

    Identical bytes would make every file after the first a cache hit, which
    measures deduplication rather than ingest and would flatter any
    implementation that reaches the dedup check sooner.
    """
    directory.mkdir(parents=True, exist_ok=True)
    filler = os.urandom(size)
    for index in range(count):
        prefix = f"{index:016d}".encode()
        (directory / f"f{index:06d}.bin").write_bytes(prefix + filler[len(prefix) :])


def drop_cached_objects(cache: Path) -> None:
    chmod_writable(cache)
    shutil.rmtree(object_dir(cache), ignore_errors=True)


def count_tier(binary: Binary, root: Path, label: str, files: int, size: int) -> dict:
    """One workspace, five measurements, taken in the order a user would hit them."""
    context = prepare_workspace(binary, root / label, SETUP)
    write_payload(context["repo"] / "data", files, size)

    timings = {"add": measure(context, ["add", "data"])}
    subprocess.run(
        ["git", "add", "-A"], cwd=context["repo"], check=True, capture_output=True
    )
    subprocess.run(
        ["git", "commit", "-qm", "bench"],
        cwd=context["repo"],
        check=True,
        capture_output=True,
    )
    timings["push"] = measure(context, ["push"])
    drop_cached_objects(context["cache"])
    timings["pull"] = measure(context, ["pull"])
    timings["verify"] = measure(context, ["verify", "--no-check-remote"])
    timings["rehash"] = measure(context, ["verify", "--rehash"])
    return timings


def throughput_tier(binary: Binary, root: Path, label: str, size: int) -> dict:
    context = prepare_workspace(binary, root / label, SETUP)
    write_payload(context["repo"] / "data", 1, size)
    return {"add_large": measure(context, ["add", "data"])}


def best_of(samples: dict[str, list[float]]) -> dict[str, float]:
    """Minimum per operation.

    The fastest observed run is the one least polluted by scheduling noise,
    background load, and page-cache misses. Averaging would fold that noise into
    the number a gate later compares against.
    """
    return {name: min(values) for name, values in samples.items() if values}


def collect(binary: Binary, root: Path, args) -> dict[str, float]:
    samples: dict[str, list[float]] = {name: [] for name in OPERATIONS}
    for repetition in range(args.repetitions):
        prefix = f"{binary.name}-{repetition}"
        for name, value in count_tier(
            binary, root, f"count-{prefix}", args.files, args.file_bytes
        ).items():
            samples[name].append(value)
        for name, value in throughput_tier(
            binary, root, f"large-{prefix}", args.large_bytes
        ).items():
            samples[name].append(value)
        print(f"  {binary.name}: repetition {repetition + 1}/{args.repetitions} done")
    return best_of(samples)


def machine() -> dict:
    return {
        "platform": f"{platform.system()} {platform.machine()}",
        "release": platform.release(),
        "cpus": os.cpu_count(),
    }


def binary_version(binary: Binary) -> str:
    completed = subprocess.run(
        [str(binary.path), "--version"], capture_output=True, text=True
    )
    return completed.stdout.strip() or completed.stderr.strip() or "unknown"


def render(results: dict[str, dict[str, float]], baseline: str, args) -> str:
    names = list(results)
    header = f"{'operation':<12}" + "".join(f"{name:>14}" for name in names)
    if len(names) > 1:
        header += "".join(f"{name + ' vs ' + baseline:>18}" for name in names[1:])
    lines = [header, "-" * len(header)]
    for operation in OPERATIONS:
        if operation not in results[baseline]:
            continue
        row = f"{operation:<12}"
        for name in names:
            row += f"{results[name][operation] * 1000:>13.1f}ms"
        for name in names[1:]:
            ratio = results[name][operation] / results[baseline][operation]
            row += f"{ratio:>17.2f}x"
        lines.append(row)

    throughput = args.large_bytes / (1024 * 1024)
    lines.append("")
    for name in names:
        rate = throughput / results[name]["add_large"]
        lines.append(f"{name}: add of one {throughput:.0f} MiB file = {rate:.0f} MiB/s")
    return "\n".join(lines)


def exceeding(
    results: dict[str, dict[str, float]], baseline: str, limit: float
) -> list[tuple[str, str, float]]:
    """Operations where a binary is slower than the baseline by more than limit.

    Compares binaries measured in the same run rather than against a committed
    number, because an absolute time is a property of the machine that produced
    it.
    """
    over = []
    for name, timings in results.items():
        if name == baseline:
            continue
        for operation, seconds in timings.items():
            ratio = seconds / results[baseline][operation]
            if ratio > limit:
                over.append((name, operation, ratio))
    return over


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=parse_binary,
        action="append",
        required=True,
        metavar="NAME=PATH",
    )
    parser.add_argument("--files", type=int, default=DEFAULT_FILES)
    parser.add_argument("--file-bytes", type=int, default=DEFAULT_FILE_BYTES)
    parser.add_argument("--large-bytes", type=int, default=DEFAULT_LARGE_BYTES)
    parser.add_argument("--repetitions", type=int, default=DEFAULT_REPETITIONS)
    parser.add_argument("--json", type=Path, help="write the capture here")
    parser.add_argument(
        "--max-ratio",
        type=float,
        default=DEFAULT_MAX_RATIO,
        help="fail when any operation is this much slower than the first binary",
    )
    args = parser.parse_args()

    print(
        f"{args.files} x {args.file_bytes}B files, "
        f"one {args.large_bytes // (1024 * 1024)} MiB file, "
        f"best of {args.repetitions}"
    )
    root = workspace_root("git-sfs-bench-")
    results: dict[str, dict[str, float]] = {}
    try:
        for binary in args.binary:
            results[binary.name] = collect(binary, root, args)
    except CommandFailed as failure:
        sys.exit(f"benchmark: {failure}")
    finally:
        chmod_writable(root)
        shutil.rmtree(root, ignore_errors=True)

    baseline = args.binary[0].name
    print()
    print(render(results, baseline, args))

    regressions = exceeding(results, baseline, args.max_ratio)
    if regressions:
        print()
        for name, operation, ratio in regressions:
            print(
                f"REGRESSION {name}: {operation} is {ratio:.2f}x {baseline} "
                f"(limit {args.max_ratio:.2f}x)"
            )
        sys.exit(f"{len(regressions)} operation(s) past the threshold")

    if args.json:
        capture = {
            "machine": machine(),
            "workload": {
                "files": args.files,
                "file_bytes": args.file_bytes,
                "large_bytes": args.large_bytes,
                "repetitions": args.repetitions,
            },
            "versions": {b.name: binary_version(b) for b in args.binary},
            "seconds": results,
        }
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(capture, indent=2) + "\n")
        print(f"\nwrote {args.json}")


if __name__ == "__main__":
    main()
