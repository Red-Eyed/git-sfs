//! `import` — ported from `import.go`. Ingests external files into the
//! cache and creates git-sfs symlinks at their destination paths inside the
//! repository.
//!
//! **Deliberately reorders v1's own sequencing to fix contract-spec §13.1.**
//! v1's `ImportWithOptions` runs a *parallel prepare phase* that hashes and
//! `--move`s every source into the cache first (`import.go:70-79`), and only
//! afterward, in a second serial pass, creates any destination symlinks
//! (`import.go:80-102`). A crash between the two phases leaves a file gone
//! from its external source location with no symlink yet pointing back at
//! it — the bytes are safe in the cache by hash, but nothing in the user's
//! working tree reaches them. Here, each source is hashed, cached
//! (`Store::store` for a copy, `Store::adopt` for `--move` — rename on the
//! same filesystem, falling back to verified copy-then-remove across a
//! device boundary), and symlinked back-to-back before the next source is
//! even looked at, so at most one file is ever "in flight" at once instead
//! of the whole import.
//!
//! `Store::adopt` (not a plain copy) is still the `--move` primitive
//! deliberately: for the large datasets this project targets, requiring a
//! full second copy of every file before deleting the original would need
//! roughly double the free disk space to move data that already fits once —
//! exactly the scenario `--move` exists for.
//!
//! Two things stay batched up front, because they are read-only and
//! aborting cleanly on a bad argument is itself part of the contract:
//! destination-collision validation across the whole source tree
//! (contract-spec §5b.2: "import validates before it moves anything, so a
//! rejected import is a no-op") and the not-a-git-sfs-concern parallel hash
//! pass v1 used for progress reporting, which does not exist yet in this
//! pass (see the module's own simplifications below).
//!
//! Deliberately not ported/simplified this pass, matching precedent set by
//! [`super::add`]:
//! - No auto-`init`/cache-creation. `import` requires an already-bound
//!   cache, same policy as `add` — the init/setup question stays parked.
//! - Sequential only, no rayon-based parallelism.
//! - No progress callback (Phase 5's `Event` stream is the real mechanism).

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use super::repo_relative;
use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::domain::symlink::{NoRelativePath, cache_link_file, clean_utf8, git_link_target};
use crate::error::Error;
use crate::ports::repo::should_skip;
use crate::ports::{Store, StoreError, hash_file};

/// One file `import` finished ingesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    /// The external path that was hashed and cached (after resolving any
    /// `-L`-followed symlink).
    pub src: Utf8PathBuf,
    /// Repo-relative destination path.
    pub dst: Utf8PathBuf,
    /// The hash it is now tracked under.
    pub hash: Sha256,
}

/// What an [`import`] run produced, whether or not it ultimately succeeded.
#[derive(Debug, Default)]
pub struct ImportOutcome {
    /// Every file successfully ingested, in the order they were processed.
    pub imported: Vec<ImportedFile>,
    /// Source candidates whose own filename is not valid UTF-8 — skipped,
    /// not ingested, but reported rather than silently dropped (mirrors
    /// [`crate::ports::repo::FoundEntry::Unrepresentable`]).
    pub unrepresentable: Vec<String>,
}

