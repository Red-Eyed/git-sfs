//! Pure disk-space guard for `pull` — v1's fixed 110%-margin policy
//! (`pull.go:96-123`), ported.
//!
//! Kept as its own step rather than folded into [`super::pull::plan_pull`]:
//! it needs different I/O-observed inputs — remote object sizes and the
//! cache filesystem's available space — that a caller may not always have
//! obtained (or want to pay for) at the same time it decides the download
//! set itself.
//!
//! contract-spec §13.3 flags v1's guard as failing open twice: a hash
//! missing from a remote listing silently contributes zero bytes, and a
//! `statfs` failure only warns and proceeds. Neither gap is reproduced here
//! by construction, not by extra defensive code: [`sum_needed_bytes`] simply
//! sums whatever `remote_sizes` map it is given, and
//! `ports::remote::RcloneRemote::file_sizes` already returns `Err` rather
//! than a partial map on listing failure — so as long as a caller propagates
//! that `Err` instead of catching it, "the remote is unreachable" can never
//! read as "nothing needs space" here. The `statfs` half is entirely the
//! caller's own I/O to get right; this module only ever sees the byte count
//! already determined.

use std::collections::HashMap;

use thiserror::Error;

use crate::domain::hash::Sha256;

/// `pull` refuses to proceed unless the cache volume has at least this much
/// headroom over what is actually needed (`pull.go:119`) — a fixed 10%
/// margin, not a config knob, matching v1 exactly.
const REQUIRED_MARGIN_PERCENT: u64 = 110;

/// The cache volume does not have enough free space for
/// [`super::pull::plan_pull`]'s download set.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("insufficient disk space: need ~{needed_bytes} bytes, have {available_bytes} available")]
pub struct InsufficientDiskSpace {
    /// Bytes the selected objects actually need (before the safety margin).
    pub needed_bytes: u64,
    /// Bytes actually available on the cache volume.
    pub available_bytes: u64,
}

/// Total bytes `download` needs, from a `hash -> remote size` map that must
/// be complete — see the module doc for why a caller must never pass a
/// partial map here to represent "the listing failed".
#[must_use]
pub fn sum_needed_bytes(download: &[Sha256], remote_sizes: &HashMap<Sha256, u64>) -> u64 {
    download
        .iter()
        .filter_map(|hash| remote_sizes.get(hash))
        .sum()
}

/// Checks `needed_bytes` (from [`sum_needed_bytes`]) against
/// `available_bytes` (the cache volume's free space) with v1's fixed 110%
/// margin.
///
/// # Errors
///
/// Returns [`InsufficientDiskSpace`] if the margin is not met.
pub fn check_disk_space(
    needed_bytes: u64,
    available_bytes: u64,
) -> Result<(), InsufficientDiskSpace> {
    let required = needed_bytes.saturating_mul(REQUIRED_MARGIN_PERCENT) / 100;
    if required > available_bytes {
        return Err(InsufficientDiskSpace {
            needed_bytes,
            available_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> Sha256 {
        Sha256::from_digest([byte; 32])
    }

    #[test]
    fn sum_needed_bytes_totals_only_the_requested_hashes() {
        let sizes = HashMap::from([(hash(1), 100), (hash(2), 200), (hash(3), 999)]);
        assert_eq!(sum_needed_bytes(&[hash(1), hash(2)], &sizes), 300);
    }

    #[test]
    fn sum_needed_bytes_ignores_a_hash_absent_from_the_map() {
        // Absence here means "the caller already excluded it" (e.g. it's
        // confirmed on a smaller/irrelevant tier), never "the listing
        // failed" -- that case must never reach this function as a partial
        // map. See the module doc.
        let sizes = HashMap::from([(hash(1), 100)]);
        assert_eq!(sum_needed_bytes(&[hash(1), hash(2)], &sizes), 100);
    }

    #[test]
    fn passes_when_nothing_is_needed() {
        assert!(check_disk_space(0, 0).is_ok());
    }

    #[test]
    fn requires_a_ten_percent_margin_over_what_is_needed() {
        // Exactly 110% of needed is the boundary: available == required must
        // still pass (v1's comparison is strictly `>`, not `>=`).
        assert!(check_disk_space(1000, 1100).is_ok());
        assert!(check_disk_space(1000, 1099).is_err());
    }

    #[test]
    fn error_reports_the_actual_bytes_needed_not_the_margin_inflated_figure() {
        // v1's message reports the raw `needed`, not `needed*110/100`
        // (pull.go:120) -- the number a human should see is what the
        // objects actually cost, not the safety threshold.
        let err = check_disk_space(1000, 0).unwrap_err();
        assert_eq!(err.needed_bytes, 1000);
    }
}
