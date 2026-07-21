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
snapshot.py            tree -> canonical manifest. Reusable on its own.
harness.py             binaries, workspaces, polling. Knows nothing about git-sfs.
cache_state.py         queries over a cache tree: object paths, modes, hashes.
divergences.py         differences that are fixes, declared in advance
coverage.py            contract-spec clause -> the assertion holding it down
run.py                 driver: run scenarios, snapshot, diff
lib.sh                 helpers available to every scenario
scenarios/             one scenario per file
fake-rclone/           recording, fault-injecting stand-in for rclone
replicated-setup.sh    fixture: object cached, committed, and on the remote
lock_contention.py     second entry point; see below
lock-setup.sh          workspace prep for it
cancellation.py        third entry point; see below
mode_preservation.py   fourth entry point; see below
```

`harness.py` and `cache_state.py` split along a deliberate line: the first would
work for any CLI, the second encodes the layout contract-spec §4 freezes. The
entry points then hold only what is distinct about them.

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

## Mode preservation

`mode_preservation.py` covers the one contract-spec calls "the single most
dangerous invariant in the contract" (§4.1). The read-only bit on a cache object
is not decoration — `HasValid` treats it as *proof the bytes were hash-verified
when written*, and therefore never re-hashes. Everything downstream inherits
that trust.

The spec then lists the environments where the bit can lie: exFAT/FAT, some FUSE
and network mounts, SMB/NFS with unusual id mapping, Docker volume copies,
`rsync` without `-p`, archive extraction. Any of them can hand you unverified
bytes wearing a read-only bit.

```sh
just mode-preservation
```

Mounting exFAT in CI is not portable, so the harness constructs the **state**
such a filesystem leaves behind rather than intercepting the chmod that produces
it. Three combinations of (mode, content) are reachable and each is built
directly:

| Mode | Content | What it models | Expected |
|---|---|---|---|
| writable | intact | older cache, or a copy that dropped the bit | §4.1's MUST: hash-verify, then protect in place |
| writable | rotted | the same, plus real damage | the write bit forces a re-hash, so `pull` repairs from the remote |
| protected | rotted | `rsync -a` without `-p`, archive extraction, bit rot | **v1 trusts it forever** |

The third row is the hazard, so plain commands there are OBSERVE — v1 is
permitted to be wrong and v2 is encouraged to diverge. What *is* asserted is
that `verify --with-integrity` and `verify --rehash` still catch it: whatever a
version decides about trusting the bit, a command whose entire job is re-hashing
must not miss corruption.

The second row doubles as a test of replication-as-repair-source
(rust-rewrite-plan §8): one copy means rot is fatal, two means it is repairable.

### What this cannot cover

A chmod that **silently fails at the moment of writing**. Detecting that needs a
real `LD_PRELOAD` / `DYLD_INSERT_LIBRARIES` interposer, which costs a C shim and
a compiler in CI. It is deferred deliberately: it only becomes testable behavior
once v2 ships the §7b probe that checks whether the cache filesystem preserves
modes at all. Until then there is nothing for it to assert against.

### Trusting it

Same bar again — proven to fail before being trusted to pass:

- `HasValid` returning `true` for any file that exists, mode and content unread:
  three assertions fail on the rotted-writable case.
- Re-protection required **two** mutations to trip, which is a finding in itself:
  `HasValid` chmods legacy files ([cache.go:76](../../internal/cache/cache.go#L76))
  *and* `Protect` chmods them again, so disabling either alone changes nothing.
  Only with both disabled does the assertion fire.

### It found a live defect

The push case was written as an assertion and failed against v1, which is how
contract-spec §13.4's first entry came to exist. `push` admits an object on
`HasValid` alone — for a read-only file that is the mode bit and nothing else,
no bytes read — and `CopyToRemote` omits `--ignore-existing`, so the upload
overwrites. A locally rotted object therefore **destroys the good remote copy**
and `push` exits `0`. Replication running backwards: the tier that exists to
repair the other is overwritten by the damaged one.

It is OBSERVE today only so the v1 baseline stays green. Flipping it to a
positive assertion of v2's behavior is the outstanding Phase 0 item "encode each
§13 divergence as a positive assertion".

## Performance

`benchmark.py` captures the baselines rust-rewrite-plan §9b gates Phase 7 on. It
is **not** part of `just check` — it takes minutes, and belongs to the nightly
tier.

```sh
just perf                 # baseline capture
just perf-selfcheck       # one binary, two names: establishes the noise floor
test/differential/benchmark.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs
```

It drives the CLI rather than internal packages. That is not incidental: the Go
benchmarks that exist today (`BenchmarkStore8MiB` and friends) measure seams an
idiomatic rewrite deletes, so they cannot serve as a cross-implementation
baseline. The command surface is the only thing both versions share.

Two tiers, because §9b's "tests run at the wrong scale" applies here too — every
workflow scenario uses twelve-byte files:

| Tier | Shape | Catches |
|---|---|---|
| count | 1000 × 1 KiB | per-object overhead, locks, syscalls, walk cost |
| throughput | 1 × 256 MiB | the hashing hot path, where the SHA-NI claim lives |

Payload bytes differ per file on purpose. Identical contents would make every
file after the first a dedup hit, which measures the dedup check rather than
ingest.

**Absolute numbers gate nothing.** A time from one laptop says nothing about
another machine, so the acceptance criterion is the ratio between two binaries
measured side by side in one run — available throughout, since v1 survives on
`go-legacy`. Files under `baselines/` are reference captures, not thresholds.

The self-check is what makes a threshold defensible: one binary under two names
returns 0.99–1.08×, so ~8% is noise, concentrated in `add_large` where a single
short I/O-bound measurement is most exposed to page-cache state. The plan sets
the gate at 1.25×. Re-run the self-check on whatever machine does the gating —
the floor is machine-specific.

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
tests that are wrong and green. That has already happened twice (see "Agreement
is not correctness"). This is why the fake **does not replace** real rclone: it
is layer 3 of the plan's five, and layer 4 — real rclone against a local
directory — stays exactly where it is.

### Why not an in-process mock instead

Reasonable question, since a Rust `Remote` mock would be faster, typed, and need
no subprocess. **v2 should have one** — it is layer 2 of the plan's five, and it
is the right tool for exec orchestration, retry policy, cancellation, and event
emission. It does not replace this, for two reasons.

**It substitutes away the code under test.** An in-process mock implements the
`Remote` trait, so it stands *above* the argv boundary. Everything below —
building `copy --checksum --files-from <list> <src> <dst>`, parsing `lsjson`
output, mapping exit codes and stderr onto error classes — is exactly the code
the mock replaces. A mock hands back a typed `HashMap<Sha256, u64>`; the JSON
parser never runs.

That is not hypothetical. The `Name`/`Path` bug below was a wrong belief about
rclone's JSON contract, and it silently disabled the remote half of `verify` and
`status`. An in-process mock cannot expose that class of error *by construction*
— it tests our code against our own belief, with the belief never written down
where it can be compared to the real thing. The subprocess fake at least forces
the belief into bytes on a pipe, which is diffable against real rclone.

**It cannot instrument the Go binary.** The differential harness captures the
argv stream from v1 *and* v2 and diffs them (§5.2b) — that is the entire
comparison artifact for the remote half, since a remote has no tree to walk. One
of those two binaries is not Rust, so an in-process Rust mock can never produce
the v1 side of that diff. An external recorder is the only thing both
implementations can be driven through.

The division that follows: **mock what you own, fake what you shell out to.**

`--files-from` points at a temp file whose name is random per run; the recorder
logs the file's **contents** and discards its path, because which objects move is
the payload and the path is noise. `copyto`'s temp destination is canonicalized
the same way.

The fake also injects failures a local directory can never produce:

```sh
inject_fault '{"subcommand": "lsjson", "contains": "/files/sha256/",
               "exit": 1, "stderr": "403 Forbidden"}'