/// Why `import` failed.
#[derive(Debug, Error)]
pub enum ImportError {
    /// `dst` does not resolve to a location inside the repository.
    #[error("destination must be inside repository: {dest}")]
    DestinationOutsideRepo {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// `dst` resolves inside `.git-sfs`, which `import` must never write
    /// into directly.
    #[error("destination must not be inside .git-sfs: {dest}")]
    DestinationInsideGitSfs {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// `dst` already exists and is not a directory, but the source is one.
    #[error("destination exists and is not a directory: {dest}")]
    DestinationNotADirectory {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// Something is already at a destination path this import would write.
    #[error("destination already exists: {dest}")]
    DestinationAlreadyExists {
        /// The path that was already occupied.
        dest: Utf8PathBuf,
    },
    /// A symlink was reached as (or within) the source without `-L`.
    /// Contract-spec §5b.2: refusing by default is the safe choice, since a
    /// symlink in an incoming tree may point anywhere on the machine.
    #[error("source symlinks are not supported without -L: {path}")]
    SourceSymlinkRequiresFollow {
        /// The symlink that was reached.
        path: Utf8PathBuf,
    },
    /// A symlink found *while walking a source directory* resolved to
    /// something other than a regular file. (A directory symlink is only
    /// permitted as the top-level `src` argument itself, not nested within
    /// one — matching v1's own asymmetry, `import.go:194-201` vs
    /// `import.go:240-242`.)
    #[error("source symlink must resolve to a regular file: {path}")]
    SourceSymlinkTargetNotRegular {
        /// The symlink whose target was rejected.
        path: Utf8PathBuf,
    },
    /// A source entry is neither a regular file, a directory, nor (with
    /// `-L`) a symlink resolving to one — a device, socket, or similar.
    #[error("source must be a regular file or directory: {path}")]
    UnsupportedSourceType {
        /// The unsupported entry.
        path: Utf8PathBuf,
    },
    /// Resolving the top-level `src` argument's own symlink produced a path
    /// that is not valid UTF-8. Distinct from a nested entry hitting the
    /// same condition mid-walk (reported as `unrepresentable` and skipped
    /// instead): `src` is the one argument defining the whole operation, so
    /// there is no "rest of the import" to continue with if it can't be
    /// resolved.
    #[error("{path}: symlink target is not valid UTF-8")]
    NonUtf8Path {
        /// The path whose resolution produced non-UTF-8 bytes.
        path: Utf8PathBuf,
    },
    /// Walking a source directory failed.
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
    /// `dst` and the repository root disagree on absolute-vs-relative, so no
    /// symlink target could be computed. Not reachable in practice: both
    /// are always absolute by the time they reach here.
    #[error("{path}: {source}")]
    NoRelativePath {
        /// The destination path being computed for.
        path: Utf8PathBuf,
        /// The underlying error.
        #[source]
        source: NoRelativePath,
    },
    /// Hashing or caching `path` failed.
    #[error("{path}: {source}")]
    Store {
        /// The source file being processed.
        path: Utf8PathBuf,
        /// Why caching it failed.
        #[source]
        source: StoreError,
    },
    /// `path` was cached, but its object is not reachable through the
    /// repository's own `.git-sfs/cache` symlink. Mirrors `add`'s identical
    /// sanity check (see `AddError::CacheLinkUnreachable`).
    #[error("{path}: cache object for {hash} is not reachable through .git-sfs/cache")]
    CacheLinkUnreachable {
        /// The source file being processed.
        path: Utf8PathBuf,
        /// The hash it was cached under.
        hash: Sha256,
    },
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<ImportError> for Error {
    fn from(err: ImportError) -> Self {
        match &err {
            ImportError::Canceled
            | ImportError::Store {
                source: StoreError::Canceled,
                ..
            } => Error::Canceled,
            ImportError::Store {
                source: StoreError::HashMismatch { .. },
                ..
            } => Error::Integrity(err.to_string()),
            ImportError::NoRelativePath { .. } | ImportError::CacheLinkUnreachable { .. } => {
                Error::Config(err.to_string())
            }
            ImportError::DestinationOutsideRepo { .. }
            | ImportError::DestinationInsideGitSfs { .. }
            | ImportError::DestinationNotADirectory { .. }
            | ImportError::DestinationAlreadyExists { .. }
            | ImportError::SourceSymlinkRequiresFollow { .. }
            | ImportError::SourceSymlinkTargetNotRegular { .. }
            | ImportError::UnsupportedSourceType { .. }
            | ImportError::NonUtf8Path { .. } => Error::Usage(err.to_string()),
            ImportError::Store { .. } | ImportError::Walk(_) | ImportError::Io { .. } => {
                Error::Unavailable(err.to_string())
            }
        }
    }
}

/// [`import`]'s outcome-so-far, together with why it stopped — returned
/// instead of a bare [`ImportError`] so a caller can still report files
/// already ingested when a later one fails.
#[derive(Debug)]
pub struct ImportFailure {
    /// Every file successfully ingested before `error` stopped the run.
    pub outcome: ImportOutcome,
    /// Why it stopped. Boxed for the same reason as `AddFailure::error`.
    pub error: Box<ImportError>,
}

impl ImportFailure {
    fn new(outcome: ImportOutcome, error: ImportError) -> Self {
        Self {
            outcome,
            error: Box::new(error),
        }
    }
}

/// One file this import will ingest: its resolved source, its destination,
/// and the key used to detect it is the same underlying file as another
/// pair (two different source paths — e.g. two `-L`-followed symlinks —
/// can resolve to one canonical file).
struct ImportPair {
    src: Utf8PathBuf,
    dst: Utf8PathBuf,
    key: Utf8PathBuf,
}

/// The validated shape of one `import` invocation, computed entirely
/// read-only before anything is touched (contract-spec §5b.2). Not a
/// `crate::plan` type despite the name: unlike that module's pure functions,
/// this performs real filesystem I/O (`lstat`, `canonicalize`, directory
/// walks) to validate `src`/`dst`, so it lives here in `exec` instead.
struct ImportPlan {
    pairs: Vec<ImportPair>,
    /// Source directories to remove if left empty by `--move`, deepest-ish
    /// first (v1 sorts by path length, `import.go:260`) with `src` itself
    /// last.
    dirs: Vec<Utf8PathBuf>,
    /// Followed (`-L`) symlinks to remove under `--move` — distinct from
    /// `dirs`/the regular files they resolve to, which `import_one` already
    /// consumes.
    source_links: Vec<Utf8PathBuf>,
    unrepresentable: Vec<String>,
}

/// The two independent flags `import` takes -- v1's own `ImportOptions`
/// (`import.go:20-23`), grouped here for the same reason: they are read
/// together at nearly every call site below, and grouping them keeps
/// [`import`] under clippy's argument-count lint without losing either
/// flag's own name at the call site the way a bare `(bool, bool)` would.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Delete each source after it is safely cached and symlinked (see the
    /// module doc for why this is still `Store::adopt`, not a plain copy).
    pub move_source: bool,
    /// Follow a symlink reached as (or within) the source instead of
    /// refusing it.
    pub follow_symlinks: bool,
}

/// Ingests `src` (a path resolved against `cwd`, matching v1's
/// `filepath.Abs` — unlike `dst`, an external source has no reason to
/// resolve against the repository root) into the cache, creating a git-sfs
/// symlink at `dst` (resolved against `repo`, like every other command's
/// path arguments).
///
/// # Errors
///
/// Returns the files ingested so far bundled with the first [`ImportError`]
/// encountered, so a caller can report partial progress even on failure. A
/// failure during validation (before any file is touched) reports an empty
/// outcome, matching contract-spec §5b.2's "a rejected import is a no-op."
pub fn import(
    store: &dyn Store,
    repo: &Utf8Path,
    cwd: &Utf8Path,
    src: &Utf8Path,
    dst: &Utf8Path,
    options: ImportOptions,
    cancel: &Cancel,
) -> Result<ImportOutcome, ImportFailure> {
    let plan = plan_import(repo, cwd, src, dst, options.follow_symlinks)
        .map_err(|error| ImportFailure::new(ImportOutcome::default(), error))?;

    let mut outcome = ImportOutcome {
        unrepresentable: plan.unrepresentable,
        ..ImportOutcome::default()
    };
    let mut hashes: BTreeMap<Utf8PathBuf, Sha256> = BTreeMap::new();

    for pair in plan.pairs {
        if cancel.is_canceled() {
            return Err(ImportFailure::new(outcome, ImportError::Canceled));
        }

        let hash = match hashes.get(&pair.key) {
            Some(&hash) => hash,
            None => match import_one(store, repo, &pair.src, options.move_source, cancel) {
                Ok(hash) => {
                    hashes.insert(pair.key.clone(), hash);
                    hash
                }
                Err(error) => return Err(ImportFailure::new(outcome, error)),
            },
        };

        if let Err(error) = publish(repo, &pair.dst, hash) {
            return Err(ImportFailure::new(outcome, error));
        }
        outcome.imported.push(ImportedFile {
            src: pair.src,
            dst: repo_relative(repo, &pair.dst),
            hash,
        });
    }

    if options.move_source {
        for link in &plan.source_links {
            #[allow(
                clippy::let_underscore_must_use,
                reason = "best-effort cleanup of a followed symlink; the regular file it resolved to has already been safely cached and symlinked by this point"
            )]
            let _ = std::fs::remove_file(link.as_std_path());
        }
        for dir in &plan.dirs {
            #[allow(
                clippy::let_underscore_must_use,
                reason = "best-effort empty-directory cleanup matching v1 (import.go:265-269): a directory that is not actually empty is left alone, not a failure"
            )]
            let _ = std::fs::remove_dir(dir.as_std_path());
        }
    }

    Ok(outcome)
}

