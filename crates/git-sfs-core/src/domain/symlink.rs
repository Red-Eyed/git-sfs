//! The committed symlink: construction and validation.
//!
//! contract-spec §3. Both directions are pure path arithmetic over strings the
//! caller already has — the actual `readlink()`/`symlink()` syscalls are I/O
//! and belong to a port (Phase 3); everything here only needs the text a
//! syscall would produce or consume, so it is tested with zero filesystem.

use camino::{Utf8Path, Utf8PathBuf};
use path_clean::PathClean;
use thiserror::Error;

use super::hash::{ALGORITHM, Sha256};

/// Where `hash`'s cache object lives, relative to `repo`:
/// `<repo>/.git-sfs/cache/files/sha256/<prefix>/<hash>` (contract-spec §3.1).
#[must_use]
pub fn cache_link_file(repo: &Utf8Path, hash: Sha256) -> Utf8PathBuf {
    repo.join(".git-sfs")
        .join("cache")
        .join("files")
        .join(ALGORITHM)
        .join(hash.prefix())
        .join(hash.to_hex())
}

/// A relative path could not be computed between two paths.
///
/// The only cause: `file` and `repo` disagree on absolute-vs-relative, so no
/// relative traversal between them exists.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("cannot express a path from {file} to the cache in .git-sfs/cache")]
pub struct NoRelativePath {
    /// The file the symlink is being constructed for.
    pub file: Utf8PathBuf,
}

/// The relative symlink target to commit for a file with content `hash`.
///
/// contract-spec §3.1: `target = relative_path_from(dirname(file), cache_link_file)`.
/// Targets must be relative — the indirection through `.git-sfs/cache` is what
/// keeps machine-local cache paths out of committed metadata, and a relative
/// target is what makes that indirection actually opaque to Git.
///
/// # Errors
///
/// Returns [`NoRelativePath`] if `file` and `repo` are not both absolute or
/// both relative, since no relative path connects them in that case.
pub fn git_link_target(
    repo: &Utf8Path,
    file: &Utf8Path,
    hash: Sha256,
) -> Result<Utf8PathBuf, NoRelativePath> {
    let link_file = cache_link_file(repo, hash);
    let file_dir = file.parent().unwrap_or_else(|| Utf8Path::new("."));
    pathdiff::diff_utf8_paths(&link_file, file_dir).ok_or_else(|| NoRelativePath {
        file: file.to_owned(),
    })
}

/// Why a committed symlink's target failed validation.
///
/// Every variant maps to contract-spec §3.2's `ErrInvalidSymlink` → exit 3;
/// this enum exists so that mapping is a `match` arm, not a string comparison.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InvalidSymlinkTarget {
    /// Rule 2: the target is an absolute path.
    #[error("git symlink {file} has absolute target {target}")]
    AbsoluteTarget {
        /// The symlink whose target was rejected.
        file: Utf8PathBuf,
        /// The rejected target text.
        target: String,
    },
    /// Rule 3: the resolved target does not lie under
    /// `.git-sfs/cache/files/sha256`.
    #[error("git symlink {file} does not point into .git-sfs/cache")]
    OutsideCache {
        /// The symlink whose target was rejected.
        file: Utf8PathBuf,
    },
    /// Rule 4: the path under the cache root does not have exactly two
    /// components (`<prefix>/<hash>`).
    #[error("git symlink {file} has invalid file path")]
    WrongComponentCount {
        /// The symlink whose target was rejected.
        file: Utf8PathBuf,
    },
    /// Rule 5: the second component is not a valid SHA-256 hex digest.
    #[error("git symlink {file} points at an invalid hash")]
    InvalidHash {
        /// The symlink whose target was rejected.
        file: Utf8PathBuf,
        /// Why the hash failed to parse.
        #[source]
        source: super::hash::HashParseError,
    },
    /// Rule 6: the first component does not equal the hash's own prefix.
    ///
    /// Redundant with the hash, but deliberately enforced (contract-spec
    /// §3.2) so a stale or hand-edited link fails loudly instead of resolving
    /// to the wrong object.
    #[error("git symlink {file} prefix {prefix:?} does not match hash")]
    PrefixMismatch {
        /// The symlink whose target was rejected.
        file: Utf8PathBuf,
        /// The first path component actually found.
        prefix: String,
    },
}

/// Validates a committed symlink's target text against contract-spec §3.2 and,
/// on success, returns the hash it names.
///
/// `target` is the text a `readlink()` on `file` already produced — this
/// function performs no I/O of its own, so a corrupt-symlink test can hand it
/// any string without touching a filesystem.
///
/// # Errors
///
/// Returns [`InvalidSymlinkTarget`] for any of the six validation rules
/// contract-spec §3.2 enumerates (rule 1, "`readlink` succeeds", is the
/// caller's concern — it already has `target` in hand by the time this runs).
pub fn validate_symlink_target(
    repo: &Utf8Path,
    file: &Utf8Path,
    target: &str,
) -> Result<Sha256, InvalidSymlinkTarget> {
    let target_path = Utf8Path::new(target);
    if target_path.is_absolute() {
        return Err(InvalidSymlinkTarget::AbsoluteTarget {
            file: file.to_owned(),
            target: target.to_owned(),
        });
    }

    let file_dir = file.parent().unwrap_or_else(|| Utf8Path::new("."));
    let resolved = clean_utf8(&file_dir.join(target_path));
    let cache_root = repo
        .join(".git-sfs")
        .join("cache")
        .join("files")
        .join(ALGORITHM);

    let outside_cache = || InvalidSymlinkTarget::OutsideCache {
        file: file.to_owned(),
    };
    let rel = pathdiff::diff_utf8_paths(&resolved, &cache_root).ok_or_else(outside_cache)?;
    // Go's `filepath.Rel` returns "." when the two paths are identical; `pathdiff`
    // has no such special case and returns an empty path instead (its component
    // loop never pushes anything when every component matches). Both mean the
    // target points at the cache root itself, not an object inside it, so both
    // must be rejected here.
    if rel.as_str().is_empty() || rel == "." || rel.as_str().starts_with("..") {
        return Err(outside_cache());
    }

    let parts: Vec<&str> = rel.as_str().split('/').collect();
    let [prefix, hex] = parts.as_slice() else {
        return Err(InvalidSymlinkTarget::WrongComponentCount {
            file: file.to_owned(),
        });
    };

    let hash = Sha256::parse(hex).map_err(|source| InvalidSymlinkTarget::InvalidHash {
        file: file.to_owned(),
        source,
    })?;
    if *prefix != hash.prefix() {
        return Err(InvalidSymlinkTarget::PrefixMismatch {
            file: file.to_owned(),
            prefix: (*prefix).to_owned(),
        });
    }
    Ok(hash)
}

