# git-sfs → Rust: rewrite plan

**Branch:** `rust` (long-standing; `main` keeps shipping Go until cutover)
**Target:** v2.0.0

**Companion documents — read in this order:**

1. [contract-spec.md](contract-spec.md) — normative. What v2 must satisfy
2. [failure-modes.md](failure-modes.md) — inverted design review of v1. What v2
   must *not* inherit

The second is not background reading. Its findings are folded into contract-spec
§13 as an explicit do-not-reproduce list, because contract-only parity creates a
trap: a harness that verifies "v2 matches v1" will happily certify v2 for
faithfully reproducing v1's defects.

---

## 1. Decisions

| Axis | Decision |
|---|---|
| Driver | **Correctness** — convert runtime-checked invariants into compiler-enforced ones |
| Approach | **Idiomatic rewrite, not a port.** No structural correspondence to the Go tree |
| Parity | **Contract-only.** On-disk format, exit codes, JSON shapes frozen; all human output free |
| Type rigor | **Newtypes + typed errors.** No typestate, no phantom types |
| Runtime | **Sync.** `AtomicBool` cancellation + `rayon`. No async runtime |
| Dependencies | **Generous.** Compose the ecosystem; do not hand-roll what a good crate provides |
| Release | **cargo-zigbuild**, single Linux runner, all four targets |

### The line

The contract in [contract-spec.md](contract-spec.md) is frozen because users'
datasets and already-installed binaries depend on it. **Everything behind that
boundary is unconstrained.** The deliverable is not "translate 30 packages" but
"design the tool you would design if Rust had been the first choice, then verify
it satisfies the same observable contract."

---

## 2. Correctness thesis

The rewrite is only worth it if invariants currently held in comments and runtime
checks become invariants the compiler enforces. Four qualify.

### 2.1 `Hash` is a string alias with a silent-degradation path

`hash.go:17` is `type Hash string`. `Parse` validates, but nothing forces
construction through it — `Hash("")` is legal everywhere and error paths return
exactly that. Then `hash.go:73-79`:

```go
func (h Hash) Prefix() string {
    if len(s) < 2 { return "" }   // silently yields files/sha256//<hash>
}
```

**Rust:** `Sha256([u8; 32])`, private field, no `Default`, no `From<String>`.
Two constructors only — `parse()` and `of_file()`. The length invariant holds at
construction, so `prefix()` becomes total and **that guard clause is deleted, not
ported.** Every "invalid hash" check in the codebase collapses to the parse
boundary.

### 2.2 "Verified" is a mode-bit convention, not a type

The highest-value one. `cache.go:58-62` states the load-bearing rule in prose:

> protection is set by Protect only after hash verification, so the file cannot
> have been mutated since

`HasValid` returns `bool`, and every caller must remember what that boolean
licenses. **Rust:** a `CacheEntry` constructible *only* by code paths that
verify. Functions needing trustworthy bytes take `&CacheEntry`, never `Sha256`.
Passing an unverified hash where verified content is required stops compiling.

Go cannot express this — any package can construct a struct it can see. This is
the invariant that most justifies the language change.

### 2.3 Publish-on-failure cleanup is manual

`AtomicCopy` handles the temp file correctly via `defer`, but each of seven error
branches re-calls `tmp.Close()`, and `cache.go:123` needs an explicit
`removeOnErr(dst)` after a post-rename mismatch.

**Rust:** `tempfile::NamedTempFile` — `Drop` handles cleanup, `persist()`
consumes the value. Error, panic, and Ctrl-C all unwind to the same cleanup.
There is no branch where an author can forget.

### 2.4 Cancellation is per-loop discipline

`AGENTS.md` mandates a `ctx` check per chunk in every byte-moving loop; each loop
hand-writes it today. **Rust:** a `Cancellable<R: Read>` adapter checking the flag
on each `read()`. Hashing and copying then use plain `io::copy` and inherit
prompt cancellation structurally.

### 2.5 Where types do not help

The filesystem is shared mutable state. `CacheEntry` proves *verified at time T*,
not *verified now* — another process can chmod a cache file. The runtime
re-checks in `HasValid` stay. Types shrink the vigilance surface; they do not
eliminate it, and the spec's §4.1 remains mandatory.

---

## 3. Architecture

Two-crate workspace, and the split does real work:

```
git-sfs-core/   lib — cannot print, cannot exit
git-sfs/        bin — clap, indicatif, exit codes
```

The core crate does not depend on the CLI, so the functional-core/imperative-shell
boundary is **enforced by the dependency graph** rather than by discipline. It
also lets integration tests drive the library directly.

### 3.1 Layers inside core

