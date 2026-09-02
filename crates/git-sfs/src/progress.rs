//! Binary-side progress rendering.
//!
//! Core owns command semantics; this module only decorates side-effecting ports
//! at the CLI edge so long-running rclone calls have terminal feedback without
//! teaching `git-sfs-core` about terminals, quiet mode, or `indicatif`.

use std::collections::HashMap;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use indicatif::{ProgressBar, ProgressStyle};

use git_sfs_core::Cancel;
use git_sfs_core::domain::Sha256;
use git_sfs_core::ports::{
    CacheEntry, FoundEntry, Remote, RemoteError, Repo, RepoError, ScannedEntry, Store, StoreError,
};

const TICK: Duration = Duration::from_millis(120);

type RemoteResult<T> = std::result::Result<T, RemoteError>;
type RepoResult<T> = std::result::Result<T, RepoError>;
type StoreResult<T> = std::result::Result<T, StoreError>;

pub(crate) fn with_spinner<T, E>(
    enabled: bool,
    message: impl Into<String>,
    work: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let bar = spinner(enabled, message);
    let result = work();
    bar.finish_and_clear();
    result
}

pub(crate) struct ProgressRepo<R> {
    inner: R,
    enabled: bool,
}

impl<R> ProgressRepo<R> {
    pub(crate) fn new(inner: R, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

impl<R: Repo> Repo for ProgressRepo<R> {
    fn scan(&self, scope: &Utf8Path, cancel: &Cancel) -> RepoResult<Vec<ScannedEntry>> {
        with_spinner(self.enabled, format!("scanning {scope}"), || {
            self.inner.scan(scope, cancel)
        })
    }

    fn find_files(&self, scope: &Utf8Path, cancel: &Cancel) -> RepoResult<Vec<FoundEntry>> {
        with_spinner(self.enabled, format!("finding files in {scope}"), || {
            self.inner.find_files(scope, cancel)
        })
    }
}

pub(crate) struct ProgressStore<S> {
    inner: S,
    enabled: bool,
}

impl<S> ProgressStore<S> {
    pub(crate) fn new(inner: S, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

impl<S: Store> Store for ProgressStore<S> {
    fn object_path(&self, hash: Sha256) -> Utf8PathBuf {
        self.inner.object_path(hash)
    }

    fn verified(&self, hash: Sha256, cancel: &Cancel) -> StoreResult<Option<CacheEntry>> {
        with_spinner(
            self.enabled,
            format!("checking cache object {}", hash.short()),
            || self.inner.verified(hash, cancel),
        )
    }

    fn rehash_object(&self, hash: Sha256, cancel: &Cancel) -> StoreResult<Option<CacheEntry>> {
        with_spinner(
            self.enabled,
            format!("verifying cache object {}", hash.short()),
            || self.inner.rehash_object(hash, cancel),
        )
    }

    fn object_size(&self, hash: Sha256) -> StoreResult<Option<u64>> {
        with_spinner(
            self.enabled,
            format!("checking cache object {}", hash.short()),
            || self.inner.object_size(hash),
        )
    }

    fn object_hashes(&self) -> StoreResult<Vec<Sha256>> {
        with_spinner(self.enabled, "listing cache objects", || {
            self.inner.object_hashes()
        })
    }

    fn available_bytes(&self) -> StoreResult<u64> {
        with_spinner(self.enabled, "checking cache free space", || {
            self.inner.available_bytes()
        })
    }

    fn store(&self, source: &Utf8Path, hash: Sha256, cancel: &Cancel) -> StoreResult<CacheEntry> {
        with_spinner(self.enabled, format!("storing {source}"), || {
            self.inner.store(source, hash, cancel)
        })
    }

    fn adopt(&self, source: &Utf8Path, hash: Sha256, cancel: &Cancel) -> StoreResult<CacheEntry> {
        with_spinner(self.enabled, format!("adopting {source}"), || {
            self.inner.adopt(source, hash, cancel)
        })
    }

    fn remove_object(&self, hash: Sha256) -> StoreResult<()> {
        with_spinner(
            self.enabled,
            format!("removing cache object {}", hash.short()),
            || self.inner.remove_object(hash),
        )
    }
}

pub(crate) struct ProgressRemote<R> {
    inner: R,
    enabled: bool,
}

impl<R> ProgressRemote<R> {
    pub(crate) fn new(inner: R, enabled: bool) -> Self {
        Self { inner, enabled }
    }

    fn step<T>(
        &self,
        message: impl Into<String>,
        work: impl FnOnce(&R) -> RemoteResult<T>,
    ) -> RemoteResult<T> {
        with_spinner(self.enabled, message, || work(&self.inner))
    }
}

impl<R: Remote> Remote for ProgressRemote<R> {
    fn check_backend(&self, cancel: &Cancel) -> RemoteResult<()> {
        self.step("checking remote backend", |inner| {
            inner.check_backend(cancel)
        })
    }

    fn check_path(&self, cancel: &Cancel) -> RemoteResult<()> {
        self.step("checking remote path", |inner| inner.check_path(cancel))
    }

    fn file_sizes(&self, hashes: &[Sha256], cancel: &Cancel) -> RemoteResult<HashMap<Sha256, u64>> {
        self.step(
            format!("checking {} remote object(s)", hashes.len()),
            |inner| inner.file_sizes(hashes, cancel),
        )
    }

    fn copy_to_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> RemoteResult<()> {
        self.step(
            format!("pushing {} remote object(s)", rel_paths.len()),
            |inner| inner.copy_to_remote(cache_files_dir, rel_paths, cancel),
        )
    }

    fn copy_from_remote(
        &self,
        cache_files_dir: &Utf8Path,
        rel_paths: &[Utf8PathBuf],
        cancel: &Cancel,
    ) -> RemoteResult<()> {
        self.step(
            format!("pulling {} remote object(s)", rel_paths.len()),
            |inner| inner.copy_from_remote(cache_files_dir, rel_paths, cancel),
        )
    }
}

fn spinner(enabled: bool, message: impl Into<String>) -> ProgressBar {
    let bar = if enabled {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    };
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg} ({elapsed_precise})")
            .expect("static progress style template is valid"),
    );
    bar.set_message(message.into());
    bar.enable_steady_tick(TICK);
    bar
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use git_sfs_core::ports::{FakeRepo, FakeStore};

    use super::*;

    fn hash_bytes(bytes: &[u8]) -> Sha256 {
        use sha2::{Digest as _, Sha256 as Sha256Hasher};
        Sha256::from_digest(Sha256Hasher::digest(bytes).into())
    }

    #[derive(Default)]
    struct RecordingRemote {
        calls: Mutex<Vec<String>>,
    }

    impl RecordingRemote {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("recording mutex poisoned").clone()
        }

        fn record(&self, call: &str) {
            self.calls
                .lock()
                .expect("recording mutex poisoned")
                .push(call.to_owned());
        }
    }

    impl Remote for RecordingRemote {
        fn check_backend(&self, _cancel: &Cancel) -> RemoteResult<()> {
            self.record("check_backend");
            Ok(())
        }

        fn check_path(&self, _cancel: &Cancel) -> RemoteResult<()> {
            self.record("check_path");
            Ok(())
        }

        fn file_sizes(
            &self,
            hashes: &[Sha256],
            _cancel: &Cancel,
        ) -> RemoteResult<HashMap<Sha256, u64>> {
            self.record("file_sizes");
            Ok(hashes.iter().map(|hash| (*hash, 7)).collect())
        }

        fn copy_to_remote(
            &self,
            _cache_files_dir: &Utf8Path,
            _rel_paths: &[Utf8PathBuf],
            _cancel: &Cancel,
        ) -> RemoteResult<()> {
            self.record("copy_to_remote");
            Ok(())
        }

        fn copy_from_remote(
            &self,
            _cache_files_dir: &Utf8Path,
            _rel_paths: &[Utf8PathBuf],
            _cancel: &Cancel,
        ) -> RemoteResult<()> {
            self.record("copy_from_remote");
            Ok(())
        }
    }

    #[test]
    fn delegates_remote_operations_without_changing_results() {
        let remote = ProgressRemote::new(RecordingRemote::default(), false);
        let cancel = Cancel::new();
        let hash =
            Sha256::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap();
        let cache_files_dir = Utf8Path::new("/cache/files");
        let rel_paths = vec![Utf8PathBuf::from("sha256/aa/aaaaaaaa")];

        assert!(remote.check_backend(&cancel).is_ok());
        assert!(remote.check_path(&cancel).is_ok());
        assert_eq!(
            remote.file_sizes(&[hash], &cancel).unwrap().get(&hash),
            Some(&7)
        );
        assert!(
            remote
                .copy_to_remote(cache_files_dir, &rel_paths, &cancel)
                .is_ok()
        );
        assert!(
            remote
                .copy_from_remote(cache_files_dir, &rel_paths, &cancel)
                .is_ok()
        );
        assert_eq!(
            remote.inner.calls(),
            [
                "check_backend",
                "check_path",
                "file_sizes",
                "copy_to_remote",
                "copy_from_remote"
            ]
        );
    }

    #[test]
    fn delegates_repo_operations_without_changing_results() {
        let repo = FakeRepo::new(Utf8PathBuf::from("/repo"));
        repo.seed_file("data/a.bin");
        let repo = ProgressRepo::new(repo, false);
        let cancel = Cancel::new();

        assert!(repo.scan(Utf8Path::new("."), &cancel).unwrap().is_empty());
        let found = repo.find_files(Utf8Path::new("."), &cancel).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path(), Some(Utf8Path::new("data/a.bin")));
    }

    #[test]
    fn delegates_store_operations_without_changing_results() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = Utf8PathBuf::from_path_buf(source_dir.path().join("data.bin")).unwrap();
        let bytes = b"dataset bytes";
        std::fs::write(&source, bytes).unwrap();
        let hash = hash_bytes(bytes);
        let store = ProgressStore::new(FakeStore::new(), false);
        let cancel = Cancel::new();

        assert!(store.verified(hash, &cancel).unwrap().is_none());
        assert_eq!(store.store(&source, hash, &cancel).unwrap().hash(), hash);
        assert!(store.verified(hash, &cancel).unwrap().is_some());
        assert_eq!(store.object_size(hash).unwrap(), Some(bytes.len() as u64));
        assert_eq!(store.object_hashes().unwrap(), vec![hash]);
        store.remove_object(hash).unwrap();
        assert!(store.verified(hash, &cancel).unwrap().is_none());
    }
}
