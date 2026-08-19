//! `git-sfs push`: upload referenced cache objects to the remote.
//!
//! This layer composes existing ports and the pure [`crate::plan::plan_push`]
//! function. It does not decide how to render warnings or progress; callers get
//! a [`PushOutcome`] and print it at the CLI edge.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::{ALGORITHM, Sha256};
use crate::error::Error;
use crate::plan::{PlanPushError, SkippedObject, TrackedLink, plan_push};
use crate::ports::{Remote, RemoteError, Repo, RepoError, ScannedEntry, Store, StoreError};

/// What `push` completed or planned before any failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOutcome {
    /// Objects handed to the remote for upload.
    pub uploaded: Vec<Sha256>,
    /// Objects intentionally omitted under `--skip-missing`.
    pub skipped: Vec<SkippedObject>,
}

/// A `push` failure together with the outcome known before it failed.
#[derive(Debug)]
pub struct PushFailure {
    /// Work discovered or completed before the failure.
    pub outcome: PushOutcome,
    /// The failure itself.
    pub error: Box<PushError>,
}

/// Why `push` failed.
#[derive(Debug, Error)]
pub enum PushError {
    /// The repository scan failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// Local cache verification failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Planning refused to proceed.
    #[error(transparent)]
    Plan(#[from] PlanPushError),
    /// Remote upload failed.
    #[error(transparent)]
    Remote(#[from] RemoteError),
}

impl From<PushError> for Error {
    fn from(err: PushError) -> Self {
        match err {
            PushError::Repo(RepoError::Canceled)
            | PushError::Store(StoreError::Canceled)
            | PushError::Remote(RemoteError::Canceled) => Error::Canceled,
            PushError::Repo(err) => Error::from(err),
            PushError::Store(err) => Error::from(err),
            PushError::Plan(PlanPushError::MissingCachedFile { .. }) => {
                Error::Missing(err.to_string())
            }
            PushError::Remote(err) => Error::from(err),
        }
    }
}

/// Uploads cache objects referenced by git-sfs symlinks at or below `scope`.
///
/// Invalid symlinks are ignored here, matching the compatibility behavior.
/// `collectGitSFSSymlinks`; `verify` reports them instead.
///
/// # Errors
///
/// Returns [`PushFailure`] when the remote upload fails after planning, so the
/// caller can still report skipped objects. Earlier failures are returned with
/// an empty/default outcome.
pub fn push(
    repo: &dyn Repo,
    store: &dyn Store,
    remote: &dyn Remote,
    cache_files_dir: &Utf8Path,
    scope: &Utf8Path,
    skip_missing: bool,
    cancel: &Cancel,
) -> Result<PushOutcome, PushFailure> {
    let links = tracked_links(repo.scan(scope, cancel)?);
    let present = verified_hashes(store, &links, cancel)?;
    let plan = plan_push(&links, &present, skip_missing)?;
    let outcome = PushOutcome {
        uploaded: plan.upload.clone(),
        skipped: plan.skipped,
    };

    let rel_paths = plan
        .upload
        .iter()
        .map(|hash| remote_rel_path(*hash))
        .collect::<Vec<_>>();
    // One batched rclone transfer. `copy_to_remote` writes these paths to a
    // temporary `--files-from` list; it must not become one subprocess per
    // object.
    remote
        .copy_to_remote(cache_files_dir, &rel_paths, cancel)
        .map_err(|error| PushFailure {
            outcome: outcome.clone(),
            error: Box::new(PushError::Remote(error)),
        })?;
    Ok(outcome)
}

fn tracked_links(entries: Vec<ScannedEntry>) -> Vec<TrackedLink> {
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            ScannedEntry::Tracked { path, hash } => Some(TrackedLink { path, hash }),
            ScannedEntry::Invalid { .. } | ScannedEntry::Unrepresentable { .. } => None,
        })
        .collect()
}

fn verified_hashes(
    store: &dyn Store,
    links: &[TrackedLink],
    cancel: &Cancel,
) -> Result<BTreeSet<Sha256>, PushError> {
    let mut present = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for link in links {
        if !seen.insert(link.hash) {
            continue;
        }
        if store.verified(link.hash, cancel)?.is_some() {
            present.insert(link.hash);
        }
    }
    Ok(present)
}