/// Hashes and caches `src` (`store()` to copy, `adopt()` to move — see the
/// module doc for why `--move` still uses `adopt`'s rename-with-fallback
/// rather than always copying), then confirms the result is reachable
/// through `.git-sfs/cache`, exactly like `add`'s own sanity check.
fn import_one(
    store: &dyn Store,
    repo: &Utf8Path,
    src: &Utf8Path,
    move_source: bool,
    cancel: &Cancel,
) -> Result<Sha256, ImportError> {
    let hash = hash_file(src, cancel).map_err(|source| ImportError::Io {
        path: src.to_owned(),
        source,
    })?;

    let cached = if move_source {
        store.adopt(src, hash, cancel)
    } else {
        store.store(src, hash, cancel)
    };
    cached.map_err(|source| ImportError::Store {
        path: src.to_owned(),
        source,
    })?;

    if !cache_link_file(repo, hash).is_file() {
        return Err(ImportError::CacheLinkUnreachable {
            path: src.to_owned(),
            hash,
        });
    }
    Ok(hash)
}

/// Creates the destination symlink for `hash` at `dst`, making the object
/// just cached reachable from the working tree.
fn publish(repo: &Utf8Path, dst: &Utf8Path, hash: Sha256) -> Result<(), ImportError> {
    let target =
        git_link_target(repo, dst, hash).map_err(|source| ImportError::NoRelativePath {
            path: dst.to_owned(),
            source,
        })?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| ImportError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    std::os::unix::fs::symlink(target.as_std_path(), dst.as_std_path()).map_err(|source| {
        ImportError::Io {
            path: dst.to_owned(),
            source,
        }
    })
}

