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
| Exit codes | **Only `0` vs non-zero** — CI depends on it. Taxonomy is free (§9) |
| `--json` shapes | **Unfrozen** — no external consumers (§10.1) |
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

**The comparison is between canonicalized paths, not between strings.**
`BindCache` resolves both sides through `canonicalPath` — `EvalSymlinks`, falling
back to `Clean` — and joins a relative existing target against the link's own
directory first (`localstate.go:74-82`). Re-binding a cache to where it already
points is therefore a no-op returning success, not an error.

A literal string comparison rejects the common case rather than the dangerous
one. Any symlinked path component makes two spellings name one directory — a
home directory relocated onto another volume, a mounted network share, macOS's
`/var` → `/private/var` — and each produces a spurious mismatch. An
implementation that compares literally makes `setup` fail on correctly-configured
repos, and the natural user response is to delete the link and relink by hand:
precisely the operation this clause exists to prevent.

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

### 3.3 Links are the unit of operation, not the objects behind them

`mv` parses and rewrites symlink entries and MUST NOT require the referenced
cache object to exist — nothing on its path stats the target (`mv.go:36-38`).
Moving a tracked path whose object is absent succeeds and produces a correctly
retargeted dangling link.

This is a recovery path, not an edge case. §13.4b's default puts the cache at
`.git-sfs/.cache`, where `git clean -x` deletes it; a fresh clone before any
`pull` is in the same state. In both, *every* link in the tree dangles at once,
and wanting to reorganize the tree before restoring several hundred gigabytes is
reasonable. A v2 that resolves or validates the target before moving turns the
recovery path into a dead end at exactly the moment the user has no local copy.

Two rules from the same code path:

- A source that is not a valid git-sfs symlink is rejected (`mv.go:36-38`).
  `mv` never touches regular files; `git mv` is the tool for those.
- Destination semantics are POSIX: an existing **directory** destination means
  "move the source inside it" (`mv.go:40-43`), and an existing non-directory
  destination is an error rather than an overwrite (`mv.go:44-46`).

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

**`import --move` must survive a cross-filesystem source.** The move stages via
`os.Rename`, and on `EXDEV` falls back to copy-then-remove, publishing the
staging file at its final mode (`cache.go:158-190`). This is not an exotic path:
§13.4b directs v2 to default the cache *outside* the working tree, which makes
"source and cache on different filesystems" the common case rather than the rare
one — an import from an external drive, a scratch mount, or a network share. An
implementation that treats `rename` as infallible fails precisely the workflow
the recommended default creates.

### 4.3 `tmp/` is purged unlocked, by one command only

`pull` calls `PurgeTmp` — `RemoveAll(tmp/)` then recreate at `0o755` — as its
first cache operation (`pull.go:30`, `cache.go:51-56`). No other command purges,
and the purge happens **before** `pull` acquires its lock (`pull.go:33`).

Recorded because two facts stated elsewhere in this document combine into a
data-loss bug that does not exist in v1 and that v2 would introduce by following
v1's own advice:

- §13.2 tells v2 to stage temp files in `tmp/` instead of inside
  `files/sha256/<prefix>/`. Correct on its own terms.
- §8 establishes that commands take **different** locks, so an `add` and a `pull`
  run concurrently by design.

v1 is safe only by accident: it stages inside the object store, so the directory
`pull` wipes holds nothing but rclone's own scratch. Move staging into `tmp/`
while keeping the unlocked purge, and a concurrent `pull` deletes an in-flight
`add`'s staging file out from under it. During migration the two processes need
not even be the same version — a v1 `pull` will happily purge a v2 `add`'s
staging.

**v2 requirements:** purge under the same lock that guards writes to what is
being purged, and purge selectively — by age, or by owning pid — rather than
wiping a directory other live processes are writing into. Reclaiming abandoned
scratch is a convenience; destroying another process's in-flight write is not an
acceptable price for it.

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

### 5.1 URL composition

The composition rule is frozen for the same reason the layout is: two users
pushing into one bucket must land on identical paths (`command.go:41-61`).

