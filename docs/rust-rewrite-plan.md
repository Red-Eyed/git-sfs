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
checks become invariants the compiler enforces. Five qualify — and §2.5, keeping
errors from being discarded, is the one with the widest blast radius, since an
entire section of the do-not-reproduce list turns out to be downstream of it.

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

### 2.5 Discarded errors — the largest one, and the reason §13.3 exists

The whole problem in one line (`status.go:96`):

```go
sizes, _ := r.FileSizes(ctx, hashes) // on error treat all as absent
```

A remote returning 403, or timing out, or holding expired credentials, is
reported to the user as **a remote containing none of their data** — and
`status` exits `0` while saying it. The comment is not an oversight; someone
wrote down the decision to discard. Go made it a two-character change and
nothing downstream can tell the difference afterwards.

**§13.3 "Reporting untruths" is not five unrelated bugs. It is five instances of
this one affordance:** `HasFile` and `CheckFile` both `return false, nil` on any
error, the disk-space guard fails open twice, and `isRemotePathNotFound` greps
English because the error class was never carried as data in the first place.

Three things change in Rust, in increasing order of value:

1. `Result` is `#[must_use]`, so ignoring one warns by default. `let _ = ` still
   compiles but is explicit, greppable, and deniable via
   `clippy::let_underscore_must_use` at the workspace level.
2. Typed errors mean collapsing a class is a `match` arm someone has to write,
   not an absence of code.
3. **The signature stops permitting it.** `(bool, error)` makes `(false, nil)` a
   legal return on failure, conflating "absent" with "could not determine". A
   three-state result makes that conflation unrepresentable rather than merely
   discouraged — the same reasoning as carrying a *reason* for absence instead of
   a bare `None`.

The third is the real fix. The first two catch a developer who forgot; only the
third stops a developer who decided.

This is why v1 is an oracle for **mechanism** and never for **policy** (§5.1).
Where v1 discards an error, matching it faithfully would reproduce the defect
with the harness certifying it.

*Evidence, not theory:* the fake-rclone harness was pointed at a remote denying
every object listing. `status --remote` exited `0` and reported a successfully
pushed object as absent — while `verify`, which propagates the same error,
exited non-zero. One error, two commands, opposite honesty.

### 2.6 Where types do not help

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

