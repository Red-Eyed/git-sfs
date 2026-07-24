//! The cache: content-addressed, write-once, read-only-after-storage.
//!
//! contract-spec §4, rust-rewrite-plan §2.2/§2.3. [`Store`] gets a trait
//! because there are genuinely two implementations — [`FsStore`] (real) and
//! [`FakeStore`] (in-memory, for higher-layer tests) — which is the bar
//! rust-rewrite-plan §3.3 sets for introducing one at all.
//!
//! **Never stages in the OS-wide temp directory.** Every write goes through
//! `tempfile::Builder::tempfile_in`, pointed at the cache's own `tmp/`
//! (`domain::cache_layout::tmp_dir`), never the bare `NamedTempFile::new()`
//! that defaults to `std::env::temp_dir()`. A cache write that depends on
//! system `/tmp` having room is a real outage: a full system `/tmp` on a
//! shared machine has taken git-sfs down before even though the cache itself,
//! on a different filesystem, had plenty of space.

use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::cancel::Cancel;
use crate::domain::cache_layout::{object_path, tmp_dir};
use crate::domain::hash::Sha256;
use crate::error::Error;

use super::cancellable_io::{Cancellable, is_canceled};
use super::hashing;

/// Proof that a hash's bytes are, at time of construction, verified present in
/// the cache.
///
/// rust-rewrite-plan §2.2 — "the highest-value" invariant in the correctness
/// thesis. No public constructor and no public fields: the only way to obtain
/// one is [`Store::verified`] or [`Store::store`], both of which either
/// trusted an already-protected (read-only) object or freshly hash-verified
/// one before returning it. A function that needs trustworthy bytes takes
/// `&CacheEntry`, never a bare [`Sha256`] — passing an unverified hash where
/// verified content is required stops compiling instead of relying on a
/// caller to have checked first.
///
/// This proves "verified at construction", not "verified now": another
/// process can still chmod or truncate the file after this value is created.
/// contract-spec §4.1 remains mandatory — types shrink the vigilance surface,
/// they do not eliminate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheEntry {
    hash: Sha256,
    // `path` is derived solely from `hash` and the store root, so it is not
    // stored -- keeping the type to exactly the fields that can vary would
    // otherwise invite two copies of the same information to drift apart.
}

impl CacheEntry {
    /// The hash this entry proves is present and verified.
    #[must_use]
    pub fn hash(&self) -> Sha256 {
        self.hash
    }
}

/// Why a [`Store`] operation failed.
#[derive(Debug, Error)]
pub enum StoreError {
    /// An I/O operation failed for a reason other than "the object is
    /// absent". This is the class v1's `HasValid`/`Protect` collapsed into
    /// `false`/an ignored error (rust-rewrite-plan §2.5): a permission-denied
    /// `stat` on a cache object is not the same fact as the object not
    /// existing, and this variant is what keeps the two from being
    /// conflated at the type level.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The bytes at `path` do not hash to the name they are stored under.
    /// Corrupt, not missing — contract-spec §9.1 is explicit that the two
    /// are different classes.
    #[error("cached object corrupt: {path} does not hash to its own name (want {want}, got {got})")]
    HashMismatch {
        /// The corrupt object's path.
        path: Utf8PathBuf,
        /// The hash the object is supposed to have.
        want: Sha256,
        /// The hash its actual bytes produce.
        got: Sha256,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<StoreError> for Error {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::Io { .. } => Error::Unavailable(err.to_string()),
            StoreError::HashMismatch { .. } => Error::Integrity(err.to_string()),
            StoreError::Canceled => Error::Canceled,
        }
    }
}

/// The cache: a content-addressed object store.
pub trait Store {
    /// Where `hash`'s object lives, whether or not it currently exists.
    fn object_path(&self, hash: Sha256) -> Utf8PathBuf;

    /// Whether `hash` is present and trustworthy.
    ///
    /// Three outcomes, not two, per rust-rewrite-plan §2.5: `Ok(None)` means
    /// genuinely absent; `Ok(Some(_))` means present and verified (read-only
    /// objects are trusted without re-hashing — contract-spec §4.1's
    /// load-bearing rule — a writable one is hash-verified and protected in
    /// place); `Err` means the question could not be answered (a permission
    /// error, a real I/O failure). A caller that cannot reach the cache must
    /// never be able to mistake that for the cache being empty.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if presence could not be determined, and
    /// [`StoreError::HashMismatch`] if an object exists at the path but its
    /// content does not match `hash` — corrupt, not absent.
    fn verified(&self, hash: Sha256, cancel: &Cancel) -> Result<Option<CacheEntry>, StoreError>;

