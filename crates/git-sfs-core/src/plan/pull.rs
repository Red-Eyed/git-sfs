//! Pure planning for `pull`.
//!
//! Unlike push, pull never fails on an absent object — restoring exactly
//! that is what the command exists to do.

use std::collections::BTreeSet;

use crate::domain::hash::Sha256;

use super::TrackedLink;

/// What `pull` should download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullPlan {
    /// Unique objects to download, sorted by hash for deterministic output.
    pub download: Vec<Sha256>,
}

/// Decides what `pull` downloads: every hash `links` references that is not
/// already in `present`.
///
/// `links` is the tracked-symlink subset of a [`crate::ports::Repo::scan`]
/// within the requested scope; `present` is every hash it references that
/// [`crate::ports::Store::verified`] already confirmed present.
#[must_use]
pub fn plan_pull(links: &[TrackedLink], present: &BTreeSet<Sha256>) -> PullPlan {
    let download: BTreeSet<Sha256> = links
        .iter()
        .map(|link| link.hash)
        .filter(|hash| !present.contains(hash))
        .collect();
    PullPlan {
        download: download.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

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
    fn downloads_every_hash_not_already_present() {
        let links = vec![
            link("a.bin", hash(1)),
            link("b.bin", hash(2)),
            link("c.bin", hash(3)),
        ];
        let present = BTreeSet::from([hash(2)]);

        let plan = plan_pull(&links, &present);
        assert_eq!(plan.download, vec![hash(1), hash(3)]);
    }

    #[test]
    fn nothing_to_download_when_everything_is_already_present() {
        let links = vec![link("a.bin", hash(1))];
        let present = BTreeSet::from([hash(1)]);
        assert!(plan_pull(&links, &present).download.is_empty());
    }

    #[test]
    fn an_empty_link_list_downloads_nothing() {
        assert!(plan_pull(&[], &BTreeSet::new()).download.is_empty());
    }

    #[test]
    fn download_set_is_deduplicated_by_hash() {
        let links = vec![link("a.bin", hash(1)), link("b.bin", hash(1))];
        let plan = plan_pull(&links, &BTreeSet::new());
        assert_eq!(plan.download, vec![hash(1)]);
    }
}