1. If `backend` is empty, the URL is `path` unchanged.
2. Otherwise **trailing** `/` are stripped from `path`.
3. If the result begins with `/` **or** is a Windows absolute path (`D:/…`), the
   URL is `backend + ":" + path`.
4. Otherwise leading `/` are stripped too, then the URL is `backend + ":" + path`.
5. Trailing `/` are stripped from the composed URL (`command.go:52-54`).

Object paths are `<url>/files/sha256/<prefix>/<hash>` (`command.go:67-69`).

Each case below has a plausible wrong answer, which is why they are enumerated:

| `backend` | `path` | URL |
|---|---|---|
| `local` | `/srv/data` | `local:/srv/data` |
| `s3` | `dataset/root` | `s3:dataset/root` |
| `s3` | `/dataset/root/` | `s3:/dataset/root` |
| `s3` | `D:/data` | `s3:D:/data` |
| `s3` | *(empty)* | `s3:` |
| *(empty)* | `/abs/path` | `/abs/path` |

Skipping step 2 yields `s3:dataset/root//files/sha256/…`, and several backends
treat that as a distinct key from the single-slash form. The result is §5's
stated failure mode — a shared remote silently partitioned into two disjoint
stores — reached through one character.

---

## 5b. Operation scope — which files a command acts on

`status`, `verify`, `push`, and `pull` each take an optional path argument
scoping the operation to a subtree; `.` means the whole repository. All four
route through one walk (`collectGitSFSSymlinks(repo, path)`, `walk.go:18`) and so
share selection semantics: every valid git-sfs symlink at or below the path,
deduplicated by hash for operations that act on objects rather than links.

This is contract, not convenience. A partial checkout is the normal state of a
large dataset — users pull one subtree of a multi-terabyte repo — so a command
that could only act on the whole tree would be unusable against the working set
they actually have. `pull <path>` restores the selected subtree and MUST NOT
restore siblings; `verify <path>` and `status <path>` report only that subtree.

### 5b.1 `push` and missing cache objects

`push` without `--skip-missing` fails when any selected symlink's object is not
present-and-valid locally, and the error names a **working-tree path** rather
than a hash (`push.go:59-60`, `push.go:120-125`). The path comes from a
path-sorted link list, so the same repo state always names the same file.

Scoping is what makes that failure recoverable. A partially-pulled dataset would
otherwise be unpushable, because a subtree the user never pulled blocks pushing
the subtree they did: `push want/` succeeds where `push .` correctly fails.

`--skip-missing` (added v1.21) trades completeness for progress — it uploads what
is cached, leaves the rest, and exits `0`. Its contract:

- Objects are admitted via `HasValid` (`push.go:76-88`), so **an object failing
  verification is treated as missing and is never uploaded.** Subject to §4.1's
  limit: `HasValid` trusts the read-only bit, so this catches rot only where the
  mode bit does not hide it. §13.4 is the unresolved half of the same problem.
- The omission is reported on **stderr**, where it survives piping and cannot be
  mistaken for success (`push.go:99-119`).
- The report counts **both** unique objects and the symlinks referencing them.
  The two differ whenever paths share content, and counting objects alone
  understates how much of the tree is unbacked.
- The per-path listing is capped (v1: 10, `push.go:90`) with an "and N more"
  line, so a heavily partial checkout cannot bury the result.

The dual count is the part worth preserving; the wording around it is free
under §12.

### 5b.2 `import` and symlinked sources

A symlink reached by `import` — whether it *is* the source, or is found while
walking a source directory — is an **error unless `-L` is given**
(`import.go:179-182`, `import.go:228-230`). With `-L` the link is resolved and
the resolved file is ingested; under `--move` the link and its target are both
removed, and a source directory left empty is removed too.

Refusing by default is the safe choice and must be kept. A symlink in an incoming
tree may point anywhere on the machine, and silently following one under `--move`
deletes a file outside the directory the user named. Under `import` without
`--move` it would still hash something other than what the tree appears to
contain.

The failure must be clean: rejecting a symlinked source leaves **both the link
and its target untouched** on disk. This is the general rule for the whole
command — `import` validates before it moves anything, so a rejected import is a
no-op rather than a partial ingest. §13.1 requires v2 to go further and publish
destinations before removing sources.

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

