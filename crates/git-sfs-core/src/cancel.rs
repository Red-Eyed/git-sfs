//! Cooperative cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

/// A cancellation flag shared between the signal handler that sets it and the
/// work that polls it.
///
/// This is the injected side of cancellation: core never installs a signal
/// handler, it only observes a flag the binary owns. Tests drive the same type
/// directly and need no signals at all.
///
/// Cloning shares the flag rather than copying it, so a `Cancel` handed to a
/// worker thread observes a cancellation requested anywhere.
///
/// Byte-moving loops wrap their reader with this flag, so hashing and copying
/// inherit prompt cancellation from ordinary read calls instead of relying on
/// each loop to remember a separate check.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A flag that has not been canceled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent, and safe to call from a signal
    /// handler.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// [`Error::Canceled`] once cancellation has been requested, `Ok` before.
    ///
    /// This is the form work loops use, so that cancellation propagates as an
    /// ordinary `?` rather than as a separate branch someone has to remember to
    /// write.
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            return Err(Error::Canceled);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_uncanceled_and_permits_work() {
        let cancel = Cancel::new();
        assert!(!cancel.is_canceled());
        assert!(cancel.check().is_ok());
    }

    #[test]
    fn cancellation_is_visible_through_a_clone() {
        let cancel = Cancel::new();
        let worker = cancel.clone();

        cancel.cancel();

        assert!(worker.is_canceled());
        assert!(matches!(worker.check(), Err(Error::Canceled)));
    }

    #[test]
    fn cancelling_twice_is_harmless() {
        let cancel = Cancel::new();
        cancel.cancel();
        cancel.cancel();
        assert!(cancel.is_canceled());
    }
}
