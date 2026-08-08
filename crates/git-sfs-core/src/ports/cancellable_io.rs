//! Cancellation for byte-moving loops.
//!
//! Hashing, copying, and downloading must notice Ctrl-C within one chunk
//! instead of at the end of the operation. This adapter centralizes that check
//! so byte-moving code can use ordinary [`std::io::copy`] while still
//! propagating cancellation promptly.

use std::io::{self, Read};

use crate::cancel::Cancel;

/// Marks an [`io::Error`] as originating from cancellation rather than a real
/// I/O failure, so callers can tell the two apart after the fact.
///
/// Deliberately **not** [`io::ErrorKind::Interrupted`]: `std::io::copy`'s
/// generic implementation treats that kind as EINTR-style and silently
/// retries the read, which would make a canceled copy loop instead of
/// stopping. [`io::ErrorKind::Other`] carrying this marker propagates
/// immediately and is recovered with [`is_canceled`].
#[derive(Debug)]
struct CanceledMarker;

impl std::fmt::Display for CanceledMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("canceled")
    }
}

impl std::error::Error for CanceledMarker {}

/// Whether `err` originated from a [`Cancellable`] reader observing
/// cancellation, as opposed to a genuine I/O failure.
#[must_use]
pub fn is_canceled(err: &io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<CanceledMarker>())
}

/// Wraps a [`Read`], checking `cancel` before every `read()` call.
///
/// `io::copy(&mut Cancellable::new(reader, cancel), &mut writer)` therefore
/// stops within one chunk of cancellation, with no per-loop discipline for
/// the caller to remember — the mechanism this module exists to centralize.
pub struct Cancellable<R> {
    inner: R,
    cancel: Cancel,
}

impl<R> Cancellable<R> {
    /// Wraps `inner`, checking `cancel` on every read.
    pub fn new(inner: R, cancel: Cancel) -> Self {
        Self { inner, cancel }
    }
}

impl<R: Read> Read for Cancellable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_canceled() {
            return Err(io::Error::other(CanceledMarker));
        }
        self.inner.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn reads_through_when_not_canceled() {
        let cancel = Cancel::new();
        let mut reader = Cancellable::new(Cursor::new(b"hello".to_vec()), cancel);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn stops_within_the_first_read_once_canceled() {
        let cancel = Cancel::new();
        cancel.cancel();
        let mut reader = Cancellable::new(Cursor::new(b"hello".to_vec()), cancel);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert!(is_canceled(&err));
        assert!(out.is_empty());
    }

    #[test]
    fn io_copy_stops_instead_of_retrying_forever() {
        // The property this module exists for: a plain `io::copy` over a
        // Cancellable source must actually stop, not spin -- which is
        // exactly what ErrorKind::Interrupted would not do.
        let cancel = Cancel::new();
        let large = vec![0u8; 1 << 20];
        let mut reader = Cancellable::new(Cursor::new(large), cancel.clone());
        cancel.cancel();
        let mut sink = io::sink();
        let err = io::copy(&mut reader, &mut sink).unwrap_err();
        assert!(is_canceled(&err));
    }

    #[test]
    fn a_genuine_io_error_is_not_mistaken_for_cancellation() {
        struct AlwaysFails;
        impl Read for AlwaysFails {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            }
        }
        let cancel = Cancel::new();
        let mut reader = Cancellable::new(AlwaysFails, cancel);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).unwrap_err();
        assert!(!is_canceled(&err));
    }
}