1. Parse with both the `toml` crate and the v1 line scanner, and error when the
   two readings differ. See §6.5 — this is the load-bearing one.
2. Keep the closed-schema validation of §6.2 — `serde`'s `deny_unknown_fields`
   plus explicit checks. Real TOML parsing must not silently widen what is
   accepted.
3. Cover the divergent cases in the differential test suite explicitly, asserting
   that `migrate` reproduces v1's reading rather than TOML's.

### 6.5 Version floor and legacy configs

Two separate populations need two separate mechanisms. They are complementary,
not alternatives.

**v2 parses every config with both parsers and compares.**

| Outcome | Action |
|---|---|
| Both succeed, identical result | Use it — the overwhelmingly common case |
| Both succeed, results differ | **Error.** Report the field and both readings |
| TOML fails, v1 succeeds | Use v1's reading — v1-only syntax |
| Both fail | Error |

The mechanism keys on **disagreement, not failure**, and that distinction is the
whole design.

A fallback chain — try TOML, fall back to v1 on error — does not work, because
the dangerous case is not an error. Given `path = "run#1"`, TOML yields `run#1`
and v1 yields `run`; **both succeed.** A fallback would silently take TOML's
answer, address a location that has never existed, and present as a vanished
dataset. The failure mode of a fallback is precisely the scenario the rule exists
to prevent.

Comparison catches it because it does not ask "did parsing work?" but "do the two
readings agree?" — which is the actual question.

On disagreement, error rather than silently preferring v1. The config is
genuinely ambiguous and the tool cannot know which reading was intended.
Reporting both (`run` vs `run#1`) lets the user resolve it in a single edit.
Silently preferring v1 would preserve behavior but leave a file whose plain text
means something different from what the tool does with it — a trap for the next
human or program to read it.

#### The error must carry the fix

The message is doing the real work, so it is specified rather than left to the
implementer:

```
error: .git-sfs/config.toml: remotes.default.path is ambiguous

  as written:          "datasets/run#1"
  git-sfs 1.x read:     datasets/run      ← your objects are here
  strict TOML reads:    datasets/run#1

  Change the line to make it unambiguous:
      path = "datasets/run"
```

The `your objects are here` line is essential. Without it the user must guess
which reading is correct, and the two look equally plausible in isolation.

**Canonicalize by truncation, not deletion.** v1 truncates at the `#` and
discards the remainder, so `run#1` becomes `run`. Deleting only the `#` character
would yield `run1` — a third value matching neither parser and addressing a third
nonexistent location. The suggested replacement MUST reproduce v1's reading
exactly.

#### git-sfs MUST NOT apply the fix itself

`config.toml` is committed and shared. Rewriting it on read would silently dirty
a tracked file during an unrelated command such as `status`, and a user who
commits that change propagates it to the whole team. It also violates the
project rule that git-sfs never modifies a file the user did not explicitly hand
to it.

There is a second reason. Disagreement arises only when `#` appears **inside**
the quotes — a genuine trailing comment (`path = "data"  # prod bucket`) parses
identically under both readings and never triggers this path. So the user typed
`#` within a quoted string, which under TOML semantics means they intended it in
the value. Their *intent* was `run#1`; the *effect* under v1 was `run`.

v2 canonicalizes to the effect, because that is where the bytes physically are.
But that is an inference about a file the user authored: if they genuinely wanted
`run#1` and had been investigating why pushes seemed to disappear, silently
rewriting the value to `run` would cement the bug and destroy the evidence.
Surface the ambiguity; let the human resolve it.

**No fix command either.** The obvious next step — a `git-sfs config fix` that
applies the correction with the user's consent — is explicitly rejected. It would
not violate the modification rule, but it fires only on a rare malformed value
and exists solely to save a single line-edit. That does not earn a place on the
CLI surface, and the surface is the thing hardest to shrink later. The error
message already contains the exact replacement line; copying it is the whole
remedy.

**Consequences:**

- **No forced version floor.** v1 and v2 coexist on unambiguous configs, which is
  effectively all of them. No partial-adoption partition. (§14 item 4 therefore
  stands as originally written: leave `min_git_sfs_version` commented out.)