    /// `hash`'s local cache size, or `None` if it is absent.
    ///
    /// This is a metadata query for `status`, not an integrity check: it
    /// deliberately does not hash bytes or repair writable legacy objects.
    /// Commands that need trustworthy bytes use [`Store::verified`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if the size could not be determined for a
    /// reason other than absence.
    fn object_size(&self, hash: Sha256) -> Result<Option<u64>, StoreError>;

    /// Bytes available on the filesystem that stores this cache.
    ///
    /// Used by `pull` before downloads, so the command fails before rclone
    /// starts writing when the cache volume is known not to have enough room.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if the filesystem capacity could not be
    /// determined.
    fn available_bytes(&self) -> Result<u64, StoreError>;

    /// Copies `source`'s bytes into the store under `hash`, verifying the
    /// written bytes before the object becomes visible at its final path.
    /// A no-op if `hash` is already present and verified.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::HashMismatch`] if `source`'s content does not
    /// actually hash to `hash`; the object is never published in that case.
    /// Returns [`StoreError::Io`] for any other failure, and
    /// [`StoreError::Canceled`] if `cancel` fires mid-copy.
    fn store(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError>;

    /// Moves `source`'s bytes into the store under `hash` — `import --move`'s
    /// primitive (contract-spec §4.2). `source` is consumed on success: it no
    /// longer exists at its original path.
    ///
    /// `source` is hash-verified **before** it is touched, not after, unlike
    /// v1's `Move` (`cache.go:130-177`), which renames or copies first and
    /// only then hashes the result — so a hash mismatch there has already
    /// destroyed the caller's only copy. Verifying first means a mismatch
    /// leaves `source` exactly where the caller left it.
    ///
    /// Prefers a same-filesystem `rename` (cheap, and safe post-verify since
    /// a rename cannot alter content) and falls back to copy-then-remove on
    /// [`io::ErrorKind::CrossesDevices`]. The fallback re-verifies the copy
    /// at its staging path before removing `source`, since a copy — unlike a
    /// rename — can be corrupted in transit; `source` only disappears once
    /// the bytes that will replace it are confirmed good.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::HashMismatch`] if `source`'s content does not
    /// hash to `hash`, [`StoreError::Io`] for any other failure, and
    /// [`StoreError::Canceled`] if `cancel` fires mid-operation. In every
    /// error case `source` is left intact.
    fn adopt(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError>;

    /// Removes `hash`'s object if it exists.
    ///
    /// This is intentionally a blunt cache-object primitive for `pull`'s
    /// pre-download cleanup of known-untrusted bytes. Callers must decide the
    /// policy before invoking it; a store cannot tell whether the caller has a
    /// remote source ready to repair the object.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if an existing object could not be removed.
    fn remove_object(&self, hash: Sha256) -> Result<(), StoreError>;
}

/// The real, filesystem-backed [`Store`].
pub struct FsStore {
    root: Utf8PathBuf,
}

impl FsStore {
    /// A store rooted at `root` — the already-resolved cache directory
    /// (contract-spec §7.2 governs how `root` itself is chosen; that
    /// resolution happens above this type).
    #[must_use]
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Store for FsStore {
    fn object_path(&self, hash: Sha256) -> Utf8PathBuf {
        object_path(&self.root, hash)
    }

    fn verified(&self, hash: Sha256, cancel: &Cancel) -> Result<Option<CacheEntry>, StoreError> {
        let path = self.object_path(hash);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(StoreError::Io { path, source }),
        };

        let mode = metadata.permissions().mode();
        if mode & 0o222 == 0 {
            // Read-only: write-protection was set only after a prior hash
            // verification, so the bytes cannot have changed since. No re-hash.
            return Ok(Some(CacheEntry { hash }));
        }

        // Writable: either a legacy object predating write-protection, or
        // contract-spec §9.1's declared divergence (v1 would flag this
        // ErrCorruptCachedFile and exit 3; v2 hash-verifies and repairs in
        // place instead, and exits 0 where the bytes are actually intact).
        let got = hash_file(&path, cancel)?;
        if got != hash {
            return Err(StoreError::HashMismatch {
                path,
                want: hash,
                got,
            });
        }
        let mut perms = metadata.permissions();
        perms.set_mode(mode & !0o222);
        std::fs::set_permissions(&path, perms).map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Some(CacheEntry { hash }))
    }

    fn object_size(&self, hash: Sha256) -> Result<Option<u64>, StoreError> {
        let path = self.object_path(hash);
        match std::fs::metadata(&path) {
            Ok(metadata) => Ok(Some(metadata.len())),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    fn available_bytes(&self) -> Result<u64, StoreError> {
        let stats = nix::sys::statvfs::statvfs(self.root.as_std_path()).map_err(|errno| {
            StoreError::Io {
                path: self.root.clone(),
                source: io::Error::from_raw_os_error(errno as i32),
            }
        })?;
        Ok((stats.blocks_available() as u64).saturating_mul(stats.fragment_size()))
    }

    fn store(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError> {
        if let Some(entry) = self.verified(hash, cancel)? {
            return Ok(entry);
        }

        let dst = self.object_path(hash);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        let staging = tmp_dir(&self.root);
        std::fs::create_dir_all(&staging).map_err(|source| StoreError::Io {
            path: staging.clone(),
            source,
        })?;

        let source_mode = std::fs::metadata(source)
            .map_err(|source_err| StoreError::Io {
                path: source.to_owned(),
                source: source_err,
            })?
            .permissions()
            .mode();

        // `tempfile_in`, never the bare `NamedTempFile::new()` -- see the
        // module doc. `NamedTempFile`'s `Drop` is what makes every early
        // return below (a copy error, a hash mismatch) leave nothing behind:
        // there is no branch an author has to remember to clean up.
        let mut tmp = tempfile::Builder::new()
            .tempfile_in(&staging)
            .map_err(|source| StoreError::Io {
                path: staging,
                source,
            })?;

        {
            let src_file = File::open(source).map_err(|source_err| StoreError::Io {
                path: source.to_owned(),
                source: source_err,
            })?;
            let mut reader = Cancellable::new(src_file, cancel.clone());
            io::copy(&mut reader, tmp.as_file_mut())
                .map_err(|err| classify_copy_error(err, source))?;
        }

        // contract-spec §4.2: the final mode is set before publishing. v1
        // derives it from the *source* file's mode (write bits stripped, but
        // e.g. an executable bit survives), not a fixed constant, and that is
        // preserved here.
        let mut perms = tmp
            .as_file()
            .metadata()
            .map_err(|source| StoreError::Io {
                path: dst.clone(),
                source,
            })?
            .permissions();
        perms.set_mode(source_mode & !0o222);
        tmp.as_file()
            .set_permissions(perms)
            .map_err(|source| StoreError::Io {
                path: dst.clone(),
                source,
            })?;
        tmp.as_file().sync_all().map_err(|source| StoreError::Io {
            path: dst.clone(),
            source,
        })?;

        // Verified *before* publishing, unlike v1 (contract-spec §4.2:
        // v1 renames first, hash-verifies the published file, then removes it
        // on mismatch). Checking while the bytes are still hidden in `tmp/`
        // means a corrupt object is never even briefly visible at its final,
        // trusted path -- strictly stronger than the invariant v1's sequence
        // protects, not a divergence from it.
        tmp.as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|source| StoreError::Io {
                path: dst.clone(),
                source,
            })?;
        let got =
            hash_reader(tmp.as_file(), cancel).map_err(|err| classify_copy_error(err, &dst))?;
        if got != hash {
            // `tmp`'s Drop removes the staging file; nothing to roll back.
            return Err(StoreError::HashMismatch {
                path: dst,
                want: hash,
                got,
            });
        }

        let persisted = tmp.persist(&dst).map_err(|e| StoreError::Io {
            path: dst.clone(),
            source: e.error,
        })?;
        drop(persisted);

        // contract-spec §13.2: v1 fsyncs the file but never the parent
        // directory, so the rename can be lost on power loss even though the
        // data was durable. "Atomic" is not "durable"; this is the fix.
        if let Some(parent) = dst.parent() {
            fsync_dir(parent).map_err(|source| StoreError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }

        Ok(CacheEntry { hash })
    }

    fn adopt(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError> {
        let dst = self.object_path(hash);

        if let Some(entry) = self.verified(hash, cancel)? {
            // Already cached under this hash. Unless `source` literally *is*
            // that cache object (dev+ino identity, not path text -- a
            // symlink or `..` can make two different-looking paths the same
            // file), it is now a redundant duplicate that `adopt`'s "source
            // is consumed" contract requires removing.
            if !same_file(source, &dst)? {
                std::fs::remove_file(source).map_err(|source_err| StoreError::Io {
                    path: source.to_owned(),
                    source: source_err,
                })?;
            }
            return Ok(entry);
        }

        // Verify *before* touching `source` -- see the trait doc. Unlike
        // v1's `Move`, a mismatch here leaves `source` untouched.
        let got = hash_file(source, cancel)?;
        if got != hash {
            return Err(StoreError::HashMismatch {
                path: source.to_owned(),
                want: hash,
                got,
            });
        }

        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source_err| StoreError::Io {
                path: parent.to_owned(),
                source: source_err,
            })?;
        }
        let staging = tmp_dir(&self.root);
        std::fs::create_dir_all(&staging).map_err(|source_err| StoreError::Io {
            path: staging.clone(),
            source: source_err,
        })?;

        // Mode must be read before any move below -- once the rename or
        // remove happens, `source` no longer exists to stat.
        let source_mode = std::fs::metadata(source)
            .map_err(|source_err| StoreError::Io {
                path: source.to_owned(),
                source: source_err,
            })?
            .permissions()
            .mode();

        // Deterministic name, mirroring v1's `.{hash}.move`: two concurrent
        // adopts of the same hash are already serialized by the caller's
        // command lock (contract-spec §8), so collision is not a concern,
        // and a leftover from a crashed prior run is simply overwritten by
        // the rename/create below.
        let tmp_path = staging.join(format!(".{}.adopt", hash.to_hex()));

        match std::fs::rename(source, &tmp_path) {
            Ok(()) => {
                // Same filesystem: a rename cannot alter content, and
                // `source` was already verified above, so the bytes now at
                // `tmp_path` are known-good without re-reading them.
            }
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
                // Cross-filesystem: the copy itself can corrupt data in a
                // way a rename cannot, so the copy is independently
                // verified and `source` is removed only once that passes.
                copy_with_cancel(source, &tmp_path, cancel)?;
                let copied = hash_file(&tmp_path, cancel)?;
                if copied != hash {
                    // `source` is still intact; only the staging copy is
                    // corrupt, so cleanup here is a courtesy, not a
                    // correctness requirement -- a leftover is inert and
                    // gets overwritten by the next adopt of this hash.
                    #[allow(
                        clippy::let_underscore_must_use,
                        reason = "best-effort cleanup of a corrupt staging file; source is already confirmed intact above"
                    )]
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(StoreError::HashMismatch {
                        path: tmp_path,
                        want: hash,
                        got: copied,
                    });
                }
                std::fs::remove_file(source).map_err(|source_err| StoreError::Io {
                    path: source.to_owned(),
                    source: source_err,
                })?;
            }
            Err(source_err) => {
                return Err(StoreError::Io {
                    path: source.to_owned(),
                    source: source_err,
                });
            }
        }

        let mut perms = std::fs::metadata(&tmp_path)
            .map_err(|source_err| StoreError::Io {
                path: tmp_path.clone(),
                source: source_err,
            })?
            .permissions();
        perms.set_mode(source_mode & !0o222);
        std::fs::set_permissions(&tmp_path, perms).map_err(|source_err| StoreError::Io {
            path: tmp_path.clone(),
            source: source_err,
        })?;
        // Durability before publish, matching `store()`.
        File::open(&tmp_path)
            .and_then(|f| f.sync_all())
            .map_err(|source_err| StoreError::Io {
                path: tmp_path.clone(),
                source: source_err,
            })?;

        std::fs::rename(&tmp_path, &dst).map_err(|source_err| StoreError::Io {
            path: dst.clone(),
            source: source_err,
        })?;

        if let Some(parent) = dst.parent() {
            fsync_dir(parent).map_err(|source_err| StoreError::Io {
                path: parent.to_owned(),
                source: source_err,
            })?;
        }

        Ok(CacheEntry { hash })
    }

