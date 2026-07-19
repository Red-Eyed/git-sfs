# Differential harness

Runs the same scenario against two or more git-sfs binaries and diffs what each
one left behind. Built for the Rust rewrite (see
[../../docs/rust-rewrite-plan.md](../../docs/rust-rewrite-plan.md) §5), where v1
and v2 must agree on the conformance contract while being free to differ
everywhere else.

## What it compares, and why only this

Contract-only parity unfreezes human output, the exit-code taxonomy, and the
`status --json` schema (contract-spec §12). What remains cross-checkable between
two binaries is **filesystem state plus whether each command succeeded** — so
that is exactly what the manifest records:

| Recorded | Frozen by |
|---|---|
| Symlink targets, verbatim | contract-spec §3.1 |
| Cache paths and layout | §4 |
| File and directory permission bits | §4.1 — write bits are load-bearing |
| Content hashes | §4.2 |
| Remote object layout | §5 |
| Per-command exit status | §9 — only `0` vs non-zero is contract |

Deliberately **not** recorded: human output, mtimes, `.git` internals, and
symlink permission bits (Linux reports `0o777`, macOS `0o755`; the difference is
meaningless and would fire on every cross-platform run).

## Usage

```sh
# Compare two binaries.
test/differential/run.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs

# Self-check: same binary twice must always agree. Guards the harness itself
# against nondeterminism creeping in.
just differential

# Narrow to one scenario and keep the workspaces for inspection.
test/differential/run.py --binary a=./git-sfs --binary b=./git-sfs \
  --scenario 02 --keep
```

Exits non-zero when any pair diverges, printing a unified diff of the manifests.

## Layout

```
snapshot.py         tree -> canonical manifest. Reusable on its own.
run.py              driver: run scenarios, snapshot, diff
lib.sh              helpers available to every scenario
scenarios/          one scenario per file
fake-rclone/        recording, fault-injecting stand-in for rclone
lock_contention.py  second entry point; see below
lock-setup.sh       workspace prep for it
```

## Lock contention

`lock_contention.py` is a separate entry point because it tests a property no
tree diff can see: two real processes competing for one cache. contract-spec §8
makes the lock protocol an inter-version contract — during migration a user will
run v1 in one shell and v2 in another — and if the two disagree about the lock
path, both acquire "the lock" at once and write concurrently.

```sh
just lock-contention                                    # single binary
test/differential/lock_contention.py \
  --binary v1=./git-sfs --binary v2=./target/release/git-sfs
```

It reports two kinds of line, and the split is deliberate:

- **ASSERT** — frozen mechanism (§8): lock path, directory mode `0755`, owner
  file `pid: <N>\n` at mode `0644`, release on completion, and that each binary
  blocks on a lock the other created. Failures fail the run.
- **OBSERVE** — policy v1 gets wrong (§8.1), recorded as a baseline rather than
  asserted, because v2 is *required* to diverge here. Asserting v1's behavior
  would mean inverting the test later, which is how a harness teaches people to
  ignore it.