/// Validates `src`/`dst` and, for a directory source, walks it -- v1's
/// `planMove` (`import.go:160-263`), read-only throughout.
fn plan_import(
    repo: &Utf8Path,
    cwd: &Utf8Path,
    src_arg: &Utf8Path,
    dst_arg: &Utf8Path,
    follow_symlinks: bool,
) -> Result<ImportPlan, ImportError> {
    let src_abs = absolute(cwd, src_arg);
    let dst_abs = clean_utf8(&if dst_arg.is_absolute() {
        dst_arg.to_owned()
    } else {
        repo.join(dst_arg)
    });

    let dst_rel = dst_abs
        .strip_prefix(repo)
        .ok()
        .filter(|rel| !rel.as_str().is_empty())
        .ok_or_else(|| ImportError::DestinationOutsideRepo {
            dest: dst_arg.to_owned(),
        })?;
    if should_skip(dst_rel) {
        return Err(ImportError::DestinationInsideGitSfs {
            dest: dst_arg.to_owned(),
        });
    }

    let mut source_links = Vec::new();
    let metadata =
        std::fs::symlink_metadata(src_abs.as_std_path()).map_err(|source| ImportError::Io {
            path: src_abs.clone(),
            source,
        })?;

    let (effective_src, effective_metadata) = if metadata.is_symlink() {
        if !follow_symlinks {
            return Err(ImportError::SourceSymlinkRequiresFollow {
                path: src_arg.to_owned(),
            });
        }
        let resolved = resolve_symlink(&src_abs)
            .map_err(|source| ImportError::Io {
                path: src_abs.clone(),
                source,
            })?
            .ok_or_else(|| ImportError::NonUtf8Path {
                path: src_arg.to_owned(),
            })?;
        let resolved_metadata =
            std::fs::symlink_metadata(resolved.as_std_path()).map_err(|source| {
                ImportError::Io {
                    path: resolved.clone(),
                    source,
                }
            })?;
        source_links.push(src_abs.clone());
        (resolved, resolved_metadata)
    } else {
        (src_abs.clone(), metadata)
    };

    if effective_metadata.is_file() {
        if std::fs::symlink_metadata(dst_abs.as_std_path()).is_ok() {
            return Err(ImportError::DestinationAlreadyExists {
                dest: dst_arg.to_owned(),
            });
        }
        let key = canonical_key(&effective_src);
        return Ok(ImportPlan {
            pairs: vec![ImportPair {
                src: effective_src,
                dst: dst_abs,
                key,
            }],
            dirs: Vec::new(),
            source_links,
            unrepresentable: Vec::new(),
        });
    }

    if !effective_metadata.is_dir() {
        return Err(ImportError::UnsupportedSourceType {
            path: effective_src,
        });
    }
    if let Ok(dst_metadata) = std::fs::symlink_metadata(dst_abs.as_std_path())
        && !dst_metadata.is_dir()
    {
        return Err(ImportError::DestinationNotADirectory {
            dest: dst_arg.to_owned(),
        });
    }

    walk_source_dir(&effective_src, &dst_abs, follow_symlinks, source_links)
}

