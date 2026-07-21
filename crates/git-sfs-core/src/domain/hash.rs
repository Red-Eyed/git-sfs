//! The content-address type.
//!
//! rust-rewrite-plan §2.1: v1's `Hash` is `type Hash string` with a `Parse`
//! function nothing forces callers through — `Hash("")` is legal everywhere,
//! and error paths return exactly that. [`Sha256`] closes that: the only ways
//! to build one are [`Sha256::parse`] and [`Sha256::from_digest`], both of
//! which fix the length invariant at construction, so [`Sha256::prefix`]
//! becomes total. The `len(s) < 2` guard clause in v1's `Prefix` (`hash.go:88`)
//! is therefore deleted here, not ported — a value that exists at all already
//! has exactly 32 bytes.

use std::fmt;

use thiserror::Error;

/// SHA-256 digests are 32 bytes, always — the one algorithm contract-spec §6.1
/// admits (`algorithm = "sha256"`, no other value validates).
const DIGEST_LEN: usize = 32;

/// A hex-encoded SHA-256 digest is exactly this many characters.
pub const HEX_LEN: usize = DIGEST_LEN * 2;

/// The fanout-directory name git-sfs stores objects under: `files/<ALGORITHM>/…`.
pub const ALGORITHM: &str = "sha256";

/// A validated SHA-256 digest.
///
/// No `Default`, no `From<String>`, no public field. The only ways to obtain
/// one are [`Sha256::parse`] (validates external text) and
/// [`Sha256::from_digest`] (accepts bytes a hasher already produced), so
/// "invalid hash" cannot exist past either boundary — every downstream
/// function that takes a `Sha256` gets a value that is already known-good.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256([u8; DIGEST_LEN]);

/// Why a string failed to parse as a [`Sha256`].
///
/// contract-spec §3.2 rule 5: exactly 64 characters, each `[0-9a-f]`.
/// Uppercase is rejected — deliberately, not an oversight, so this type does
/// not use a case-insensitive decoder even though one would parse more inputs.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HashParseError {
    /// The input was not exactly [`HEX_LEN`] characters.
    #[error("invalid sha256 length for {input:?}: want {HEX_LEN} hex characters, got {got}")]
    WrongLength {
        /// The rejected input, for the error message.
        input: String,
        /// Its actual length.
        got: usize,
    },
    /// The input had the right length but contained a character outside
    /// lowercase `0-9a-f` — including uppercase hex, which v1 also rejects.
    #[error("invalid sha256 hex {input:?}: must be lowercase hex")]
    NotLowercaseHex {
        /// The rejected input, for the error message.
        input: String,
    },
}

impl Sha256 {
    /// Parses a canonical lowercase hex string.
    ///
    /// # Errors
    ///
    /// Returns [`HashParseError`] if `s` is not exactly [`HEX_LEN`] lowercase
    /// hex characters.
    pub fn parse(s: &str) -> Result<Self, HashParseError> {
        if s.len() != HEX_LEN {
            return Err(HashParseError::WrongLength {
                input: s.to_owned(),
                got: s.len(),
            });
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(HashParseError::NotLowercaseHex {
                input: s.to_owned(),
            });
        }

        let mut digest = [0u8; DIGEST_LEN];
        // `s` was just confirmed to be `HEX_LEN` lowercase hex characters, so
        // decoding cannot fail; a case-insensitive decoder would accept more
        // than this type permits, which is why the check above runs first.
        hex::decode_to_slice(s, &mut digest).expect("validated lowercase hex decodes");
        Ok(Self(digest))
    }

    /// Wraps a digest a hasher already produced.
    ///
    /// This is the boundary Phase 3's file-hashing port will call once actual
    /// bytes are read; nothing in this crate reads a file today.
    #[must_use]
    pub fn from_digest(digest: [u8; DIGEST_LEN]) -> Self {
        Self(digest)
    }

    /// The raw digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }

    /// The full 64-character lowercase hex encoding.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// The two-character fanout directory this hash's cache object lives
    /// under: `files/sha256/<prefix>/<hash>` (contract-spec §4).
    ///
    /// Total, unlike v1's `Prefix` — see the module doc.
    #[must_use]
    pub fn prefix(&self) -> String {
        hex::encode([self.0[0]])
    }

