//! Explicit cache eviction. Selection completes before any object is unlinked.
//! Each unlink is atomic; a multi-object eviction is retryable, not transactional.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use crate::domain::hash::Sha256;
use crate::ports::repo::resolve_scope;
use crate::ports::{Repo, ScannedEntry, Store};
use crate::{Cancel, Error, Result};

/// One present cache object selected for eviction. Sizes come from metadata;
/// previewing never reads, verifies, or changes the object's bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct PreviewObject {
    /// Cache object path, rather than a symlink in the working tree.
    pub path: Utf8PathBuf,
    /// Logical file size in bytes; shared objects are counted once.
    pub size: u64,
}

/// Present objects that an eviction would remove. Missing objects are omitted.
#[derive(Debug, PartialEq, Eq)]
pub struct CachePreview {
    /// Deterministically ordered by object hash.
    pub objects: Vec<PreviewObject>,
}

impl CachePreview {
    /// Total logical bytes selected, without filesystem allocation estimates.
    #[must_use]
    pub fn total_bytes(&self) -> u128 {
        self.objects
            .iter()
            .map(|object| u128::from(object.size))
            .sum()
    }
}

/// Preview `rm` using its exact selection rules, without modifying the cache.
///
/// # Errors
/// Returns the same selection errors as [`rm`], plus cache metadata failures
/// and cancellation. Missing objects are not an error.
pub fn preview_rm(
    repo: &impl Repo,
    store: &impl Store,
    root: &Utf8Path,
    paths: &[Utf8PathBuf],
    cancel: &Cancel,
) -> Result<CachePreview> {
    preview_selected(store, select_rm(repo, root, paths, cancel)?, cancel)
}

/// Preview `prune` using its exact selection rules, without modifying the cache.
///
/// # Errors
/// Returns the same selection errors as [`prune`], plus cache metadata failures
/// and cancellation.
pub fn preview_prune(
    repo: &impl Repo,
    store: &impl Store,
    cancel: &Cancel,
) -> Result<CachePreview> {
    preview_selected(store, select_prune(repo, store, cancel)?, cancel)
}

/// Evict objects referenced by files or directories, leaving every symlink intact.
/// Paths are repository-relative, matching the other dataset commands.
///
/// # Errors
/// Rejects paths outside the working tree, invalid symlinks, unreadable scopes,
/// and scopes containing no git-sfs links before deleting anything. Missing
/// cache objects are harmless. I/O errors or cancellation during deletion can
/// leave a partially completed batch; rerunning is safe.
pub fn rm(
    repo: &impl Repo,
    store: &impl Store,
    root: &Utf8Path,
    paths: &[Utf8PathBuf],
    cancel: &Cancel,
) -> Result<usize> {
    remove_selected(store, select_rm(repo, root, paths, cancel)?, cancel)
}

fn select_rm(
    repo: &impl Repo,
    root: &Utf8Path,
    paths: &[Utf8PathBuf],
    cancel: &Cancel,
) -> Result<BTreeSet<Sha256>> {
    if paths.is_empty() {
        return Err(Error::Usage("cache rm requires at least one path".into()));
    }
    let mut selected = BTreeSet::new();
    for path in paths {
        cancel.check()?;
        let scope = resolve_scope(root, path);
        let relative = scope.strip_prefix(root).map_err(|_| {
            Error::Usage(format!("cache rm path is outside the repository: {path}"))
        })?;
        if relative.starts_with(".git") || relative.starts_with(".git-sfs") {
            return Err(Error::Usage(format!(
                "cache rm requires dataset paths: {path}"
            )));
        }
        let hashes = references(repo, &scope, cancel)?;
        if hashes.is_empty() {
            return Err(Error::Usage(format!("no git-sfs symlinks found: {path}")));
        }
        selected.extend(hashes);
    }
    Ok(selected)
}