Both §8.1 predictions are confirmed against v1 rather than merely inferred from
the source: a zero-byte `owner` file produces `panic: runtime error: slice bounds
out of range [:-1]` ([lock.go:62](../../internal/lock/lock.go#L62)), and a lock
held by a dead pid blocks forever.

Non-vacuity was verified the same way as the tree diff: a Go build with its lock
path changed to `.lock2` fails four assertions, including a waiter that races
through in 0.08s — two processes writing one cache, which is exactly the outcome
§8 exists to prevent.

`snapshot.py` keeps a functional core (`normalize`, `render_manifest`, and each
entry's `render`) separate from its only side-effecting function (`walk`), so
manifest rendering is testable without a filesystem. It takes `--replace` and
`--exclude` as generic parameters rather than knowing the git-sfs layout; the
caller supplies the domain meaning.

## Writing a scenario

A scenario is a bash file in `scenarios/`, sourced with `lib.sh` already loaded.
The driver provides `GIT_SFS`, `WORK`, `REPO`, `CACHE`, `REMOTE`, and `OUTCOMES`.

```sh
setup_repo
write_local_remote_config          # only if the scenario needs a remote

printf 'payload\n' > "$REPO/data/blob.bin"
(cd "$REPO" && record add git_sfs add data)
commit_all "track dataset"
```

Use `record <label> <command...>` rather than calling `git_sfs` directly for
anything whose success matters. It captures the exit status into the manifest
**without aborting the scenario**, which is what lets the harness compare the
tree left behind by a *failing* command — often the more interesting case.

Use `require` instead for setup whose failure would hollow out the scenario.
The distinction is load-bearing; see "Agreement is not correctness" below.

## The remote half

The remote has no tree to diff, but every remote operation is an rclone
subprocess, so the **argv stream is the equivalent artifact**. A scenario calling
`use_fake_rclone` gets `fake-rclone/rclone` ahead of the real one on `PATH`, and
its invocations become a manifest section that diffs like everything else.

`--files-from` points at a temp file whose name is random per run; the recorder
logs the file's **contents** and discards its path, because which objects move is
the payload and the path is noise. `copyto`'s temp destination is canonicalized
the same way.

The fake also injects failures a local directory can never produce:

```sh
inject_fault '{"subcommand": "lsjson", "contains": "/files/sha256/",
               "exit": 1, "stderr": "403 Forbidden"}'
```

`contains` matters more than it looks. git-sfs runs a connectivity preflight
before per-object queries, so a fault matching *every* `lsjson` is caught by the
preflight and never reaches the per-object path — which is exactly where
contract-spec §13.3's defect lives. Scenario 05 uses it to pin that baseline: a
403 on object queries makes `status --remote` **exit 0** and report a
successfully pushed object as `remote=missing`.

## Agreement is not correctness

The harness compares binaries against each other, so **a broken fixture fails
identically on both sides and reads as green.** This is not hypothetical: the
first version of the fake mis-parsed `--files-from`, every `copy` failed, and all
scenarios passed.

Two things guard against it, and neither is optional:

- `require` for preconditions, which writes a sentinel the driver checks. It
  writes a file rather than just calling `exit`, because `require` normally runs
  inside a `( cd "$REPO"; ... )` subshell where `exit` ends only the subshell and
  the scenario would carry on as though the precondition had held.
- Reading the manifests when adding a scenario. A section reading `(not
  recorded)` or an outcome of `2` where `0` was intended compares equal to itself
  perfectly well.

## Determinism

Anything varying between two runs of one scenario must be normalized, or the
harness reports differences that are not divergences. Currently handled:

- **Workspace paths.** Each run gets a fresh temp directory; every occurrence
  collapses to `{WORK}`. Both the literal and `resolve()`d forms are registered,
  because `.git-sfs/cache` stores a canonicalized target (spec §2) that on macOS
  differs from the path we created (`/var/folders/…` vs `/private/var/folders/…`).
- **Directory order.** Manifest lines are sorted by path.
- **Commit metadata.** Pinned via `GIT_AUTHOR_DATE` / `GIT_COMMITTER_DATE`,
  though `.git` is excluded anyway.

Normalization is applied to file content and symlink targets through the same
function, so no file has to be classified as text or binary first.

## Trusting the harness

Green means nothing until the harness is shown to fail when it should. Both
directions were verified against the Go binary before any Rust existed:

- **No false positives** — two separately built Go binaries with different
  version stamps compare equal across all scenarios.
- **No false negatives** — a Go binary mutated to skip cache write-bit stripping
  (one line in `internal/cache/cache.go`) is caught, both as a direct `mode=0644`
  vs `mode=0444` diff and as a downstream `verify` exit-status change.

Re-run those checks after changing `snapshot.py`.
