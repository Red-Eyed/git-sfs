# Where git-sfs Dies

> "All I want to know is where I'm going to die, so I'll never go there." — Charlie Munger

Inverted design review. Instead of asking "how do we make git-sfs good?", this asks
"how does git-sfs destroy someone's dataset?" — then refuses to go there.

git-sfs is a data management tool. The failure that matters is not a crash; it is a
crash that *looks like success*. Every item below is a place where the tool can lose
bytes, accept wrong bytes, or tell the user something untrue.

Ordered by how dead you are, not by how likely it is.

---

## 1. The cache is the only copy

The single most lethal property of the design: `git-sfs add` **deletes the user's
file** and replaces it with a symlink ([add.go:88-93](../internal/core/add.go#L88-L93)).
After that, Git holds a symlink, and the bytes exist in exactly one place on earth —
a local directory that Git does not track, does not back up, and does not know about.

- [ ] **Cache on scratch/tmp/ephemeral storage.** `add` + commit + reboot = dataset gone,
      repo full of dangling symlinks. Nothing in `setup` or `doctor` warns that the chosen
      cache root is on `/tmp`, a ramdisk, a container layer, or an auto-cleaned scratch mount.
- [ ] **Add without push.** Between `add` and `push` there is no redundancy. The window is
      unbounded and invisible — no command says "you have N unpushed objects." (`status`
      should make this the loudest line it prints.)
- [ ] **One cache, many repos.** The cache root is shared and content-addressed. `rm -rf`
      on it, or "cleaning up disk space," destroys data for every repo bound to it, and
      no repo knows the others exist.
- [ ] **Orphan accumulation with no reaper.** `verify` counts orphaned objects and prints
      `run git-sfs gc to reclaim` ([verify.go:343](../internal/core/verify.go#L343)) —
      **`git-sfs gc` does not exist.** The CHANGELOG records removing the docs for it;
      the string in the code survived. Users will either ignore the advice or hand-roll
      a deletion script against a content-addressed store, which is how people delete
      live data.
- [ ] **Cache path lost, not the cache.** The binding lives only in local state
      ([localstate.go:35-50](../internal/localstate/localstate.go#L35-L50)). Lose it and a
      full cache is indistinguishable from no cache.

**Never go there:** treat "bytes exist in exactly one unreplicated place" as the tool's
defining hazard. Warn at `setup` time about ephemeral cache roots; make unpushed-object
count a first-class output; don't advertise commands that don't exist.

---

## 2. Integrity rests on a filesystem permission bit

`HasValid` treats *"the file is read-only"* as proof *"the bytes were hash-verified"*
([cache.go:58-81](../internal/cache/cache.go#L58-L81)). Verified files get the write
bits stripped, and thereafter a read-only file is trusted **without re-hashing**.

That is a fast and elegant invariant, and it is only as strong as the filesystem's
willingness to keep the bit.

- [ ] **A filesystem that doesn't preserve modes.** exFAT/FAT, some FUSE and network
      mounts, SMB/NFS with odd id mapping, Docker volume copies, restores from a backup
      or `rsync` without `-p`, an unzip. Any of these can present unverified bytes wearing
      a read-only bit — trusted forever, never re-hashed.
- [ ] **Running as root.** Root writes through the read-only bit. The protection is
      advisory against exactly the user most likely to be running batch jobs.
- [ ] **A user "fixing" a corrupt file** with `chmod -w`. Now it is permanently trusted.
- [ ] **Bit rot after protection.** Nothing re-verifies a protected file on read.
      `--rehash` exists but is opt-in and manual; there is no schedule, no reminder,
      and at terabyte scale the sampling mode is the only realistic option.

**Never go there:** don't let a mode bit be the sole carrier of a correctness claim on a
filesystem you didn't choose. `doctor` should probe whether the cache filesystem actually
preserves permissions (write a file, chmod it, re-stat it) and refuse to trust the
fast path when it doesn't.

---

## 3. Crash windows that leave state neither here nor there

Cache *publication* is genuinely atomic (temp + rename). The operations *around* it
are not.

- [ ] **`add`: remove-then-symlink.** `os.Remove(file)` then `os.Symlink(target, file)`
      ([add.go:88-93](../internal/core/add.go#L88-L93)). A crash between them leaves the
      path empty with no symlink. The bytes survive in the cache, but nothing on disk
      records which hash belonged at that path — recovery means guessing. Symlink to a
      temp name and `rename` over the original instead.
- [ ] **`add`/`import`: partial conversion on error.** The publish loop returns on the
      first error ([add.go:77-79](../internal/core/add.go#L77-L79)), leaving some files
      converted and some not, with no summary of which.
- [ ] **`import --move` deletes sources before destinations exist.** `c.Move` renames the
      source away ([cache.go:130](../internal/cache/cache.go#L130)) during the parallel
      prepare phase; symlinks are created afterward. A failure in between means the source
      tree is gone and the destination was never written.
- [ ] **`import --move` deletes a source that was never copied.** If the hash already
      exists in cache, `Move` just `os.Remove(src)`
      ([cache.go:144-149](../internal/cache/cache.go#L144-L149)). Correct dedup — and a
      silent unlink of the user's file based on trusting §2's read-only bit.
- [ ] **`mv` on a directory is not atomic and not cancelable.** `mvDir` renames the tree,
      *then* rewrites each symlink target in a loop
      ([mv.go:96-115](../internal/core/mv.go#L96-L115)). Interrupt it and you have a
      directory where some links resolve and some are dangling at the wrong relative depth.
      `Mv` takes **no `context.Context`** — the only core operation that can't be
      cancelled, contradicting the project's own cancellation requirement.
- [ ] **Stray temp files in the object store.** `AtomicCopy` creates
      `.git-sfs-tmp-*` *in the destination directory*
      ([fsutil.go:52](../internal/fsutil/fsutil.go#L52)) — i.e. inside
      `files/sha256/<prefix>/`. `defer os.Remove` doesn't run on SIGKILL, and `PurgeTmp`
      only cleans `tmp/`. These accumulate inside the content-addressed tree and inflate
      the orphan count for the `gc` that doesn't exist.
- [ ] **`rename` is not fsynced.** `AtomicCopy` fsyncs the file but never the parent
      directory ([fsutil.go:74-81](../internal/fsutil/fsutil.go#L74-L81)). On power loss
      the rename can be lost even though the data was durable. "Atomic" ≠ "durable."

**Never go there:** any sequence that destroys the old state before the new state is
visible. The ordering rule is: publish the replacement, then remove the original — never
the reverse.

---

## 4. Telling the user something untrue

The quietest deaths. These do not fail; they report.

- [ ] **Remote errors collapse into "not found."** `HasFile`
      ([command.go:169-178](../internal/remote/command.go#L169-L178)),
      `CheckFile` ([command.go:195-200](../internal/remote/command.go#L195-L200)), and
      `FileSize` ([command.go:506-515](../internal/remote/command.go#L506-L515)) all
      return "absent" on *any* rclone error. Expired credentials, a 403, a DNS failure,
      a rate limit — all render as **"missing remote file."** A user seeing that will
      re-push, or worse, conclude the remote lost their data.
- [ ] **`verify --check-remote` only checks existence.** Without `--integrity` it asks
      for a listing and records "found"
      ([verify.go:268-280](../internal/core/verify.go#L268-L280)) — it does not compare
      the size it just fetched against anything. A **zero-byte or truncated remote object
      passes `verify`.** This is the CI-facing command; it is precisely where a false
      green is most expensive.
- [ ] **Disk-space guard fails open, twice.** Hashes missing from the listing contribute
      0 bytes, and if the total comes out ≤ 0 the check returns `nil`
      ([pull.go:110-112](../internal/core/pull.go#L110-L112)); a `statfs` failure warns and
      proceeds ([pull.go:114-117](../internal/core/pull.go#L114-L117)). Combined with the
      bullet above — where listing errors look like "not found" — an unreachable remote
      produces a *silently skipped* space check, then a pull that fills the disk.
- [ ] **Error classification by English substring.** `isRemotePathNotFound`
      ([command.go:106-117](../internal/remote/command.go#L106-L117)) makes control-flow
      decisions by grepping rclone's message text for `"directory not found"`, and bails
      out if the message happens to contain `"config"`. This breaks on an rclone wording
      change, a localized message, or a path that literally contains the word "config."
- [ ] **`verify` flags every regular file as "unconverted."** Any non-symlink in the
      scanned tree becomes an issue ([verify.go:134-141](../internal/core/verify.go#L134-L141)).
      Point it at a subtree containing a README and it fails. A check that cries wolf gets
      `|| true`'d in CI, and then it protects nothing.
- [ ] **Docs describe behavior the code doesn't have.**
      [safety.md](safety.md) states remote writes "should upload to a temporary remote path
      and then publish to the final path." `CopyToRemote` copies straight to the final path
      ([command.go:248](../internal/remote/command.go#L248)). Aspirational documentation in
      a *safety* document is worse than no documentation.

**Never go there:** never map an unknown error onto a specific known state. "I could not
determine" must be its own outcome, distinct from "it is absent" — and it must be loud.

---

## 5. Stuck forever

- [ ] **Stale locks are never reclaimed.** `Acquire` spins on `os.Mkdir` indefinitely
      ([lock.go:29-53](../internal/lock/lock.go#L29-L53)). A SIGKILL, an OOM kill, a
      container eviction, or a crashed CI runner leaves the lock directory behind, and
      **every subsequent `add`/`import`/`push`/`pull` waits forever.** The owner PID is
      recorded but never checked for liveness, and there is no timeout, no staleness
      threshold, and no `--force` escape. The recovery is `rm -rf` inside the cache —
      the exact operation §1 says is lethal.
- [ ] **Lock owner file is read unguarded.** `lockOwner` slices `data[:len(data)-1]`
      ([lock.go:62](../internal/lock/lock.go#L62)) with no length check. The write that
      creates it ignores its own error ([lock.go:33](../internal/lock/lock.go#L33)), so a
      zero-byte owner file is reachable — and reading it panics.
- [ ] **Locks are per-cache, not cross-machine.** A shared network cache has no mutual
      exclusion between hosts.

**Never go there:** a lock with no liveness check and no escape hatch is a deadlock with
extra steps. Record the PID *and* check it; offer a documented break-glass command so the
recovery path isn't "delete things inside the data store."

---

## 6. The environment is not what you assumed

The entire design rests on symlinks behaving like symlinks.

- [ ] **`core.symlinks=false`.** Git then checks out symlinks as *regular text files
      containing the target path*. Running `git-sfs add` in such a clone would hash and
      store those pointer texts as if they were the dataset. Nothing checks this Git
      setting anywhere in the codebase.
- [ ] **Filesystems and tools that flatten or refuse symlinks.** Windows (documented as
      unsupported), some network mounts, `docker cp`, `tar` without `-h`/with `-h`
      depending on intent, archive extraction, S3 sync tools. Any of these silently change
      what the repo means.
- [ ] **A clone with no cache is a repo of dangling symlinks.** Every downstream
      consumer — `open()`, `tar`, a dataloader, a build — fails with a confusing
      ENOENT that names the symlink target, not the actual problem.
- [ ] **`doctor` doesn't check the things that kill.** It verifies repo, config, versions,
      cache writability, rclone, and connectivity — but not: whether the cache filesystem
      preserves permission bits (§2), whether `core.symlinks` is true, whether the cache
      is on ephemeral storage, or how much free space exists.
- [ ] **Latent trap: `IsInside` is wrong for single-character names.**
      `len(rel) >= 2 && rel[:2] != ".."` ([fsutil.go:110-113](../internal/fsutil/fsutil.go#L110-L113))
      returns `false` for a path like `<root>/a`, reporting a child as outside its root.
      It is currently unused outside its own test — which means it is a loaded gun waiting
      for the first caller who reaches for a containment check.

**Never go there:** don't let an unstated environmental assumption be load-bearing. If the
tool requires working symlinks and mode-preserving storage, it should *verify* that, not
assume it.

---

## 7. Trusting the mover

- [ ] **Push verifies nothing after upload.** `Push` gates on local `HasValid` and hands
      the list to rclone ([push.go:40-50](../internal/core/push.go#L40-L50)). What actually
      landed on the remote is never checked. The remote copy — the one that exists so the
      cache isn't the only copy (§1) — is the only artifact never hash-verified on write.
- [ ] **`--checksum` degrades silently.** On backends that expose no hash, rclone falls
      back to size+modtime ([command.go:243-248](../internal/remote/command.go#L243-L248)).
      The comment acknowledges this. On those backends, a same-size corrupt remote object
      is never re-uploaded and never detected without `--integrity`.
- [ ] **Retries are indiscriminate.** `retryLoop`
      ([command.go:377-405](../internal/remote/command.go#L377-L405)) retries permanent
      failures — bad credentials, missing path, permission denied — with exponential
      backoff, turning a clear immediate error into a slow one.
- [ ] **rclone is an unpinned external binary.** A version check exists but is opt-in via
      `min_rclone_version`. Behavior of `--ignore-existing`, `--checksum`, and `--temp-dir`
      is assumed across all versions and all backends.

**Never go there:** don't let the redundant copy be the unverified one. Push should
confirm what landed, at least by size, and `verify --check-remote` should compare it (§4).

---

## Using this list

Run it as an inversion pass, not a bug list. For each change ask:

1. **Does this create a window where the only copy of the bytes is unprotected?** (§1, §3)
2. **Does this trust a claim I did not verify in this process?** (§2, §7)
3. **Does this destroy old state before new state is durable?** (§3)
4. **Can this report a specific state when it actually knows nothing?** (§4)
5. **Can this leave a process, or the next process, stuck with no escape?** (§5)
6. **What is this assuming about the filesystem, Git, or rclone that nothing checks?** (§6)

The consistent theme: git-sfs is careful *at the moment it writes bytes* and much less
careful *about everything around that moment* — the ordering of operations, the meaning
of an error, the durability of the surrounding state, and the truthfulness of its own
reports. That gap is where the tool goes to die.