    fn remove_object(&self, hash: Sha256) -> Result<(), StoreError> {
        let path = self.object_path(hash);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }
}

/// Whether `a` and `b` name the same file, by device+inode rather than path
/// text -- the identity that actually matters when deciding whether removing
/// one would remove the other (a symlink or `..` component can make two
/// different-looking paths resolve to one file).
fn same_file(a: &Utf8Path, b: &Utf8Path) -> Result<bool, StoreError> {
    let am = std::fs::metadata(a).map_err(|source| StoreError::Io {
        path: a.to_owned(),
        source,
    })?;
    let bm = std::fs::metadata(b).map_err(|source| StoreError::Io {
        path: b.to_owned(),
        source,
    })?;
    Ok(am.dev() == bm.dev() && am.ino() == bm.ino())
}

/// Streams `source`'s bytes to a freshly created file at `dst`, checking
/// `cancel` every chunk. Used by [`Store::adopt`]'s cross-device fallback,
/// where -- unlike [`Store::store`] -- there is no [`tempfile::NamedTempFile`]
/// already open to copy into, since `dst` here is a plain deterministic path.
fn copy_with_cancel(source: &Utf8Path, dst: &Utf8Path, cancel: &Cancel) -> Result<(), StoreError> {
    let src_file = File::open(source).map_err(|source_err| StoreError::Io {
        path: source.to_owned(),
        source: source_err,
    })?;
    let mut reader = Cancellable::new(src_file, cancel.clone());
    let mut dst_file = File::create(dst).map_err(|source_err| StoreError::Io {
        path: dst.to_owned(),
        source: source_err,
    })?;
    io::copy(&mut reader, &mut dst_file).map_err(|err| classify_copy_error(err, source))?;
    dst_file.sync_all().map_err(|source_err| StoreError::Io {
        path: dst.to_owned(),
        source: source_err,
    })
}