/// Evict objects unreferenced by the current repository's working tree.
/// Unstaged links protect objects; other branches and repositories do not.
///
/// # Errors
/// Aborts before deletion if the reference scan or cache listing fails, or a
/// symlink cannot be validated. Deletion failures and cancellation can leave
/// a partially completed batch, with every remaining object's bytes intact.
pub fn prune(repo: &impl Repo, store: &impl Store, cancel: &Cancel) -> Result<usize> {
    remove_selected(store, select_prune(repo, store, cancel)?, cancel)
}

fn select_prune(repo: &impl Repo, store: &impl Store, cancel: &Cancel) -> Result<BTreeSet<Sha256>> {
    cancel.check()?;
    let referenced = references(repo, Utf8Path::new("."), cancel)?;
    let mut selected = BTreeSet::new();
    for hash in store.object_hashes()? {
        cancel.check()?;
        if !referenced.contains(&hash) {
            selected.insert(hash);
        }
    }
    Ok(selected)
}

fn references(repo: &impl Repo, scope: &Utf8Path, cancel: &Cancel) -> Result<BTreeSet<Sha256>> {
    let mut hashes = BTreeSet::new();
    for entry in repo.scan(scope, cancel)? {
        cancel.check()?;
        match entry {
            ScannedEntry::Tracked { hash, .. } => {
                hashes.insert(hash);
            }
            ScannedEntry::Invalid { path, reason } => {
                return Err(Error::Integrity(format!(
                    "cannot evict cache: {path}: {reason}"
                )));
            }
            ScannedEntry::Unrepresentable { description } => {
                return Err(Error::Integrity(format!(
                    "cannot evict cache: symlink path is not UTF-8: {description}"
                )));
            }
        }
    }
    Ok(hashes)
}

fn preview_selected(
    store: &impl Store,
    selected: BTreeSet<Sha256>,
    cancel: &Cancel,
) -> Result<CachePreview> {
    let mut objects = Vec::new();
    for hash in selected {
        cancel.check()?;
        if let Some(size) = store.object_size(hash)? {
            objects.push(PreviewObject {
                path: store.object_path(hash),
                size,
            });
        }
    }
    Ok(CachePreview { objects })
}

