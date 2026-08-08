//! Hashing with cooperative cancellation.
//!
//! Shared by every port that needs to verify bytes on the way past —
//! [`super::store`] verifying cache objects, [`super::remote`] verifying
//! what actually landed on a remote — and `pub`, not `pub(crate)`, because
//! `exec::add` (the first consumer outside `ports`) needs to hash an
//! arbitrary source file before handing it to [`super::store::Store::store`],
//! which is a different concern from either port's own object store and
//! does not belong on either of their trait surfaces.
//!
//! Not a trait: multiple callers, but exactly one implementation — the same
//! reasoning that keeps [`super::lock::Lock`] a plain type rather than a
//! trait.

use std::fs::File;
use std::io;

use camino::Utf8Path;

use crate::cancel::Cancel;
use crate::domain::hash::Sha256;

use super::cancellable_io::Cancellable;

/// Hashes the file at `path`, checking `cancel` every chunk via
/// [`Cancellable`] rather than reading it in one shot.
///
/// # Errors
///
/// Returns any [`io::Error`] opening or reading `path`.
pub fn hash_file(path: &Utf8Path, cancel: &Cancel) -> io::Result<Sha256> {
    let file = File::open(path)?;
    hash_reader(&file, cancel)
}

/// Hashes `reader`'s remaining bytes, checking `cancel` every chunk.
///
/// A manual read loop rather than `io::copy`: `sha2`'s hasher does not
/// implement [`io::Write`], so there is no writer side for `io::copy` to
/// target. Reading through [`Cancellable`] still checks `cancel` on every
/// chunk, which is the property that actually matters here.
///
/// # Errors
///
/// Returns any [`io::Error`] reading from `reader`.
pub fn hash_reader(reader: impl io::Read, cancel: &Cancel) -> io::Result<Sha256> {
    use std::io::Read as _;

    use sha2::{Digest, Sha256 as Sha256Hasher};

    let mut cancellable = Cancellable::new(reader, cancel.clone());
    let mut hasher = Sha256Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = cancellable.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Sha256::from_digest(hasher.finalize().into()))
}
