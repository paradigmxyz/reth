//! Cooperative cancellation of spawned work.

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

/// The work is still in progress.
const RUNNING: u8 = 0;
/// The work should wrap up and keep what it has produced so far.
const FINALIZATION_REQUESTED: u8 = 1;
/// The work should stop and discard what it has produced so far.
const CANCELLED: u8 = 2;

/// Cancels execution on drop and supports cooperative finalization.
///
/// If dropped, it will set the `cancelled` flag to true.
///
/// This is most useful when a spawned job should stop once its owner goes away, e.g. a payload
/// job or a blocking RPC call whose caller disconnected.
#[derive(Default, Clone, Debug)]
pub struct CancelOnDrop(Arc<AtomicU8>);

// === impl CancelOnDrop ===

impl CancelOnDrop {
    /// Returns true if the current work should be interrupted.
    pub fn is_interrupted(&self) -> bool {
        self.0.load(Ordering::Relaxed) != RUNNING
    }

    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed) == CANCELLED
    }

    /// Requests that the current work be finalized without cancelling it.
    pub fn request_finalization(&self) {
        let _ = self.0.compare_exchange(
            RUNNING,
            FINALIZATION_REQUESTED,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// Returns true if finalization was requested.
    pub fn is_finalization_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed) == FINALIZATION_REQUESTED
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(CANCELLED, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_cancelled() {
        let c = CancelOnDrop::default();
        assert!(!c.is_interrupted());
        assert!(!c.is_cancelled());
    }

    #[test]
    fn test_cancelondrop_clone_behavior() {
        let cancel = CancelOnDrop::default();
        assert!(!cancel.is_cancelled());

        // Clone the CancelOnDrop
        let cloned_cancel = cancel.clone();
        assert!(!cloned_cancel.is_cancelled());

        // Drop the original - this should set the cancelled flag
        drop(cancel);

        // The cloned instance should now see the cancelled flag as true
        assert!(cloned_cancel.is_interrupted());
        assert!(cloned_cancel.is_cancelled());
    }

    #[test]
    fn test_cancelondrop_multiple_clones() {
        let cancel = CancelOnDrop::default();
        let clone1 = cancel.clone();
        let clone2 = cancel.clone();
        let clone3 = cancel.clone();

        assert!(!cancel.is_cancelled());
        assert!(!clone1.is_cancelled());
        assert!(!clone2.is_cancelled());
        assert!(!clone3.is_cancelled());

        // Drop one clone - this should cancel all instances
        drop(clone1);

        assert!(cancel.is_interrupted());
        assert!(cancel.is_cancelled());
        assert!(clone2.is_cancelled());
        assert!(clone3.is_cancelled());
    }

    #[test]
    fn test_cancel_on_drop_finalization_request() {
        let cancel = CancelOnDrop::default();
        let clone = cancel.clone();

        cancel.request_finalization();

        assert!(clone.is_interrupted());
        assert!(clone.is_finalization_requested());
        assert!(!clone.is_cancelled());

        drop(cancel);

        assert!(clone.is_cancelled());
        assert!(!clone.is_finalization_requested());

        clone.request_finalization();

        assert!(clone.is_cancelled());
        assert!(!clone.is_finalization_requested());
    }
}