| Layer | Contents | Property |
|---|---|---|
| `domain` | `Sha256`, `ObjectId`, `RepoPath`, `RemoteName`, `Config` | Pure values; illegal states unrepresentable |
| `plan` | `plan_push`, `plan_pull`, `plan_verify` → `Plan` | **Pure functions.** No I/O, no clock |
| `ports` | `Store`, `Remote`, `Repo` traits | The only side-effecting seams |
| `exec` | Executes a `Plan` against ports; emits events | Where effects live |

Commands become `scan → plan → execute → report`. Planning is a pure function
over data, so the interesting logic — what needs uploading, what is corrupt, what
is orphaned — is unit-tested with zero filesystem.

### 3.2 Go-isms deliberately dropped

**The `App` god-object.** `app.go` bundles config, cache, stderr, and quiet, and
every command hangs off it as a method. `Verify` receives the whole `App` when it
needs a store and a remote. Fails Interface Segregation.

**Observation threaded through business logic.** `verify.go:120`:

```go
func scan(ctx, repo, path, c, cfg, r, checkRemote, withIntegrity bool,
          stderr io.Writer, quiet bool)
```

Ten parameters, two booleans, two present only so a caller can watch. This is
the Open/Closed violation `CLAUDE.md` calls the most commonly missed. **Rust:**
`exec` emits an `Event` stream; the binary decides whether that renders as an
`indicatif` bar, JSON lines, or nothing. `quiet` and `stderr` never enter core,
and every such signature collapses.

**Also dropped:** stringly-typed domain values, booleans as mode parameters,
sentinel errors with `errors.Join`, hand-rolled worker pools.

### 3.3 Trait discipline

`Remote` and `Store` get traits because there are genuinely two implementations
each — real plus a test fake. That is what justifies the abstraction under the
"no abstraction before a second implementation" rule. **Nothing else gets a
trait.** A trait per module would be the cathedral; `CLAUDE.md` correctly calls
that a smell.

---

## 4. Dependencies

The "one production dependency" rule in `AGENTS.md` is a *Go-shaped* decision.
Go ships `crypto/sha256`, `encoding/json`, `os/exec`, and `context` in its
stdlib, so hand-rolling the rest is a small delta. Rust's stdlib is deliberately
minimal and the ecosystem is designed to be composed. Porting Go's dependency
posture would mean hand-writing a TOML parser and a progress renderer strictly
worse than `toml` and `indicatif`.

For a correctness-driven rewrite this cuts harder still: `toml` has orders of
magnitude more adversarial input exposure than anything written for this project.
**Using well-tested crates is the correctness play.**

What survives from the old rule is different: **minimalism is obsolete,
diligence is not.** This project ships binaries to user machines via
`self update` and already verifies SHA-256 of downloads. That argues for auditing
*what* is pulled in, not for pulling in less.

### 4.1 Runtime

| Area | Crate | Rationale |
|---|---|---|
| CLI | `clap` v4 derive | Replaces kong; `clap_complete` / `clap_mangen` add completions and man pages |
| Config | `serde` + `toml`, `toml_edit` for writes | Deletes ~200 lines; `toml_edit` preserves comments on rewrite. **See spec §6.3** |
| Hashing | `sha2` + `hex` | SHA-NI hardware acceleration — real throughput win on the hot path |
| Paths | `camino` (`Utf8PathBuf`) | Paths flow into config and JSON; removes lossy `Path`↔`String` conversion |
| Walking | `walkdir` | Symlink-loop handling, deterministic ordering |
| Atomic writes | `tempfile` | `NamedTempFile::persist()` *is* §2.3 |
| Parallelism | `rayon` | Replaces `run.go`; `ThreadPoolBuilder` maps onto `-j`. Sync, no runtime |
| Cancellation | `ctrlc` + `AtomicBool` | Per decision |
| Progress | `indicatif` | Deletes all 233 lines of `progress.go`; adds real multi-bar for parallel jobs |
| Errors | `thiserror` + `anyhow` | Typed in core, ergonomic in bin |
| JSON | `serde_json` | Frozen output shapes + rclone `lsjson` parsing |
| Semver | `semver` | Replaces hand-rolled `ParseSemver`; correct prerelease handling |
| HTTP | `ureq` + rustls | Sync-native. **See §7 trap** |

### 4.2 Testing

| Crate | Role |
|---|---|
| `assert_cmd` + `predicates` | Drive the real binary end-to-end |
| `insta` | Snapshot testing — **only for frozen JSON output**, not human text |
| `proptest` | Property tests for `Sha256` and path invariants |
| `rstest` | Parameterized scenarios |

### 4.3 Supply chain

