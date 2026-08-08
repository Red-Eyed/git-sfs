# Conformance Harness

The harness runs real `git-sfs` binaries against shared scenarios and records
the filesystem state they leave behind. It is used both as a self-check for the
current binary and as a compatibility check when another binary is supplied.

## What It Compares

The manifest records stable behavior:

- symlink targets
- cache paths and layout
- file and directory permission bits
- content hashes
- remote object layout
- per-command success vs failure

It deliberately ignores human output, mtimes, `.git` internals, and symlink mode
bits that differ across platforms.

## Usage

```sh
just differential
test/differential/run.py --binary a=./target/release/git-sfs --binary b=./target/release/git-sfs
test/differential/run.py --binary current=./target/release/git-sfs --scenario 02 --keep
```

Every specialized entry point accepts one or more `--binary NAME=PATH` values:

```sh
just lock-contention
just cancellation
just mode-preservation
just downgrade
just spec-coverage
```

## Layout

```text
snapshot.py            tree -> canonical manifest
harness.py             binaries, workspaces, polling
cache_state.py         cache queries: object paths, modes, hashes
divergences.py         known behavior fixes declared in advance
coverage.py            contract clause -> assertion evidence map
run.py                 scenario runner and manifest differ
lib.sh                 shared shell helpers
scenarios/             one scenario per file
fake-rclone/           recording and fault-injecting rclone stand-in
replicated-setup.sh    fixture with local and remote object copies
lock_contention.py     cross-process lock checks
cancellation.py        SIGINT safety checks
mode_preservation.py   cache mode/content checks
downgrade.py           stable-state handoff checks
benchmark.py           optional performance comparison
```

The fake rclone is intentionally below the adapter boundary. High-level tests
assert `git-sfs` behavior; adapter tests may assert specific rclone argv.