fn remote_rel_path(hash: Sha256) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash.to_hex()))
}

impl From<RepoError> for PushFailure {
    fn from(error: RepoError) -> Self {
        Self {
            outcome: empty_outcome(),
            error: Box::new(PushError::Repo(error)),
        }
    }
}

impl From<PushError> for PushFailure {
    fn from(error: PushError) -> Self {
        Self {
            outcome: empty_outcome(),
            error: Box::new(error),
        }
    }
}

impl From<StoreError> for PushFailure {
    fn from(error: StoreError) -> Self {
        Self {
            outcome: empty_outcome(),
            error: Box::new(PushError::Store(error)),
        }
    }
}

impl From<PlanPushError> for PushFailure {
    fn from(error: PlanPushError) -> Self {
        Self {
            outcome: empty_outcome(),
            error: Box::new(PushError::Plan(error)),
        }
    }
}

fn empty_outcome() -> PushOutcome {
    PushOutcome {
        uploaded: Vec::new(),
        skipped: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use crate::domain::symlink::git_link_target;
    use crate::ports::{FakeRemote, FakeRepo, FakeStore};

    use super::*;

    fn hash_bytes(bytes: &[u8]) -> Sha256 {
        use sha2::{Digest, Sha256 as Sha256Hasher};
        Sha256::from_digest(Sha256Hasher::digest(bytes).into())
    }

    fn seed_link(repo: &FakeRepo, path: &str, hash: Sha256) {
        let root = Utf8Path::new("/repo");
        let file = root.join(path);
        let target = git_link_target(root, &file, hash).unwrap();
        repo.seed(path, target.to_string());
    }

    fn store_bytes(store: &FakeStore, hash: Sha256, bytes: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("object")).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        store.store(&path, hash, &Cancel::new()).unwrap();
    }

    fn write_cache_file(cache_files_dir: &Utf8Path, hash: Sha256, bytes: &[u8]) {
        let rel = remote_rel_path(hash);
        let path = cache_files_dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn uploads_each_verified_hash_once() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_files_dir = Utf8PathBuf::from_path_buf(cache.path().join("files")).unwrap();
        let cancel = Cancel::new();
        let bytes = b"dataset bytes";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "a.bin", hash);
        seed_link(&repo, "copy.bin", hash);
        store_bytes(&store, hash, bytes);
        write_cache_file(&cache_files_dir, hash, bytes);

        let outcome = push(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            false,
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.uploaded, vec![hash]);
        assert!(outcome.skipped.is_empty());
        assert_eq!(
            remote.file_sizes(&[hash], &cancel).unwrap().get(&hash),
            Some(&(bytes.len() as u64))
        );
    }

    #[test]
    fn missing_cache_file_fails_by_default_with_the_first_path() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_files_dir = Utf8PathBuf::from_path_buf(cache.path().join("files")).unwrap();
        let cancel = Cancel::new();
        let hash = Sha256::from_digest([7; 32]);
        seed_link(&repo, "a.bin", hash);

        let failure = push(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            false,
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            PushError::Plan(PlanPushError::MissingCachedFile { .. })
        ));
    }

    #[test]
    fn skip_missing_uploads_present_objects_and_reports_skipped_objects() {
        let repo = FakeRepo::new("/repo");
        let store = FakeStore::new();
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_files_dir = Utf8PathBuf::from_path_buf(cache.path().join("files")).unwrap();
        let cancel = Cancel::new();
        let present_bytes = b"present";
        let present = hash_bytes(present_bytes);
        let missing = Sha256::from_digest([8; 32]);
        seed_link(&repo, "missing.bin", missing);
        seed_link(&repo, "present.bin", present);
        store_bytes(&store, present, present_bytes);
        write_cache_file(&cache_files_dir, present, present_bytes);

        let outcome = push(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            true,
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.uploaded, vec![present]);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].hash, missing);
        assert_eq!(
            outcome.skipped[0].paths,
            vec![Utf8PathBuf::from("missing.bin")]
        );
        assert_eq!(
            remote
                .file_sizes(&[present], &cancel)
                .unwrap()
                .get(&present),
            Some(&(present_bytes.len() as u64))
        );
    }
}
