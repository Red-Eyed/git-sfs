# git-sfs Conformance Contract

**Status:** normative for the Rust rewrite (v2).
**Derived from:** the Go implementation at v1.21.0.
**Audience:** anyone implementing or reviewing a git-sfs binary.

This document defines the observable contract that any git-sfs implementation
must satisfy. It exists because the v2 rewrite adopts **contract-only parity**:
internal architecture and human-readable output are free to change, while
everything enumerated here is frozen.

With byte-identical parity, the previous binary *is* the specification and no
document is needed. Under contract-only parity that is no longer true — without
this file, "the contract" is undefined and regressions are undetectable. Every
clause below cites the Go source it was derived from so it can be re-verified
rather than trusted.

A clause is **frozen** because breaking it corrupts user data, breaks
interoperability between versions sharing a cache or remote, or breaks
already-installed binaries. Nothing is frozen merely because it is current
behavior.

## 0. Frozen mechanism vs. free policy

**This distinction governs the whole document and must not be collapsed.**

- **Mechanism** — the formats, paths, and codes that two binaries must agree on
  to interoperate. Frozen. A v1 and a v2 process sharing a cache, a remote, or a
  lock must read and write identical bytes in identical places.
- **Policy** — what an implementation *decides* on the basis of that mechanism:
  what it trusts without re-checking, how it classifies an ambiguous error, when
  it gives up waiting. **Not frozen.** v2 may be stricter, more skeptical, or
  more informative.

Being *stricter* than v1 never breaks interoperability. Reproducing v1's
trust assumptions and error classifications would freeze known defects into v2,
which is the opposite of the rewrite's purpose.

Concretely: the cache's read-only bit (§4.1) is mechanism; treating it as
sufficient proof of verification is policy. The lock directory path (§8) is
mechanism; waiting forever without a liveness check is policy. See §13 for the
defects that must not be reproduced.

---

## 1. Why each area is frozen

| Area | Frozen because |
|---|---|
| Symlink format | Committed to Git. Existing repos contain these; misreading them loses data references |
| Cache layout & modes | Shared on disk between versions; mode bits are semantically load-bearing |
| Remote layout | Shared between *users* and versions pushing to the same bucket |
| Lock protocol | Inter-process mutex; v1 and v2 binaries will coexist during migration |
| `config.toml` | Committed to Git; must be readable by both versions |
| Exit codes | Scripted against, documented, used by CI |
| `--json` shapes | Machine-readable by definition |
| Release artifacts | Installed v1 binaries self-update by fetching these exact names |

---

## 2. Repository layout

```
<repo>/
  .git-sfs/
    config.toml        committed
    cache -> <abs>     symlink to cache root; MUST NOT be committed
  <tracked files>      relative symlinks into .git-sfs/cache/
```

`.git-sfs/` is created with mode `0o755`
(`localstate.go:53-62`).

The `.git-sfs/cache` symlink target is the **canonicalized absolute path** of
the cache root — `filepath.EvalSymlinks`, falling back to `filepath.Clean` when
resolution fails (`localstate.go:97-103`).

**Rebinding is an error, not an overwrite.** If `.git-sfs/cache` exists and
resolves to a different path than requested, implementations MUST fail with an
invalid-config error rather than relink (`localstate.go:73-85`). Silently
repointing a cache is a data-loss vector.

---

## 3. Symlink format

### 3.1 Construction

For a tracked file at `<file>` in `<repo>` with content hash `h`:

```
cache_link_file = <repo>/.git-sfs/cache/files/sha256/<h[0:2]>/<h>
target          = relative_path_from(dirname(<file>), cache_link_file)
```

(`sfspath.go:15-22`)

Targets MUST be relative. The indirection through `.git-sfs/cache` is what keeps
machine-local cache paths out of committed metadata.

Example — file `<repo>/data/train.bin`, hash `ab3f…` (64 hex):

```
data/train.bin -> ../.git-sfs/cache/files/sha256/ab/ab3f…
```

### 3.2 Validation

A symlink is a valid git-sfs link if and only if all hold
(`sfspath.go:25-55`):

