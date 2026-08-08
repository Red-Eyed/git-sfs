//! `git-sfs pull`: download missing cache objects from the remote.
//!
//! Like `push`, this composes existing ports and pure planning. The remote
//! work is intentionally batch-shaped: one metadata listing for disk-space
//! planning and one `copy_from_remote` transfer for the object list.

use std::collections::BTreeSet;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::hash::{ALGORITHM, Sha256};
use crate::error::Error;
use crate::plan::{
    InsufficientDiskSpace, TrackedLink, check_disk_space, plan_pull, sum_needed_bytes,
};
use crate::ports::{Remote, RemoteError, Repo, RepoError, ScannedEntry, Store, StoreError};

/// What `pull` completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOutcome {
    /// Objects downloaded and verified into the cache.
    pub downloaded: Vec<Sha256>,
}

/// Why `pull` failed.
#[derive(Debug, Error)]
pub enum PullError {
    /// The repository scan failed.
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// Local cache inspection, cleanup, or post-download verification failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Remote metadata or download failed.
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// The remote does not contain an object this working tree references.
    #[error("remote object missing: {hash}")]
    MissingRemoteObject {
        /// Hash that was needed but absent from the remote listing.
        hash: Sha256,
    },
    /// The cache volume lacks enough free space for the planned download.
    #[error(transparent)]
    DiskSpace(#[from] InsufficientDiskSpace),
    /// rclone reported success, but the cache object still was not present
    /// and verified afterwards.
    #[error("downloaded object missing from cache after pull: {hash}")]
    DownloadedObjectMissing {
        /// Hash that should have arrived.
        hash: Sha256,
    },
    /// Scratch-space setup for staged downloads failed.
    #[error("{path}: {source}")]
    Scratch {
        /// The path being prepared.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
}

impl From<PullError> for Error {
    fn from(err: PullError) -> Self {
        match err {
            PullError::Repo(RepoError::Canceled)
            | PullError::Store(StoreError::Canceled)
            | PullError::Remote(RemoteError::Canceled) => Error::Canceled,
            PullError::Repo(err) => Error::from(err),
            PullError::Store(err) => Error::from(err),
            PullError::Remote(err) => Error::from(err),
            PullError::MissingRemoteObject { .. } | PullError::DownloadedObjectMissing { .. } => {
                Error::Missing(err.to_string())
            }
            PullError::DiskSpace(err) => Error::Unavailable(err.to_string()),
            PullError::Scratch { .. } => Error::Unavailable(err.to_string()),
        }
    }
}

/// Downloads every cache object referenced by git-sfs symlinks at or below
/// `scope` that is not already verified locally.
///
/// Invalid symlinks are ignored here; `verify` reports them instead.
///
/// # Errors
///
/// Returns [`PullError`] if the repository cannot be scanned, local cache state
/// cannot be determined or repaired, the remote cannot satisfy the batch, or
/// post-download verification fails.
pub fn pull(
    repo: &dyn Repo,
    store: &dyn Store,
    remote: &dyn Remote,
    cache_files_dir: &Utf8Path,
    scope: &Utf8Path,
    cancel: &Cancel,
) -> Result<PullOutcome, PullError> {
    let links = tracked_links(repo.scan(scope, cancel)?);
    let cache_state = inspect_cache(store, &links, cancel)?;
    let plan = plan_pull(&links, &cache_state.present);
    if plan.download.is_empty() {
        return Ok(PullOutcome {
            downloaded: Vec::new(),
        });
    }

    let remote_sizes = remote.file_sizes(&plan.download, cancel)?;
    if let Some(&hash) = plan
        .download
        .iter()
        .find(|hash| !remote_sizes.contains_key(hash))
    {
        return Err(PullError::MissingRemoteObject { hash });
    }
    let needed = sum_needed_bytes(&plan.download, &remote_sizes);
    check_disk_space(needed, store.available_bytes()?)?;

    for hash in cache_state.untrusted {
        if plan.download.contains(&hash) {
            store.remove_object(hash)?;
        }
    }

    let rel_paths = plan
        .download
        .iter()
        .map(|hash| remote_rel_path(*hash))
        .collect::<Vec<_>>();
    let staged = staged_download(cache_files_dir)?;
    remote.copy_from_remote(&staged.files_dir, &rel_paths, cancel)?;

    for &hash in &plan.download {
        let staged_path = staged.files_dir.join(remote_rel_path(hash));
        if !staged_path.is_file() {
            return Err(PullError::DownloadedObjectMissing { hash });
        }
        store.adopt(&staged_path, hash, cancel)?;
        if store.verified(hash, cancel)?.is_none() {
            return Err(PullError::DownloadedObjectMissing { hash });
        }
    }
    Ok(PullOutcome {
        downloaded: plan.download,
    })
}

#[derive(Debug, Default)]
struct CacheState {
    present: BTreeSet<Sha256>,
    untrusted: BTreeSet<Sha256>,
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

fn inspect_cache(
    store: &dyn Store,
    links: &[TrackedLink],
    cancel: &Cancel,
) -> Result<CacheState, PullError> {
    let mut state = CacheState::default();
    let mut seen = BTreeSet::new();
    for link in links {
        if !seen.insert(link.hash) {
            continue;
        }
        match store.verified(link.hash, cancel) {
            Ok(Some(_)) => {
                state.present.insert(link.hash);
            }
            Ok(None) => {}
            Err(StoreError::HashMismatch { .. }) => {
                state.untrusted.insert(link.hash);
            }
            Err(error) => return Err(PullError::Store(error)),
        }
    }
    Ok(state)
}

fn remote_rel_path(hash: Sha256) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{ALGORITHM}/{}/{}", hash.prefix(), hash.to_hex()))
}

struct StagedDownload {
    _scratch: tempfile::TempDir,
    files_dir: Utf8PathBuf,
}

fn staged_download(cache_files_dir: &Utf8Path) -> Result<StagedDownload, PullError> {
    let scratch_root = cache_files_dir
        .parent()
        .map(|cache_root| cache_root.join("tmp"))
        .unwrap_or_else(|| Utf8PathBuf::from("tmp"));
    std::fs::create_dir_all(&scratch_root).map_err(|source| PullError::Scratch {
        path: scratch_root.clone(),
        source,
    })?;
    let scratch = tempfile::Builder::new()
        .prefix("git-sfs-pull-")
        .tempdir_in(&scratch_root)
        .map_err(|source| PullError::Scratch {
            path: scratch_root,
            source,
        })?;
    let files_dir = Utf8PathBuf::from_path_buf(scratch.path().join("files")).map_err(|path| {
        PullError::Scratch {
            path: Utf8PathBuf::from(path.to_string_lossy().into_owned()),
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "staged pull path is not valid UTF-8",
            ),
        }
    })?;
    std::fs::create_dir_all(&files_dir).map_err(|source| PullError::Scratch {
        path: files_dir.clone(),
        source,
    })?;
    Ok(StagedDownload {
        _scratch: scratch,
        files_dir,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    use crate::domain::symlink::git_link_target;
    use crate::ports::{FakeRemote, FakeRepo, FsStore};

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

    fn write_file(path: &Utf8Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::File::create(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    fn store_bytes(store: &FsStore, hash: Sha256, bytes: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("object")).unwrap();
        write_file(&path, bytes);
        store.store(&path, hash, &Cancel::new()).unwrap();
    }

    fn seed_remote(remote: &FakeRemote, cache_files_dir: &Utf8Path, hash: Sha256, bytes: &[u8]) {
        let rel = remote_rel_path(hash);
        write_file(&cache_files_dir.join(&rel), bytes);
        remote
            .copy_to_remote(cache_files_dir, &[rel], &Cancel::new())
            .unwrap();
        std::fs::remove_file(cache_files_dir.join(remote_rel_path(hash))).unwrap();
    }

    #[test]
    fn downloads_missing_objects_and_verifies_them() {
        let repo = FakeRepo::new("/repo");
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_root = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        let cache_files_dir = cache_root.join("files");
        let store = FsStore::new(cache_root);
        let bytes = b"remote-only object";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        seed_remote(&remote, &cache_files_dir, hash, bytes);

        let outcome = pull(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            &Cancel::new(),
        )
        .unwrap();

        assert_eq!(outcome.downloaded, vec![hash]);
        assert!(store.verified(hash, &Cancel::new()).unwrap().is_some());
    }

    #[test]
    fn already_verified_objects_are_not_downloaded() {
        let repo = FakeRepo::new("/repo");
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_root = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        let cache_files_dir = cache_root.join("files");
        let store = FsStore::new(cache_root);
        let bytes = b"already local";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        store_bytes(&store, hash, bytes);

        let outcome = pull(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            &Cancel::new(),
        )
        .unwrap();

        assert!(outcome.downloaded.is_empty());
    }

    #[test]
    fn corrupt_writable_object_is_removed_before_the_batch_download() {
        let repo = FakeRepo::new("/repo");
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_root = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        let cache_files_dir = cache_root.join("files");
        let store = FsStore::new(cache_root);
        let bytes = b"correct remote bytes";
        let hash = hash_bytes(bytes);
        seed_link(&repo, "data.bin", hash);
        seed_remote(&remote, &cache_files_dir, hash, bytes);

        let object_path = store.object_path(hash);
        write_file(&object_path, b"local rot");
        let mut perms = std::fs::metadata(&object_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&object_path, perms).unwrap();

        let outcome = pull(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            &Cancel::new(),
        )
        .unwrap();

        assert_eq!(outcome.downloaded, vec![hash]);
        assert_eq!(std::fs::read(store.object_path(hash)).unwrap(), bytes);
        assert!(store.verified(hash, &Cancel::new()).unwrap().is_some());
    }

    #[test]
    fn remote_absence_is_reported_before_download() {
        let repo = FakeRepo::new("/repo");
        let remote = FakeRemote::new();
        let cache = tempfile::tempdir().unwrap();
        let cache_root = Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap();
        let cache_files_dir = cache_root.join("files");
        let store = FsStore::new(cache_root);
        let hash = Sha256::from_digest([9; 32]);
        seed_link(&repo, "data.bin", hash);

        let error = pull(
            &repo,
            &store,
            &remote,
            &cache_files_dir,
            Utf8Path::new("."),
            &Cancel::new(),
        )
        .unwrap_err();

        assert!(matches!(error, PullError::MissingRemoteObject { hash: got } if got == hash));
    }
}