/// Lexically normalizes `.`/`..` components without touching the filesystem —
/// the `Utf8Path` equivalent of Go's `filepath.Clean`, needed because
/// `Utf8Path::join` (like `Path::join`) does not resolve them on its own.
///
/// `pub(crate)`: [`super::super::ports::repo`] reuses this for the same
/// reason (`absFromRepo`'s `filepath.Clean` on an absolute scope argument)
/// rather than duplicating a second lexical-clean helper.
pub(crate) fn clean_utf8(path: &Utf8Path) -> Utf8PathBuf {
    let cleaned = path.as_std_path().clean();
    Utf8PathBuf::from_path_buf(cleaned)
        .expect("cleaning a UTF-8 path cannot introduce invalid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(hex: &str) -> Sha256 {
        Sha256::parse(hex).unwrap()
    }

    const H: &str = "ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a";

    #[test]
    fn construction_matches_the_contract_spec_example() {
        // contract-spec 3.1's own worked example: repo/data/train.bin, hash
        // starting ab3f..., target ../.git-sfs/cache/files/sha256/ab/ab3f....
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let target = git_link_target(repo, file, hash(H)).unwrap();
        assert_eq!(
            target,
            format!("../.git-sfs/cache/files/sha256/{}/{H}", &H[..2])
        );
    }

    #[test]
    fn construction_from_a_nested_file_climbs_one_level_per_directory() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/a/b/c/train.bin");
        let target = git_link_target(repo, file, hash(H)).unwrap();
        assert_eq!(
            target,
            format!("../../../.git-sfs/cache/files/sha256/{}/{H}", &H[..2])
        );
    }

    #[test]
    fn a_constructed_target_validates_round_trip() {
        // What `add` writes, `verify` must accept -- the two directions have
        // to agree or every file this crate ever links would fail its own
        // validation.
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let target = git_link_target(repo, file, hash(H)).unwrap();
        assert_eq!(
            validate_symlink_target(repo, file, target.as_str()).unwrap(),
            hash(H)
        );
    }

    #[test]
    fn rejects_absolute_targets() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let err = validate_symlink_target(repo, file, "/etc/passwd").unwrap_err();
        assert!(matches!(err, InvalidSymlinkTarget::AbsoluteTarget { .. }));
    }

    #[test]
    fn rejects_targets_that_escape_the_cache_root() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        // Climbs out of .git-sfs/cache entirely instead of into it.
        let err = validate_symlink_target(repo, file, "../../../../etc/passwd").unwrap_err();
        assert!(matches!(err, InvalidSymlinkTarget::OutsideCache { .. }));
    }

    #[test]
    fn rejects_a_target_pointing_exactly_at_the_cache_root() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let err =
            validate_symlink_target(repo, file, "../.git-sfs/cache/files/sha256").unwrap_err();
        assert!(matches!(err, InvalidSymlinkTarget::OutsideCache { .. }));
    }

    #[test]
    fn rejects_wrong_component_count() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let prefix = &H[..2];
        let too_deep = format!("../.git-sfs/cache/files/sha256/{prefix}/extra/{H}");
        let err = validate_symlink_target(repo, file, &too_deep).unwrap_err();
        assert!(matches!(
            err,
            InvalidSymlinkTarget::WrongComponentCount { .. }
        ));
    }

    #[test]
    fn rejects_an_invalid_hash_component() {
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let target = "../.git-sfs/cache/files/sha256/ab/not-a-hash";
        let err = validate_symlink_target(repo, file, target).unwrap_err();
        assert!(matches!(err, InvalidSymlinkTarget::InvalidHash { .. }));
    }

    #[test]
    fn rejects_a_prefix_that_does_not_match_the_hash() {
        // contract-spec 3.2 rule 6: deliberately redundant with the hash, so
        // a stale or hand-edited link fails loudly instead of resolving to
        // the wrong object.
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let target = format!("../.git-sfs/cache/files/sha256/00/{H}");
        let err = validate_symlink_target(repo, file, &target).unwrap_err();
        assert!(matches!(err, InvalidSymlinkTarget::PrefixMismatch { .. }));
    }

    #[test]
    fn tolerates_dot_segments_that_still_resolve_into_the_cache() {
        // filepath.Clean-equivalent lexical resolution, exercised directly:
        // a target with a redundant "./" still lands in the cache and
        // validates, matching v1's Clean(Join(...)) behavior.
        let repo = Utf8Path::new("/repo");
        let file = Utf8Path::new("/repo/data/train.bin");
        let prefix = &H[..2];
        let target = format!("../.git-sfs/./cache/files/sha256/{prefix}/{H}");
        assert_eq!(
            validate_symlink_target(repo, file, &target).unwrap(),
            hash(H)
        );
    }
}