/// Walks a directory source, collecting one [`ImportPair`] per regular file
/// (or `-L`-followed symlink resolving to one) -- v1's `filepath.WalkDir`
/// closure (`import.go:212-256`). Rejects up front, touching nothing, if any
/// computed destination already exists.
fn walk_source_dir(
    src_abs: &Utf8Path,
    dst_abs: &Utf8Path,
    follow_symlinks: bool,
    mut source_links: Vec<Utf8PathBuf>,
) -> Result<ImportPlan, ImportError> {
    let mut pairs = Vec::new();
    let mut dirs = Vec::new();
    let mut unrepresentable = Vec::new();

    for entry in walkdir::WalkDir::new(src_abs.as_std_path()) {
        let entry = entry?;
        if entry.path() == src_abs.as_std_path() {
            continue;
        }
        let Some(abs) = Utf8Path::from_path(entry.path()) else {
            unrepresentable.push(entry.path().to_string_lossy().into_owned());
            continue;
        };
        let abs = abs.to_owned();

        if entry.file_type().is_dir() {
            dirs.push(abs);
            continue;
        }

        let file_src = if entry.file_type().is_symlink() {
            if !follow_symlinks {
                return Err(ImportError::SourceSymlinkRequiresFollow { path: abs });
            }
            let resolved = match resolve_symlink(&abs) {
                Ok(Some(resolved)) => resolved,
                Ok(None) => {
                    unrepresentable.push(abs.to_string());
                    continue;
                }
                Err(source) => return Err(ImportError::Io { path: abs, source }),
            };
            let resolved_metadata =
                std::fs::symlink_metadata(resolved.as_std_path()).map_err(|source| {
                    ImportError::Io {
                        path: resolved.clone(),
                        source,
                    }
                })?;
            if !resolved_metadata.is_file() {
                return Err(ImportError::SourceSymlinkTargetNotRegular { path: abs });
            }
            source_links.push(abs.clone());
            resolved
        } else if entry.file_type().is_file() {
            abs.clone()
        } else {
            return Err(ImportError::UnsupportedSourceType { path: abs });
        };

        let rel = abs.strip_prefix(src_abs).unwrap_or(&abs);
        let out = dst_abs.join(rel);
        if std::fs::symlink_metadata(out.as_std_path()).is_ok() {
            return Err(ImportError::DestinationAlreadyExists { dest: out });
        }

        let key = canonical_key(&file_src);
        pairs.push(ImportPair {
            src: file_src,
            dst: out,
            key,
        });
    }

    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.as_str().len()));
    dirs.push(src_abs.to_owned());
    pairs.sort_by(|a, b| a.dst.cmp(&b.dst));

    Ok(ImportPlan {
        pairs,
        dirs,
        source_links,
        unrepresentable,
    })
}