1. `readlink` succeeds.
2. Target is **not** absolute.
3. `resolved = clean(join(dirname(file), target))` lies under
   `<repo>/.git-sfs/cache/files/sha256`, i.e. the relative path from that root
   neither begins with `..` nor equals `.`.
4. That relative path has **exactly two** components.
5. Component 2 is a valid SHA-256: exactly 64 characters, each in `[0-9a-f]`.
   Uppercase is rejected (`hash.go:82-92`).
6. Component 1 equals the first two characters of component 2.

Rule 6 is redundant with the hash but is deliberately enforced so that stale or
hand-edited links fail loudly instead of resolving to the wrong object.

Violations produce `ErrInvalidSymlink` → **exit code 3**.

---

## 4. Cache layout

```
<cache_root>/
  files/sha256/<prefix>/<hash>   content-addressed objects
  tmp/                           staging for in-flight writes
  locks/                         inter-process locks
  trash/<utc-ts>/<prefix>/<hash> v2 addition — reclaimed objects, recoverable
```

(`cache.go:29-47`)

`trash/` is introduced by v2 (see rust-rewrite-plan §7) and is **additive and
migration-safe**: v1's `countOrphans` walks only `files/sha256/` and `PurgeTmp`
touches only `tmp/`, so a v1 binary sharing the cache neither sees nor disturbs
it. Objects there retain their read-only mode, so a restored object still
satisfies §4.1 without re-hashing.

`FilePath(h) = <root>/files/sha256/<h[0:2]>/<h>` — deterministic and
content-addressed. Directories are created via `EnsureDir`; `PurgeTmp` recreates
`tmp/` with `0o755` (`cache.go:51-56`).

### 4.1 File modes are load-bearing

**Cache objects are stored with all write bits stripped** (`mode &^ 0o222`), and
this is not cosmetic. `HasValid` treats a read-only file at the
content-addressed path as *proof that the bytes were hash-verified when
written*, and therefore skips re-hashing (`cache.go:58-81`).

Any implementation MUST:

- Strip write bits **before** the object becomes visible at its final path.
- Treat a cache file that still carries write bits as **unverified**, hash-verify
  it, and only then protect it in place. This is the one-time legacy migration
  path, and it MUST be preserved — caches written by older versions rely on it.

Publishing a writable file at a content-addressed path silently defeats every
later integrity check in the system. This is the single most dangerous invariant
in the contract.

**Mechanism vs. policy (§0).** *Writing* objects read-only is mechanism and is
frozen — v1 will read this cache and depends on it. *Trusting* the read-only bit
as sufficient proof of verification is **policy, and v2 need not inherit it.**

That trust is only as strong as the filesystem's willingness to preserve the bit,
and several realistic environments do not: exFAT/FAT, some FUSE and network
mounts, SMB/NFS with unusual id mapping, Docker volume copies, `rsync` without
`-p`, archive extraction. Any of these can present unverified bytes wearing a
read-only bit, which is then trusted permanently and never re-hashed. Root writes
through the bit entirely.

v2 is therefore free — and encouraged — to probe whether the cache filesystem
actually preserves modes (write, chmod, re-stat) and to fall back to hash
verification where it does not. Being stricter than v1 here cannot break
interoperability.

### 4.2 Write protocol

Writes are temp-file-plus-rename. The final mode is set on the temp file
*before* the rename, so no post-rename `chmod` window exists
(`cache.go:105-127`, `fsutil.go:48-78`).

After rename, the published file is hash-verified. **On mismatch the published
file MUST be removed** before returning the error, so a later `HasValid` cannot
trust a corrupt entry (`cache.go:122-125`).

Cache objects are write-once and immutable thereafter.

---

## 5. Remote layout

```
<remote_url>/files/sha256/<prefix>/<hash>
```

(`command.go:67-70`, `command.go:207-209`)

Mirrors the local cache layout. This is frozen **across users**, not merely
across versions: several people and several git-sfs versions push into the same
bucket concurrently. A layout change silently partitions a shared remote into
two disjoint stores, and the symptom is "my colleague's push didn't arrive."

`rclone` is the only supported mover. `<remote_url>` is composed from the
remote's `backend` and `path` config fields.

---

## 6. `config.toml`

### 6.1 Schema