    /// The leading 12 hex characters, for display next to a path.
    ///
    /// Matches v1's `Short()` (`hash.go:76-83`): full 64 characters wrap
    /// terminal lines without telling the reader anything more; 12 still
    /// distinguishes objects by eye.
    #[must_use]
    pub fn short(&self) -> String {
        hex::encode(&self.0[..6])
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({})", self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "ab3fce1234567890abcdef1234567890abcdef1234567890abcdef123456789a";

    #[test]
    fn parses_a_canonical_lowercase_digest() {
        assert!(Sha256::parse(EXAMPLE).is_ok());
    }

    #[test]
    fn rejects_uppercase() {
        // contract-spec 3.2 rule 5: uppercase is rejected, deliberately, not a
        // case-insensitivity gap.
        let upper = EXAMPLE.to_uppercase();
        assert!(matches!(
            Sha256::parse(&upper),
            Err(HashParseError::NotLowercaseHex { .. })
        ));
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(matches!(
            Sha256::parse(&EXAMPLE[..63]),
            Err(HashParseError::WrongLength { got: 63, .. })
        ));
        assert!(matches!(
            Sha256::parse(&format!("{EXAMPLE}a")),
            Err(HashParseError::WrongLength { got: 65, .. })
        ));
    }

    #[test]
    fn rejects_non_hex_characters_of_the_right_length() {
        let bad = "g".repeat(HEX_LEN);
        assert!(matches!(
            Sha256::parse(&bad),
            Err(HashParseError::NotLowercaseHex { .. })
        ));
    }

    #[test]
    fn rejects_empty_string() {
        // The v1 defect this type closes: `Hash("")` was a legal value
        // everywhere. Parsing "" must fail outright, not yield a valid zero
        // value.
        assert!(Sha256::parse("").is_err());
    }

    #[test]
    fn prefix_is_the_first_two_hex_characters() {
        let h = Sha256::parse(EXAMPLE).unwrap();
        assert_eq!(h.prefix(), &EXAMPLE[..2]);
    }

    #[test]
    fn short_is_the_first_twelve_hex_characters() {
        let h = Sha256::parse(EXAMPLE).unwrap();
        assert_eq!(h.short(), &EXAMPLE[..12]);
    }

    #[test]
    fn to_hex_round_trips_through_parse() {
        let h = Sha256::parse(EXAMPLE).unwrap();
        assert_eq!(Sha256::parse(&h.to_hex()).unwrap(), h);
    }

    #[test]
    fn display_matches_to_hex() {
        let h = Sha256::parse(EXAMPLE).unwrap();
        assert_eq!(h.to_string(), h.to_hex());
    }

    proptest::proptest! {
        /// Every valid digest round-trips through hex encode/parse, and the
        /// prefix/short views always agree with the full encoding — no length
        /// of input can make `prefix`/`short` panic or disagree with `to_hex`,
        /// which is the totality guarantee the module doc promises.
        #[test]
        fn round_trips_and_views_agree_for_any_digest(bytes in proptest::array::uniform32(0u8..=255)) {
            let h = Sha256::from_digest(bytes);
            let hex = h.to_hex();
            proptest::prop_assert_eq!(hex.len(), HEX_LEN);
            proptest::prop_assert_eq!(Sha256::parse(&hex).unwrap(), h);
            proptest::prop_assert_eq!(h.prefix(), &hex[..2]);
            proptest::prop_assert_eq!(h.short(), &hex[..12]);
        }

        /// Any string that is not exactly `HEX_LEN` lowercase hex characters
        /// must be rejected -- fuzzing the boundary the hand-written unit
        /// tests only sample.
        #[test]
        fn parse_never_panics_on_arbitrary_input(s in ".*") {
            #[allow(clippy::let_underscore_must_use, reason = "the point of this test is that parse() does not panic; the Result itself is not the assertion")]
            let _ = Sha256::parse(&s);
        }
    }
}
