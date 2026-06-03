//! Cancellation helpers for EVM work.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

/// A marker that can be used to cancel execution.
///
/// If dropped, it will set the `cancelled` flag to true.
#[derive(Default, Clone, Debug)]
pub struct CancelOnDrop(Arc<AtomicBool>);

impl CancelOnDrop {
    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A marker that can be used to cancel execution.
///
/// If dropped, it will not set the `cancelled` flag to true. If `cancel` is called, the
/// `cancelled` flag will be set to true.
#[derive(Default, Clone, Debug)]
pub struct ManualCancel(Arc<AtomicBool>);

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