```toml
version = 1                        # REQUIRED, must be exactly 1

[remotes.<name>]
backend = "..."                    # REQUIRED — rclone remote name
path    = "..."                    # optional — path within backend
config  = "..."                    # optional — rclone config path, relative to .git-sfs

[settings]
algorithm          = "sha256"      # optional, default "sha256"; only sha256 accepted
n_jobs             = 0             # optional, default 0 (auto); must be >= 0
retry_max          = 3             # optional
min_rclone_version = "1.67.0"      # optional
min_git_sfs_version = "1.6.0"      # optional
```

(`config.go:84-271`)

Note `n_jobs` in the file maps to the internal `Jobs` field — the TOML key is
`n_jobs`, not `jobs`.

### 6.2 Validation rules

Parsing is **strict and closed**. All of these are errors
(`ErrInvalidConfig` → exit 1):

- Any unknown top-level field, unknown section, or unknown field within
  `[settings]` or `[remotes.*]`.
- A `cache` field or any `[cache]` / `[cache.*]` section — rejected with a
  dedicated message. Local cache paths must never enter committed config.
- `version` absent or not `1`.
- `algorithm` present and not `"sha256"`.
- `n_jobs` negative or non-integer; `retry_max` non-integer.
- Any remote missing `backend`.
- An empty remote name (`[remotes.]`).
- A key/value line appearing under `[remotes.` before a remote name is set.

### 6.3 The parser is NOT TOML — divergence warning

**This is the highest-risk item for the rewrite.** The v1 parser is a
hand-rolled line scanner (`config.go:153-306`), not a TOML implementation. It
differs from real TOML in ways that change parsed values:

| Behavior | v1 parser | Real TOML |
|---|---|---|
| `#` inside a quoted string | **Truncates the value there** (`stripComment`, `config.go:301`) | Part of the string |
| Quote handling | `strings.Trim(s, "\"'")` — strips *all* leading/trailing quote chars | Proper string parse |
| Escape sequences | Not interpreted | Interpreted |
| Multi-line strings, arrays, inline tables, datetimes | Unsupported | Supported |
| Duplicate keys | Last wins silently | Error |

The dangerous case is concrete: `path = "datasets/run#1"` parses as
`datasets/run` under v1 and as `datasets/run#1` under the `toml` crate. That is
a **different remote path**, so v2 would read and write a different location
than v1 for the same committed config — silent divergence, not a crash.

Adopting the `toml` crate is still the right call (a hand-rolled parser is worse
in every other respect), but it is a deliberate, documented divergence rather
than an accident. Required mitigations:

1. Reject or warn on any config value containing `#`, `\`, or an interior quote,
   where v1 and TOML semantics disagree.
2. Keep the closed-schema validation of §6.2 — `serde`'s `deny_unknown_fields`
   plus explicit checks. Real TOML parsing must not silently widen what is
   accepted.
3. Cover the divergent cases in the differential test suite explicitly.

### 6.4 Defaulting

- Missing `algorithm` defaults to `"sha256"` **after** parsing, then is validated.
- `Default()` (`config.go:108-116`) produces the in-memory default; `init` writes
  the annotated template at `config.go:125-151` with mode `0o644`.

---

## 7. Local state resolution

### 7.1 Repository discovery

Walk upward from the current working directory until a `.git` entry exists.
`.git` may be a **directory** (normal repo) or a **file** (submodule or
worktree) — both are accepted, via `os.Stat` rather than a directory check
(`localstate.go:16-32`). Stop at filesystem root and error.

### 7.2 Cache resolution precedence

Strictly ordered (`localstate.go:35-50`):

1. `--cache` flag, if non-empty
2. `GIT_SFS_CACHE` environment variable, if non-empty
3. `.git-sfs/cache` symlink target
4. Otherwise `ErrMissingCacheConfig` → **exit 1**

All resolved paths are made absolute. A missing `.git-sfs/cache` symlink is
**not** an error at resolution time — it yields an empty value that falls
through to case 4 (`config.go:273-286`).

---

## 8. Lock protocol

```
<cache_root>/locks/<name>.lock/     directory, mode 0o755
<cache_root>/locks/<name>.lock/owner   contains "pid: <N>\n", mode 0o644
```

(`lock.go:19-63`)

Mutual exclusion is achieved by `mkdir` succeeding atomically. On contention,
poll every **100 ms** until acquired or the context is canceled. Release is
`RemoveAll` of the lock directory.

**This is an inter-version contract.** During migration a user will plausibly run
a v1 binary in one shell and v2 in another against the same cache. If v2 changes
the lock path, the directory name, or the acquisition mechanism, both processes
acquire "the lock" simultaneously and write concurrently to the same cache.

No single-binary test can detect this. The differential suite MUST include a
cross-binary contention case: hold the lock with the v1 binary, confirm v2
blocks, and vice versa.

The `owner` file's `pid:` format is advisory (used for the human "waiting for
lock, held by…" notice) and its wording is not frozen — but the file's presence
and location are, since the other binary reads it.

### 8.1 Waiting forever is policy, not contract

The mkdir mechanism and lock path are frozen. **Indefinite waiting is not, and
v2 must not reproduce it.**

v1 spins on `os.Mkdir` with no timeout, no staleness threshold, and no escape
hatch. The owner PID is recorded but never checked for liveness. A SIGKILL, OOM
kill, container eviction, or crashed CI runner therefore leaves a lock directory
that blocks **every subsequent `add`/`import`/`push`/`pull` forever**, and the
only recovery is `rm -rf` inside the cache — the single most dangerous operation
a user can perform on a content-addressed store.

v2 requirements:

- Record the PID (mechanism, frozen) **and check it for liveness** (policy, new).
- Provide a documented break-glass path so recovery is never "delete things
  inside the data store."
- Read the `owner` file defensively. v1's `lockOwner` slices `data[:len(data)-1]`
  with no length check (`lock.go:62`) while the write that creates it ignores its
  own error (`lock.go:33`), so a zero-byte owner file is reachable and reading it
  panics. v2 must tolerate a missing, empty, or malformed owner file.

Liveness checking is strictly more conservative than v1's behavior and cannot
cause a v1 process's lock to be broken while that process is alive.

**Known limitation, unchanged:** locks are per-cache and offer no mutual
exclusion between hosts sharing a network cache. v2 does not fix this; it should
not pretend otherwise.

---

## 9. Exit codes

Frozen (`main.go:15-42`):

| Code | Meaning | Triggers |
|---|---|---|
| `0` | Success | — |
| `1` | Config or usage error | `ErrInvalidConfig`, `ErrMissingCacheConfig` |
| `2` | I/O or remote error | Default for unclassified errors |
| `3` | Integrity failure | `ErrCorruptCachedFile`, `ErrCorruptRemoteFile`, `ErrWrongCachePermissions`, `ErrInvalidSymlink` |
| `130` | Canceled by user | SIGINT / SIGTERM |

**Cancellation takes precedence over every other classification.** Ctrl-C must
read as canceled, not as whatever partial error the aborted operation happened
to produce (`main.go:27-30`).

Errors print to **stderr** prefixed `git-sfs: `; cancellation prints
`git-sfs: canceled`. The prefix and stream are frozen; the message wording after
the prefix is not.

### 9.1 Per-command guarantees

- `verify` exits **non-zero on integrity failure** — it is the CI gate.
- `status` **always exits 0**, including when files are missing or corrupt. It
  reports; it does not judge.

The sentinel error set (`errs/errors.go`) maps to codes as above.
`ErrMissingCachedFile` and `ErrMissingRemoteFile` are **not** in the exit-3 set
and fall through to exit 2 — missing is not corrupt.

### 9.2 The mapping is frozen; the trigger set is not

**The code→meaning mapping above is contract. The exact set of conditions that
produces each code is not.**

This distinction matters because v1 currently exits `0` in situations it should
not. `verify --check-remote` without `--integrity` records "found" from a listing
without comparing the size it just fetched against anything, so a **zero-byte or
truncated remote object passes `verify`** (`verify.go:268-280`). That is a false
green in the CI-facing command — the place a false green is most expensive.

v2 correcting this will produce exit `3` where v1 produced `0`. That is a **bug
fix, not a contract break.** The differential harness must therefore compare
exit codes against the *specified* meaning, not against v1's observed output, and
every intentional divergence of this kind belongs in §13.

The same reasoning applies to `verify` flagging every regular file in a scanned
tree as "unconverted" (`verify.go:134-141`) — pointing it at a subtree containing
a README fails the run. A check that cries wolf gets suppressed in CI, and then
it protects nothing. v2 may narrow this.

---

## 10. JSON output schemas

Frozen: field names, types, nullability, and nesting. Key order and indentation
are not (current output uses two-space indent).

### 10.1 `status --json`

```jsonc
{
  "tracked":        int,      // number of tracked links
  "unique_files":   int,      // distinct hashes
  "cached":         int,
  "missing_local":  int,      // unique_files - cached
  "total_size":     int64,
  "remote_checked": bool,
  "on_remote":      int,      // present ONLY when remote_checked
  "unpushed":       int,      // present ONLY when remote_checked
  "files": [
    {
      "path":   string,
      "hash":   string,
      "size":   int64,
      "cached": bool,
      "remote": bool         // present ONLY when remote was checked
    }
  ]
}
```

(`status.go:122-159`)

`on_remote`, `unpushed`, and per-file `remote` are `omitempty` pointers: absent
when no remote check ran, present otherwise. Implementations MUST distinguish
*absent* from *false* — an unchecked remote is not an empty remote. This is the
`Option<bool>` distinction and must not collapse.

`files` is always present, and is an empty array rather than null when nothing
is tracked.

#### Unresolved tension: the schema cannot express "unknown"

`remote` is `Option<bool>` — absent means *not checked*, present means
*checked, and this is the answer*. There is **no representation for "checked, but
could not determine."**

This collides with a real v1 defect. `HasFile`, `CheckFile`, and `FileSize`
(`command.go:169-178`, `195-200`, `506-515`) return *absent* on **any** rclone
error — expired credentials, a 403, DNS failure, a rate limit all render as
"missing remote file." A user seeing that will re-push, or conclude the remote
lost their data. Under `CLAUDE.md`'s own rule, absence must carry its reason, and
"I could not determine" must be a distinct outcome from "it is absent."

v2 will model this correctly internally (`Present | Absent{reason} |
Unknown{cause}`), but the frozen JSON shape has nowhere to put the third state.
Options, in preference order:

1. **Add a nullable `remote_error` / `remote_unknown` field.** Additive, so
   existing consumers reading `remote` are unaffected. Recommended.
2. Emit `remote: false` and report the cause on stderr. Preserves the schema
   exactly but perpetuates the lie in machine-readable output.
3. Fail the command outright on an undeterminable remote. Safest, most
   disruptive to existing scripts.

**Decision required before Phase 4.** Recorded in §13.

### 10.2 `remotes --json`

```jsonc
{
  "remotes": [
    {
      "name":    string,
      "backend": string,
      "path":    string,   // omitted when empty
      "config":  string,   // omitted when empty
      "default": bool      // true for the remote named "default"
    }
  ]
}
```

(`remotes.go:18-24`, `remotes.go:67-70`)

`remotes` reads only committed config and MUST NOT contact a backend.

---

## 11. Release artifacts

Frozen (`build-release.sh`, `scripts/install.sh`, `internal/cli/self.go`):

```
git-sfs-<version>-<os>-<arch>.tar.gz    containing a single executable: git-sfs
SHA256SUMS                              sha256sum format, one line per archive
```

- `<os>` ∈ `linux`, `darwin`; `<arch>` ∈ `amd64`, `arm64` — **Go's naming, not
  Rust's target triples.** `x86_64-unknown-linux-musl` must be published as
  `linux-amd64`.
- Binaries must be **statically linked** on Linux; the installer assumes a
  downloaded tarball runs anywhere.
- SHA-256 of every downloaded artifact is verified before install.

**Installed v1.21.0 binaries will self-update into the first v2 release.** They
resolve these exact names and verify against this exact `SHA256SUMS` format. Any
deviation breaks self-update permanently for every existing user — they would
have to reinstall by hand, and there is no way to push a correction to a binary
that can no longer update itself.

`--version` output must remain parseable as the tag (`v1.21.0` form); the
workflow suite asserts on it (`test/workflows/lib/install.sh:39`).

---

## 12. Explicitly NOT frozen

Under contract-only parity these are free to change:

- All human-readable stdout/stderr wording, including error text after the
  `git-sfs: ` prefix.
- Progress rendering, spinners, bar format, TTY behavior.
- `--verbose` / debug output entirely.
- Log ordering and interleaving under parallelism.
- Internal module structure, type design, concurrency strategy.
- The `owner` file's internal text format.
- JSON key ordering and whitespace.

**Highest drift risk:** `doctor` and `status` text output are nearly all
human-readable, so they are almost unconstrained here. Their intended behavior
should be designed deliberately in the rewrite rather than discovered by
accident.

---

## 13. Known v1 defects — do not reproduce

Sourced from [failure-modes.md](failure-modes.md), which should be read in full
before Phase 2. These are **policy** under §0: v2 is free to correct them, and
correcting them does not break interoperability. They are listed here so the
differential harness treats the divergence as *expected* rather than as a
regression.

### 13.1 Ordering — destroys old state before new state exists

| Defect | Source | v2 requirement |
|---|---|---|
| `add` does `os.Remove(file)` then `os.Symlink(...)`. A crash between leaves the path empty with nothing recording which hash belonged there | `add.go:88-93` | Symlink to a temp name, then `rename` over the original |
| `import --move` renames sources away during the parallel prepare phase; symlinks are created afterward | `cache.go:130` | Publish destination before removing source |
| `import --move` unlinks a source that was never copied when the hash already exists, trusting §4.1's mode bit | `cache.go:144-149` | Verify before unlinking the user's only copy |
| `mv` on a directory renames the tree, *then* rewrites symlink targets in a loop; interrupt leaves some links dangling | `mv.go:96-115` | Make the operation resumable or atomic |
| `Mv` takes no `context.Context` — the only uncancelable core operation, contradicting the project's own cancellation requirement | `mv.go` | Cancelable like every other operation |

**Rule:** publish the replacement, then remove the original. Never the reverse.

### 13.2 Durability

- `AtomicCopy` fsyncs the file but **never the parent directory**
  (`fsutil.go:74-81`). On power loss the rename can be lost even though the data
  was durable. "Atomic" is not "durable" — v2 must fsync the parent.
- Temp files are created as `.git-sfs-tmp-*` **inside the object store**
  (`fsutil.go:52`), i.e. within `files/sha256/<prefix>/`. `defer os.Remove` does
  not run on SIGKILL and `PurgeTmp` only cleans `tmp/`, so these accumulate
  inside the content-addressed tree. v2 should stage in `tmp/` — this is
  compatible, since v1 never reads another process's temp files.

### 13.3 Reporting untruths

- Remote errors collapsing into "not found" — see §10.1.
- `verify --check-remote` accepting a truncated remote object — see §9.2.
- Disk-space guard fails open twice: hashes missing from a listing contribute 0
  bytes, and a `statfs` failure warns and proceeds (`pull.go:110-117`). Combined
  with errors-look-like-absent, an unreachable remote silently skips the space
  check and then fills the disk.
- Error classification by English substring: `isRemotePathNotFound`
  (`command.go:106-117`) greps rclone's message text for `"directory not found"`
  and bails if it contains `"config"`. Breaks on an rclone wording change, a
  localized message, or a path containing the word "config." v2 should classify
  on exit codes and structured output.
- `verify` prints `run git-sfs gc to reclaim` (`verify.go:343`) — **`git-sfs gc`
  does not exist.** Human output is free under contract-only parity, so v2 simply
  must not advertise a command it does not ship.

### 13.4 Trusting the mover

- **Push verifies nothing after upload** (`push.go:40-50`). The remote copy —
  which exists precisely so the cache is not the only copy — is the only artifact
  never hash-verified on write. v2 should confirm what landed, at minimum by size.
- `--checksum` degrades silently to size+modtime on backends exposing no hash
  (`command.go:243-248`), so a same-size corrupt remote object is never detected
  without `--integrity`.
- `retryLoop` (`command.go:377-405`) retries permanent failures — bad
  credentials, missing path, permission denied — turning a clear immediate error
  into a slow one. v2 should retry only transient classes.

### 13.5 Unverified environmental assumptions

Nothing in v1 checks these, and each silently changes what the repo means:

- **`core.symlinks=false`** — Git checks out symlinks as regular text files
  containing the target path. `git-sfs add` in such a clone would hash and store
  those pointer texts as if they were the dataset. v2 must check this.
- Cache root on ephemeral storage (`/tmp`, ramdisk, container layer, auto-cleaned
  scratch). `add` + commit + reboot = dataset gone, repo full of dangling
  symlinks.
- Whether the cache filesystem preserves permission bits (§4.1).
- Available free space.

`doctor` currently verifies repo, config, versions, cache writability, rclone,
and connectivity — none of the four above. v2's `doctor` should cover them.

### 13.6 Latent

`IsInside` is wrong for single-character names: `len(rel) >= 2 && rel[:2] != ".."`
(`fsutil.go:110-113`) reports `<root>/a` as outside its root. Currently unused
outside its own test — a loaded gun for the first caller reaching for a
containment check. v2 must not port the bug along with the function.

---

## 14. Open divergences requiring a decision

| # | Issue | Recommendation |
|---|---|---|
| 1 | `#`-in-value parsing (§6.3) | Adopt `toml`; reject values containing `#` with a clear error |
| 2 | Duplicate keys: v1 last-wins, TOML errors | Accept the stricter TOML behavior; document it |
| 3 | v1 accepts unterminated/mismatched quotes via `Trim` | Accept stricter behavior |
| 4 | `min_git_sfs_version` vs. a `2.x` binary | `2.0.0 > 1.x` passes the existing comparison; no change needed. But v2 MUST NOT write a `min_git_sfs_version` of `2.x` into new configs by default, or v1 binaries are locked out of repos they could otherwise read |
| 5 | `status --json` cannot express "remote unknown" (§10.1) | Add a nullable `remote_error` field — additive, existing consumers unaffected. **Blocks Phase 4** |
| 6 | `verify` correctness fixes change exit `0` → `3` (§9.2) | Accept as bug fix; enumerate each case so the harness expects it |
| 7 | ~~Orphan reaping advertised but unimplemented~~ | **Resolved:** v2 ships `trash` (move, recoverable), never `gc` (unlink). Design in rust-rewrite-plan §7; layout in §4 |
| 8 | `countOrphans` derives "unreferenced" from a single repo while the cache serves many (`verify.go:315-331`) | **Open.** v2 requires explicit `--repo` scoping for unreferenced-object reclamation and defaults `trash` to the remote-replicated class instead. Whether to additionally record repo backlinks in the cache is undecided — see below |

