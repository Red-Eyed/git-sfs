//! `add` — ported from `add.go`. Hashes each regular file found under the
//! given paths, stores it in the cache, and replaces it with a git-sfs
//! symlink.
//!
//! Deliberately minimal for this pass:
//!
//! - **Sequential.** No rayon-based parallelism yet — that is a performance
//!   layer on top of correct sequential behavior, not part of getting the
//!   operation right the first time.
//! - **No progress callback.** rust-rewrite-plan §3.2: core does not take a
//!   writer or a callback so a caller can observe it; Phase 5's `Event`
//!   stream is the real mechanism, added uniformly across every command
//!   rather than ad hoc here. [`add`] returning the outcome-so-far alongside
//!   an error (rather than only one or the other) is what lets the binary
//!   still report partial progress without it.
//! - **Stops at the first failure**, matching v1's own observed behavior
//!   (`add.go:73-79`): files are hashed and stored best-effort, but the loop
//!   that then publishes symlinks returns on the first error rather than
//!   skipping past it. Re-running `add` afterward is safe and effectively
//!   resumes: [`crate::ports::Store::store`] is idempotent, so already-cached
//!   objects are a fast no-op the second time.
//! - **Relative path arguments resolve against the repository root, not the
//!   current directory** — matching v1's `absFromRepo(repo, p)` exactly
//!   (`add.go:47`, `walk.go:80-85`). This is a real usability quirk (`cd
//!   data && git-sfs add foo.bin` looks for `<repo>/foo.bin`, not
//!   `<repo>/data/foo.bin`) worth reconsidering, but changing it is an
//!   argument-semantics decision, not a mechanical port, so it is preserved
//!   here rather than silently changed.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::domain::symlink::{NoRelativePath, cache_link_file, git_link_target};
use crate::error::Error;
use crate::ports::repo::FoundEntry;
use crate::ports::{Repo, RepoError, Store, StoreError, hash_file};

/// One file `add` finished converting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedFile {
    /// Repo-relative path that was converted.
    pub path: Utf8PathBuf,
    /// The hash it is now tracked under.
    pub hash: Sha256,
}

/// What an [`add`] run produced, whether or not it ultimately succeeded.
#[derive(Debug, Default)]
pub struct AddOutcome {
    /// Every file successfully converted, in the order they were processed.
    pub added: Vec<AddedFile>,
    /// Candidates whose own filename is not valid UTF-8 — skipped, not
    /// converted, but reported rather than silently dropped (mirrors
    /// [`crate::ports::repo::ScannedEntry::Unrepresentable`]).
    pub unrepresentable: Vec<String>,
}

