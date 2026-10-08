//! Cooperative cancellation of spawned work.

use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
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

    /// Runs `f` with this as the cancellation state of the current thread, see [`is_cancelled`].
    ///
    /// This lets code deep inside `f`, such as a loop over transactions, observe cancellation
    /// without threading the [`CancelOnDrop`] through every call. The previous state of the
    /// thread is restored once `f` returns or unwinds. Leaving the scope does not cancel.
    pub fn scope<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<Arc<AtomicU8>>);

        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.set(self.0.take());
            }
        }

        let _restore = Restore(CURRENT.replace(Some(self.0.clone())));
        f()
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(CANCELLED, Ordering::Relaxed);
    }
}

/// A marker that can be used to cancel execution.
///
/// If dropped, it will NOT set the `cancelled` flag to true.
/// If `cancel` is called, the `cancelled` flag will be set to true.
///
/// This is useful when an external signal should cancel many tasks at once.
#[derive(Default, Clone, Debug)]
pub struct ManualCancel(Arc<AtomicBool>);

// === impl ManualCancel ===

impl ManualCancel {
    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Drops the [`ManualCancel`], setting the cancelled flag to true.
    pub fn cancel(self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Returns `true` if the work running on the current thread was cancelled.
///
/// This reads the state of the innermost [`CancelOnDrop::scope`] on this thread, for example a
/// blocking RPC call whose request was dropped because the client disconnected. Long running work
/// should check this between units of work, such as transactions, and stop early since nobody
/// waits for the result.
///
/// Always returns `false` outside of a [`CancelOnDrop::scope`].
pub fn is_cancelled() -> bool {
    CURRENT.with_borrow(|state| {
        state.as_ref().is_some_and(|state| state.load(Ordering::Relaxed) == CANCELLED)
    })
}

thread_local! {
    /// Cancellation state of the innermost [`CancelOnDrop::scope`] on this thread.
    static CURRENT: RefCell<Option<Arc<AtomicU8>>> = const { RefCell::new(None) };
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
    fn test_set_cancel_task() {
        let c = ManualCancel::default();
        assert!(!c.is_cancelled());
        let c2 = c.clone();
        let c3 = c.clone();
        c.cancel();
        assert!(c3.is_cancelled());
        assert!(c2.is_cancelled());
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

    #[test]
    fn test_scope_is_cancelled() {
        assert!(!is_cancelled());

        let outer = CancelOnDrop::default();
        let inner = CancelOnDrop::default();
        outer.scope(|| {
            assert!(!is_cancelled());
            inner.scope(|| {
                drop(inner.clone());
                assert!(is_cancelled());
            });
            // the outer state is restored
            assert!(!is_cancelled());
            outer.request_finalization();
            assert!(!is_cancelled());
        });

        // leaving the scope does not cancel
        assert!(!is_cancelled());
        assert!(!outer.is_cancelled());
    }

    #[test]
    fn test_scope_restores_on_panic() {
        let cancel = CancelOnDrop::default();
        drop(cancel.clone());

        let res = std::panic::catch_unwind(|| cancel.scope(|| panic!("scope panicked")));
        assert!(res.is_err());
        assert!(!is_cancelled());
    }
}