The batching rule applies to every rclone command path, not just `push`.
Command code should minimize rclone subprocesses by calling batched remote port
methods (`copy_to_remote`, `copy_from_remote`, `file_sizes`) and formatting
object lists into rclone inputs such as `--files-from`. Loops over objects are
for local classification, planning, or integrity checks; if a caller needs one
rclone invocation per object, the operation must genuinely lack a batch form
and say why.

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
- [x] Mine the Go test suite for edge cases; fold into the spec — 14 clause
      groups added (spec §2, §3.3, §4.2, §4.3, §5.1, §5b, §6.4, §6.6, §8.2, §9.1).
      Two change v2's design: the lock is **five** locks named per command
      (§8.2), so the obvious consolidation to one `cache.lock` silently removes
      all v1↔v2 exclusion; and `semver::Version::parse` (§4.1's chosen crate)
      rejects the `v`-prefixed tag form git-sfs feeds its own version check
      (§6.6)
- [x] Decouple `test/workflows/` from `go build` via `GIT_SFS_BIN`
- [x] Loosen `scenarios.sh:215` to a structural assertion
- [x] Build the tree-diff harness; prove it green **Go against Go**
- [x] Build the fake-rclone recorder; capture v1's argv stream as the baseline
- [x] Build the cross-binary lock-contention harness
- [x] Build the SIGINT driver; prove against v1
- [x] Build the mode fault-injection hook; prove against v1 — found §13.4's
      push-replicates-rot defect. The chmod-interposer half is deferred until v2
      ships the §7b mode-preservation probe it would test
- [x] Capture v1 performance baselines (§9b) — threshold set at 1.25×, above the
      measured 1.08× noise floor
- [x] Build the downgrade test: v2 workflow → install v1 → same workflow (§7c) —
      also pins that v1 rejects unknown config keys, so v2 cannot add one
- [x] Enumerate every contract-spec clause with its coverage status
      (`test/differential/coverage.py`); 22 asserted, 8 structural, 6 observed,
      30 uncovered
- [x] Build the expected-divergence mechanism (`test/differential/divergences.py`,
      `run.py --candidate`) so an enumerated §13 fix is normalized *and* asserted
      to have happened, rather than reading as a regression
- [ ] Encode the remaining §13 divergences as positive assertions (§5.1) — one
      declared so far; most need v2 before their shape is knowable

Proving the harness against the Go binary first is what makes it trustworthy
before it becomes load-bearing.

### Phase 1 — skeleton + docs

- [x] Workspace: `git-sfs-core` (lib, cannot print/exit) + `git-sfs` (bin).
      `unsafe_code = "forbid"` and `clippy::let_underscore_must_use = "deny"`
      set at the workspace level — §2.5 as a compiler setting, not a convention
- [x] Full clap surface, all 14 commands parsing. Every command is currently a
      stub returning `Error::NotImplemented` (exit 70, `sysexits.h`
      `EX_SOFTWARE`, deliberately outside git-sfs's own 1–5/130 range) rather
      than succeeding quietly — an unported command must fail loudly, not read
      as success to the differential harness. The exit-code *taxonomy* itself
      (§9: Usage/Config/Integrity/Missing/Unavailable/Canceled, non-exhaustive
      by construction so a new variant fails to compile until classified) is
      designed and tested; per-command correctness of *which* code a given
      failure produces waits for Phase 4, when there are real failures to map
- [x] **Rewrite `AGENTS.md`** — split Go-minimalism vs. Rust-generosity
      Dependencies/Style sections instead of one contradicting the other, plus
      a "Status: mid-rewrite" orientation section for the two-tree reality
- [x] Rust `Justfile` (`just/rust.just`, wired into root `check`), CI workflow —
      a `rust` job (fmt/clippy/test/build) plus a `rust-musl` job proving the
      static-link requirement (§8) from Phase 1 rather than discovering it at
      Phase 6. The musl job is unverified on a real Linux runner as of this
      writing — it could not be tested locally on Darwin and needs its first
      GitHub Actions run to confirm

### Phase 2 — domain + pure plan layer

Newtypes, typed errors, pure `plan_*` functions. Unit-tested with zero
filesystem. **This is where the correctness thesis pays out** — get it wrong and
everything above it inherits the mistake.

- [x] `Sha256` (§2.1) — `crates/git-sfs-core/src/domain/hash.rs`. Only
      `parse()`/`from_digest()` construct one, so `prefix()` is total and the
      Go `len(s) < 2` guard is deleted, not ported. Proptest round-trips any
      32-byte digest and fuzzes `parse()` for panics
- [x] Symlink construction + validation (§3.1/§3.2) —
      `crates/git-sfs-core/src/domain/symlink.rs`. All six validation rules,
      taking the `readlink()` text as a plain argument so nothing touches a
      real symlink. Found a real bug in-flight: `pathdiff` returns `""` for
      identical paths where Go's `filepath.Rel` returns `"."`, which would have
      let a target pointing exactly at the cache root slip past the
      containment check; fixed, with a regression test
- [x] Remote naming + URL composition (§5.1) —
      `crates/git-sfs-core/src/domain/remote.rs`, ported from the actual
      `command.go` branches rather than the spec's own prose summary of them
      (the prose elides that `newRcloneRemote` trims trailing slashes on every
      branch). Every row of the spec's composition table is asserted verbatim
- [x] Version-floor comparison (§6.6) —
      `crates/git-sfs-core/src/domain/version_floor.rs`. Hand-rolled and
      deliberately not the `semver` crate: bare semver rejects git-sfs's own
      `v1.21.0` tag form, which would break every repo with
      `min_git_sfs_version` set in one release
- [x] `config.toml` schema, validation, and the dual-parser divergence check
      (§6.2/§6.3/§6.5) — **the plan's own "highest-risk item for the
      rewrite."** `crates/git-sfs-core/src/domain/config/`: `mod.rs` runs real
      TOML (`serde`+`toml`, `deny_unknown_fields` at every level, encoding most
      of §6.2's closed schema structurally rather than as hand-written checks
      — e.g. `n_jobs: Option<u32>` makes a negative value a deserialize error
      instead of a runtime one) alongside `legacy_scanner.rs`, a faithful port
      of v1's hand-rolled line scanner kept as its own module specifically
      because no generic parser could replicate its quirks. Reconciliation
      distinguishes a genuine TOML *grammar* failure (defers to v1's reading,
      §6.5 row 3) from a *semantic* one (always errors, regardless of what v1
      made of the same text) — found during testing that collapsing this
      distinction would have let `[remotes.""]` (valid TOML for an empty key,
      but invisible as such to v1's quote-unaware header parsing) and a
      truncated `algorithm = "sha256#not-sha256"` both slip through silently.
      The exact ambiguity message contract-spec §6.5 specifies is asserted
      character-for-character, and the `init` template is asserted to parse
      identically under both parsers (§6.4)
- [ ] Pure `plan_push`/`plan_pull`/`plan_verify` — deliberately deferred until
      Phase 3's `Store`/`Remote` port traits are sketched, so plan's input data
      shape is informed by what those ports can cheaply observe rather than
      guessed at in isolation and redesigned later
      - [x] `plan_push` + `plan_pull` — `crates/git-sfs-core/src/plan/`
      - [ ] `plan_verify` — deferred alongside the `verify` command itself
            (Phase 4): its planning shape is a genuinely different size than
            push/pull's binary present/absent split. Note for that design,
            surfaced in review of this step: `Remote::verify_file`'s
            download-then-hash-locally is not the only mechanism available —
            some backends (SFTP servers exposing the checksum extension; rclone's
            own `hashsum`/`--checksum` machinery more generally) can return a
            hash without transferring the object, cheaper than a full download.
            Whether to special-case that, and how to detect it without
            per-backend guesswork, is open — resolve it when `plan_verify`'s
            cheap-vs-expensive tiering is actually designed, not before

### Phase 3 — ports

`Store`, `Remote`, `Repo` traits; rclone and filesystem implementations; test
fakes.

- [x] `Cancellable<R: Read>` (§2.4) —
      `crates/git-sfs-core/src/ports/cancellable_io.rs`. Checks a `Cancel` flag
      on every `read()`, so `io::copy` over a `Cancellable` source inherits
      prompt cancellation structurally. Deliberately does **not** signal via
      `io::ErrorKind::Interrupted` — `io::copy`'s generic path treats that kind
      as EINTR-style and silently retries, which would make a canceled copy
      loop instead of stop; a private marker type on `ErrorKind::Other`,
      recovered via `is_canceled`, is used instead
- [x] `Store` trait + `CacheEntry` (§2.2, "the highest-value" invariant) +
      `FsStore` (real) + `FakeStore` (in-memory) —
      `crates/git-sfs-core/src/ports/store.rs`. `CacheEntry` has no public
      constructor or fields; the only way to obtain one is `Store::verified`
      or `Store::store`, both of which trusted an already-read-only object or
      freshly hash-verified one first. `verified()` returns `Result<Option<_>,
      StoreError>` — three states, not two, so a permission-denied `stat`
      (§2.5's exact defect class: v1's `HasValid` collapsed *any* stat error
      into "absent") cannot be mistaken for genuine absence; a present-but-
      wrong-content object is `Err(HashMismatch)`, not `Ok(None)`, since
      corrupt and missing are different classes (§9.1). A writable object is
      hash-verified and chmod'd read-only in place — both the §4.1 legacy-
      migration path and §9.1's declared wrong-permissions divergence.
      `store()` verifies the copied bytes **before** publishing (stricter than
      v1's verify-after-rename-then-remove-on-failure sequence: a corrupt
      object is never even briefly visible at its trusted path), and fsyncs
      the parent directory after rename — a durability gap §13.2 flags in v1
      ("atomic" is not "durable"). Every staging write goes through
      `tempfile::Builder::tempfile_in` pointed at the cache's own `tmp/`,
      never the bare `NamedTempFile::new()` that defaults to system `/tmp` —
      a full system-wide `/tmp` on a shared cluster has taken git-sfs down
      before even though the cache itself, on a separate filesystem, had
      room; now documented as its own failure mode (failure-modes.md §7,
      contract-spec §13.4)
- [x] `Store::adopt` (rename/move semantics for `import --move`, including the
      cross-device `EXDEV` fallback contract-spec §4.2 requires) —
      `crates/git-sfs-core/src/ports/store.rs`. Stricter than v1's `Move`
      (`cache.go:130-177`): `source` is hash-verified **before** it is
      touched, not after, so a mismatch never destroys the caller's only
      copy the way v1's rename/copy-then-hash ordering can. The `EXDEV`
      fallback re-verifies the copy at its staging path and only removes
      `source` once that passes, since a copy — unlike a rename — can
      corrupt data in transit. A dev+ino identity check guards the
      pathological case where `source` already *is* the object's own cache
      path, so "consume the source" can never mean deleting the object.
      Genuine cross-device rename cannot be produced portably in CI, so the
      fallback's copy primitive is unit-tested directly instead
- [x] `Lock` (§8) — `crates/git-sfs-core/src/ports/lock.rs`. Deliberately
      **not** a trait: one real implementation (mkdir-based, frozen
      name/mechanism), no second one contemplated, so per §3.3 it does not
      clear the bar for an abstraction. `LockName` is a closed five-variant
      enum rather than a bare `&str`, so the frozen names of §8.2 cannot
      drift by typo. Adds the policy §8.1 requires and v1 lacks: a
      definitely-dead owner PID (`kill(pid, 0)` → `ESRCH`, via the `nix`
      crate) is auto-broken and acquisition retried immediately, rather than
      blocking forever; a missing, empty, or malformed owner file is treated
      as *unknown*, not dead, so a live v1 process — which sometimes fails to
      write its own owner file (`lock.go:33` ignores that error) — can never
      be broken. `Lock::force_break` is the documented, named escape hatch
      for what liveness-checking cannot resolve on its own (a reused PID, a
      network-shared cache), replacing "the only recovery is `rm -rf` inside
      the cache." Test coverage includes a real blocking-then-release case
      and a real dead-process case (spawn-and-reap a child for a
      deterministic dead PID), not just the mechanism in isolation
- [x] `Remote` trait + rclone subprocess implementation + fake —
      `crates/git-sfs-core/src/ports/remote.rs`. `RcloneRemote` (real) +
      `FakeRemote` (in-memory) is the pair that earns the trait per §3.3;
      `rclone` stays the only backend (AGENTS.md), so this is not
      multi-backend polymorphism. Its methods are deliberately batch-shaped:
      `copy_to_remote`/`copy_from_remote` write rclone file lists, and
      `file_sizes` answers many metadata questions from one listing. Command
      implementations should compose those methods rather than loop over
      per-object rclone subprocesses. Three fixes over v1, all policy, not
      mechanism (the five rclone subcommands and the `<url>/files/sha256/...`
      layout are unchanged):
      1. Error classification by rclone's own documented exit codes
         (3/4 = confirmed not found, 5 = temporary, everything else
         permanent — <https://rclone.org/docs/#exit-code>) instead of
         `isRemotePathNotFound`'s English-substring grep (§13.3). `has_file`/
         `file_size`/`file_sizes` return three states — confirmed-absent,
         present, or `Err` when the question could not be answered — closing
         the exact `sizes, _ := r.FileSizes(...)` defect (§2.5) this rewrite
         exists to fix; a fake-rclone-on-PATH unit test
         (`has_file_returns_an_error_when_the_remote_cannot_be_reached`)
         asserts an unreachable remote is `Err`, never a silent `false`.
      2. `--temp-dir` is a required constructor argument routed through both
         copy directions, not optional-and-warned-on-pull/omitted-on-push
         (`command.go:234-274`) — the fix this checklist item was written to
         require, closing the full-system-`/tmp` outage class `Store`
         already refuses to reproduce.
      3. `copy_to_remote` gains `--ignore-existing`, which v1's push lacks —
         without it, push overwrites an already-good remote object with a
         locally-rotted read-only file trusted without re-hashing
         (`Store::verified`), destroying the one replica that could have
         repaired it (§13.4: "push replicates local rot over a good remote
         copy, and exits 0"). This one is not explicitly named by this
         checklist item; recorded here as a deliberate addition, not an
         oversight.

      Retries are restricted to rclone's own exit-5 "temporary" class,
      unlike v1's `retryLoop`, which retries every failure including bad
      credentials (§13.4) — verified by a unit test asserting a permanent
      failure is attempted exactly once regardless of `retry_max`.
      Cancellation polls and kills the child (100ms cadence, matching
      `Lock`'s poll interval) rather than checking per read chunk like
      `Cancellable`, since the byte-moving loop belongs to the rclone
      subprocess, not to this process; stdout/stderr are drained on their own
      threads so a chatty `copy` cannot deadlock the poll loop by filling its
      pipe buffer first. `git-sfs-core` cannot print, so no progress/debug
      writer is threaded through — rclone's output is captured and surfaces
      only in `RemoteError::Failed`'s message; live progress rendering
      belongs to Phase 5. Hashing-with-cancellation is shared with `Store` via
      a new private `ports::hashing` module rather than duplicated a second
      time. Unit tests drive a small fake `rclone` POSIX-sh fixture on an
      absolute path (never `PATH` — safe under `cargo test`'s parallel
      execution, unlike mutating the process-global `PATH` or env vars would
      be) for argv/exit-code behavior, plus `FakeRemote` contract tests
      proving it honors the same three-state shape.
- [x] `Repo` trait (symlink scanning, §3.3/§5b operation scope) —
      `crates/git-sfs-core/src/ports/repo.rs`. `FsRepo` (real, `walkdir`-backed)
      + `FakeRepo` (in-memory) earns the trait per §3.3. `Repo::scan` is one
      shared mechanism standing in for v1's *two* walks over the same tree:
      `collectGitSFSSymlinks` (`walk.go:18`, used by `push`/`pull`/`status`/
      `init`), which silently drops anything invalid, and `verify`'s own walk
      (`verify.go:120-155`), which reports each invalid one as a "broken git
      symlink" issue. `scan` returns every candidate —
      `ScannedEntry::Tracked`/`Invalid`/`Unrepresentable` — so which policy a
      command applies (drop vs. report) is a Phase 4 decision, not baked into
      the walk; `git-sfs-core` cannot print, so even the "report" side stays
      data here, to be rendered by a command layer, not this port.
      `should_skip` (`walk.go:58-65`) is ported onto the repo-relative path
      instead of v1's three separate absolute-path string comparisons: "any
      path component equals `.git-sfs`" is one check that produces the
      identical excluded set, verified by a test constructing the case where
      the two approaches could in principle diverge (a `.git-sfs`-named
      directory nested below root). A directory-read failure or a
      non-existent scope aborts the whole scan (§2.5: "cannot determine" is
      an error, not a partial silent result); a single symlink failing
      validation does not, matching v1's per-entry skip exactly.

      One case decided by discussion rather than by a v1 precedent: a
      symlink whose own *filename* (not its target) is not valid UTF-8. v1
      has no equivalent concern (Go strings are arbitrary bytes). Chosen
      behavior — skip it, but report it as `ScannedEntry::Unrepresentable`
      (a lossy display string, since there is no lossless `Utf8PathBuf` to
      give it) rather than aborting the whole scan or dropping it silently —
      keeps every other file in a large tree unblocked by one oddly-named
      entry while still surfacing it for a command layer to warn about.
      Tested on Linux only: Darwin's APFS enforces valid UTF-8 in filenames
      at the syscall level, so the scenario cannot even be constructed there
      (`symlink()` itself fails with `EILSEQ`).

      `FakeRepo` is seeded with raw target *text*, not a pre-classified
      Tracked/Invalid, and runs it through the exact same
      `validate_symlink_target` real repos use — a fake that let a test
      hand-declare an entry's classification could disagree with real
      validation and pass higher-layer tests against behavior no real `Repo`
      would produce. `walkdir` (rust-rewrite-plan §4.1) is a new runtime
      dependency, finally pulled in here.

**Phase 3 complete** — `Store`, `Lock`, `Remote`, `Repo` are all built. Phase
4 (commands) can begin; Phase 2's deferred `plan_push`/`plan_pull`/
`plan_verify` are unblocked too, now that all three port shapes exist to
design their input data around.

### Phase 4 — commands

Dependency order: `init`/`setup` → `add`/`import`/`mv` → `status`/`remotes` →
`push`/`pull` → `verify`/`doctor`. Differential harness runs continuously from
the first command.

`self update` is intentionally not part of this command phase. Unlike the normal
repository commands, it cannot be validated before Rust release artifacts and
checksum metadata exist; updating without something released to update to is a
fake test.

- [x] `init`/`setup` — `crates/git-sfs-core/src/exec/init.rs` and
      `crates/git-sfs-core/src/exec/setup.rs`, with CLI wiring in
      `crates/git-sfs/src/dispatch.rs`. `init` owns new project metadata and
      cache binding; `setup` owns clone-local cache binding only. Committed
      project state stays in `.git-sfs/`; local machine state defaults under
      `<git-dir>/sfs/`, with the cache at `<git-dir>/sfs/cache`. The
      repo-facing `.git-sfs/cache` symlink remains the single cache handle for
      normal commands. Existing repos are preserved: an existing cache symlink
      wins, and an old `.git-sfs/.cache` directory is recognized when the
      symlink is missing. `setup` does not probe cache objects or rewrite
      tracked file symlinks; those already point through `.git-sfs/cache`.
- [x] `add` — `crates/git-sfs-core/src/exec/add.rs`, ported from `add.go`.
      Needed three pieces of new port-layer plumbing along the way, added in
      the same commit:
      - `ports::local_state` (`discover_repo`/`resolve_cache_root`) — read
        normal command cache state from `.git-sfs/cache`; `init`/`setup` use
        the same module's binding helpers to create that symlink. If nothing
        is bound yet, resolution fails with a config error. Side-effecting
        inputs (cwd, requested cache path) are read once by the caller and passed in
        rather than read internally, both for injection/testability and
        because mutating a real env var from a test is unsafe under `cargo
        test`'s parallel execution.
      - `Repo::find_files` — a second trait method alongside `scan`, finding
        regular files instead of symlinks under a scope, for `add`'s own
        candidate discovery. Shares `scan`'s `should_skip`/walk mechanics via
        a new `filtered_walk` helper (only the walkdir/filter_entry setup is
        factored out — the trickiest closure-capture part — not the two
        loop bodies, which stayed separate since forcing them through one
        generic shape got awkward). A non-UTF-8-named candidate is skipped
        and reported as `FoundEntry::Unrepresentable`, not silently dropped
        and not an aborting error — the same treatment `scan` already gives
        this case, settled by discussion rather than any v1 precedent (Go
        strings are raw bytes, so v1 has no equivalent situation): CJK and
        other non-ASCII Unicode names are unaffected either way, since
        UTF-8 encodes all of it; this is only about byte sequences that
        are not valid UTF-8 at all.
      - `ports::hashing` made `pub` (was `pub(crate)`, shared only between
        `Store` and `Remote` until now) — `add` needs to hash an arbitrary
        source file before handing it to `Store::store`, and that is a
        different concern from either port's own object store.

      `add::add` returns `Result<AddOutcome, AddFailure>`, where
      `AddFailure` bundles whatever succeeded *before* the failure alongside
      it (boxed, since `AddError`'s largest variant carries a full
      `StoreError` and clippy's `result_large_err` correctly objects to that
      living unboxed in every `Ok` return path too) — this is what lets the
      binary still report partial progress without Phase 5's `Event` stream
      existing yet. Two more deliberate simplifications for this pass, both
      called out in the module doc rather than silently assumed: sequential
      only (no rayon-based parallelism — a performance layer on top of
      correct sequential behavior, not part of getting it right first), and
      relative path arguments resolve against the repository root, not the
      current directory, matching v1's `absFromRepo` exactly even though
      that is a real usability quirk (`cd data && git-sfs add foo.bin` means
      `<repo>/foo.bin`, not `<repo>/data/foo.bin`) worth reconsidering later
      as an argument-semantics decision, not something to silently change
      while porting. `core.symlinks=false` detection (contract-spec §13.5) is
      not in this pass either — new v2-only protection, no v1 mechanism to
      port, same reasoning as the other deferred §7b environmental checks.
      Verified against the real built binary end to end (hash, store,
      symlink, content round-trips through the link, re-`add` on an
      already-converted file is a silent no-op), not just unit tests.
- [x] `mv` — `crates/git-sfs-core/src/exec/mv.rs`, ported from `mv.go`.
      Smaller than `add`: contract-spec §3.3 is explicit that a committed
      symlink is the unit of operation, not the object behind it, so `mv`
      never touches the cache or the `Store` port at all, needs no lock (v1's
      `mv.go` takes none either — locks exist to serialize cache mutation,
      and `mv` never mutates the cache), and needed no new port-layer
      plumbing beyond bumping `ports::repo::resolve_scope` (v1's
      `absFromRepo`) to `pub(crate)` so `mv`'s own `src`/`dest` resolve the
      same way `Repo::scan`'s scope argument does, rather than duplicating
      that absolute-vs-relative branch a third time.

      Two cases, both ported behavior-for-behavior from v1's `mvLink`/`mvDir`
      (`mv.go:35-117`), including the sequencing choices that are load-bearing
      rather than stylistic:
      - **Single symlink.** Validate `src` as a git-sfs symlink (any failure —
        not a symlink, non-UTF-8 target, a target failing contract-spec
        §3.2's rules — reported uniformly as `MvError::NotATrackedLink`,
        matching v1's own undifferentiated wrap), apply POSIX
        place-inside-an-existing-directory semantics to `dst`, write the new
        symlink *before* removing the old one, and roll the new one back if
        removing the old one fails — never a window with neither in place.
      - **Directory.** Collect every `Repo::scan`-`Tracked` symlink under
        `src` *before* renaming anything (a relative target only validates
        against its current location), perform the whole move as one atomic
        `rename()`, then rewrite each relocated symlink's target for its new
        depth from the repository root. Entries `scan` reports as `Invalid`
        or `Unrepresentable` ride along with the rename untouched — same
        "skip, don't abort" policy as v1's inline walk, which silently
        `return nil`s past a `ParseGitSymlink` failure rather than treating
        it as an error.

      `mv::mv` returns `Result<Vec<MovedLink>, MvFailure>` — same
      outcome-so-far-plus-boxed-error shape as `AddFailure`, since the
      directory case's retargeting loop can fail partway through and a
      caller should still see which links already moved. Fixed a related gap
      surfaced while writing this: `AddError`'s own `From` impl classified a
      `RepoError::Canceled` bubbling through `AddError::Repo(_)` as
      `Error::Unavailable` instead of `Error::Canceled`, losing cancellation's
      precedence over every other classification — corrected alongside `mv`'s
      own (correct from the start) handling of the same case.

      Verified against the real built binary end to end: single-file mv
      within a directory, mv across a depth change (confirms the climb count
      in the rewritten target actually changes, not just that *a* target is
      written), and a whole-directory mv containing two tracked links at
      different depths, each confirmed still readable through its rewritten
      symlink afterward.
- [x] `import` — `crates/git-sfs-core/src/exec/import.rs`, ported from
      `import.go`, with one deliberate sequencing change contract-spec §13.1
      requires rather than permits: v1's `ImportWithOptions` runs a parallel
      "prepare" phase that hashes and `--move`s *every* source into the
      cache first (`import.go:70-79`), and only afterward, in a second
      serial pass, creates any destination symlinks (`import.go:80-102`). A
      crash between the two phases leaves a file gone from its external
      location with no symlink yet reaching it — safe in the cache by hash,
      but unreachable from the working tree. Here, each source is hashed,
      cached, verified reachable, and symlinked back-to-back before the next
      source is even looked at, so at most one file is ever mid-flight
      instead of the whole import.

      This reopened a real design question during review: `Store::adopt`
      (Phase 3's `--move` primitive, a same-filesystem rename with a
      verified-copy-then-remove fallback across a device boundary) removes
      its source as an inherent side effect of succeeding, one step before
      the caller can create any symlink — so even perfect per-file
      interleaving can't make `adopt`-then-symlink satisfy "publish before
      remove" for that one file. The alternative (always `Store::store` —
      copy — then delete the original only after publish) was rejected on
      the user's own correctness ground, not just style: for the large
      datasets this project targets, requiring a full second copy before
      deleting the original needs roughly double the free disk space to
      move data that already fits once, which can turn an otherwise-routine
      `--move` of a mostly-full drive into an impossible one. `adopt` stays
      the `--move` primitive; what changed is *when* it's called (per file,
      immediately followed by that file's own symlink) rather than *what*
      it does.

      Two more things follow directly from `import`'s source living outside
      the repository, unlike every other Phase 4 command's arguments:
      - `src` resolves against the *current directory*, not the repository
        root (v1's `filepath.Abs`, `import.go:161`) — `dst` still resolves
        against `repo`, like everywhere else. `exec::import` is the first
        command needing both, so `import()` takes `cwd` as an explicit
        parameter (`dispatch.rs`'s three commands needing it now share one
        `current_dir_utf8()` helper instead of repeating the same two-line
        UTF-8-validation dance).
      - A directory source's destination-collision validation, `-L`
        symlink handling, and empty-git-sfs-directory rejection are batched
        entirely up front and read-only (`plan_import`, not a `crate::plan`
        function despite the name — it does real filesystem I/O, so it
        lives in `exec` instead) — contract-spec §5b.2 requires a rejected
        import to be a no-op, and validating before any byte moves is what
        makes that true regardless of the sequencing change above. Also
        preserved from v1: two source paths resolving to the same canonical
        file (`std::fs::canonicalize`, matching `filepath.EvalSymlinks`) are
        deduplicated so the underlying file is only hashed/cached once, and
        a directory source merges its contents directly into an existing
        destination directory rather than nesting under the source's own
        basename — deliberately different from `mv`'s placement rule,
        since nothing flags v1's import behavior here as a defect.

      Same simplifications as `add`, called out for the same reason
      (flagged, not silently assumed): no auto-`init`/cache-creation
      (requires an already-bound cache — the init/setup question stays
      parked), sequential only, no progress callback.

      Fixed a related classification gap while writing this module's own
      `From<ImportError> for Error`: fully delegating a wrapped `StoreError`
      to its own classification (`HashMismatch` → `Integrity`, `Canceled` →
      `Canceled`) rather than the blanket `Unavailable` `AddError`'s
      equivalent conversion currently uses — noted here rather than changed
      there, since unlike the `RepoError::Canceled` gap fixed alongside
      `mv`, this one is a closer judgment call (whether a same-file
      TOCTOU hash mismatch during `add` is better read as "integrity" or
      "retry might help") that deserves its own decision, not a drive-by.

      Verified against the real built binary end to end: copy-by-default
      leaves the source intact, `--move` removes it only after the
      destination is confirmed published, a directory import merges into an
      existing destination directory, and both rejection paths (destination
      already exists; a symlink source without `-L`) leave every involved
      path — source, symlink, existing destination — completely untouched.

      Also surfaced, unrelated to this change: `ports::remote::tests::
      cancellation_stops_promptly_even_mid_subprocess` is intermittently
      flaky (timing-sensitive subprocess-cancellation assertion, not touched
      this pass) — noted for a separate look, not investigated here.
- [x] `status`/`remotes` — `crates/git-sfs-core/src/exec/status.rs`,
      `crates/git-sfs-core/src/exec/remotes.rs`, and
      `crates/git-sfs/src/status_output.rs`. `status` scans once, deduplicates
      by hash for object counts, and uses `Remote::file_sizes` for remote
      metadata so a broken remote reports `unknown` rather than "absent".
      `remotes` reads committed config only and never contacts rclone.
- [x] `push` — `crates/git-sfs-core/src/exec/push.rs`, with the CLI wiring in
      `crates/git-sfs/src/dispatch.rs`. The command verifies local cache
      objects before planning, honors `--skip-missing`, and hands the complete
      upload set to `Remote::copy_to_remote` as one batch rather than one
      rclone call per object. The project-wide rclone batching rule is now
      recorded in AGENTS.md and §5.2b above.
- [x] `pull` — `crates/git-sfs-core/src/exec/pull.rs`, with the CLI wiring in
      `crates/git-sfs/src/dispatch.rs`. The command scans once, treats
      writable hash mismatches as untrusted bytes to remove before download,
      performs one batched remote size listing for the disk-space guard, and
      performs one batched `copy_from_remote` before verifying the downloaded
      objects through `Store::verified`.
- [x] `verify` — `crates/git-sfs-core/src/exec/verify.rs`, with CLI wiring in
      `crates/git-sfs/src/dispatch.rs`. The command reports invalid symlinks,
      missing/corrupt local objects, missing remote objects, remote size
      mismatches, and advisory orphan counts. Default remote checks use one
      batched `Remote::file_sizes` call; `--with-integrity` still avoids a
      per-object rclone loop by batch-downloading the remote object set to a
      scratch tree with `copy_from_remote` and hashing those bytes locally.
- [x] `doctor` — `crates/git-sfs-core/src/exec/doctor.rs`, with CLI wiring in
      `crates/git-sfs/src/dispatch.rs`. The command reports repository,
      config, version, cache, rclone, and per-remote diagnostics; v2 adds the
      cheap environmental checks called out in §7b (`core.symlinks` and cache
      permission-bit preservation). Rclone work is intentionally minimal:
      detect the rclone version once, then run backend/path checks only for
      the requested remote set.

### Phase 5 — reporting

- [x] Binary-side reporting foundation — `crates/git-sfs/src/reporting.rs`.
      Command rendering moved out of `dispatch.rs`, so orchestration now
      receives core outcomes and hands them to one terminal-output layer
      instead of growing ad hoc `println!` helpers beside every command arm.
      `--quiet` is also centralized as a binary-only `RenderMode`: it
      suppresses success chatter for byte-moving/renaming commands
      (`add`/`mv`/`import`/`push`/`pull`) and `verify ok`, while warnings,
      failure reports, and primary command output (`status`, `remotes`,
      `doctor`) stay visible. Core remains untouched and still takes no
      writer, quiet flag, or progress callback.
- [x] Live progress foundation → `indicatif`. `crates/git-sfs/src/progress.rs`
      wraps the existing `Remote` port at construction sites, so `push`,
      `pull`, `status --remote`, and remote `verify` show coarse rclone-call
      progress without adding writers, quiet flags, or callbacks to core. This
      deliberately stops at wrapper-visible facts; a richer core event stream
      should only be added if later UX needs per-file or per-byte details that
      cannot be observed at the binary edge.
- [ ] JSON output review. `status --json` and `remotes --json` exist; any
      further JSON shape work belongs here, not in command execution.

### Phase 6 — release

Build and CI packaging come here, near the end, because they are the surface
users actually install. This phase produces the release artifacts that
`self update` can later consume: zigbuild, musl targets, `ureq`+rustls, archive
naming per spec §11, checksum generation, and release workflow wiring. **The
risk phase** — mistakes here are user-visible and hard to reverse.

### Phase 7 — self update

Implement and test `git-sfs self update` only after Phase 6 can produce a real
Rust release artifact and checksum metadata. The command should exercise the
same download-verify-replace path a user will hit, against either an actual
release or a local release fixture shaped exactly like one.

### Phase 8 — cutover

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

- **Downgrade test, in Phase 0 scaffolding and Phase 8 gating:** run a full
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

Phase 8 gates on no regression past a stated threshold. Today the entire
benchmark surface is `BenchmarkStore8MiB`.

### Captured — and what the numbers may be used for

`test/differential/benchmark.py` drives the **binary**, not internal packages.
The existing Go benchmarks measure seams that an idiomatic rewrite deletes, so
they cannot be the baseline; only the command surface is comparable across both
implementations. `just perf` runs it, `just perf-selfcheck` establishes noise.

**Absolute times do not gate anything.** A millisecond count from one laptop says
nothing about another machine or another CI runner. The gate is the **ratio
between two binaries measured side by side in a single run**, which is available
throughout because v1 survives on `go-legacy`. Committed baselines under
`test/differential/baselines/` are reference material: they record what the
workload cost on a named machine, nothing more.

First capture (v1.21.0, Darwin arm64, 10 cpus, 1000×1 KiB files, one 256 MiB
file, best of 3):

| Operation | v1 |
|---|---|
| `add` (1000 files) | 3811 ms |
| `push` (1000 objects) | 222 ms |
| `pull` (1000 objects) | 347 ms |
| `verify` | 15 ms |
| `verify --rehash` | 75 ms |
| `add` (one 256 MiB file) | 353 ms — **725 MiB/s** |

**Threshold: 1.25×, per operation.** The self-check — one binary measured under
two names — returns 0.99–1.08×, so roughly 8% is measurement noise rather than
signal, concentrated in `add_large` where a single short I/O-bound measurement is
most exposed to page-cache state. A threshold has to clear that noise floor
without being so loose it waves through the `rayon`-contention regression this
section exists to catch; 1.25× does both. Re-run `just perf-selfcheck` on the
gating machine before trusting it, since the floor is machine-specific.

The SHA-NI claim (§4.1) is now falsifiable: 725 MiB/s on this machine is the
number to beat, and `add_large` is where it shows up.

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