/// Why `add` failed.
#[derive(Debug, Error)]
pub enum AddError {
    /// Walking one of the given paths failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// Hashing or storing `path` failed.
    #[error("{path}: {source}")]
    Store {
        /// The file being processed.
        path: Utf8PathBuf,
        /// Why storing it failed.
        #[source]
        source: StoreError,
    },
    /// `path` hashed and stored, but its cache object is not reachable
    /// through the repository's own `.git-sfs/cache` symlink — publishing a
    /// working-tree symlink through it would just be dangling on arrival.
    /// Mirrors v1's `materialize.Link` sanity check (`materialize.go:11-20`).
    #[error("{path}: cache object for {hash} is not reachable through .git-sfs/cache")]
    CacheLinkUnreachable {
        /// The file being processed.
        path: Utf8PathBuf,
        /// The hash it was stored under.
        hash: Sha256,
    },
    /// `path` and the repository root disagree on absolute-vs-relative, so
    /// no symlink target could be computed for it. Not reachable in
    /// practice: both are always absolute by the time they reach here.
    #[error("{path}: {source}")]
    NoRelativePath {
        /// The file being processed.
        path: Utf8PathBuf,
        /// The underlying error.
        #[source]
        source: NoRelativePath,
    },
    /// Removing the original file or writing its replacement symlink
    /// failed.
    #[error("{path}: {source}")]
    Io {
        /// The file being processed.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<AddError> for Error {
    fn from(err: AddError) -> Self {
        match &err {
            // A RepoError::Canceled reaching here means find_files noticed
            // the cancellation before add's own per-file loop got a chance
            // to; it must still classify as Canceled, not Unavailable --
            // cancellation outranks every other classification (see the
            // Error::Canceled doc).
            AddError::Repo(RepoError::Canceled) | AddError::Canceled => Error::Canceled,
            AddError::Repo(_) | AddError::Store { .. } | AddError::Io { .. } => {
                Error::Unavailable(err.to_string())
            }
            AddError::CacheLinkUnreachable { .. } | AddError::NoRelativePath { .. } => {
                Error::Config(err.to_string())
            }
        }
    }
}

/// [`add`]'s outcome-so-far, together with why it stopped — returned
/// instead of a bare [`AddError`] so a caller can still report partial
/// progress on failure.
#[derive(Debug)]
pub struct AddFailure {
    /// What succeeded before `error` stopped the run.
    pub outcome: AddOutcome,
    /// Why it stopped. Boxed because `AddError`'s largest variant carries a
    /// full `StoreError`, and clippy's `result_large_err` correctly flags an
    /// unboxed `Result<_, AddFailure>` as bloating every `Ok` return path
    /// with that size even on success.
    pub error: Box<AddError>,
}

impl AddFailure {
    fn new(outcome: AddOutcome, error: AddError) -> Self {
        Self {
            outcome,
            error: Box::new(error),
        }
    }
}

/// Hashes, stores, and symlinks every regular file found under `paths`.
///
/// `paths` are exactly the command-line arguments — see the module doc for
/// how a relative one resolves. `repo_port` finds the candidate files (and
/// applies the same `.git`/`.git-sfs` exclusions every other command
/// honors); `store` is where their bytes end up.
///
/// # Errors
///
/// Returns the outcome collected so far bundled with the first [`AddError`]
/// encountered, so a caller can report partial progress even on failure.
pub fn add(
    repo_port: &dyn Repo,
    store: &dyn Store,
    repo: &Utf8Path,
    paths: &[Utf8PathBuf],
    cancel: &Cancel,
) -> Result<AddOutcome, AddFailure> {
    let mut outcome = AddOutcome::default();
    let mut files = BTreeSet::new();
    for path in paths {
        // v1's absFromRepo: an absolute argument is used as-is, a relative
        // one resolves against the repository root -- see the module doc.
        let scope = if path.is_absolute() {
            path.clone()
        } else {
            repo.join(path)
        };
        let found = repo_port
            .find_files(&scope, cancel)
            .map_err(|err| AddFailure::new(AddOutcome::default(), AddError::Repo(err)))?;
        for entry in found {
            match entry {
                FoundEntry::File(rel_path) => {
                    files.insert(rel_path);
                }
                FoundEntry::Unrepresentable { description } => {
                    outcome.unrepresentable.push(description);
                }
            }
        }
    }

    for rel_path in files {
        if cancel.is_canceled() {
            return Err(AddFailure::new(outcome, AddError::Canceled));
        }
        match add_one(store, repo, &rel_path, cancel) {
            Ok(added) => outcome.added.push(added),
            Err(err) => return Err(AddFailure::new(outcome, err)),
        }
    }
    Ok(outcome)
}

/// Hashes, stores, and symlinks the single file at repo-relative `rel_path`.
fn add_one(
    store: &dyn Store,
    repo: &Utf8Path,
    rel_path: &Utf8Path,
    cancel: &Cancel,
) -> Result<AddedFile, AddError> {
    let abs_path = repo.join(rel_path);

    let hash = hash_file(&abs_path, cancel).map_err(|source| AddError::Io {
        path: rel_path.to_owned(),
        source,
    })?;

    store
        .store(&abs_path, hash, cancel)
        .map_err(|source| AddError::Store {
            path: rel_path.to_owned(),
            source,
        })?;

    // Sanity-check the repository's own cache symlink actually reaches the
    // object just stored, before publishing a working-tree symlink that
    // depends on it -- see AddError::CacheLinkUnreachable.
    if !cache_link_file(repo, hash).is_file() {
        return Err(AddError::CacheLinkUnreachable {
            path: rel_path.to_owned(),
            hash,
        });
    }

    let target =
        git_link_target(repo, &abs_path, hash).map_err(|source| AddError::NoRelativePath {
            path: rel_path.to_owned(),
            source,
        })?;

    std::fs::remove_file(&abs_path).map_err(|source| AddError::Io {
        path: rel_path.to_owned(),
        source,
    })?;
    std::os::unix::fs::symlink(target.as_std_path(), abs_path.as_std_path()).map_err(|source| {
        AddError::Io {
            path: rel_path.to_owned(),
            source,
        }
    })?;

    Ok(AddedFile {
        path: rel_path.to_owned(),
        hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{FakeStore, FsRepo, FsStore};

    fn init_repo() -> (tempfile::TempDir, Utf8PathBuf, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let cache = repo.join(".git-sfs/cache-real");
        std::fs::create_dir_all(&cache).unwrap();
        std::os::unix::fs::symlink(
            cache.as_std_path(),
            repo.join(".git-sfs/cache").as_std_path(),
        )
        .unwrap();

        (dir, repo, cache)
    }

    #[test]
    fn adds_a_single_file_and_replaces_it_with_a_valid_symlink() {
        let (_dir, repo, cache) = init_repo();
        std::fs::write(repo.join("data.bin"), b"large research dataset").unwrap();

        let repo_port = FsRepo::new(repo.clone());
        let store = FsStore::new(cache);
        let cancel = Cancel::new();

        let outcome = add(
            &repo_port,
            &store,
            &repo,
            &[Utf8PathBuf::from("data.bin")],
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.added.len(), 1);
        assert_eq!(outcome.added[0].path, "data.bin");
        assert!(outcome.unrepresentable.is_empty());

        let file_path = repo.join("data.bin");
        let metadata = std::fs::symlink_metadata(&file_path).unwrap();
        assert!(
            metadata.file_type().is_symlink(),
            "original file must become a symlink"
        );
        // Confirm the symlink actually resolves to real content, not just
        // that a symlink exists at the path.
        assert_eq!(
            std::fs::read(&file_path).unwrap(),
            b"large research dataset"
        );

        let hash = outcome.added[0].hash;
        assert_eq!(
            std::fs::read(cache_link_file(&repo, hash)).unwrap(),
            b"large research dataset"
        );
    }

    #[test]
    fn re_running_add_on_an_already_converted_file_is_a_harmless_noop() {
        let (_dir, repo, cache) = init_repo();
        std::fs::write(repo.join("data.bin"), b"content").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let store = FsStore::new(cache);
        let cancel = Cancel::new();
        let paths = [Utf8PathBuf::from("data.bin")];

        add(&repo_port, &store, &repo, &paths, &cancel).unwrap();
        // The file is now a symlink, so a second add over the same scope
        // finds no regular files at all -- nothing to do, not an error.
        let outcome = add(&repo_port, &store, &repo, &paths, &cancel).unwrap();
        assert!(outcome.added.is_empty());
    }

    #[test]
    fn adding_multiple_files_deduplicates_identical_content_but_converts_every_path() {
        let (_dir, repo, cache) = init_repo();
        std::fs::write(repo.join("a.bin"), b"same bytes").unwrap();
        std::fs::write(repo.join("b.bin"), b"same bytes").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let store = FsStore::new(cache);
        let cancel = Cancel::new();

        let outcome = add(
            &repo_port,
            &store,
            &repo,
            &[Utf8PathBuf::from("a.bin"), Utf8PathBuf::from("b.bin")],
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.added.len(), 2);
        assert_eq!(outcome.added[0].hash, outcome.added[1].hash);
    }

    #[test]
    fn a_missing_scope_returns_the_error_with_an_empty_outcome() {
        let (_dir, repo, cache) = init_repo();
        let repo_port = FsRepo::new(repo.clone());
        let store = FsStore::new(cache);
        let cancel = Cancel::new();

        let failure = add(
            &repo_port,
            &store,
            &repo,
            &[Utf8PathBuf::from("does/not/exist")],
            &cancel,
        )
        .unwrap_err();

        assert!(failure.outcome.added.is_empty());
        assert!(matches!(*failure.error, AddError::Repo(_)));
    }

    #[test]
    fn a_relative_path_argument_resolves_against_the_repo_root_not_the_scope_of_a_subdirectory() {
        // Documents the deliberately-preserved v1 quirk from the module doc:
        // "foo.bin" always means "<repo>/foo.bin", regardless of where the
        // caller conceptually "is".
        let (_dir, repo, cache) = init_repo();
        std::fs::create_dir_all(repo.join("subdir")).unwrap();
        std::fs::write(repo.join("subdir/foo.bin"), b"nested").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let store = FsStore::new(cache);
        let cancel = Cancel::new();

        let outcome = add(
            &repo_port,
            &store,
            &repo,
            &[Utf8PathBuf::from("subdir")],
            &cancel,
        )
        .unwrap();
        assert_eq!(outcome.added[0].path, "subdir/foo.bin");
    }

    #[test]
    fn stops_at_the_first_store_failure_but_reports_files_already_added() {
        // A FakeStore that always reports a hash mismatch simulates a store
        // failure without needing to corrupt a real filesystem mid-run.
        struct AlwaysCorruptStore(FakeStore);
        impl Store for AlwaysCorruptStore {
            fn object_path(&self, hash: Sha256) -> Utf8PathBuf {
                self.0.object_path(hash)
            }
            fn verified(
                &self,
                hash: Sha256,
                cancel: &Cancel,
            ) -> Result<Option<crate::ports::CacheEntry>, StoreError> {
                self.0.verified(hash, cancel)
            }
            fn store(
                &self,
                _source: &Utf8Path,
                hash: Sha256,
                _cancel: &Cancel,
            ) -> Result<crate::ports::CacheEntry, StoreError> {
                Err(StoreError::HashMismatch {
                    path: self.0.object_path(hash),
                    want: hash,
                    got: hash,
                })
            }
            fn adopt(
                &self,
                source: &Utf8Path,
                hash: Sha256,
                cancel: &Cancel,
            ) -> Result<crate::ports::CacheEntry, StoreError> {
                self.0.adopt(source, hash, cancel)
            }
        }

        let (_dir, repo, _cache) = init_repo();
        std::fs::write(repo.join("a.bin"), b"a").unwrap();
        let repo_port = FsRepo::new(repo.clone());
        let store = AlwaysCorruptStore(FakeStore::new());
        let cancel = Cancel::new();

        let failure = add(
            &repo_port,
            &store,
            &repo,
            &[Utf8PathBuf::from("a.bin")],
            &cancel,
        )
        .unwrap_err();

        assert!(failure.outcome.added.is_empty());
        assert!(matches!(*failure.error, AddError::Store { .. }));
    }
}
