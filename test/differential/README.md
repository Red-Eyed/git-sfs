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
harness.py          shared by all three entry points: binaries, workspaces, polling
run.py              driver: run scenarios, snapshot, diff
lib.sh              helpers available to every scenario
scenarios/          one scenario per file
fake-rclone/        recording, fault-injecting stand-in for rclone
lock_contention.py  second entry point; see below
lock-setup.sh       workspace prep for it
cancellation.py     third entry point; see below
cancel-setup.sh     workspace prep for it
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

## Cancellation

`cancellation.py` is the third entry point, for the same reason the second one
exists: it tests a property no tree diff can see. AGENTS.md makes cancellation a
safety requirement — "never publish a partial file, and surface the interrupt as
a clean cancellation, not a corrupt result" — and an interrupt lands at a point
neither binary controls, so the tree afterwards differs legitimately between two
runs of *one* binary. There is nothing to diff. Invariants are asserted instead.

```sh
just cancellation
test/differential/cancellation.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
```

Three operations get interrupted mid-flight, each chosen for a different way to
lose data:

| Interrupted | The hazard it covers |
|---|---|
| `pull` | rclone is handed `<cache>/files` directly, so a half-finished download is a partial file **at a content-addressed path**, not in staging |
| `push` | the replica that exists so the cache is not the only copy |
| `add` | the user's only copy is in play — `add` unlinks the source before creating the symlink (§13.1) |

The assertion that matters most is that **no object is ever both read-only and
mismatched with its own name.** Under §4.1 a stripped write bit *is* the proof
that bytes were verified, so a partial file published read-only is trusted
forever and every later integrity check passes on corrupt data. It is checked
after the interrupt *and* after the recovery run, because a regression can
publish the bad object during either.

Each case then re-runs the interrupted command and requires it to succeed — a
clean cancellation that leaves the operation unrepeatable is not clean.

SIGINT goes to git-sfs alone, deliberately not to the process group. A real
Ctrl-C would also hit the rclone child directly, which would mask whether
git-sfs propagates cancellation to its own subprocess — and that propagation is
the property under test.

### Trusting it

Same bar as the tree diff: proven to fail before being trusted to pass. Two
mutant Go binaries were built and caught.

- Deleting the partial-file unlink in `pullMissingFiles`
  ([pull.go:79-87](../../internal/core/pull.go#L79-L87)) — the retry's
  `--ignore-existing` then skips the half-written object. Three assertions fail.
- That, plus inverting `Protect` to chmod *before* verifying — the retry publishes
  a read-only object that does not match its hash, and the §4.1 assertion names
  it.

The second mutant is why the invariant is checked at two points rather than one:
with the check only after the interrupt, it passed while the mutant was actively
producing the exact state the check exists to forbid.

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

### Why a fake rclone at all

Three separate jobs, and only the last is about speed:

1. **The remote is not a filesystem we can walk.** git-sfs reaches every remote
   *exclusively* through rclone, using exactly five subcommands, so its complete
   observable remote behavior **is** its argv stream. Recording that stream is
   what gives the remote half an artifact at all — and you cannot record argv
   without standing between git-sfs and rclone.
2. **A real rclone pointed at a local directory cannot fail interestingly.** That
   is what the shell suite does today: hermetic, fast, and incapable of producing
   a 403, a rate limit, expired credentials, or truncated `lsjson`. Those are
   exactly the paths contract-spec §13.3's defects live on — remote errors
   collapsing into "not found", and `isRemotePathNotFound` classifying by
   grepping rclone's English for `"directory not found"`. Unreachable without
   injected failures.
3. **Interruption needs a window to aim at.** Real rclone copying a local file
   finishes in microseconds, so a SIGINT test against it is a race that passes
   for the wrong reason. The `stall` fault writes half an object, `fsync`s, then
   pauses — making the partial file exact and the interrupt window deterministic.

The cost is that the fake is *our model* of rclone, so a wrong model produces
tests that are wrong and green. That has already happened once (see "Agreement is
not correctness"). This is why the fake **does not replace** real rclone: it is
layer 3 of the plan's five, and layer 4 — real rclone against a local directory —
stays exactly where it is.

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
