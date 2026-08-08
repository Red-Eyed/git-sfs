//! Pure planning for `push`.
//!
//! Whether an object is actually reachable on the remote is not this
//! module's concern: push always attempts every locally-present object, and
//! `ports::remote`'s `--ignore-existing` is what makes that cheap when the
//! remote already has it. Planning only decides *which local objects*
//! qualify for that attempt.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use thiserror::Error;

use crate::domain::hash::Sha256;

use super::TrackedLink;

/// What `push` should do, once local cache presence is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlan {
    /// Unique objects to upload, sorted by hash for deterministic output.
    pub upload: Vec<Sha256>,
    /// Objects left out because they are not cached locally — populated only
    /// under `--skip-missing`; otherwise `plan_push` fails outright instead
    /// (see [`PlanPushError::MissingCachedFile`]).
    pub skipped: Vec<SkippedObject>,
}

/// One object `push --skip-missing` left out, and every tracked path that
/// references it. Reporting only the object total understates how much of the
/// tree is unbacked, since one object can back many paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedObject {
    /// The absent object.
    pub hash: Sha256,
    /// Every tracked path referencing `hash`, in the same order [`plan_push`]
    /// received `links` in (sorted by path, per [`super::TrackedLink`]'s
    /// contract).
    pub paths: Vec<Utf8PathBuf>,
}

/// Why [`plan_push`] refused to produce a plan.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PlanPushError {
    /// A referenced cache object is not present locally and `--skip-missing`
    /// was not given. Names a working-tree path, not a bare hash, so the user
    /// sees a file they recognize and the exact command that restores it.
    #[error("cache file missing for {path} ({hash}): run: git-sfs pull {path}")]
    MissingCachedFile {
        /// The path named in the error.
        path: Utf8PathBuf,
        /// The missing object it references.
        hash: Sha256,
    },
}

/// Decides what `push` uploads.
///
/// `links` is the tracked-symlink subset of a [`crate::ports::Repo::scan`]
/// within the requested scope, sorted by path; `present` is every hash
/// `links` references that [`crate::ports::Store::verified`] already
/// confirmed present. A hash in `links` but not in `present` is missing
/// locally.
///
/// # Errors
///
/// Returns [`PlanPushError::MissingCachedFile`] if something is missing and
/// `skip_missing` is `false`.
pub fn plan_push(
    links: &[TrackedLink],
    present: &BTreeSet<Sha256>,
    skip_missing: bool,
) -> Result<PushPlan, PlanPushError> {
    let mut upload = BTreeSet::new();
    let mut missing = BTreeSet::new();
    for link in links {
        if present.contains(&link.hash) {
            upload.insert(link.hash);
        } else {
            missing.insert(link.hash);
        }
    }

    if !missing.is_empty() && !skip_missing {
        let first_missing = links
            .iter()
            .find(|link| missing.contains(&link.hash))
            .expect("missing is non-empty, so some link must reference one of its hashes");
        return Err(PlanPushError::MissingCachedFile {
            path: first_missing.path.clone(),
            hash: first_missing.hash,
        });
    }

    let skipped = missing
        .into_iter()
        .map(|hash| SkippedObject {
            hash,
            paths: links
                .iter()
                .filter(|link| link.hash == hash)
                .map(|link| link.path.clone())
                .collect(),
        })
        .collect();

    Ok(PushPlan {
        upload: upload.into_iter().collect(),
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> Sha256 {
        Sha256::from_digest([byte; 32])
    }

    fn link(path: &str, hash: Sha256) -> TrackedLink {
        TrackedLink {
            path: Utf8PathBuf::from(path),
            hash,
        }
    }

    #[test]
    fn uploads_every_present_hash_when_nothing_is_missing() {
        let links = vec![link("a.bin", hash(1)), link("b.bin", hash(2))];
        let present = BTreeSet::from([hash(1), hash(2)]);

        let plan = plan_push(&links, &present, false).unwrap();

        assert_eq!(plan.upload, vec![hash(1), hash(2)]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn fails_naming_the_first_missing_path_in_sorted_order_when_skip_missing_is_false() {
        // Links must already be path-sorted (Repo::scan's contract); "a.bin"
        // sorts before "b.bin", so its hash is the one named even though
        // both are missing.
        let links = vec![link("a.bin", hash(1)), link("b.bin", hash(2))];
        let present = BTreeSet::new();

        let err = plan_push(&links, &present, false).unwrap_err();
        assert_eq!(
            err,
            PlanPushError::MissingCachedFile {
                path: Utf8PathBuf::from("a.bin"),
                hash: hash(1),
            }
        );
    }

    #[test]
    fn skip_missing_uploads_present_objects_and_reports_every_path_referencing_a_skipped_one() {
        let links = vec![
            link("present.bin", hash(1)),
            link("dup-a.bin", hash(2)),
            link("dup-b.bin", hash(2)), // shares hash(2) with dup-a.bin
        ];
        let present = BTreeSet::from([hash(1)]);

        let plan = plan_push(&links, &present, true).unwrap();

        assert_eq!(plan.upload, vec![hash(1)]);
        assert_eq!(
            plan.skipped,
            vec![SkippedObject {
                hash: hash(2),
                paths: vec![
                    Utf8PathBuf::from("dup-a.bin"),
                    Utf8PathBuf::from("dup-b.bin")
                ],
            }]
        );
    }

    #[test]
    fn upload_is_deduplicated_by_hash_not_by_path() {
        // Two different paths, same content -- a legitimate dedup case, not
        // an error.
        let links = vec![link("a.bin", hash(1)), link("b.bin", hash(1))];
        let present = BTreeSet::from([hash(1)]);

        let plan = plan_push(&links, &present, false).unwrap();
        assert_eq!(plan.upload, vec![hash(1)]);
    }

    #[test]
    fn an_empty_link_list_produces_an_empty_plan() {
        let plan = plan_push(&[], &BTreeSet::new(), false).unwrap();
        assert!(plan.upload.is_empty());
        assert!(plan.skipped.is_empty());
    }
}