Item 4 is subtle and worth restating: writing a v2-minimum into a freshly
initialized config would make the repo unreadable to v1 for no benefit. Leave
the field commented out, as v1 does.

### On item 8 — repo backlinks

`.git-sfs/cache` points repo → cache, and symlinks cannot be reversed. The cache
therefore cannot enumerate the repos it serves, which is why single-repo orphan
detection is unsound.

A tempting fix is a backlink directory — `BindCache` also writing
`<cache>/repos/<id> -> <repo>`. It is implemented purely as symlinks, so it
arguably respects the "no manifests, no hidden metadata" rule the way
`.git-sfs/cache` does in the other direction.

**Recommended against, for now.** It is only populated by v2-era binds, so during
migration it is *confidently incomplete* — worse than obviously absent, because
reclamation logic would trust it. It also introduces cache state that must be
kept consistent as repos are moved or deleted, which is the class of drift the
project avoids on principle.

Preferred instead: default reclamation to the remote-replicated class, where
soundness does not depend on knowing the repo set at all.

---

## 15. Verification

This document is the input to the differential test harness. Every clause should
map to at least one executable assertion:

- **Filesystem state** — symlink targets, cache paths, file modes, content
  hashes after each scenario. Strict.
- **Exit codes** — per scenario, including error paths.
- **JSON output** — snapshot-tested; frozen shapes make this safe.
- **Cross-binary** — lock contention and cache interop between v1 and v2.

The existing shell workflow suite (`test/workflows/`) already asserts at this
level: file contents, `assert_symlink` targets, and exit status. Exactly one
assertion inspects human output (`scenarios.sh:215`,
`"corrupt cache files: 1"`) and must be loosened to a structural check.

Clauses not covered by an assertion are aspirational, not contractual.
