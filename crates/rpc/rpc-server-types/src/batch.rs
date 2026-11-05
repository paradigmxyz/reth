//! State updates that depend on accepting an entire JSON-RPC batch response.

use parking_lot::Mutex;
use std::{fmt, sync::Arc};

/// Shared request extension for deferring state updates until a batch response fits.
///
/// Batch dispatchers insert the same instance into each request and call [`Self::commit`] only
/// after accepting the complete response. Dropping it on an error or cancellation discards the
/// updates. This does not acknowledge network delivery to the client.
#[derive(Clone, Default)]
pub struct BatchResponseCommit {
    updates: Arc<Mutex<Vec<DeferredUpdate>>>,
}

impl BatchResponseCommit {
    /// Defers an update until the enclosing batch response has been accepted.
    pub fn defer(&self, update: impl FnOnce() + Send + 'static) {
        self.updates.lock().push(Box::new(update));
    }

    /// Applies all deferred updates without yielding. Call only after the batch fits its limit.
    pub fn commit(&self) {
        let updates = std::mem::take(&mut *self.updates.lock());
        for update in updates {
            update();
        }
    }
}

impl fmt::Debug for BatchResponseCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchResponseCommit").finish_non_exhaustive()
    }
}

type DeferredUpdate = Box<dyn FnOnce() + Send>;
