# FAQ

Questions with uncomfortable answers.

[failure-modes.md](failure-modes.md) asks "where does git-sfs kill a dataset?" from the
design side. This asks the same thing from the user's side: you are about to do something
reasonable, and you want to know what actually happens. Every answer is what the code does
today, with the source to check it against — not what the design intends.

If an answer says "nothing detects this," that is not a wish list. That is the answer.

---

## Two users, one remote

### Two users push the same file at the same time, and the remote does not have it yet. What happens?

Both compute the same SHA-256, so both write to the **same remote path**
(`files/sha256/<prefix>/<hash>`). git-sfs does nothing to coordinate them: locks live under
the local cache root ([lock.go:19-54](../internal/lock/lock.go#L19-L54)), so two machines
have zero mutual exclusion. Both `rclone copy` calls run
([push.go:70](../internal/core/push.go#L70)).

What saves you is content addressing — both writers are uploading **identical bytes**, so
whoever wins, the result is correct.

What does *not* save you is the overlap window, and its danger depends entirely on the
backend:

- **Object stores (S3, GCS, Azure):** each upload is a single atomic PUT. Last writer wins,
  both wrote the same object, outcome is correct.
- **SFTP, WebDAV, local/NFS/SMB paths:** rclone writes to the destination path. Two
  concurrent writers to one path can interleave and leave a **mixed or truncated object**.
  Nothing in git-sfs prevents it and nothing detects it at push time.

Push adds `--checksum` and no `--ignore-existing`
([command.go:248](../internal/remote/command.go#L248)), so the second pusher's behavior
depends on timing: if the first upload already finished, the checksum matches and the
second is skipped; if it is midway, the partial object's checksum differs and the second
pusher re-uploads over it.

**Honest bottom line:** the outcome is benign on atomic backends and unguarded elsewhere.
Push never verifies what landed, so if the object *is* corrupt, you find out at the next
`verify --check-remote --with-integrity`, or when someone's `pull` rejects it by hash. The
pull-side rejection is the real safety net — no wrong bytes ever enter a cache — but it
fires long after the push reported success.

### Two users push *different* files at the same time. Safe?

Yes. Different content, different keys, no shared path. The only shared precondition is
that the remote root already exists, which preflight checks before any transfer
([app.go:144-158](../internal/core/app.go#L144-L158)).

### One of my cache objects rotted. I push. What happens to the good copy on the remote?

**It is overwritten with the rotted bytes, and push exits 0.**

Push admits an object on `HasValid` alone
([push.go:76-85](../internal/core/push.go#L76-L85)), and for a read-only file `HasValid` is
the mode bit and nothing else ([cache.go:63-81](../internal/cache/cache.go#L63-L81)) — no
re-hash. `CopyToRemote` then uses `--checksum` with no `--ignore-existing`, so a differing
checksum means *upload*, not *skip*.

The tier that exists to repair the other gets destroyed by the damaged one. This is
confirmed against the shipped binary by
[test/differential/mode_preservation.py](../test/differential/mode_preservation.py), not
merely inferred from the source. Note that the doc comment on
[push.go:23](../internal/core/push.go#L23) says "Existing remote files are never
overwritten" — that comment does not match the flags.

**Rule:** run `git-sfs verify --rehash` *before* a push you are treating as a backup, not
after.

### Two people add the same bytes at different paths — or in different repos sharing a cache. Who owns the object?

Nobody. There is one object, referenced by every symlink that hashes to it. That is the
dedup win, and it is also why there is no safe "delete the files for my project" operation:
another repo you have never heard of may reference the same object.

### Same content, but one of us marked it executable. Which mode survives?

**Whoever added it first.** `Store` returns early when the object already exists
([cache.go:116-118](../internal/cache/cache.go#L116-L118)), so the second add never writes
and never re-chmods. Git tracks the symlink, not the target's mode, so the executable bit
travels with the *cache object*, globally, first-writer-wins.

If your colleague added `train.sh` as `0644` and you add byte-identical content as `0755`,
your local file becomes a symlink to a `0444` object. It is no longer executable, and no
command reports this.

**Rule:** do not put executables under git-sfs. It stores dataset bytes; modes are not part
of what it versions.

---

## Locks

### `git-sfs push` was killed (OOM, CI timeout, SIGKILL). Now every command hangs. Why?

The lock is a directory, and it is only removed by a clean release
([lock.go:65-70](../internal/lock/lock.go#L65-L70)). `Acquire` spins on `os.Mkdir` forever
— no timeout, no staleness threshold, no `--force`
([lock.go:29-53](../internal/lock/lock.go#L29-L53)). The owner PID is written but **never
checked for liveness**.

Recovery is to delete the lock directory by hand:

```sh
rm -rf "$(readlink .git-sfs/cache)/locks/push.lock"
```

Which is the exact `rm -rf`-inside-the-data-store operation that everything else in these
docs tells you never to do. Aim carefully: the path must end in `/locks/<name>.lock`.

Related trap: if the `owner` file ended up zero-length, printing the "waiting for lock"
notice **panics** — `lockOwner` slices `data[:len(data)-1]` with no length check
([lock.go:62](../internal/lock/lock.go#L62)).

### Does `git-sfs add` block a concurrent `git-sfs pull`?

No. There are **five separate locks**, one per command — `add`, `import`, `setup`, `pull`,
`push`. Only two instances of the *same* command serialize. An `add` and a `pull` against
one cache both proceed by design.

That is mostly fine, with one sharp edge:

**`git-sfs pull` can delete an in-flight `git-sfs import --move`'s staging file.** `pull`
calls `PurgeTmp` — `RemoveAll(<cache>/tmp)` — as its first cache operation, and does it
*before* taking its lock ([pull.go:30-33](../internal/core/pull.go#L30-L33)). `import
--move` stages the user's file at `<cache>/tmp/.<hash>.move`
([cache.go:156](../internal/cache/cache.go#L156)) — after having already renamed the
**source away**. Different locks, so nothing prevents the overlap. Lose that window and the
source is gone and the destination was never written.

`add` is safe here only by accident: `AtomicCopy` stages inside the destination directory
([fsutil.go:52](../internal/fsutil/fsutil.go#L52)), not in `tmp/`, so the purge cannot reach
it. `verify --with-integrity` is *not* safe — its download temps go to the same `tmp/`
([command.go:180-194](../internal/remote/command.go#L180-L194)) — but there the loss is a
spurious error, not data.

**Rule:** never run `import --move` concurrently with a `pull` against the same cache.

### Two machines share a network cache (NFS/SMB). Are they mutually excluded?

No. Locks are per-cache-*directory* and rely on local `mkdir` atomicity, which network
filesystems do not reliably provide across hosts. git-sfs does not fix this and should not
be assumed to.

---

## When git-sfs tells you something untrue

### `git-sfs verify --check-remote` is green. Is my data really on the remote?

It means **a listing entry with that name exists**. Without `--with-integrity`, verify
fetches sizes in one batch call and checks only for presence
([verify.go:268-279](../internal/core/verify.go#L268-L279)) — it never compares the size it
just retrieved against anything.

**A zero-byte remote object passes `verify --check-remote`.** So does a truncated one. This
is the CI-facing command, which is exactly where a false green costs the most.

Use `--with-integrity` when the answer needs to mean something. It downloads and hash-checks
every object, which is expensive, which is why it is not the default — and why the default
is weaker than it reads.

### `git-sfs status --remote` says everything is unpushed. Did the remote lose my data?

Probably not. `status` fetches every size in **one** listing call and **discards the error**
([status.go:96](../internal/core/status.go#L96)). If that single call fails — a rate limit,
a transient 5xx, a token refresh — every file is reported `remote=missing` and `unpushed`
equals your whole dataset.

A dead backend *is* caught earlier by preflight
([app.go:144-158](../internal/core/app.go#L144-L158)), so this is the transient-failure case
specifically. `verify --check-remote` propagates the same error correctly and fails loudly;
only `status` swallows it.

**Rule:** treat a sudden all-unpushed `status` as "ask again," not as a fact. Confirm with
`verify --check-remote`.

### `verify --check-remote --with-integrity` reports "missing remote file". Is it missing?

Maybe. `CheckFile` returns `(false, nil)` on **any** rclone error
([command.go:195-200](../internal/remote/command.go#L195-L200)). An expired credential, a
403, a DNS hiccup, a throttle — every one of them is rendered as *missing remote file*, the
same words used for an object that genuinely is not there.

The distinction that matters — "absent" versus "I could not determine" — does not exist in
the output. A user who believes the first will re-push a terabyte, or conclude the remote
ate their data.

### `git-sfs push` printed "uploading 412 file(s)" and exited 0. Is it all there?

The only evidence is rclone's exit status. Push hands the list to rclone and returns
([push.go:70](../internal/core/push.go#L70)); nothing is read back, nothing is size-checked,
nothing is hash-checked. The remote copy — the one that exists precisely so the cache is not
the only copy — is the single artifact git-sfs never verifies on write.

`--checksum` helps only where the backend exposes a hash. Where it does not, rclone falls
back to size+modtime, and a same-size corrupt remote object is never re-uploaded and never
noticed.

---

## The cache

### A cache file was corrupt, so I ran `chmod -w` on it. What now?

It is trusted permanently. `HasValid` treats *read-only* as proof of *hash-verified*, and
never re-hashes a read-only file ([cache.go:63-81](../internal/cache/cache.go#L63-L81)).
You have just certified corrupt bytes.

The bit is load-bearing, not cosmetic. Never set it by hand.

### I restored my cache from a backup / rsync / a tarball. Is it trusted?

Depends on which way the mode bit came out, and the intuitive case is the *safe* one:

- **Write bits present** (the usual outcome of `rsync` without `-p`, most unzips): git-sfs
  treats it as a legacy file, **re-hashes it**, and re-protects it if valid
  ([cache.go:72-80](../internal/cache/cache.go#L72-L80)). Correct behavior, one-time cost.
- **Read-only preserved on bytes that were already damaged** (e.g. `tar -xp` from a rotted
  archive): trusted forever, never re-hashed.

The second case is why `doctor` should — and does not — probe whether the cache filesystem
preserves modes at all. exFAT/FAT, some FUSE and SMB mounts, and Docker volume copies can
all hand you unverified bytes wearing a read-only bit.

**Rule:** after any restore, run `git-sfs verify --rehash` before trusting the cache.

### `verify --rehash` says `CORRUPT`. Can git-sfs repair it?

No. git-sfs holds no redundancy of its own — it can detect rot and never fix it. Repair
comes from the other tier: `rm` the object and `git-sfs pull` it back, if the remote has a
good copy.

Two consequences worth internalizing:

- Rot in a **pushed** object is a nuisance. Rot in an **unpushed** object is permanent loss.
- Storage-level integrity (ZFS, btrfs, RAID, a checksumming backend) is the actual
  mechanism here. git-sfs is the detector, not the repair.

And see the push question above: repairing from the remote only works if you have not
already pushed the rotted copy over it.

### I ran git-sfs as root. Does anything change?

Yes. Root writes straight through the read-only bit, so the cache's immutability guarantee
evaporates — a stray redirect or a buggy script can rewrite a cache object in place, and it
stays trusted because the mode never changed. Batch jobs and containers are exactly where
this happens.

### Can I delete cache objects I no longer need?

There is no supported way. `verify` prints `run git-sfs gc to reclaim`
([verify.go:343](../internal/core/verify.go#L343)) — **`git-sfs gc` does not exist.** The
message is a leftover.

Do not hand-roll it. Orphan detection here only knows about symlinks in your *current
working tree*: an object referenced by another branch, by history, or by a different repo
sharing the cache counts as an orphan and is not one. A deletion script against that count
deletes live data.

**Rule:** treat cache growth as a disk-provisioning problem, not a cleanup problem.

---

## Git interactions

### I ran `git clean -xfd`. What did I just do?

By default the cache lives at `.git-sfs/.cache`, **inside the working tree**
([init.go:45](../internal/core/init.go#L45)), and `init` adds it to `.gitignore`
([init.go:141](../internal/core/init.go#L141)). `git clean -x` removes ignored files.

So the standard "give me a clean tree" command **deletes your entire cache**. For anything
not yet pushed, that is total and unrecoverable. Same for `rm -rf` on a clone you thought
was disposable.

**Rule:** point the cache outside the repo at `init` time (`--cache /path/outside` or
`GIT_SFS_CACHE`). The in-tree default is convenient and is the single worst default in the
tool.

### I used `git mv` instead of `git-sfs mv`. Why is the file broken?

Symlink targets are relative to the link's own directory
([sfspath.go:20-22](../internal/sfspath/sfspath.go#L20-L22)), so a target is valid only at
the depth it was created for. Moving `data/a.bin` to `data/sub/a.bin` leaves a target
computed for the old depth: it now resolves to nothing.

Nothing detects it until `verify`. `git-sfs mv` rewrites the target
([mv.go:35-63](../internal/core/mv.go#L35-L63)); plain `mv` and `git mv` do not. Moving a
whole directory with `mv` breaks every link inside it at once.

### I checked out an old branch and my files are ENOENT.

Expected. The checkout swapped symlinks to hashes your cache may not hold, and nothing warns
you — the failure surfaces when something *reads* the file, as an ENOENT naming the cache
path rather than the actual problem.

Run `git-sfs pull` after the checkout. There is no `post-checkout` hook doing it for you.

### A fresh clone is nothing but dangling symlinks.

Correct, and unavoidable by design. The cache binding is a machine-local symlink at
`.git-sfs/cache`, which is gitignored — a clone has the pointers and none of the bytes.

```sh
git-sfs setup   # bind a cache; relink anything already present
git-sfs pull    # fetch the bytes
```

### What if Git isn't materializing symlinks (`core.symlinks=false`, Windows, an odd mount)?

Then Git checks symlinks out as **regular text files containing the target path**, and
`git-sfs add` on that tree will happily hash and store those pointer texts as if they were
your dataset. Nothing in the codebase checks this Git setting.

The same applies to anything that flattens symlinks in transit: `docker cp`, `tar` without
the intent you meant, archive extraction, S3 sync tools. If symlinks do not survive, the
repo silently means something different.

---

## Interrupts

### I hit Ctrl-C during `git-sfs push` or `pull` — or the process gets killed outright (OOM, crash, machine/server restart) or the network drops. Recoverable?

Depends on which of three things happened, and they have three different answers.

**Plain Ctrl-C (or a graceful shutdown signal), one press.** This is the safe case.
`main` turns SIGINT *and* SIGTERM into context cancellation
([main.go:45](../cmd/git-sfs/main.go#L45)), so a single Ctrl-C, and a normal "your
process has 30 seconds" shutdown signal from systemd/CI/a container orchestrator, both
unwind cleanly: the in-flight `rclone` subprocess is killed
([command.go:347](../internal/remote/command.go#L347)), the call returns
`context.Canceled`, and the deferred lock release still runs
([push.go:46](../internal/core/push.go#L46),
[pull.go:37](../internal/core/pull.go#L37)) because that's ordinary Go code unwinding,
not a hard kill. The command prints `canceled` and exits `130`
([main.go:47-54](../cmd/git-sfs/main.go#L47-L54)). **Rule: just re-run the same
command.** Push re-diffs by cache presence and `--checksum`
([push.go:58](../internal/core/push.go#L58),
[command.go:248](../internal/remote/command.go#L248)); pull re-diffs by `HasValid` and
`--ignore-existing` ([pull.go:69](../internal/core/pull.go#L69)) — already-transferred
files are skipped, not redone.

**A hard kill (`kill -9`, OOM killer, power loss, host crash, a second impatient
Ctrl-C).** This is where recovery stops being automatic. None of the Go code above runs
— no deferred lock release — so the `push.lock` or `pull.lock` directory is left behind,
and it has no staleness check, no PID-liveness check, and no timeout
([lock.go:29-53](../internal/lock/lock.go#L29-L53), covered above under *Locks*). The
next invocation of that same command, on that machine, waits on the lock **forever**.
Recovery is the same manual step as the OOM-killed-push case above: `rm -rf` the
specific `<cache>/locks/<name>.lock` directory, then re-run. This is the only step that
isn't automatic — everything else below still holds.

**What happens to the data itself in a hard kill, independent of the lock:**

- *Local cache, during `pull`.* Safe. Downloads stage through rclone's own temp file
  before the final rename — the comment at
  [command.go:261-263](../internal/remote/command.go#L261-L263) is explicit that this
  is why `--temp-dir` is routed through `<cache>/tmp`. A kill mid-download leaves at
  worst an orphaned temp file, never a truncated file at the real cache path, and the
  *next* `pull` calls `PurgeTmp()` — `RemoveAll(<cache>/tmp)` — as its first action
  before it does anything else ([pull.go:30-32](../internal/core/pull.go#L30-L32)),
  clearing it automatically. Even in the narrow case where a kill lands between
  rclone's rename and git-sfs's own hash-verify-and-protect step, the file is left
  writable, and `HasValid` treats a writable file as unverified and re-hashes it
  before trusting it ([cache.go:63-81](../internal/cache/cache.go#L63-L81)) — corrupt
  ones get deleted and redownloaded automatically
  ([pull.go:80-87](../internal/core/pull.go#L80-L87)). One sharp edge shared with any
  `pull`, not specific to crashes: that same startup `PurgeTmp()` can destroy an
  in-flight `import --move`'s staging file if the two race (see the *Locks* section
  above).
- *Local cache, during `push`.* Never at risk — push only reads the cache, it never
  writes to it ([push.go:70](../internal/core/push.go#L70)).
- *Remote copy, during `push`.* The honest answer is "depends on the backend, and
  git-sfs cannot tell you which happened." `CopyToRemote` writes straight to the final
  remote path with no temp-path-then-publish step of its own
  ([command.go:248](../internal/remote/command.go#L248); the "Remote Writes" section of
  [safety.md](safety.md) claims otherwise — code doesn't implement it, see the "When
  git-sfs tells you something untrue" section above). On an object store (S3/GCS/Azure)
  each write is one atomic PUT, so a kill mid-upload means nothing landed — the remote
  is simply missing the object, and the next `push` uploads it cleanly. On SFTP/WebDAV/
  local/NFS/SMB destinations, whether a kill mid-write leaves a truncated object at the
  final path depends on rclone's own write mode for that backend, which git-sfs does not
  control or verify (same hazard as the "two users push at once" question above). Push
  never reads back what it sent — nothing here is checked automatically. **Rule: after
  any hard-killed push to a non-object-store backend, run `git-sfs verify --check-remote
  --with-integrity` before trusting the remote as a backup.**

**A network drop that doesn't kill the process.** Handled below the level you'd notice.
Every `rclone` invocation is wrapped in a retry loop — up to 3 attempts by default,
exponential backoff starting at 1 second — and only gives up and returns a real,
non-zero-exit error after that's exhausted
([command.go:374-402](../internal/remote/command.go#L374-L402)). A transient blip is
usually invisible; a genuinely dead connection surfaces as a normal command failure, not
a silent success, and re-running is safe for the same idempotency reasons as the Ctrl-C
case.

### I hit Ctrl-C during `git-sfs add`. What state am I in?

A mixed one. Files already processed are symlinks; the rest are still regular files, and
there is no summary of which is which
([add.go:73-95](../internal/core/add.go#L73-L95)). Cached bytes are safe — publication is
temp-file-plus-rename — so re-running `add` resumes cheaply.

The narrow bad window is between `os.Remove(file)` and `os.Symlink(target, file)`
([add.go:88-92](../internal/core/add.go#L88-L92)). A crash *there* leaves the path empty
with no symlink: the bytes are in the cache, but nothing on disk records which hash belonged
at that path. Recovery means re-adding from the original source.

`git status` after an interrupted `add` is the fastest way to see where you are.

### I hit Ctrl-C during `git-sfs mv` on a directory.

Worse than `add`. `mvDir` renames the whole tree first, *then* rewrites each symlink target
in a loop ([mv.go:96-115](../internal/core/mv.go#L96-L115)). Interrupt it and some links
resolve and some dangle at the wrong relative depth.

`Mv` also takes no `context.Context` — it is the one core operation that cannot be canceled
cleanly, so what stops it is the process dying mid-loop. Run `git-sfs verify
--no-check-remote` afterward and re-run `git-sfs mv` on the leftovers.

### `import --move` finished instantly and my source file is gone. Was it even copied?

If the content was already in the cache, `Move` skips the copy entirely and just
`os.Remove(src)` ([cache.go:144-149](../internal/cache/cache.go#L144-L149)). That is correct
dedup — and it is a silent unlink of your file based on trusting the read-only bit
(see the `chmod -w` question above). If the existing cache object was corrupt but protected,
you just deleted the last good copy.

`import` without `--move` copies and leaves the source alone. Prefer it until the push has
landed.

---

## Configuration and capacity

### Will `git-sfs init` make me commit my rclone credentials?

It sets you up to. The default config ships `config = "rclone.conf"`
([config.go:137](../internal/config/config.go#L137)), resolved relative to `.git-sfs` — so
the natural location is `.git-sfs/rclone.conf`, **inside a tracked directory**. `.gitignore`
covers only `.git-sfs/cache` and `.git-sfs/.cache`
([init.go:141](../internal/core/init.go#L141)).

A single `git add .git-sfs` commits cloud credentials into shared history, where they must
be treated as compromised and rotated.

**Rule:** point `config` at a path outside the repo (`~/.config/rclone/rclone.conf`), or add
`.git-sfs/rclone.conf` to `.gitignore` yourself, today.

### I typo'd the remote `path`. Does git-sfs catch it?

Only if the path does not exist — `RequireExists` refuses to create a missing remote root
([command.go:159-167](../internal/remote/command.go#L159-L167)). A typo that lands on an
*existing* directory is written into without complaint. Content addressing means the damage
is confined to adding files, not overwriting foreign ones, but the objects go somewhere you
did not mean.

### My disk filled up during a pull. Wasn't there a space check?

There is, and it fails open twice
([pull.go:99-123](../internal/core/pull.go#L99-L123)):

- Hashes missing from the remote listing contribute **0 bytes**, so the estimate is low. If
  every hash is missing, the total is 0, and the check returns `nil` without looking at the
  disk at all ([pull.go:110-112](../internal/core/pull.go#L110-L112)).
- A `statfs` failure prints a warning and proceeds
  ([pull.go:113-117](../internal/core/pull.go#L113-L117)).

It guards the ordinary case and disappears in the interesting ones. Check free space
yourself before a large pull; `git-sfs status --remote` reports the total size you are about
to fetch.

---

## Where else to look

- [failure-modes.md](failure-modes.md) — the same hazards organized by how they kill,
  written for people changing the code.
- [contract-spec.md](contract-spec.md) — §13 enumerates the known v1 defects that must not
  be reproduced, with the reasoning behind each.
- [safety.md](safety.md) — what the guarantees are meant to be. Note that its "Remote
  Writes" section describes a temp-path-then-publish protocol the code does not implement.