`cargo-deny` in CI for advisories and licenses. **`self update` stays custom** —
the `self_update` crate exists, but spec §11 pins exact archive naming and
`SHA256SUMS` format, and this is the path where a compromise means arbitrary code
on user machines. Delegating download-verify-replace to a moderately-maintained
crate is the wrong trade. This is the diligence exception, not a minimalism
relapse.

---

## 5. Verification strategy

Contract-only parity is the loosest option, chosen alongside the most aggressive
rewrite. **Those compound.** Maximum architectural freedom with the weakest
automated net — which makes the harness the thing that determines whether this
succeeds.

### 5.1 The differential trap

Naive differential testing asks "does v2 match v1?" For the defects in
contract-spec §13 the correct answer is **no**, and a harness that flags those as
regressions will train everyone to ignore it.

The harness therefore compares against the **specification**, not the v1 binary:

- v1 is the oracle for everything in contract-spec §2–§11 (mechanism).
- The **spec** is the oracle wherever §13 says v1 is wrong (policy).
- Every intentional divergence is enumerated in §13/§14 and asserted *positively*
  — v2 must exit `3` where v1 exits `0` on a truncated remote object, and the
  test asserts the new behavior, not the old.

An unenumerated divergence is a regression. An enumerated one is a fix. The
difference is written down in advance, never adjudicated after a red run.

### 5.2 Layers

Snapshots of human output are useless here by construction. What replaces them:

1. **Filesystem differential testing — the backbone.** Drive the v1 and v2
   binaries through identical scenario sequences, then diff resulting trees:
   symlink targets, cache paths, file modes, content hashes. Fully frozen, so
   these assertions stay strict.
2. **Success/failure assertions** per scenario — `0` vs non-zero only, since the
   taxonomy is unfrozen (spec §9).
3. **Cross-binary interop** — lock contention and cache sharing between v1 and
   v2 (spec §8). No single-binary test can catch this class.
4. **Existing shell suite** — already contract-level. One assertion loosened.
   **Kept in bash, unchanged, for the duration of the port** (§5.2c).
5. **`insta` snapshots of v2's own output** — regression tests, *not* parity
   checks, now that `status --json` is unfrozen (spec §10.1).

### 5.2b Test architecture — the local/remote split

git-sfs has two halves with different testing shapes: local filesystem work, and
remote work that goes **exclusively** through rclone subprocesses.

**The remote half has no filesystem to diff — but it has an equivalent.** Every
remote operation is an rclone invocation; v1 uses exactly five subcommands —
`lsd`, `lsjson`, `copyto`, `copy`, `version` (`command.go:124`, `141`, `170`,
`195`, `248`, `272`, `459`, `507`, `525`). So the tool's complete observable
remote behavior *is* its rclone argv stream. Capture it from v1, capture it from
v2, diff it.

Two argv details are not incidental. `copy` passes `--files-from <tempfile>`, so
*which* objects move lives in a file whose path is random per run — the recorder
must log the file's contents and discard its path, or the most important part of
a push becomes invisible while noise makes the stream nondeterministic. `copyto`
likewise targets a randomly named temp file, which is canonicalized away.

| Half | Differential artifact |
|---|---|
| Local | Filesystem tree — symlink targets, cache paths, modes, content hashes |
| Remote | rclone argv stream — commands, flags, path lists, ordering |

That remote access is *only* via rclone is what makes this work: a narrow,
capturable interface rather than a diffuse one. The argv diff also pins the
frozen remote layout (contract-spec §5) exactly, which matters because that
layout is shared across users and versions, not just across our two binaries.

#### Five layers

| Layer | Uses | What only it can test |
|---|---|---|
| Pure `plan` tests | No I/O | Planning, dedup, orphan classification, config ambiguity |
| `Remote` trait fake | In-process | exec orchestration, retry, cancellation, event emission |
| **Fake `rclone` on `PATH`** | Subprocess, canned output | **Argv construction, output parsing, error classification** |
| Real rclone → local dir | Subprocess, real backend | Byte movement, real `lsjson` shapes. *Exists today* |
| Real rclone → cloud | Network | Nothing routine. Optional smoke, credentials-gated |

**Layer 3 is missing today and is the important addition.** The existing suite
runs real rclone against a local directory (`scenarios.sh:266`), which is
hermetic and fast but *cannot fail interestingly* — a local directory never
returns a 403, a rate limit, expired credentials, or truncated `lsjson`. That is
precisely where contract-spec §13.3's defects live: remote errors collapsing into
"not found", and `isRemotePathNotFound` classifying by grepping English for
`"directory not found"` while bailing on `"config"`.

A fake rclone binary on `PATH` that records argv and emits scripted stdout and
exit codes provides both halves of the need: **record** for the argv diff,
**replay** for error injection. One tool, two uses.

