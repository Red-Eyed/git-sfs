# Changelog

## v2.0.0

### Highlights

- Ship the Rust implementation as the stable `git-sfs` release line.
- Make cache, remote, and installer writes atomic: bytes stage in project-owned
  temp locations and publish only after verification.
- Batch remote transfers and metadata checks through rclone instead of spawning
  one process per object.
- Show rclone's live transfer progress, speed, and ETA during `push` and
  `pull`.
- Show elapsed status spinners for long-running local work instead of leaving
  the terminal blank.

### Backwards Incompatible Changes

#### Source Builds

##### Source builds use the Rust workspace

The repository now ships the Rust workspace as the maintained implementation.
Contributors building from source should use the Rust toolchain, `cargo`, and
the `just` recipes documented in `docs/development.md`. The retired source tree
and module files have been removed.

#### Configuration

##### `config.toml` is validated as a closed schema

Unknown top-level keys, unknown sections, unknown remote fields, and unknown
settings now fail validation. Local cache configuration is also rejected from
the committed config file. Remove misspelled or local-only keys before
upgrading.

##### Ambiguous string values are rejected

Values that would be read differently by the compatibility scanner and strict
TOML parsing now fail with an ambiguity error. For example, a quoted `#` inside
a remote path is reported instead of silently choosing one interpretation.
Rewrite the value to the path shown by the error before continuing.

#### Status Output

##### `status --json` represents unknown remote state explicitly

Remote lookup failures are no longer collapsed into `false`. JSON consumers
must handle a remote state that can be present, absent, or unknown with a cause.

### New Features

- Add `git-sfs self update` to update both `git-sfs` and `rclone` with checksum
  verification and atomic binary replacement.
- Add `--pre` to the installer and `git-sfs self update` to opt into eligible
  prerelease versions while keeping stable releases as the default.
- Add `git-sfs llms-txt` for an offline, bundled reference document.
- Add `git-sfs doctor` checks for repository, config, cache, version, rclone,
  and remote preconditions.
- Add `git-sfs status` and `git-sfs remotes` JSON output for automation.
- Add `git-sfs verify --rehash` and `--rehash-sample` for explicit cache-wide
  integrity audits.

### Improvements

- Default new cache bindings under the private Git directory so `git clean -x`
  does not remove unpushed cache objects.
- Preserve existing cache bindings during `setup`, including the old
  `.git-sfs/.cache` location when no `.git-sfs/cache` symlink exists.
- Make `add`, `import`, `mv`, `pull`, and byte-moving operations cancellable;
  interrupted writes leave no final partial cache object.
- Stage `push` uploads under a remote temp prefix and publish final remote
  objects only after the batch transfer and size verification pass.
- Stage `pull` downloads in cache-owned temp storage, verify SHA-256, then adopt
  objects into the cache.
- Show rclone's own transfer progress for `push` and `pull`, and silence it
  with `--quiet`.
- Show elapsed status spinners while scanning, hashing, verifying, importing,
  moving, setup, and self-update phases are running.
- Skip downloading or hashing already-verified cache objects during ordinary
  pulls; writable cache objects are treated as unverified, hash-checked, and
  protected again.
- Keep `push`, `pull`, `status`, and `verify` scoped to the requested path
  rather than scanning or transferring unrelated siblings.
- Surface unreachable remotes as unavailable or unknown state instead of
  reporting an empty remote.
- Retry only rclone's temporary-error exit class; permanent backend failures now
  fail promptly.

### Bug Fixes

- Let prerelease binaries satisfy older `min_git_sfs_version` floors while
  correctly remaining below the matching final release.
- Refuse to convert files that are already tracked by Git when running
  `git-sfs add`.
- Let a freshly initialized repository run local verification without requiring
  a configured remote.
- Reject truncated or corrupt remote objects during remote integrity checks.
- Prevent `push` from overwriting a good remote object with a protected local
  object whose bytes no longer match its hash.
- Make `import --move` consume the source only after a verified cache object has
  been published, including cross-filesystem moves.
- Make `git-sfs mv` rewrite relative symlink targets when moving files across
  directory depths.
- Keep stale temp cleanup selective so one command does not remove another
  command's in-flight staging.

### Performance

- Use batched `rclone copy --files-from` transfers for push and pull.
- Query remote metadata by requested object prefixes instead of listing the
  entire remote for small scoped operations.
- Deduplicate work by object hash so identical file contents are stored,
  uploaded, and downloaded once.

### Documentation

- Replace rewrite-era documentation with stable contract, architecture,
  configuration, command, workflow, installation, and safety docs.
- Reset the changelog for the stable release line.

### Developers

- Add the Rust workspace, release archive builder, installer path, and CI checks
  for the maintained implementation.
- Publish semantic-version prerelease tags as non-latest GitHub prereleases and
  pin release-page install commands to the exact tag.
- Add the conformance harness for workflow parity, cancellation safety, cache
  mode behavior, lock contention, downgrade/round-trip behavior, and stable
  behavior coverage.