- **No `migrate` command.** Nothing to add to the CLI surface.
- **Zero friction** for configs without `#`, escape sequences, or irregular
  quoting.
- v2 retains the v1 line scanner in the normal read path, not quarantined. Cost
  is double-parsing a file of a few hundred bytes — negligible.
- v2's *written* configs stay within the v1-parseable subset. The schema is all
  simple `key = "value"` scalars, so this costs nothing and keeps v1 able to read
  what v2 writes.

### 6.4 Defaulting

- Missing `algorithm` defaults to `"sha256"` **after** parsing, then is validated.
- `Default()` (`config.go:108-116`) produces the in-memory default; `init` writes
  the annotated template at `config.go:125-151` with mode `0o644`.
- **The template MUST parse under the implementation's own validator** and, under
  §6.5, identically under both parsers. It carries quoted values and `#` comments
  — exactly the construct §6.3 shows the two parsers disagreeing about — so a
  template that trips its own ambiguity check would make `init` produce a repo no
  command can open.

### 6.6 Version strings are not semver

`min_git_sfs_version` and `min_rclone_version` are compared with a hand-rolled
parser (`config.go:18-33`) that is **not** semver, and every difference is
load-bearing:

- An optional leading `v` is stripped, so `v1.67.0` parses.
- Exactly three `.`-separated components are required (`SplitN(s, ".", 3)`), each
  read with `strconv.Atoi`. `1.60` is an error.
- Leading zeros are accepted — `1.07.0` reads as `1.7.0`.
- Prerelease and build metadata are **rejected**: `Atoi("0-beta")` fails, so
  `1.67.0-beta` is an error.

Comparison is lexicographic over the resulting `[3]int`, and `detected >= minimum`
passes (`config.go:60-78`).

**The `semver` crate named in rust-rewrite-plan §4.1 inverts all three of the
first rules.** `semver::Version::parse` rejects a leading `v`, rejects leading
zeros, and accepts prerelease. Adopting it unmodified is not a refactor — it
changes which committed configs load.

The leading `v` is the sharp edge, and it is not hypothetical.
`CheckGitSFSVersion` is called with git-sfs's own `version.Version`
(`app.go:57`, `doctor.go:60`), and §11 pins that to the tag form: `v1.21.0`. A v2
parsing it with bare `semver` fails to parse *its own version* and errors on
every repo that sets `min_git_sfs_version`. The rclone path is safer only by
luck — `DetectRcloneVersion` strips the `rclone v` prefix before returning
(`command.go:467-471`), so there only the config-supplied minimum carries a `v`.

v2 may be **more** permissive: accepting `1.67.0-beta`, which v1 rejects, only
widens the set of configs that load and cannot break a repo that works today. It
MUST NOT be less permissive on the forms v1 accepts.

---

## 7. Local state resolution

### 7.1 Repository discovery

Walk upward from the current working directory until a `.git` entry exists.
`.git` may be a **directory** (normal repo) or a **file** (submodule or
worktree) — both are accepted, via `os.Stat` rather than a directory check
(`localstate.go:16-32`). Stop at filesystem root and error.

### 7.2 Cache resolution

Normal commands resolve exactly one cache source:

1. `.git-sfs/cache` symlink target
2. Otherwise `ErrMissingCacheConfig` → **exit 1**

`init` and `setup` are the only commands that choose or bind that symlink.
`--cache PATH` is a binding option for those commands only, not a hidden
per-command override. `GIT_SFS_CACHE` is not part of the v2 cache model.

For new local state, the default cache root is `<git-dir>/sfs/cache`
(`.git/sfs/cache` in a normal repository). Existing repos keep their existing
`.git-sfs/cache` binding. If that symlink is missing but the old v1 default
`.git-sfs/.cache` exists, `setup` binds to the old cache rather than migrating
or probing cache contents.

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