Layer 5 stays optional. Real cloud backends are slow, flaky, and credential-bound;
nothing in the contract requires them beyond confirming that a real backend
behaves like the local one, which is a smoke test rather than a suite.

### 5.2c The bash suite stays bash

**Do not rewrite the test suite and the code it tests at the same time.** If both
change together a failure is ambiguous — implementation wrong, or new test wrong?
Holding `test/workflows/` fixed makes it a known-good constant, so every red
during the port points at the Rust code and nowhere else.

The only Phase 0 changes are therefore mechanical:

- `GIT_SFS_BIN` indirection replacing the `go build` fixture — changes *which
  binary runs*, never *what is asserted*.
- The single human-output assertion at `scenarios.sh:215` loosened to a
  structural check.

Both are made **and proven green against v1** before any Rust exists. After that
the file does not move for the duration of the port.

**It should stay bash permanently, not just until v2.** The suite exercises the
*installed artifact through the real install path* — building a release tarball,
running `install.sh` against a `file://` endpoint, resolving from `PATH`,
checking `--version` (`test/workflows/lib/install.sh`). `assert_cmd` integration
tests drive a binary from the build directory and never touch any of that. The
two do different jobs, and the bash one covers the path every real user takes.

Rust integration tests are additive here, not a replacement.

### 5.3 The net is now filesystem state, essentially alone

Worth stating plainly, because it narrowed three times in a row: human output was
freed by contract-only parity, then the exit-code taxonomy, then the
`status --json` schema. What remains cross-checkable against v1 is **filesystem
state plus whether the command succeeded.**

That is defensible — for a data tool, what ends up on disk *is* the product, and
the freed surfaces are exactly the ones where v1 was demonstrably wrong (spec
§9.2, §10.1). Adding the argv stream (§5.2b) restores a second strict artifact,
so the differential net is tree diff plus argv diff, not tree diff alone.

What neither one sees still needs deliberately written tests:

- **Cancellation** — SIGINT mid-transfer; assert no partial file is published and
  the exit is a clean cancellation, not a corrupt result.
- **Mode preservation** (spec §4.1) — requires a real filesystem, layer 4.
- **Cross-binary lock contention** (spec §8) — two real processes, v1 and v2,
  each holding against the other.
- **Filesystems that do not preserve modes** — hard to reproduce portably;
  fault-injection at the mode-setting boundary is more practical than mounting
  exFAT in CI.

These are the tests most likely to be skipped, because each needs bespoke
scaffolding rather than another row in a table-driven suite. They are also the
ones covering the failure modes that lose data.

**Scaffolding lands in Phase 0; assertions land with the code they cover.** The
SIGINT driver and the mode-setting fault-injection hook are infrastructure and can
be built and proven against the v1 binary before any Rust exists. The per-command
assertions necessarily wait for Phase 4. Splitting it this way prevents "we will
write those later" from resolving to never — the expensive half is already done
and idle.

The 3,098 lines of Go tests are a **specification to read, not code to
translate.** They test seams that will not exist. Mine them during Phase 0 for
edge cases worth promoting into the contract spec, then discard.

---

## 6. Phases

### Phase 0 — contract + harness (blocking)

Nothing else starts until this lands.

- [x] Write [contract-spec.md](contract-spec.md), grounded in the Go source
- [x] Fold [failure-modes.md](failure-modes.md) into spec §13 (do-not-reproduce)
- [x] Resolve spec §14 open divergences — all eight now Resolved or Dissolved
- [ ] Mine the Go test suite for edge cases; fold into the spec
- [x] Decouple `test/workflows/` from `go build` via `GIT_SFS_BIN`
- [x] Loosen `scenarios.sh:215` to a structural assertion
- [x] Build the tree-diff harness; prove it green **Go against Go**
- [x] Build the fake-rclone recorder; capture v1's argv stream as the baseline
- [x] Build the cross-binary lock-contention harness
- [ ] Build the SIGINT driver and mode fault-injection hook; prove against v1
- [ ] Capture v1 performance baselines (§9b)
- [ ] Build the downgrade test: v2 workflow → install v1 → same workflow (§7c)
- [ ] Encode each §13 divergence as a positive assertion (§5.1)

Proving the harness against the Go binary first is what makes it trustworthy
before it becomes load-bearing.

### Phase 1 — skeleton + docs

- Workspace: `git-sfs-core` + `git-sfs`
- Full clap surface, all 14 commands parsing, correct exit codes
- **Rewrite `AGENTS.md`** — it currently instructs contributors to hand-roll
  progress and avoid dependencies, which now contradicts the codebase. Not
  bookkeeping: stale guidance actively misleads future contributors and agents
- Rust `Justfile`, CI workflow