fn remove_selected(
    store: &impl Store,
    selected: BTreeSet<Sha256>,
    cancel: &Cancel,
) -> Result<usize> {
    for hash in &selected {
        cancel.check()?;
        store.remove_object(*hash)?;
    }
    Ok(selected.len())
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use crate::domain::symlink::git_link_target;
    use crate::ports::{FsRepo, FsStore};

    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: Utf8PathBuf,
        repo: FsRepo,
        store: FsStore,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let base = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
            let root = base.join("repo");
            let cache = base.join("cache");
            std::fs::create_dir_all(root.join(".git-sfs")).unwrap();
            std::fs::create_dir_all(&cache).unwrap();
            std::os::unix::fs::symlink(&cache, root.join(".git-sfs/cache")).unwrap();
            Self {
                repo: FsRepo::new(root.clone()),
                store: FsStore::new(cache),
                root,
                _dir: dir,
            }
        }

        fn object(&self, byte: u8) -> Sha256 {
            let hash = Sha256::from_digest([byte; 32]);
            let path = self.store.object_path(hash);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, [byte; 128]).unwrap();
            std::fs::set_permissions(&path, {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::Permissions::from_mode(0o444)
            })
            .unwrap();
            hash
        }

        fn link(&self, name: &str, hash: Sha256) -> Utf8PathBuf {
            let path = self.root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let target = git_link_target(&self.root, &path, hash).unwrap();
            std::os::unix::fs::symlink(target, &path).unwrap();
            path
        }

        fn rm(&self, paths: &[&str]) -> Result<usize> {
            rm(
                &self.repo,
                &self.store,
                &self.root,
                &paths.iter().map(Utf8PathBuf::from).collect::<Vec<_>>(),
                &Cancel::new(),
            )
        }
    }

    #[test]
    fn rm_preview_deduplicates_present_objects_without_changing_bytes_or_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = Fixture::new();
        let hash = fixture.object(10);
        let object = fixture.store.object_path(hash);
        std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o644)).unwrap();
        fixture.link("data/first", hash);
        fixture.link("copy", hash);
        fixture.link("data/missing", Sha256::from_digest([11; 32]));
        let unrelated = fixture.object(12);
        let target = std::fs::read_link(fixture.root.join("copy")).unwrap();

        let preview = preview_rm(
            &fixture.repo,
            &fixture.store,
            &fixture.root,
            &["data".into(), "copy".into()],
            &Cancel::new(),
        )
        .unwrap();

        assert_eq!(
            preview.objects,
            vec![PreviewObject {
                path: object.clone(),
                size: 128
            }]
        );
        assert_eq!(preview.total_bytes(), 128);
        assert_eq!(std::fs::read(&object).unwrap(), [10; 128]);
        assert_eq!(
            std::fs::metadata(&object).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            std::fs::read_link(fixture.root.join("copy")).unwrap(),
            target
        );
        assert!(fixture.store.object_path(unrelated).exists());
        fixture.rm(&["data", "copy"]).unwrap();
        assert!(!object.exists());
        assert!(fixture.store.object_path(unrelated).exists());
    }

    #[test]
    fn prune_preview_reports_only_orphans_and_leaves_them_present() {
        let fixture = Fixture::new();
        let keep = fixture.object(13);
        let orphan = fixture.object(14);
        fixture.link("unstaged", keep);
        let preview = preview_prune(&fixture.repo, &fixture.store, &Cancel::new()).unwrap();

        assert_eq!(
            preview.objects,
            vec![PreviewObject {
                path: fixture.store.object_path(orphan),
                size: 128,
            }]
        );
        assert_eq!(preview.total_bytes(), 128);
        assert_eq!(
            std::fs::read(fixture.store.object_path(orphan)).unwrap(),
            [14; 128]
        );
        assert!(fixture.store.object_path(keep).exists());
        assert_eq!(
            prune(&fixture.repo, &fixture.store, &Cancel::new()).unwrap(),
            preview.objects.len()
        );
        assert_eq!(
            preview_prune(&fixture.repo, &fixture.store, &Cancel::new())
                .unwrap()
                .total_bytes(),
            0
        );
    }

    #[test]
    fn previews_preserve_selection_errors_and_cancellation() {
        let fixture = Fixture::new();
        let hash = fixture.object(15);
        fixture.link("good", hash);
        std::os::unix::fs::symlink("unrelated", fixture.root.join("bad")).unwrap();
        assert!(
            preview_rm(
                &fixture.repo,
                &fixture.store,
                &fixture.root,
                &["good".into(), "bad".into()],
                &Cancel::new()
            )
            .is_err()
        );
        assert!(preview_prune(&fixture.repo, &fixture.store, &Cancel::new()).is_err());
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(
            preview_rm(
                &fixture.repo,
                &fixture.store,
                &fixture.root,
                &["good".into()],
                &cancel
            ),
            Err(Error::Canceled)
        ));
        assert!(matches!(
            preview_prune(&fixture.repo, &fixture.store, &cancel),
            Err(Error::Canceled)
        ));
        assert_eq!(
            std::fs::read(fixture.store.object_path(hash)).unwrap(),
            [15; 128]
        );
    }

    #[test]
    fn rm_unlinks_read_only_object_without_truncating_open_readers_or_links() {
        let fixture = Fixture::new();
        let hash = fixture.object(1);
        let first = fixture.link("data/first", hash);
        let second = fixture.link("copy", hash);
        let target = std::fs::read_link(&first).unwrap();
        let mut reader = std::fs::File::open(&first).unwrap();

        assert_eq!(fixture.rm(&["data", "copy"]).unwrap(), 1);
        assert!(!fixture.store.object_path(hash).exists());
        assert_eq!(std::fs::read_link(&first).unwrap(), target);
        assert!(second.is_symlink());
        assert!(!first.exists());
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1; 128]);
        assert!(fixture.rm(&["data", "copy"]).is_ok());
    }

    #[test]
    fn rm_validates_every_scope_before_removing_anything() {
        let fixture = Fixture::new();
        let hash = fixture.object(2);
        fixture.link("good", hash);
        std::os::unix::fs::symlink("elsewhere", fixture.root.join("bad")).unwrap();
        for invalid in ["bad", "missing", "../cache", ".git-sfs/cache"] {
            assert!(fixture.rm(&["good", invalid]).is_err(), "{invalid}");
            assert_eq!(
                std::fs::read(fixture.store.object_path(hash)).unwrap(),
                [2; 128]
            );
        }
        std::fs::write(fixture.root.join("regular"), "keep me").unwrap();
        assert!(fixture.rm(&["good", "regular"]).is_err());
        assert!(fixture.store.object_path(hash).exists());
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("regular")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn prune_protects_unstaged_and_dangling_working_tree_references() {
        let fixture = Fixture::new();
        let keep = fixture.object(3);
        let orphan = fixture.object(4);
        fixture.link("unstaged", keep);
        fixture.link("dangling", Sha256::from_digest([5; 32]));
        let unknown = fixture
            .store
            .object_path(orphan)
            .parent()
            .unwrap()
            .join("unknown");
        std::fs::write(&unknown, "keep unknown entries").unwrap();

        assert_eq!(
            prune(&fixture.repo, &fixture.store, &Cancel::new()).unwrap(),
            1
        );
        assert!(fixture.store.object_path(keep).exists());
        assert!(!fixture.store.object_path(orphan).exists());
        assert!(fixture.root.join("dangling").is_symlink());
        assert!(unknown.exists());
        assert_eq!(
            prune(&fixture.repo, &fixture.store, &Cancel::new()).unwrap(),
            0
        );
    }

    #[test]
    fn prune_aborts_on_invalid_references_before_deleting_orphans() {
        let fixture = Fixture::new();
        let orphan = fixture.object(6);
        std::os::unix::fs::symlink("elsewhere", fixture.root.join("invalid")).unwrap();
        assert!(prune(&fixture.repo, &fixture.store, &Cancel::new()).is_err());
        assert!(fixture.store.object_path(orphan).exists());
    }

    #[test]
    fn canceled_eviction_preserves_objects() {
        let fixture = Fixture::new();
        let hash = fixture.object(7);
        fixture.link("file", hash);
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(
            prune(&fixture.repo, &fixture.store, &cancel),
            Err(Error::Canceled)
        ));
        assert!(matches!(
            rm(
                &fixture.repo,
                &fixture.store,
                &fixture.root,
                &["file".into()],
                &cancel
            ),
            Err(Error::Canceled)
        ));
        assert!(matches!(
            remove_selected(&fixture.store, BTreeSet::from([hash]), &cancel),
            Err(Error::Canceled)
        ));
        assert!(fixture.store.object_path(hash).exists());
    }

    #[test]
    fn failed_batch_leaves_remaining_objects_intact_and_is_retryable() {
        let fixture = Fixture::new();
        let first = fixture.object(8);
        let blocked = Sha256::from_digest([9; 32]);
        let blocked_path = fixture.store.object_path(blocked);
        std::fs::create_dir_all(&blocked_path).unwrap();
        let selected = BTreeSet::from([first, blocked]);
        assert!(remove_selected(&fixture.store, selected.clone(), &Cancel::new()).is_err());
        assert!(!fixture.store.object_path(first).exists());
        assert!(blocked_path.is_dir());
        std::fs::remove_dir(&blocked_path).unwrap();
        assert!(remove_selected(&fixture.store, selected, &Cancel::new()).is_ok());
    }
}
