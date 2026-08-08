# AGENTS.md

## Status

This repository ships the Rust implementation in `crates/`. The implementation
is governed by the stable behavior contract in
[docs/contract-spec.md](docs/contract-spec.md), with operational notes in
[docs/safety.md](docs/safety.md). Treat those documents as the source of truth
for on-disk layout, command behavior, and reliability requirements.

## Project

`git-sfs` stores large file bytes outside Git while Git tracks symlinks. Its primary use case is managing large research and ML datasets — binary files that are too big for Git but must be versioned, shared across machines, and reproduced exactly. Users treat it as a data management tool, so data loss or silent corruption is unacceptable.

Reliability requirements that follow from this:

- Never silently drop, truncate, or corrupt a cached file. Hash-verify at every boundary: after hashing, after download, after copy.
- Never modify a file the user did not explicitly hand to git-sfs (e.g. do not chmod source files during import).
- Prefer failing loudly with a clear error over proceeding with incomplete state.
- Atomic writes (temp file + rename) are mandatory wherever a partial file would be worse than no file.
- Cache files are write-once and read-only after storage; treat them as immutable.
- Long-running operations must be cancelable and safe. Thread `git_sfs_core::Cancel` into every byte-moving loop (hash, copy, download) and check it each chunk so Ctrl-C (SIGINT) stops work promptly. Cancellation must leave state consistent: never publish a partial file (rely on temp + rename), and surface the interrupt as a clean cancellation, not a corrupt result.

Project direction:

- Keep `git-sfs` as small as possible.
- Treat it as a layer on top of Git, the filesystem, and well-known file movers.
- Prefer plain files, symlinks, directories, and subprocess calls over custom state.
- `rclone` is the only supported remote tool.
- Minimize rclone invocations for every remote command. Transfers must batch
  object paths into rclone (for example with `--files-from`) instead of spawning
  one rclone process per object; metadata checks should use one listing call
  where possible. Per-object rclone calls are only acceptable when the operation
  genuinely cannot be expressed as a batch, and the reason should be explicit.
- Do not add manifests, databases, background services, custom protocols, or hidden metadata.
- If a feature needs a new internal format, first ask whether Git or the filesystem already provides the needed state.
- Progress output uses `indicatif`.
- Prefer `--verbose` and `-j`/`--jobs` style flags for commands that can benefit from them.
- When concurrency is needed, keep coordination simple and explicit; prefer channels and atomics over mutex-heavy designs where practical.

Core model:

- Git tracks `.git-sfs/config.toml` and relative symlinks.
- Git symlinks point into `.git-sfs/cache/files/sha256/<prefix>/<hash>`.
- `.git-sfs/cache` is a symlink to the cache root.
- Cache files live at `<cache>/files/sha256/<prefix>/<hash>`.
- Cache path is local state and must never be committed.

## Dependencies

The stdlib is deliberately minimal and the Rust ecosystem is designed to be
composed; hand-rolling a TOML parser, archive reader, or progress renderer
would be strictly worse than using audited crates. Composing well-tested crates
is the correctness play here, not a compromise of it.

| Area | Crate | Rationale |
|---|---|---|
| CLI | `clap` v4 derive | Typed `argv` parsing, completions, man pages |
| Config | `serde` + `toml`, `toml_edit` for writes | `toml_edit` preserves comments on rewrite |
| Hashing | `sha2` + `hex` | SHA-NI hardware acceleration |
| Paths | `camino` (`Utf8PathBuf`) | Paths flow into config and JSON; no lossy `Path`↔`String` conversion |
| Walking | `walkdir` | Symlink-loop handling, deterministic ordering |
| Atomic writes | `tempfile` | `NamedTempFile::persist()` — cleanup on every exit path via `Drop` |
| Parallelism | `rayon` | Sync, no async runtime; `-j` maps onto `ThreadPoolBuilder` |
| Cancellation | `ctrlc` + `AtomicBool` | See `git_sfs_core::Cancel` |
| Progress | `indicatif` | Real multi-bar for parallel jobs |
| Errors | `thiserror` + `anyhow` | Typed in core, ergonomic in the binary |
| JSON | `serde_json` | Frozen output shapes + rclone `lsjson` parsing |
| Semver | `semver` | Correct prerelease handling |
| HTTP | `ureq` + rustls, **not** native-tls | Sync-native; keeps Linux release artifacts statically linkable |

Audit *what* is pulled in, since this project ships binaries to user machines
via `self update` and verifies SHA-256 of every download. `cargo-deny` belongs
in CI for advisories and licenses once the dependency set stabilizes enough to
be worth pinning.

