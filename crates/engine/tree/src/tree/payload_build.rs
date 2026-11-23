//! Tracking of active payload build jobs.

use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Tracks payload jobs that may access the current in-memory overlay.
#[derive(Clone, Debug)]
pub(super) struct PayloadBuildTracker {
    active: Arc<AtomicUsize>,
    finished_tx: Sender<()>,
}

impl PayloadBuildTracker {
    /// Creates a tracker and a receiver notified when its active count reaches zero.
    pub(super) fn new() -> (Self, Receiver<()>) {
        let (finished_tx, finished_rx) = crossbeam_channel::bounded(1);
        (Self { active: Arc::new(AtomicUsize::new(0)), finished_tx }, finished_rx)
    }

    /// Acquires a lease for one payload job.
    pub(super) fn acquire(&self) -> PayloadBuildLease {
        self.active.fetch_add(1, Ordering::AcqRel);
        PayloadBuildLease {
            active: Arc::clone(&self.active),
            finished_tx: self.finished_tx.clone(),
        }
    }

    /// Returns whether at least one payload job is active.
    pub(super) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) != 0
    }
}

/// A lease held for the lifetime of a payload job.
#[derive(Debug)]
pub(super) struct PayloadBuildLease {
    active: Arc<AtomicUsize>,
    finished_tx: Sender<()>,
}

impl Drop for PayloadBuildLease {
    fn drop(&mut self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "payload build lease count underflow");

        if previous == 1 {
            // The bounded channel coalesces completion notifications. The engine always checks
            // the counter again before applying a pending handoff.
            let _ = self.finished_tx.try_send(());
        }
    }
}
