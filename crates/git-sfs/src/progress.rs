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
use git_sfs_core::ports::{Remote, RemoteError};

const TICK: Duration = Duration::from_millis(120);

type RemoteResult<T> = std::result::Result<T, RemoteError>;

/// A `Remote` decorator that displays coarse progress for rclone operations.
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
        let bar = spinner(self.enabled, message);
        let result = work(&self.inner);
        bar.finish_and_clear();
        result
    }
}

impl<R: Remote> Remote for ProgressRemote<R> {
    fn require_exists(&self, cancel: &Cancel) -> RemoteResult<()> {
        self.step("checking remote", |inner| inner.require_exists(cancel))
    }

    fn check_backend(&self, cancel: &Cancel) -> RemoteResult<()> {
        self.step("checking remote backend", |inner| {
            inner.check_backend(cancel)
        })
    }

    fn check_path(&self, cancel: &Cancel) -> RemoteResult<()> {
        self.step("checking remote path", |inner| inner.check_path(cancel))
    }

    fn has_file(&self, hash: Sha256, cancel: &Cancel) -> RemoteResult<bool> {
        self.step("checking remote object", |inner| {
            inner.has_file(hash, cancel)
        })
    }

    fn file_size(&self, hash: Sha256, cancel: &Cancel) -> RemoteResult<Option<u64>> {
        self.step("checking remote object size", |inner| {
            inner.file_size(hash, cancel)
        })
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
            format!("uploading {} object(s)", rel_paths.len()),
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
            format!("downloading {} object(s)", rel_paths.len()),
            |inner| inner.copy_from_remote(cache_files_dir, rel_paths, cancel),
        )
    }

    fn verify_file(&self, hash: Sha256, cancel: &Cancel) -> RemoteResult<bool> {
        self.step("verifying remote object", |inner| {
            inner.verify_file(hash, cancel)
        })
    }
}

fn spinner(enabled: bool, message: impl Into<String>) -> ProgressBar {
    let bar = if enabled {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    };
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .expect("static progress style template is valid"),
    );
    bar.set_message(message.into());
    bar.enable_steady_tick(TICK);
    bar
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

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

        fn has_file(&self, _hash: Sha256, _cancel: &Cancel) -> RemoteResult<bool> {
            self.record("has_file");
            Ok(true)
        }

        fn file_size(&self, _hash: Sha256, _cancel: &Cancel) -> RemoteResult<Option<u64>> {
            self.record("file_size");
            Ok(Some(7))
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

        fn verify_file(&self, _hash: Sha256, _cancel: &Cancel) -> RemoteResult<bool> {
            self.record("verify_file");
            Ok(true)
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

        assert!(remote.require_exists(&cancel).is_ok());
        assert!(remote.has_file(hash, &cancel).unwrap());
        assert_eq!(remote.file_size(hash, &cancel).unwrap(), Some(7));
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
        assert!(remote.verify_file(hash, &cancel).unwrap());

        assert_eq!(
            remote.inner.calls(),
            [
                "check_backend",
                "check_path",
                "has_file",
                "file_size",
                "file_sizes",
                "copy_to_remote",
                "copy_from_remote",
                "verify_file"
            ]
        );
    }
}