### Phase 2 — domain + pure plan layer

Newtypes, typed errors, pure `plan_*` functions. Unit-tested with zero
filesystem. **This is where the correctness thesis pays out** — get it wrong and
everything above it inherits the mistake.

### Phase 3 — ports

`Store`, `Remote`, `Repo` traits; rclone and filesystem implementations; test
fakes.

### Phase 4 — commands

Dependency order: `init`/`setup` → `add`/`import`/`mv` → `status`/`remotes` →
`push`/`pull` → `verify`/`doctor`. Differential harness runs continuously from
the first command.

### Phase 5 — reporting

Event stream → `indicatif`, JSON, quiet. Entirely in the binary crate.

### Phase 6 — release

zigbuild, musl targets, `ureq`+rustls, archive naming per spec §11. **The risk
phase** — no test coverage, and mistakes here are user-visible and hard to
reverse.

### Phase 7 — cutover

**Acceptance gate — literal, not vibes.** contract-spec §15 states that clauses
without assertions are aspirational. The gate is therefore: **every contract-spec
clause maps to a passing assertion.** Enumerable, checkable, and it fails loudly
when a clause has no test rather than when a test breaks.

Plus: no performance regression past the Phase 0 baselines (§9b), and the
downgrade test green (§7c).

**Clean cutover — the gate is the bar, not elapsed time.** When the acceptance
gate passes, v2 becomes the default install. There is no soak period and no
opt-in window.

This follows from the decision not to maintain v1 (contract-spec §13.4b). A
staged rollout combined with an unmaintained v1 is the worst of both: the default
install would point at a version nobody is fixing, which is how the
`git clean -x` exposure would have persisted longest in exactly the population
least likely to opt out of it. Either stage the rollout *and* maintain v1 during
it, or cut over cleanly and retire v1. Half of each protects no one.

The corollary matters: **rollout caution must not substitute for verification.**
If the gate is not trusted enough to ship on, strengthen the gate rather than
hedging with a soak period. A soak that catches a data-corruption bug has already
corrupted someone's data.

Safety comes from two places instead:

- The gate itself — every contract clause asserted, no performance regression,
  downgrade test green.
- The §7c downgrade invariant, with v1 binaries published permanently. An escape
  hatch, tested rather than hoped for, which suits a single-maintainer project
  better than running two versions in parallel.

**Honest limit of the escape hatch:** downgrade recovers from a bad *binary*, not
from *corrupted data*. Reinstalling v1 does not undo damage v2 did to a cache.
That is why the gate is the real protection, and why the Phase 0 items most
likely to be compressed — cancellation, mode preservation, lock contention — are
the ones guarding that exact class.

Go preserved on `go-legacy`.

---

## 7. `trash` — reclamation without deletion

Resolves contract-spec §14 item 7. v2 ships **`trash`, never `gc`.**

### Why a reaper has to exist

Not shipping one does not prevent deletion — it outsources it. failure-modes §1
observes that users "hand-roll a deletion script against a content-addressed
store, which is how people delete live data." A `find | xargs rm` written under
disk pressure is strictly more dangerous than a designed command, because it has
no notion of what is referenced, what is replicated, or what is recoverable.

The "users with SSH can delete anyway" argument justifies *shipping* a path. It
does not justify shipping an unguarded one: a hand-typed `rm` is deliberate and
narrow, while a tool command is trusted and scales. The bar is higher precisely
because people believe it.

### Core rule

**No git-sfs command ever unlinks a cache object.** Reclamation moves it:

```
<cache>/files/sha256/<prefix>/<hash>   →   <cache>/trash/<utc-ts>/<prefix>/<hash>
```

Consequences that make this cheap and safe:

- **Same filesystem by construction** — trash lives inside the cache root, so the
  move is a `rename`. O(1), no copying, works at terabyte scale.
- **Zero metadata.** The store is content-addressed, so the filename *is* the
  restore key: restoring is a rename back to the path derived from the hash. No
  manifest, no index, no hidden state — this satisfies the AGENTS.md ban outright
  rather than working around it.
- **Read-only bits are preserved.** `rename` needs write permission on the parent
  directory, not the file, so the §4.1 invariant survives the round trip and a
  restored object is still trusted without re-hashing.
- **Timestamped batches** are the restore granularity, because "undo what I just
  did" is the dominant recovery need.

### Only replicated eviction ships. `--unreferenced` does not.

**Decision: v2 reclaims only objects confirmed to exist on a remote.** There is no
`--unreferenced` mode, no `--repo` scoping, and no orphan-based reclamation.

This dissolves the detection problem rather than solving it. `countOrphans`
derives "unreferenced" from a single repo while the cache serves many, and it
ignores git history besides — but neither matters once reclamation stops asking
that question.

