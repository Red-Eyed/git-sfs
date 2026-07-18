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
2. **Exit-code assertions** per scenario, including error paths.
3. **`insta` snapshots** for `--json` output only.
4. **Cross-binary interop** — lock contention and cache sharing between v1 and
   v2 (spec §8). No single-binary test can catch this class.
5. **Existing shell suite** — already contract-level. One assertion loosened.

The 3,098 lines of Go tests are a **specification to read, not code to
translate.** They test seams that will not exist. Mine them during Phase 0 for
edge cases worth promoting into the contract spec, then discard.

---

## 6. Phases

### Phase 0 — contract + harness (blocking)

Nothing else starts until this lands.

- [x] Write [contract-spec.md](contract-spec.md), grounded in the Go source
- [x] Fold [failure-modes.md](failure-modes.md) into spec §13 (do-not-reproduce)
- [ ] Resolve spec §14 open divergences — **items 5 and 7 block Phase 4**
- [ ] Mine the Go test suite for edge cases; fold into the spec
- [ ] Decouple `test/workflows/` from `go build` via `GIT_SFS_BIN`
- [ ] Loosen `scenarios.sh:215` to a structural assertion
- [ ] Build the tree-diff harness; prove it green **Go against Go**
- [ ] Build the cross-binary lock-contention harness
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

Full differential run, v2.0.0, Go preserved on `go-legacy`.

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

### The detection problem, which trash does not solve

Trash makes reclamation *recoverable*. It does not make orphan detection *sound*,
and conflating the two would be the mistake. `countOrphans` derives "unreferenced"
from a single repo while the cache serves many.

Safety therefore comes from a different criterion:

| Class | Criterion | Default |
|---|---|---|
| **Replicated** | Object confirmed present on the remote | Safe — this is *eviction*, not deletion; `pull` restores it |
| **Only copy** | Not confirmed on any remote | Refuse without an explicit override |
| **Leaked temp** | `.git-sfs-tmp-*` inside `files/` (failure-modes §3) | Always safe — not content-addressed, unreferenceable |

The reframe matters: for a pushed object, reclamation is not deletion at all. It
is cache eviction of something that exists elsewhere and can be re-fetched. That
is the operation users actually want under disk pressure, and it is fully safe.
The dangerous case — an object that was added but never pushed — is exactly the
unbounded-window hazard of failure-modes §1, and `trash` should refuse it loudly
rather than quietly widening that window.

Determining "unreferenced" still requires knowing which repos use the cache, and
the cache genuinely does not know. v2 requires explicit `--repo` scoping and warns
that unnamed repos may reference the same objects. See contract-spec §14 item 8.

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
git-sfs trash <paths|--evictable>   move objects to trash (default: replicated only)
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

**Today the remote is append-only.** git-sfs invokes exactly four rclone
subcommands — `lsjson`, `copyto`, `copy`, `version` — and has no deletion path of
any kind. That is a strong safety property and v2 should keep it.

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
