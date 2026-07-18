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
snapshot.py    tree -> canonical manifest. Reusable on its own.
run.py         driver: run scenarios, snapshot, diff
lib.sh         helpers available to every scenario
scenarios/     one scenario per file
```

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