The reasoning is that **the unreferenced case is the hazard, not an unserved
need**:

- An object **on the remote** can be reclaimed freely — `pull` restores it. This
  is eviction, not deletion.
- An object **not on the remote** holds bytes that exist nowhere else.
  Reclaiming it *is* the data-loss event. It is the unbounded only-copy window
  of failure-modes §1.

The user need is disk pressure, and replicated eviction serves essentially all of
it: anyone using the tool as intended pushes, so everything legitimately
reclaimable is replicated. What `--unreferenced` would additionally reach is
precisely the set that must never be touched.

| Class | Criterion | Action |
|---|---|---|
| **Replicated** | Present on remote **and size matches** | Evict to trash |
| **Only copy** | Not confirmed on a remote | Refuse. Report as unpushed, not as reclaimable |
| **Leaked temp** | `.git-sfs-tmp-*` inside `files/` (failure-modes §3) | Delete — not content-addressed, unreferenceable |

**Presence is not correctness.** Remote *existence* must not be the sole
criterion, because failure-modes §7 establishes that push verifies nothing after
upload. The chain is real: a truncated object lands on the remote, `HasFile`
reports present, eviction removes the good local copy, and both copies are now
bad. Eviction therefore compares **size at minimum** (`lsjson` already returns
it) and still routes through trash rather than deleting. The redundancy is
warranted because the thing being relied on is known to be unverified.

This also removes the §13.4 push-verification defect from the critical path for
reclamation — but that defect should still be fixed, since it is what makes the
extra check necessary here.

### Git history makes "unreferenced" narrower than it looks

`countOrphans` compares against a **working-tree** scan (`verify.go:315-331`).
Git history is invisible to it.

An object unreferenced in `HEAD` may still be referenced by an older commit, a
branch, or a tag. Reclaiming it makes `git checkout <old-commit>` produce a repo
of dangling symlinks — the failure is deferred, arrives long after the operation
that caused it, and looks like repository corruption rather than reclamation.

This affects **both** tiers and is the strongest argument for the
remote-replicated default: an evicted-but-pushed object is re-fetchable at any
commit, so history stays intact. Any future `--unreferenced` mode must scan
reachable history, not just the checkout, or state plainly that it does not.

### Surface

```
git-sfs trash <paths>               evict replicated objects (size-verified)
git-sfs trash list                  batches, ages, reclaimable bytes
git-sfs trash restore <batch>       rename back into the object store
git-sfs trash empty [--older-than]  THE irreversible operation
```

`empty` is the only command in git-sfs that destroys bytes. It must be explicit,
never automatic, never triggered by disk pressure, and never implied by another
command. Time-based auto-expiry is deliberately excluded — a dataset that is
unreferenced for 30 days is not thereby unwanted.

### Contract impact

`<cache>/trash/` is a new sibling of `files/`, `tmp/`, and `locks/`. It is
invisible to v1: `countOrphans` walks only `files/sha256/`, and `PurgeTmp` touches
only `tmp/`. A v1 binary sharing the cache neither sees nor disturbs it, so this
is additive and migration-safe. Recorded in contract-spec §4.

### On the remote: trash-shaped if ever, but not in v2

**Today the remote is append-only.** git-sfs invokes exactly five rclone
subcommands — `lsd`, `lsjson`, `copyto`, `copy`, `version` — and has no deletion
path of any kind. That is a strong safety property and v2 should keep it.

If remote reclamation is ever added it must be trash-shaped, not delete-shaped.
But the remote case is materially harder than the local one, in four ways that do
not transfer:

| | Local | Remote |
|---|---|---|
| Cost of "move" | `rename` on one filesystem — O(1), free | **No move on S3/GCS.** Server-side copy + delete: billed, slow, multipart above 5 GB. Cheap only on SFTP/filesystem backends |
| Backstop | Safe *because* the remote still holds it | **Nothing behind it.** If the local cache was already evicted, remote trash is the only copy |
| Sharing scope | One user's repos | The whole team's repos, across every machine |
| Referencing | One working tree, scannable | Every clone, every branch, every commit — not determinable |

The third and fourth rows are decisive. "Unreferenced on the remote" means no
repo, on any machine, belonging to anyone, at any commit, needs this hash.
git-sfs cannot determine that, and a tool that cannot determine a precondition
should not offer the operation that depends on it.

**Use the backend's own mechanism instead.** S3 and GCS versioning turns a delete
into a delete marker with the bytes recoverable for a retention window — exactly
trash semantics, implemented by the storage layer, with zero data movement and no
billing surprise. Lifecycle rules handle expiry. Reimplementing that by shuffling
objects into a `trash/` prefix would be slower, costlier, and worse.