`<name>` is not a placeholder for a single well-known string: there are **five**
locks, one per command, and which one a process takes is itself part of the
contract. See §8.2 — it is the clause most likely to be broken by an improvement.

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

  **Both predictions are now confirmed against the v1 binary**, not merely read
  out of the source, by `test/differential/lock_contention.py`: a zero-byte
  `owner` yields `panic: runtime error: slice bounds out of range [:-1]`, and a
  lock whose recorded pid is not running blocks indefinitely. The harness records
  these as observations rather than assertions, since v2 must diverge on both.

Liveness checking is strictly more conservative than v1's behavior and cannot
cause a v1 process's lock to be broken while that process is alive.

**Known limitation, unchanged:** locks are per-cache and offer no mutual
exclusion between hosts sharing a network cache. v2 does not fix this; it should
not pretend otherwise.

### 8.2 There is no single lock — there are five, and the names are frozen

Each command takes a lock named after itself:

| Command | Lock directory | Source |
|---|---|---|
| `add` | `locks/add.lock` | `add.go:40` |
| `import` | `locks/import.lock` | `import.go:59` |
| `setup` | `locks/setup.lock` | `init.go:90` |
| `pull` | `locks/pull.lock` | `pull.go:33` |
| `push` | `locks/push.lock` | `push.go:42` |

Two consequences follow, and they pull in opposite directions.

**First: different commands do not exclude each other.** A concurrent `add` and
`pull` against one cache take different directories and both proceed. Only two
instances of the *same* command serialize. Whether that is adequate is a design
question v2 should answer deliberately — §4.3 describes one place it is already
not adequate — but it is the behavior today, and it is what a v1 process running
beside a v2 process will assume.

**Second, and this is the trap: consolidating to a single lock silently removes
all cross-version mutual exclusion.** One `cache.lock` guarding every mutating
command is the obvious cleanup. It is *more* correct in isolation. It is also
exactly the failure §8 exists to prevent: v2's `add` would take `cache.lock`
while v1's `add` takes `add.lock`, the `mkdir` calls target different paths,
neither blocks, and two processes write the same cache concurrently — with both
binaries reporting that they hold the lock.

The nature of the mistake is what makes it dangerous. It arrives disguised as an
improvement, it passes every single-binary test, and it is invisible until a
migrating user runs both versions at once — which §8 already establishes is the
expected case, not an unlucky one.

**v2 requirement:** these five names are mechanism and are frozen. v2 may take
*more* locks than v1 for a given command — acquiring `add.lock` **and** a broader
lock is strictly more conservative and cannot break v1 interop — but it MUST NOT
rename them, and MUST NOT drop v1's name for a command in favor of a different
one. Strengthening exclusion means adding to the set, never substituting for it.

Ordering matters once a command takes more than one: acquire in a fixed global
order across all commands, or two v2 processes deadlock against each other while
correctly excluding v1.

---

## 9. Exit codes

**Mostly unfrozen.** git-sfs has only ever been consumed as a user-facing CLI and
in CI — no library bindings, no third-party integrations branching on specific
codes — so the *taxonomy* below is v2's to redesign.

Two things remain frozen, and neither is really git-sfs's contract:

- **`0` versus non-zero.** This is the Unix contract and CI depends on it
  absolutely. `verify` returning non-zero on failure is the entire purpose of the
  CI gate (§9.1).
- **`130` for SIGINT.** The `128 + signal` convention belongs to the shell, not to
  this tool. Keep it because breaking it surprises every user, not because a
  git-sfs consumer requires it.

Everything else — whether config errors are `1`, whether integrity gets its own
code, how many codes exist at all — is free. v2 should design the taxonomy it
wants rather than inheriting this one.

The v1 mapping, for reference (`main.go:15-42`):

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

**`status` without `--remote` makes no network calls at all.** The remote is
contacted only when a remote name is supplied: `checkRemote := remoteName != ""`
gates remote selection, preflight, and every per-file query (`status.go:53-71`).
This is the difference between a command usable on a plane and one that hangs on
a dead VPN, and it pairs with §10.2's rule for `remotes`. A v2 that resolves or
preflights the remote eagerly — to report it uniformly, say — breaks it without
changing a single line of output.