```

`contains` matters more than it looks, and is easy to get wrong. git-sfs runs a
connectivity preflight before enumerating objects, so a fault matching *every*
`lsjson` is caught by the preflight and never reaches the path where
contract-spec §13.3's defect lives. The preflight issues `lsd local:` and a
non-recursive `lsjson` on the remote root, so **matching on `--recursive`** lets
it through and denies exactly the object listing (`FileSizes`).

Matching on `/files/sha256/` does *not* work, though it reads as if it should:
no command names an individual object via `lsjson`. Scenario 05 was written that
way and the fault never fired for as long as it existed — see "Agreement is not
correctness".

With it fixed, the scenario pins the §13.3 baseline for real: a denied remote
makes `status --remote` **exit 0** while reporting every object absent, because
`status.go:96` discards the error outright (`sizes, _ :=`). `verify` propagates
it and exits non-zero. The retries visible in the argv log — six `lsjson`, three
`copyto` for a permanent 403 — are §13.4's retry-on-permanent-failure showing up
in passing.

## Agreement is not correctness

The harness compares binaries against each other, so **a broken fixture fails
identically on both sides and reads as green.** This is not hypothetical. It has
now happened twice, and the second one hid for longer:

- The first version of the fake mis-parsed `--files-from`, every `copy` failed,
  and all scenarios passed.
- The fake emitted `lsjson`'s `Name` field as the path relative to the listing
  root rather than the basename. `parseFileSizesJSON` matches objects on `Name`,
  so **every object looked absent from the remote** whenever `FileSizes` ran —
  silently disabling the remote half of `verify` and `status`. Scenario 05
  appeared to demonstrate contract-spec §13.3 and was in fact demonstrating this
  bug; its fault rule targeted `/files/sha256/`, which matches nothing git-sfs
  ever issues, so the 403 it claimed to inject never fired.

Both were found by writing a *new* test that expected a specific outcome, not by
the diff. The lesson repeats: a self-comparison proves determinism, never
correctness.

Two things guard against it, and neither is optional:

- `require` for preconditions, which writes a sentinel the driver checks. It
  writes a file rather than just calling `exit`, because `require` normally runs
  inside a `( cd "$REPO"; ... )` subshell where `exit` ends only the subshell and
  the scenario would carry on as though the precondition had held.
- Reading the manifests when adding a scenario. A section reading `(not
  recorded)` or an outcome of `2` where `0` was intended compares equal to itself
  perfectly well.

## Coverage

`coverage.py` enumerates contract-spec clauses and records what actually holds
each one down. rust-rewrite-plan §7 states the Phase 7 gate literally — *every
contract-spec clause maps to a passing assertion* — and that needs something
checkable to compare against.

```sh
just coverage                        # report; fails only on a stale claim
test/differential/coverage.py --list UNCOVERED
test/differential/coverage.py --gate # the Phase 7 criterion
```

The map is **self-verifying**: every clause claiming coverage names a file and a
fragment of the assertion covering it, and the script confirms the fragment is
still there. Rename an assertion and the map goes red instead of quietly lying,
which is where hand-maintained checklists always end up.

| Status | Meaning |
|---|---|
| `ASSERTED` | a named assertion fails if violated |
| `STRUCTURAL` | any change surfaces as a manifest or argv diff |
| `OBSERVED` | recorded as a v1 baseline; v2 must diverge, so not yet an assertion |
| `DECLARED` | enumerated in `divergences.py`; asserts itself once v2 exists |
| `V2-ONLY` | untestable until the Rust binary exists |
| `UNCOVERED` | nothing tests it |

The default run is deliberately *not* the gate. Today's gaps are known and
tracked rather than regressions, and failing on them now would only teach people
to skip the script.

**What it cannot check:** a substring proves a *mention*, not an assertion.
Building the map caught a clause claiming `benchmark.py` enforced a regression
threshold when it only printed ratios — the fragment matched, the guarantee did
not exist. (That gap is now closed; `--max-ratio` fails the run.) Evidence
fragments therefore name the failure path, not the subject matter.

## Enumerated divergences

Plan §5.1: *"An unenumerated divergence is a regression. An enumerated one is a
fix. The difference is written down in advance, never adjudicated after a red
run."* The tree diff and the argv diff cannot tell the two apart — both are just
differences — so `divergences.py` is where the writing down happens.

```sh
run.py --binary v1=./git-sfs --binary v2=./target/release/git-sfs --candidate v2
```

Without `--candidate` nothing is declared, nothing is normalized, and the
comparison is exactly as strict as it has always been. A self-check must show no
divergence at all.

**Not a suppression list.** Ignoring a known difference would make the harness
silent about whether v2 actually fixed anything, and an ignore list only grows.
Each declaration does two jobs:

| | |
|---|---|
| `normalize` | collapses the dimension allowed to differ, applied to **both** sides. Everything outside it still compares strictly. |
| `occurred` | asserts the divergence **did** happen. A v2 that quietly kept v1's behavior fails. |

The second is the half a suppression list can never provide.

Declared today: **§13.4 `retry-only-transient`** — v1 reissues identical argv on
failure, so scenario 05's manifest records a permanent 403 attempted three times.
`collapse_repeats` folds runs of adjacent identical lines, so the comparison sees
*what* was attempted without asserting *how many times*; `fewer_lines` then
requires the candidate to have actually stopped.

### Trusting it

Four properties, each proven against a Go binary mutated to behave like a v2
that fixed §13.4 (`retryLoop` capped at one attempt):

| Setup | Required | Result |
|---|---|---|
| fix, no `--candidate` | reads as a regression | FAIL, argv diff |
| fix, `--candidate v2` | passes, divergence confirmed | ok + `confirmed` |
| **no fix**, `--candidate v2` | fails — v2 kept the defect | FAIL + `MISSING` |
| fix **plus** an unrelated regression | still caught | FAIL on `mode=0444` → `0644` |

The third row is the one that matters, and the fourth shows the normalization
stays scoped to the section and scenario that declared it.

A declaration naming a scenario or section that does not exist would be inert —
normalizing nothing, asserting nothing, while reading as handled. Both `run.py`
and `coverage.py` reject that, and `run.py` validates against *all* scenarios so
running one cannot mask a stale declaration elsewhere.

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