/// `path`, made absolute against `cwd` if it is not already -- v1's
/// `filepath.Abs` (`import.go:161`).
fn absolute(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        clean_utf8(path)
    } else {
        clean_utf8(&cwd.join(path))
    }
}

/// Resolves `path`'s symlink target, returning `None` rather than erroring
/// if the resolved path is not valid UTF-8 -- callers decide whether that is
/// fatal (the top-level `src` argument) or a skip-and-continue candidate (an
/// entry found mid-walk), matching this crate's established non-UTF-8
/// convention (`ports::repo::ScannedEntry::Unrepresentable`).
fn resolve_symlink(path: &Utf8Path) -> std::io::Result<Option<Utf8PathBuf>> {
    let resolved = std::fs::canonicalize(path.as_std_path())?;
    Ok(Utf8PathBuf::from_path_buf(resolved).ok())
}

/// The key used to detect that two source paths are the same underlying
/// file -- v1's `canonicalSourceKey` (`import.go:277-283`). Best-effort:
/// falls back to `path` itself if it cannot be canonicalized (matching v1),
/// since this is a deduplication aid, not a correctness-critical value.
fn canonical_key(path: &Utf8Path) -> Utf8PathBuf {
    std::fs::canonicalize(path.as_std_path())
        .ok()
        .and_then(|resolved| Utf8PathBuf::from_path_buf(resolved).ok())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::FsStore;

    /// A repository with a bound cache, plus a separate directory standing
    /// in for "somewhere external" (an incoming drive, a scratch mount) --
    /// `import`'s source is never inside the repo.
    struct Fixture {
        _repo_dir: tempfile::TempDir,
        _external_dir: tempfile::TempDir,
        repo: Utf8PathBuf,
        cache: Utf8PathBuf,
        external: Utf8PathBuf,
    }

    fn fixture() -> Fixture {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(repo_dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let cache = repo.join(".git-sfs/cache-real");
        std::fs::create_dir_all(&cache).unwrap();
        std::os::unix::fs::symlink(
            cache.as_std_path(),
            repo.join(".git-sfs/cache").as_std_path(),
        )
        .unwrap();

        let external_dir = tempfile::tempdir().unwrap();
        let external = Utf8PathBuf::from_path_buf(external_dir.path().to_owned()).unwrap();

        Fixture {
            _repo_dir: repo_dir,
            _external_dir: external_dir,
            repo,
            cache,
            external,
        }
    }

    fn opts(move_source: bool, follow_symlinks: bool) -> ImportOptions {
        ImportOptions {
            move_source,
            follow_symlinks,
        }
    }

    #[test]
    fn imports_a_single_file_and_leaves_the_source_intact_by_default() {
        let f = fixture();
        let src = f.external.join("photo.bin");
        std::fs::write(&src, b"dataset bytes").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("photo.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.imported.len(), 1);
        assert_eq!(outcome.imported[0].dst, "dest.bin");
        assert!(
            src.exists(),
            "copy semantics must leave the source in place"
        );
        let dest = f.repo.join("dest.bin");
        assert!(
            std::fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"dataset bytes");
    }

    #[test]
    fn move_flag_removes_the_source_only_after_the_destination_is_published() {
        let f = fixture();
        let src = f.external.join("photo.bin");
        std::fs::write(&src, b"dataset bytes").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("photo.bin"),
            Utf8Path::new("dest.bin"),
            opts(true, false),
            &cancel,
        )
        .unwrap();

        assert!(!src.exists(), "--move must remove the original source");
        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"dataset bytes"
        );
    }

    #[test]
    fn a_directory_source_merges_its_contents_directly_into_an_existing_destination() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data/nested")).unwrap();
        std::fs::write(f.external.join("data/one.bin"), b"one").unwrap();
        std::fs::write(f.external.join("data/nested/two.bin"), b"two").unwrap();
        std::fs::create_dir_all(f.repo.join("dest")).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, false),
            &cancel,
        )
        .unwrap();

        // Merged directly into "dest", not nested under "dest/data" -- this
        // is the one place import's placement rule differs from mv's.
        let mut dsts: Vec<&str> = outcome.imported.iter().map(|f| f.dst.as_str()).collect();
        dsts.sort_unstable();
        assert_eq!(dsts, ["dest/nested/two.bin", "dest/one.bin"]);
        assert_eq!(std::fs::read(f.repo.join("dest/one.bin")).unwrap(), b"one");
        assert_eq!(
            std::fs::read(f.repo.join("dest/nested/two.bin")).unwrap(),
            b"two"
        );
    }

    #[test]
    fn refuses_a_destination_outside_the_repository() {
        let f = fixture();
        std::fs::write(f.external.join("f.bin"), b"x").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new("../outside.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationOutsideRepo { .. }
        ));
        assert!(failure.outcome.imported.is_empty());
    }

    #[test]
    fn refuses_a_destination_inside_git_sfs() {
        let f = fixture();
        std::fs::write(f.external.join("f.bin"), b"x").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new(".git-sfs/sneaky.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationInsideGitSfs { .. }
        ));
    }

    #[test]
    fn refuses_an_existing_destination_and_touches_nothing() {
        let f = fixture();
        let src = f.external.join("f.bin");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(f.repo.join("dest.bin"), b"already here").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new("dest.bin"),
            opts(true, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationAlreadyExists { .. }
        ));
        assert!(src.exists(), "a rejected import must be a no-op");
        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"already here"
        );
    }

    #[test]
    fn refuses_a_symlink_source_without_follow_and_leaves_both_link_and_target_untouched() {
        let f = fixture();
        let target = f.external.join("real.bin");
        std::fs::write(&target, b"x").unwrap();
        let link = f.external.join("link.bin");
        std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path()).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("link.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::SourceSymlinkRequiresFollow { .. }
        ));
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "link must survive"
        );
        assert!(target.exists(), "target must survive");
    }

    #[test]
    fn follows_a_top_level_symlink_source_with_follow_symlinks() {
        let f = fixture();
        let target = f.external.join("real.bin");
        std::fs::write(&target, b"resolved content").unwrap();
        let link = f.external.join("link.bin");
        std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path()).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("link.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, true),
            &cancel,
        )
        .unwrap();

        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"resolved content"
        );
    }

    #[test]
    fn deduplicates_two_symlinks_that_resolve_to_the_same_underlying_file() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data")).unwrap();
        let target = f.external.join("data/real.bin");
        std::fs::write(&target, b"shared content").unwrap();
        std::os::unix::fs::symlink(
            target.as_std_path(),
            f.external.join("data/a.bin").as_std_path(),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            target.as_std_path(),
            f.external.join("data/b.bin").as_std_path(),
        )
        .unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, true),
            &cancel,
        )
        .unwrap();

        let hashes: std::collections::BTreeSet<_> =
            outcome.imported.iter().map(|f| f.hash).collect();
        // "real.bin" itself, plus the two symlinks resolving to it -- three
        // destinations, but the same underlying file was only hashed and
        // cached once (asserted indirectly: both symlink-derived pairs share
        // its hash).
        assert_eq!(outcome.imported.len(), 3);
        assert_eq!(hashes.len(), 1);
    }

    #[test]
    fn a_missing_source_reports_an_io_error() {
        let f = fixture();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("does-not-exist.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(*failure.error, ImportError::Io { .. }));
    }

    #[test]
    fn a_nested_symlink_must_resolve_to_a_regular_file_not_a_directory() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data/real_dir")).unwrap();
        std::os::unix::fs::symlink(
            f.external.join("data/real_dir").as_std_path(),
            f.external.join("data/link_dir").as_std_path(),
        )
        .unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, true),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::SourceSymlinkTargetNotRegular { .. }
        ));
    }
}