**What `verify` counts as a failure.** Eight issue kinds are enumerated
(`verify.go:42-51`): unconverted file, broken git symlink, missing cache file,
corrupt cache file, wrong cache permissions, missing remote file, corrupt remote
file, invalid config. Any issue makes the run non-zero; the sentinel is chosen by
severity, with corrupt-cache and **wrong-permissions** both mapping to
`ErrCorruptCachedFile` (`verify.go:100-111`).

Orphaned cache objects are deliberately *not* an issue kind. They are counted
separately and printed as a comment, and a repo whose only finding is orphans
**exits 0** (`verify.go:85-89`, `verify.go:341-344`). This is correct and must be
preserved: an orphan is unreferenced storage, not damage, and §14 item 8 already
establishes that v1 cannot even compute the property reliably. Failing CI on it
would be a false red in the CI-facing command — §9.2's exact failure mode.

**Declared divergence on wrong permissions.** §4.1 permits v2 to treat a writable
cache object as unverified, hash-verify it, and protect it in place. Where those
bytes are intact, v2 therefore repairs and exits **0** on a repo where v1 exits
**3**. That is the intended consequence of §4.1, not a regression, and the
harness must assert the new result positively rather than diffing against v1.

### 9.2 Where v1 exits `0` and should not

Since the taxonomy is unfrozen, these are simply bugs to fix rather than
divergences to negotiate — but they are listed because they change the one thing
still frozen: whether the command succeeds.

`verify --check-remote` without `--integrity` records "found" from a listing
without comparing the size it just fetched against anything, so a **zero-byte or
truncated remote object passes `verify`** (`verify.go:268-280`). A false green in
the CI-facing command, which is where a false green costs the most.

v2 will exit non-zero there. The differential harness must therefore compare
against the *specified* behavior rather than v1's observed output, and assert the
new result positively.

Separately, `verify` flags every regular file in a scanned tree as "unconverted"
(`verify.go:134-141`), so pointing it at a subtree containing a README fails the
run. A check that cries wolf gets `|| true`'d in CI, and then it protects
nothing. v2 should narrow this — a false red is a slower way to reach the same
place as a false green.

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

#### Resolved: the schema is unfrozen

`status --json` has no external consumers beyond this project's own use, so v2
redesigns it rather than working within v1's shape.

This matters because v1's schema cannot express a state the tool genuinely
reaches. `remote` is `Option<bool>` — absent means *not checked*, present means
*checked, and here is the answer*. There is no representation for **"checked, but
could not determine."**

That is not hypothetical. `HasFile`, `CheckFile`, and `FileSize`
(`command.go:169-178`, `195-200`, `506-515`) return *absent* on **any** rclone
error: expired credentials, a 403, DNS failure, and a rate limit all render as
"missing remote file." A user seeing that will re-push, or conclude the remote
lost their data. The schema forced the lie, and then the lie became
machine-readable.

v2 models the three states explicitly — `present`, `absent` with a reason, and
`unknown` with a cause — and the JSON carries all three. This is `CLAUDE.md`'s
absence rule applied end to end: the reason travels to where the gap surfaces
instead of collapsing into a bare boolean at the output boundary.

The v1 shape above remains documented as reference for reading old output. It is
not a constraint on v2.

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
- **The exit-code taxonomy** — every code except `0`/non-zero and `130` (§9).
- **The `status --json` schema** (§10.1).
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
- **A freshly initialized repo cannot run `verify` at all in v1.** Two defaults
  combine: `verify`'s remote check defaults on, and the v1 `init` template
  writes `config = "rclone.conf"`, which resolves to `.git-sfs/rclone.conf` — a
  file `init` never creates. v2's template omits that local config path; rclone
  uses its normal user-level config unless the project explicitly declares
  otherwise.

  A false red in the CI-facing command, and §9.2 already establishes where that
  leads: a check that cries wolf gets `|| true`'d, and then it protects nothing.
  v2 should not have a fresh repo fail its own verify — either default the remote
  check off, or treat an unconfigured remote as "not checked" rather than as an
  error.

  Found by the differential harness, not by the shell suite, which never
  exercises the default config — every scenario writes a working remote config
  first (`test/workflows/lib/repo.sh:write_local_remote_config`). Worth recording
  as a class: **defaults go untested when every fixture overrides them**, and
  both halves of this defect are defaults.