**Recommendation:** v2 keeps the remote append-only and documents backend
versioning as the supported answer to remote reclamation. If a future version
adds it, gate it on explicitly user-named hashes — never on inferred orphans.

#### Rejected: a shared `to_delete.txt` on the remote

Marking objects in a list file avoids the copy cost of moving bytes, which is the
right instinct. The design still fails, for reasons beyond the obvious corruption
risk:

- **No atomic read-modify-write.** Object stores offer no locking and no portable
  compare-and-swap. Two users marking different objects concurrently both read,
  both append, both upload — last write wins and the other's marks vanish
  **silently**. Lost updates are strictly worse than corruption, which is at
  least detectable.
- **Whole-list blast radius.** Every change rewrites the file, so one bad write
  loses every mark rather than one.
- **It is a manifest**, which AGENTS.md prohibits outright: *"Do not add
  manifests, databases, background services, custom protocols, or hidden
  metadata."*

**The deeper flaw is the deferral, not the format.** A marker file only has
purpose if deletion happens later, possibly by another actor. In a
content-addressed store an object can become *live again after being marked* —
someone adds a file whose content hashes to the marked object, dedup maps it to
that hash, and the mark is now stale. The deletion pass then destroys live data
while faithfully following the record. No checksum or atomic upload fixes a
time-of-check/time-of-use race.

If deferred marking were ever required, **one marker per object beats one shared
list**: `files/sha256/<prefix>/<hash>.deleted`, zero bytes, presence is the whole
signal. Disjoint keys eliminate the concurrency problem, corruption is scoped to a
single object, and an empty file cannot be malformed. It costs a doubled listing
size and still carries the TOCTOU hazard above.

Backend versioning avoids all of it: no bytes moved, no state of our own to
corrupt, recovery handled by the storage layer.

---

## 7c. Downgrade is an invariant, not a procedure

Users self-update *forward*. Nothing in v1 goes back. If v2 ships a defect that
touches data, the exit path must already exist — writing one afterwards is too
late, because the affected users are the ones who cannot afford to experiment.

**Requirement: v1 must remain able to operate any repo or cache v2 has touched.**

This is nearly free given decisions already made — no version floor (spec §6.5),
identical cache layout, identical remote layout, identical lock protocol. v2 was
already forbidden from writing anything v1 cannot read. Stating it as an
invariant makes it *testable* rather than incidental:

- **Downgrade test, in Phase 0 scaffolding and Phase 7 gating:** run a full
  workflow under v2, install v1 over it, run the same workflow again. Cache,
  symlinks, config, and remote must all still work.
- v1 binaries and their `SHA256SUMS` remain published permanently. A downgrade
  that requires building from source is not a downgrade path.
- Anything v2 adds that v1 ignores (`trash/`, spec §4) must stay ignorable —
  additive only, never a change to something v1 reads.

The invariant also constrains future work: any later v2.x feature that breaks it
is a breaking change requiring its own deliberate decision, not an incidental
consequence.

---

## 7b. Environmental checks — prevent at the choice, diagnose on request

failure-modes §1b–§1d and §6 enumerate assumptions nothing currently verifies.
The instinct is to put them all in `doctor`. That is wrong: it reproduces the
instruction pattern the project rejects — "run `doctor` and you'll find out" —
when most are preventable at the moment the bad state is created.

| Check | Enforce at | Why there |
|---|---|---|
| Cache reachable by `git clean -x` | `init`, `setup` | The only moment it can be prevented rather than reported |
| rclone config tracked by Git | `init`, `push` | Before credentials reach shared history |
| `core.symlinks` is true | before `add` | Otherwise `add` hashes pointer text as if it were data |
| Cache filesystem preserves modes | `setup`, result cached | The foundation §4.1 trust rests on |
| Cache on ephemeral storage | `setup` | Warn while the choice is still cheap to change |
| Free space | `pull` | Already attempted, but fails open twice (§13.3) |
| **Unpushed object count** | **`status`, prominently** | The unbounded only-copy window of failure-modes §1 |

`doctor` re-runs the full set on demand for diagnosis. It is the second line, not
the first.

The last row deserves emphasis. Between `add` and `push` the bytes exist in one
unreplicated place, the window is unbounded, and **no command currently reports
it.** Since v2 makes replicated-only eviction the reclamation model (§7), the
unpushed count doubles as "how much of your cache can never be reclaimed" — one
number that answers both the safety question and the disk-pressure question.

---

## 8. Known risks

**TLS versus static musl.** Phase 6 needs `x86_64-unknown-linux-musl` for the
"download tarball, run anywhere" promise. A client pulling `native-tls`/OpenSSL
breaks static musl linking — and it surfaces in the one phase with no test
coverage. `ureq` with rustls avoids it. **Lock this in at Phase 1**, not Phase 6.

