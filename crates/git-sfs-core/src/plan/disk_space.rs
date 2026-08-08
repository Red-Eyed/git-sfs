//! Pure disk-space guard for `pull` — the fixed 110%-margin policy.
//!
//! Kept as its own step rather than folded into [`super::pull::plan_pull`]:
//! it needs different I/O-observed inputs — remote object sizes and the
//! cache filesystem's available space — that a caller may not always have
//! obtained (or want to pay for) at the same time it decides the download
//! set itself.
//!
//! A caller must pass a complete remote-size map and a known cache free-space
//! value. If the remote listing or `statfs` failed, the caller should propagate
//! that error instead of manufacturing partial inputs; otherwise "unknown"
//! could look like "nothing needs space."

use std::collections::HashMap;

use thiserror::Error;

use crate::domain::hash::Sha256;

/// `pull` refuses to proceed unless the cache volume has at least this much
/// headroom over what is actually needed: a fixed 10% margin, not a config
/// knob.
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
/// `available_bytes` (the cache volume's free space) with the fixed 110%
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
        // still pass.
        assert!(check_disk_space(1000, 1100).is_ok());
        assert!(check_disk_space(1000, 1099).is_err());
    }

    #[test]
    fn error_reports_the_actual_bytes_needed_not_the_margin_inflated_figure() {
        // The number a human should see is what the objects actually cost,
        // not the safety threshold.
        let err = check_disk_space(1000, 0).unwrap_err();
        assert_eq!(err.needed_bytes, 1000);
    }
}