### 13.4 Trusting the mover

- **Push replicates local rot over a good remote copy, and exits `0`.** `push`
  admits an object on `HasValid` alone (`push.go`), which for a read-only file is
  the §4.1 mode bit and nothing else — no bytes are read. `CopyToRemote` omits
  `--ignore-existing` (`command.go:272` uses it only on the pull direction), so
  the upload **overwrites**. A locally rotted object therefore destroys the one
  good replica, silently.

  This is the replication-as-repair-source model (rust-rewrite-plan §8) running
  backwards: the tier that exists to repair the other is overwritten *by* the
  damaged one. Rot that was recoverable a moment earlier becomes unrecoverable,
  and nothing reports it — verified end to end against v1, remote hash before and
  after, exit `0`.

  It compounds with the two entries below: nothing verifies the upload
  afterwards, and `verify --check-remote` then accepts the result (§9.2). v2 must
  not push bytes it has not read, and must not overwrite a remote object with
  content that does not match the hash naming it.
- **Push verifies nothing after upload** (`push.go:40-50`). The remote copy —
  which exists precisely so the cache is not the only copy — is the only artifact
  never hash-verified on write. v2 should confirm what landed, at minimum by size.
- `--checksum` degrades silently to size+modtime on backends exposing no hash
  (`command.go:243-248`), so a same-size corrupt remote object is never detected
  without `--integrity`.
- `retryLoop` (`command.go:377-405`) retries permanent failures — bad
  credentials, missing path, permission denied — turning a clear immediate error
  into a slow one. v2 should retry only transient classes.
- **`FileSizes` is O(entire remote)** (`command.go:521-545`). It runs
  `lsjson --recursive` across the whole remote and unmarshals the complete
  listing into a slice, then filters for the hashes it wanted. A million-object
  remote materializes roughly 150 MB of JSON twice to answer a question that may
  concern a single hash — and it is reached from `status --remote` and
  `verify --check-remote`, both casual commands. v2 should query the specific
  paths it needs, or stream-parse rather than materialize.
- **A full system-wide `/tmp` can silently break `pull`, and always affects
  `push`'s own staging.** `writeTempPathList` (`command.go:210-224`) writes the
  `--files-from` list "in `r.tempDir` (or the OS temp dir when unset)" — its own
  doc comment. `CopyFromRemote` (`command.go:261-270`) knows routing rclone's own
  `--temp-dir` through the cache keeps download staging on the same filesystem
  as the final cache files "so the final rename is atomic" — but an empty
  `tempDir` only warns to stderr and proceeds anyway, never a hard error.
  `CopyToRemote` (`command.go:234-248`), push's upload path, does not attempt to
  set `--temp-dir` for rclone at all. **Confirmed against a real incident:** a
  full system-wide `/tmp` on a shared cluster took git-sfs down even though the
  cache itself, on a separate filesystem, had room. v2 must never stage a write
  — its own, or one it delegates to rclone via `--temp-dir` — outside the
  cache's own `tmp/`, and an unconfigured temp location must be a hard error,
  not a warning a script can miss.

### 13.4b Defaults that place data in harm's way

From failure-modes §1b–§1d. These are **defaults**, not mechanism, so v2 may
change them freely.

**Decision: fixed in v2 only; not backported to v1.** Accepted risk, recorded
because two decisions interact. v2 ships staged (plan §Phase 7) — `install.sh`
keeps serving v1 until a flag day, with v2 opt-in — so users who do not opt in
retain the `git clean -x` exposure, which is silent and unrecoverable for
unpushed objects. The population still at risk is precisely the one that will not
self-select into the fix. Revisit if the flag day slips.

- **Default cache inside the repo.** v1's `.git-sfs/.cache` default is
  gitignored, and `git clean -x` removes ignored files. v2 defaults new local
  cache state to `<git-dir>/sfs/cache`, outside the working tree, while
  preserving existing `.git-sfs/cache` and old `.git-sfs/.cache` bindings.
- **rclone config not gitignored.** v2's default template does not write a local
  rclone config path into committed `.git-sfs/config.toml`; local config belongs
  under `.git/sfs/` or rclone's own user-level config.