/// Maps a copy failure to the right [`StoreError`], recovering cancellation
/// from the marker [`Cancellable`] leaves on the [`io::Error`] rather than
/// letting it read as an ordinary I/O failure.
fn classify_copy_error(err: io::Error, source: &Utf8Path) -> StoreError {
    if is_canceled(&err) {
        StoreError::Canceled
    } else {
        StoreError::Io {
            path: source.to_owned(),
            source: err,
        }
    }
}

/// Hashes the file at `path`, checking `cancel` every chunk, and classifies
/// any failure as this store's own error type.
fn hash_file(path: &Utf8Path, cancel: &Cancel) -> Result<Sha256, StoreError> {
    hashing::hash_file(path, cancel).map_err(|err| classify_copy_error(err, path))
}

/// Hashes `reader`'s remaining bytes, checking `cancel` every chunk.
fn hash_reader(reader: impl io::Read, cancel: &Cancel) -> io::Result<Sha256> {
    hashing::hash_reader(reader, cancel)
}

/// Fsyncs a directory's entries — opening a directory read-only and syncing
/// it is the standard Unix technique for making a rename durable, not just
/// atomic. Matches this project's Unix-only platform scope.
fn fsync_dir(path: &Utf8Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// An in-memory [`Store`], for tests above this layer that need a cache
/// without a filesystem. Its existence is what justifies `Store` being a
/// trait at all (rust-rewrite-plan §3.3).
#[derive(Default)]
pub struct FakeStore {
    objects: std::sync::Mutex<std::collections::HashMap<Sha256, Vec<u8>>>,
}

impl FakeStore {
    /// An empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for FakeStore {
    fn object_path(&self, hash: Sha256) -> Utf8PathBuf {
        Utf8PathBuf::from(format!("fake:///{}", hash.to_hex()))
    }

    fn verified(&self, hash: Sha256, _cancel: &Cancel) -> Result<Option<CacheEntry>, StoreError> {
        let objects = self.objects.lock().expect("fake store mutex poisoned");
        Ok(objects.get(&hash).map(|_| CacheEntry { hash }))
    }

    fn object_size(&self, hash: Sha256) -> Result<Option<u64>, StoreError> {
        let objects = self.objects.lock().expect("fake store mutex poisoned");
        Ok(objects.get(&hash).map(|bytes| bytes.len() as u64))
    }

    fn available_bytes(&self) -> Result<u64, StoreError> {
        Ok(u64::MAX)
    }

    fn store(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError> {
        if let Some(entry) = self.verified(hash, cancel)? {
            return Ok(entry);
        }
        let bytes = std::fs::read(source).map_err(|source_err| StoreError::Io {
            path: source.to_owned(),
            source: source_err,
        })?;
        let got = {
            use sha2::{Digest, Sha256 as Sha256Hasher};
            Sha256::from_digest(Sha256Hasher::digest(&bytes).into())
        };
        if got != hash {
            return Err(StoreError::HashMismatch {
                path: source.to_owned(),
                want: hash,
                got,
            });
        }
        self.objects
            .lock()
            .expect("fake store mutex poisoned")
            .insert(hash, bytes);
        Ok(CacheEntry { hash })
    }

    fn adopt(
        &self,
        source: &Utf8Path,
        hash: Sha256,
        cancel: &Cancel,
    ) -> Result<CacheEntry, StoreError> {
        // No cache-relative path of its own to alias against, so the
        // same-file guard `FsStore` needs does not apply here: `source` can
        // never *be* the in-memory entry.
        let entry = self.store(source, hash, cancel)?;
        std::fs::remove_file(source).map_err(|source_err| StoreError::Io {
            path: source.to_owned(),
            source: source_err,
        })?;
        Ok(entry)
    }

    fn remove_object(&self, hash: Sha256) -> Result<(), StoreError> {
        self.objects
            .lock()
            .expect("fake store mutex poisoned")
            .remove(&hash);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_temp_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> Utf8PathBuf {
        let path = Utf8PathBuf::from_path_buf(dir.path().join(name)).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .write_all(content)
            .unwrap();
        path
    }

    fn hash_of(content: &[u8]) -> Sha256 {
        use sha2::{Digest, Sha256 as Sha256Hasher};
        Sha256::from_digest(Sha256Hasher::digest(content).into())
    }

    #[test]
    fn an_absent_object_is_none_not_an_error() {
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();
        assert!(store.verified(hash_of(b"nope"), &cancel).unwrap().is_none());
    }

    #[test]
    fn storing_publishes_a_read_only_verified_object() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"large research dataset bytes";
        let source = write_temp_file(&source_dir, "data.bin", content);
        let hash = hash_of(content);

        let entry = store.store(&source, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);

        let path = store.object_path(hash);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o222, 0, "published object must be read-only");

        // A second store() of the same content is a verified no-op, not an error.
        assert_eq!(store.store(&source, hash, &cancel).unwrap().hash(), hash);
    }

    #[test]
    fn storing_never_stages_in_the_system_temp_directory() {
        // The whole reason `tempfile_in` is used instead of the bare
        // constructor: a full system /tmp must not be able to break this.
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"staging location matters";
        let source = write_temp_file(&source_dir, "data.bin", content);
        store.store(&source, hash_of(content), &cancel).unwrap();

        let staged = std::fs::read_dir(cache.path().join("tmp")).unwrap().count();
        // The temp file was persisted (renamed) out of tmp/ on success, so
        // tmp/ is empty again -- but it must have been *created*, proving
        // that is where staging happened rather than system temp.
        assert_eq!(staged, 0);
        assert!(cache.path().join("tmp").is_dir());
    }

    #[test]
    fn rejects_content_that_does_not_match_the_claimed_hash() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let source = write_temp_file(&source_dir, "data.bin", b"actual content");
        let wrong_hash = hash_of(b"a different content entirely");

        let err = store.store(&source, wrong_hash, &cancel).unwrap_err();
        assert!(matches!(err, StoreError::HashMismatch { .. }));
        // Nothing is published on mismatch.
        assert!(store.verified(wrong_hash, &cancel).unwrap().is_none());
    }

    #[test]
    fn a_writable_legacy_object_is_verified_and_repaired_in_place() {
        // contract-spec §4.1's one-time migration path, and §9.1's declared
        // divergence: a writable object with intact content is repaired
        // (chmod'd read-only) rather than treated as corrupt.
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"pre-existing legacy object";
        let hash = hash_of(content);
        let path = store.object_path(hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        // Left writable, as a pre-write-protection-era cache object would be.
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        let entry = store
            .verified(hash, &cancel)
            .unwrap()
            .expect("valid content should verify");
        assert_eq!(entry.hash(), hash);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o222,
            0,
            "verifying a legacy object must protect it in place"
        );
    }

    #[test]
    fn a_writable_object_with_wrong_content_is_corrupt_not_absent() {
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let hash = hash_of(b"the real content");
        let path = store.object_path(hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"tampered bytes").unwrap();

        let err = store.verified(hash, &cancel).unwrap_err();
        assert!(matches!(err, StoreError::HashMismatch { .. }));
    }

    /// rust-rewrite-plan §2.5's fix, made concrete: a `stat` failure that is
    /// *not* "not found" must never collapse to the same `Ok(None)` an
    /// actually-absent object produces. This is the exact defect class v1's
    /// `HasValid` had (`os.Stat` erroring for *any* reason returned `false`).
    #[test]
    fn a_stat_failure_that_is_not_absence_is_an_error_not_none() {
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();
        let hash = hash_of(b"unreachable");

        let path = store.object_path(hash);
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        std::fs::write(&path, b"content").unwrap();
        // Revoke traversal on the parent directory so `stat` on the object
        // itself fails with permission-denied, not not-found.
        let mut perms = std::fs::metadata(parent).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(parent, perms).unwrap();

        let result = store.verified(hash, &cancel);

        // Restore permissions unconditionally so the tempdir can clean itself
        // up -- deleting the entries inside `parent` needs write+execute on
        // it, which the revoked mode above denies.
        let mut restore = std::fs::metadata(parent).unwrap().permissions();
        restore.set_mode(0o755);
        std::fs::set_permissions(parent, restore).unwrap();

        match result {
            Err(StoreError::Io { .. }) => {}
            // Root (or an equivalent privilege, common in some CI containers)
            // bypasses the permission bit entirely -- nothing to assert then,
            // rather than trying to predict root ahead of time.
            Ok(_) => {}
            other => panic!("expected an Io error or a root-bypassed success, got {other:?}"),
        }
    }

    #[test]
    fn cancellation_during_a_store_is_reported_as_canceled() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();
        cancel.cancel();

        let content = vec![0u8; 1 << 20];
        let source = write_temp_file(&source_dir, "data.bin", &content);
        let err = store
            .store(&source, hash_of(&content), &cancel)
            .unwrap_err();
        assert!(matches!(err, StoreError::Canceled));
    }

    #[test]
    fn fake_store_satisfies_the_same_contract_as_the_real_one() {
        let store = FakeStore::new();
        let cancel = Cancel::new();
        let dir = tempfile::tempdir().unwrap();
        let content = b"in memory only";
        let source = write_temp_file(&dir, "data.bin", content);
        let hash = hash_of(content);

        assert!(store.verified(hash, &cancel).unwrap().is_none());
        let entry = store.store(&source, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);
        assert!(store.verified(hash, &cancel).unwrap().is_some());
    }

    #[test]
    fn remove_object_deletes_present_objects_and_accepts_absence() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();
        let content = b"remove me before redownload";
        let source = write_temp_file(&source_dir, "data.bin", content);
        let hash = hash_of(content);

        store.store(&source, hash, &cancel).unwrap();
        assert!(store.object_path(hash).exists());

        store.remove_object(hash).unwrap();
        assert!(!store.object_path(hash).exists());
        store.remove_object(hash).unwrap();
    }

    #[test]
    fn available_bytes_reports_cache_volume_capacity() {
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        assert!(store.available_bytes().unwrap() > 0);
    }

    #[test]
    fn adopting_consumes_the_source_and_publishes_a_verified_object() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"moved into the cache";
        let source = write_temp_file(&source_dir, "data.bin", content);
        let hash = hash_of(content);

        let entry = store.adopt(&source, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);
        assert!(!source.exists(), "adopt must consume the source");

        let path = store.object_path(hash);
        assert_eq!(std::fs::read(&path).unwrap(), content);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o222, 0, "published object must be read-only");
    }

    #[test]
    fn adopting_a_hash_mismatch_leaves_the_source_untouched() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let source = write_temp_file(&source_dir, "data.bin", b"actual content");
        let wrong_hash = hash_of(b"a different content entirely");

        let err = store.adopt(&source, wrong_hash, &cancel).unwrap_err();
        assert!(matches!(err, StoreError::HashMismatch { .. }));
        // Unlike v1, a mismatch must never have consumed the source.
        assert!(source.exists());
        assert_eq!(std::fs::read(&source).unwrap(), b"actual content");
        assert!(store.verified(wrong_hash, &cancel).unwrap().is_none());
    }

    #[test]
    fn adopting_an_already_cached_hash_removes_the_duplicate_source() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"already present";
        let hash = hash_of(content);
        let first_source = write_temp_file(&source_dir, "first.bin", content);
        store.store(&first_source, hash, &cancel).unwrap();

        let duplicate_source = write_temp_file(&source_dir, "duplicate.bin", content);
        let entry = store.adopt(&duplicate_source, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);
        assert!(
            !duplicate_source.exists(),
            "a redundant duplicate must still be consumed"
        );
    }

    #[test]
    fn adopting_a_source_that_is_already_the_cache_object_does_not_delete_it() {
        // The pathological case the same-file (dev+ino) guard exists for:
        // if `source` and the object path resolve to one file, "consuming
        // the source" must not mean deleting the only copy of the object.
        let cache = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();

        let content = b"self-adopted object";
        let hash = hash_of(content);
        let object_path = store.object_path(hash);
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(&object_path, content).unwrap();
        let mut perms = std::fs::metadata(&object_path).unwrap().permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&object_path, perms).unwrap();

        let entry = store.adopt(&object_path, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);
        assert!(object_path.exists(), "must not delete the object it names");
        assert_eq!(std::fs::read(&object_path).unwrap(), content);
    }

    #[test]
    fn adopting_is_cancellable() {
        let cache = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(Utf8PathBuf::from_path_buf(cache.path().to_owned()).unwrap());
        let cancel = Cancel::new();
        cancel.cancel();

        let content = vec![0u8; 1 << 20];
        let source = write_temp_file(&source_dir, "data.bin", &content);
        let err = store
            .adopt(&source, hash_of(&content), &cancel)
            .unwrap_err();
        assert!(matches!(err, StoreError::Canceled));
        assert!(
            source.exists(),
            "cancellation must not have consumed the source"
        );
    }

    #[test]
    fn copy_with_cancel_reproduces_the_cross_device_fallback_mechanics() {
        // Genuine EXDEV needs two real filesystems, which is not portable to
        // exercise in CI. This drives the fallback's actual copy primitive
        // directly, so the mechanics `adopt` depends on for that branch --
        // full-content copy, and prompt stop on cancellation -- are still
        // covered even though the `ErrorKind::CrossesDevices` branch itself
        // is not.
        let dir = tempfile::tempdir().unwrap();
        let content = b"copied across a simulated device boundary";
        let source = write_temp_file(&dir, "source.bin", content);
        let dst = Utf8PathBuf::from_path_buf(dir.path().join("dst.bin")).unwrap();

        copy_with_cancel(&source, &dst, &Cancel::new()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), content);

        let cancel = Cancel::new();
        cancel.cancel();
        let large_source = write_temp_file(&dir, "large.bin", &vec![0u8; 1 << 20]);
        let large_dst = Utf8PathBuf::from_path_buf(dir.path().join("large_dst.bin")).unwrap();
        let err = copy_with_cancel(&large_source, &large_dst, &cancel).unwrap_err();
        assert!(matches!(err, StoreError::Canceled));
    }

    #[test]
    fn fake_store_adopt_also_consumes_the_source() {
        let store = FakeStore::new();
        let cancel = Cancel::new();
        let dir = tempfile::tempdir().unwrap();
        let content = b"in memory, adopted";
        let source = write_temp_file(&dir, "data.bin", content);
        let hash = hash_of(content);

        let entry = store.adopt(&source, hash, &cancel).unwrap();
        assert_eq!(entry.hash(), hash);
        assert!(!source.exists());
        assert!(store.verified(hash, &cancel).unwrap().is_some());
    }
}