`self update`'s download-verify-replace path is the one exception to "use a crate": it stays hand-written even though `self_update` exists, because that is the path where a compromised dependency means arbitrary code execution on user machines.

## Commands

Required checks after code changes:

```sh
just check
```

`check` runs the Rust workspace checks plus the conformance harness. If `just`
is unavailable, run the commands from `Justfile` and the files it imports
(`just/rust.just`, `just/conformance.just`) manually. `cargo fmt`/`cargo clippy`
rewrite and lint files in place; re-read any file they touch before making
further edits.

Common user request: when asked to commit, push, and update version after changes, stage the finished changes, commit them with a focused message, create the next sequential version tag unless a specific version is named, push `main`, and push the new tag.

Release versions are tracked by Git tags and should be embedded into built binaries and installer output.

## Style

- Keep the implementation boring and explicit.
- Keep CLI parsing thin; put behavior in the `exec`/command layer, never in the argument grammar itself.
- Keep Git and filesystem state as the source of truth.
- Use temp files plus atomic rename for file writes.
- Hash-verify bytes before accepting cache or remote files.
- Comments are welcome when they explain invariants, safety behavior, or non-obvious control flow.
- Avoid comments that merely restate a line of code.

- `git-sfs-core` cannot print and cannot exit — that boundary is enforced by the dependency graph, not by discipline. A function that needs to write to a terminal, take a `quiet` flag, or accept a progress callback belongs in the binary crate, not in core. If a core function seems to need one of those, emit an event instead and let the binary decide how to render it.
- Prefer a newtype with validating constructors over a stringly-typed alias (e.g. `Sha256([u8; 32])`, not `type Hash = String`). If a guard clause exists only to handle a value that construction should have made impossible, delete the guard rather than port it.
- Prefer a typed error enum whose variants describe what the *caller* should do next (retry, fix input, stop) over a `String` or an untyped `anyhow::Error` at a public boundary. Do not mark such an enum `#[non_exhaustive]` if the whole point is that every variant must be classified somewhere (e.g. an exit-code mapping) — exhaustiveness there is a feature, not a maintenance burden.
- Never discard a `Result` with `let _ = ...` without a `#[allow(clippy::let_underscore_must_use, reason = "...")]` naming why. The workspace denies this lint because a discarded error can make a broken remote look empty.
- `unsafe` is forbidden at the workspace level (`unsafe_code = "forbid"` in the root `Cargo.toml`). If a dependency needs it, that is the dependency's business, not this codebase's.
- Traits get a real implementation and a test fake, minimum — a trait with one implementer is speculative abstraction.

## Tests

Keep all of these healthy:

- Rust unit tests colocated in `crates/git-sfs-core/src/**` and `crates/git-sfs/src/**`
- Shell workflow suite in `test/workflows/run.sh` — binary-agnostic via `GIT_SFS_BIN`
- The differential harness in `test/differential/` — compares binaries against stable behavior, not blindly against each other; an enumerated divergence is a fix to assert positively, not a regression to chase
- GitHub Actions CI in `.github/workflows/ci.yml`
- GitHub release automation in `.github/workflows/release.yml`

Coverage is reported for visibility, but do not add tests only to increase the
percentage. Prefer behavior tests that prove a user-facing invariant or a real
failure mode. Filesystem tests should use `tempfile::tempdir()` or a fixture
under it. Exercise git-sfs behavior, not standard-library behavior such as
`fs::create_dir_all` failing when a parent path is a file.

When changing storage, symlink, cache, or remote behavior, add or update tests for:

- Correct symlink target format
- Missing cache file detection
- Corrupt cache or remote file rejection
- Pull after cache file removal
- Push skipping existing remote files
- Retry-safe temp-file behavior where practical

`test/differential/coverage.py` maps stable behavior to coverage status; the
release gate is every required behavior mapped to a passing assertion.

## Documentation

`README.md` must stay brief — it is a demonstration of usefulness, not a manual:

- What the project does (2–3 sentences)
- Install command
- Minimal quick-start showing the core workflow
- Links to `docs/` for everything else

Do not duplicate `docs/commands.md`, `docs/configuration.md`, or any other reference doc in `README.md`. If you find yourself adding flag descriptions, full command lists, or detailed explanations to `README.md`, put them in the appropriate `docs/` file and add or update the link instead.

Do not put secrets, local absolute cache paths, or temporary state in committed config examples except as clearly illustrative placeholders.