- **Moves outside `git-sfs mv` dangle symlinks.** Targets are relative to the
  file's directory (`sfspath.go:20-22`), so `git mv` or shell `mv` across depths
  silently invalidates them, undetected until `verify`. Same for branch switches
  and historical checkouts against a cache lacking those hashes.

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

### 13.7 `add` doesn't check whether a candidate is already tracked by Git

`add`'s walk (`add.go:46-65`) converts every regular file it finds under the
given paths — `shouldSkip` (`walk.go:58-65`) only excludes `.git`, `.git-sfs`,
and the top-level `.gitignore`. Nothing asks Git whether a candidate is already
in its index, so `git-sfs add .`, or any glob that sweeps up a committed source
file or README alongside real data, silently deletes that file's content and
replaces it with a git-sfs symlink — see failure-modes §1e. v2 must check a
candidate against Git's index (e.g. `git ls-files`) and refuse to convert a
file that is already tracked, the same way `import` refuses a symlinked source
rather than silently ingesting the wrong thing (§5b.2).

---

## 14. Open divergences requiring a decision

| # | Issue | Recommendation |
|---|---|---|
| 1 | ~~`#`-in-value parsing~~ | **Resolved (§6.5):** parse with both `toml` and the v1 scanner; error only when the readings differ. No version floor, no migrate command, no friction on unambiguous configs |
| 2 | ~~Duplicate keys: v1 last-wins, TOML errors~~ | **Resolved:** accept TOML's error. Silent last-wins is the same class of hazard as §6.5 — a config meaning something other than it appears to |
| 3 | ~~v1 accepts unterminated/mismatched quotes~~ | **Resolved:** §6.5 already covers it — TOML fails, v1 succeeds, so v1's reading is used. No competing interpretation exists, therefore no ambiguity and no risk |
| 4 | `min_git_sfs_version` vs. a `2.x` binary | `2.0.0 > 1.x` passes the existing comparison; no change needed. v2 MUST NOT write a `2.x` floor into new configs — §6.5's comparison makes it unnecessary, and setting it would lock v1 out of repos it can read correctly. Leave the field commented out, as v1 does |
| 5 | ~~`status --json` cannot express "remote unknown"~~ | **Resolved:** schema unfrozen (§10.1). v2 carries `present` / `absent{reason}` / `unknown{cause}` directly |
| 6 | ~~`verify` fixes change exit codes~~ | **Resolved:** taxonomy unfrozen (§9). Only `0` vs non-zero is contract, and v2 correctly returns non-zero |
| 7 | ~~Orphan reaping advertised but unimplemented~~ | **Resolved:** v2 ships `trash` (move, recoverable), never `gc` (unlink). Design in rust-rewrite-plan §7; layout in §4 |
| 8 | ~~`countOrphans` derives "unreferenced" from a single repo~~ | **Dissolved:** v2 ships only remote-replicated eviction (plan §7), so reclamation never asks "is this unreferenced." No `--repo` scoping, no backlinks, no history scan. `verify` may still *report* an orphan count, but nothing acts on it |

Item 4 is subtle and worth restating: writing a v2-minimum into a freshly
initialized config would make the repo unreadable to v1 for no benefit. The
comparison rule in §6.5 already guarantees no config is ever read two different
ways, so the floor buys nothing and costs every v1 colleague their access.

### On item 8 — why backlinks are not needed

`.git-sfs/cache` points repo → cache, and symlinks cannot be reversed, so the
cache cannot enumerate the repos it serves. A backlink directory
(`BindCache` also writing `<cache>/repos/<id> -> <repo>`) would fix that, and
being pure symlinks it arguably respects the no-manifests rule.

**Not needed, because nothing asks the question any more.** Restricting
reclamation to remote-replicated objects (plan §7) removes the only consumer of
repo-set knowledge. The criterion becomes "does a copy exist elsewhere," which is
directly checkable, rather than "does anything still reference this," which is not.

Had backlinks been adopted, they would also have been *confidently incomplete*
during migration — populated only by v2-era binds, yet trusted by reclamation
logic. Absent state is safer than state that is wrong in an unknown direction.

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
