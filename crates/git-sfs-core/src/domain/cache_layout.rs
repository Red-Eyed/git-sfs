//! Pure path arithmetic for the cache root.
//!
//! `<cache_root>` itself is resolved elsewhere through the `.git-sfs/cache`
//! symlink. Resolving that symlink is I/O and belongs to a port; everything
//! here is a deterministic function of a root path and, where relevant, a hash.

use camino::{Utf8Path, Utf8PathBuf};

use super::hash::{ALGORITHM, Sha256};

/// Where `hash`'s object lives under `cache_root`:
/// `<cache_root>/files/sha256/<prefix>/<hash>`.
#[must_use]
pub fn object_path(cache_root: &Utf8Path, hash: Sha256) -> Utf8PathBuf {
    cache_root
        .join("files")
        .join(ALGORITHM)
        .join(hash.prefix())
        .join(hash.to_hex())
}

/// Staging directory for in-flight writes: `<cache_root>/tmp`.
#[must_use]
pub fn tmp_dir(cache_root: &Utf8Path) -> Utf8PathBuf {
    cache_root.join("tmp")
}

/// Inter-process lock directory: `<cache_root>/locks`.
#[must_use]
pub fn locks_dir(cache_root: &Utf8Path) -> Utf8PathBuf {
    cache_root.join("locks")
}

/// Reclaimed-object trash root: `<cache_root>/trash`.
#[must_use]
pub fn trash_dir(cache_root: &Utf8Path) -> Utf8PathBuf {
    cache_root.join("trash")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(hex: &str) -> Sha256 {
        Sha256::parse(hex).unwrap()
    }

    const H: &str = "ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a";

    #[test]
    fn object_path_matches_cache_layout() {
        let root = Utf8Path::new("/cache");
        assert_eq!(
            object_path(root, hash(H)),
            format!("/cache/files/sha256/{}/{H}", &H[..2])
        );
    }

    #[test]
    fn tmp_and_locks_are_siblings_of_files() {
        let root = Utf8Path::new("/cache");
        assert_eq!(tmp_dir(root), "/cache/tmp");
        assert_eq!(locks_dir(root), "/cache/locks");
        assert_eq!(trash_dir(root), "/cache/trash");
    }
}