**Config parser divergence.** Spec §6.3. The `#`-in-value case changes a remote
*path*, meaning v2 reads and writes a different location than v1 for the same
committed config. Silent, not a crash. Highest-severity divergence in the
rewrite.

**Lock protocol.** Spec §8. Invisible to single-binary tests; consequences are
concurrent writes to one cache.

**`doctor` and `status` drift.** Nearly all human output, so contract-only parity
leaves them almost unconstrained. Design their behavior deliberately rather than
discovering it.

**Phase 0 compression.** Under schedule pressure the harness will look skippable.
It is the only thing standing between an aggressive rewrite and silent data
corruption in a tool whose users cannot tolerate it.

**Bit rot — resolved: not git-sfs's to solve.** The tool holds no redundancy of
its own, so it can *detect* rot and never *repair* it. Detection without a repair
source is only a more precise obituary, and no scheduling policy changes that.
Storage-level integrity — ZFS, btrfs, RAID, a backend with its own checksumming —
is the actual mechanism, and reimplementing it would violate the project's own
framing as a layer over Git, the filesystem, and well-known movers.

What git-sfs *does* own is detection plus knowing which tier holds a good copy —
and replication already supplies the repair source:

| Situation | Remedy |
|---|---|
| Local object rots, present on remote | `pull` repairs it |
| Remote object rots, still cached locally | `push` repairs it |
| Both rot, or never pushed | Unrecoverable — only storage-level protection would have helped |

This is the same replicated/unpushed split that governs reclamation (§7): one
copy means rot is fatal, two means it is repairable. One concept covers both.

Consequences for v2:

- Keep `--rehash` and `--rehash-sample` as opt-in detection. Do not schedule
  them, do not enable them by default; hours of I/O over untouched data is a real
  cost and it buys detection, not protection.
- When rehash finds a corrupt object, **say whether a good copy exists** — repair
  from the other tier if replicated, and state plainly that the data is gone if
  not. Reporting corruption without saying which is the failure of §13.3 in a
  different costume.
- Document the storage-level requirement rather than approximating it. A cache on
  a filesystem with no integrity checking and no backups is a risk the tool
  should name at `setup` (§7b) and then stop pretending to manage.

**Platform scope.** Unix-only (linux, darwin) is an explicit non-goal for
Windows, matching v1. Windows *path* handling survives only where it affects
rclone remote strings such as `hwr1:F:/Storage` (`command.go:63`), not as host
support.

---

## 9b. Performance and scale — currently unmeasured

The plan claims a throughput win from SHA-NI (§4.1) and measures nothing. That is
a gap, not a detail: rewrites regress in unglamorous ways, and the classic for
this shape of tool is `rayon` defaulting to CPU-count threads on work that is
I/O-bound, turning parallelism into contention. At TB scale a 2× slowdown is a
serious user-facing regression.

**Baselines belong in Phase 0**, captured from v1 while it is still the thing
being run:

| Operation | Why it matters |
|---|---|
| Hash throughput, large single file | The hot path; SHA-NI claim lives or dies here |
| `add` / `import` of N files | Per-file overhead, lock and syscall costs |
| `push` / `pull` of N objects | rclone orchestration and concurrency |
| `verify` over a large tree | Walk plus stat cost |
| `verify --rehash` | Full re-read; the worst case |

Phase 7 gates on no regression past a stated threshold. Today the entire
benchmark surface is `BenchmarkStore8MiB`.

### Tests run at the wrong scale

Every workflow scenario uses files like `printf "one\n"` — twelve bytes — for a
tool whose domain is terabytes. Nothing exercises what only breaks at scale:
memory during walks, listing sizes, progress rendering with large counts,
file-descriptor limits, per-prefix bucket sizes.

Add a **scale tier**: generated or sparse files, thousands of objects, run
nightly rather than per-commit. It does not need to be slow to be useful —
sparse files give object *count* without byte volume, which catches most of the
per-object costs.

---

## 9. Expected outcome

| Metric | Go (v1.21.0) | Rust (projected) |
|---|---|---|
| Production source | 5,005 lines | ~2,800–3,500 lines |
| Test source | 3,098 lines Go + 610 shell | Re-derived + shell suite retained |
| Binary size | 12 MB | ~3–6 MB |
| Direct dependencies | 1 | ~14 |

The line reduction comes from deletions, not compression: `progress.go` (233),
`run.go` (47), most of `config.go` (~200), most of `hash.go` (~60), plus
shrinkage in `walk.go` and `fsutil.go`. The deleted code is disproportionately
the hand-rolled, fiddly kind.
