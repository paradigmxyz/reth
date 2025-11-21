use alloy_primitives::B256;
use parking_lot::{Condvar, Mutex};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{error::Error, fmt, sync::Arc};

/// Sorted trie data computed for an executed block.
/// These represent the complete set of sorted trie data required to persist 
/// block state and proof generation for a block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// The persisted ancestor hash this trie input is anchored to.
    pub anchor_hash: B256,
    /// Trie input constructed from in-memory overlays.
    pub trie_input: Arc<TrieInputSorted>,
}

impl PartialEq for ComputedTrieData {
    fn eq(&self, other: &Self) -> bool {
        self.hashed_state == other.hashed_state &&
            self.trie_updates == other.trie_updates &&
            self.anchor_hash == other.anchor_hash
    }
}

/// Error returned when deferred trie data computation fails.
#[derive(Debug, Clone)]
pub struct DeferredTrieDataError {
    msg: Arc<str>,
}

impl DeferredTrieDataError {
    /// Create a new deferred trie data error from message.
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: Arc::from(msg.into()) }
    }

    /// Error raised when the background computation panicked.
    pub fn panicked() -> Self {
        Self::new("deferred trie computation panicked")
    }
}

impl fmt::Display for DeferredTrieDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl Error for DeferredTrieDataError {}

#[derive(Debug, Default)]
enum DeferredState {
    #[default]
    Pending,
    Ready(ComputedTrieData),
    Failed(DeferredTrieDataError),
}

/// Shared handle to asynchronously populated trie data.
#[derive(Clone)]
pub struct DeferredTrieData {
    state: Arc<Mutex<DeferredState>>,
    ready: Arc<Condvar>,
}

impl Default for DeferredTrieData {
    fn default() -> Self {
        Self::pending()
    }
}

impl DeferredTrieData {
    /// Create a new pending handle.
    pub fn pending() -> Self {
        Self {
            state: Arc::new(Mutex::new(DeferredState::default())),
            ready: Arc::new(Condvar::new()),
        }
    }

    /// Creates a new handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// This is useful when the trie data is already available and does not need to be computed
    /// asynchronously. Any calls to [`Self::wait_cloned`] will return immediately.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        let handle = Self::pending();
        handle.set_ready(bundle);
        handle
    }

    /// Mark the handle as ready, notifying any waiters.
    pub fn set_ready(&self, bundle: ComputedTrieData) {
        let mut state = self.state.lock();
        if matches!(*state, DeferredState::Pending) {
            *state = DeferredState::Ready(bundle);
            self.ready.notify_all();
        }
    }

    /// Mark the handle as failed, notifying any waiters.
    pub fn set_error(&self, error: DeferredTrieDataError) {
        let mut state = self.state.lock();
        if matches!(*state, DeferredState::Pending) {
            *state = DeferredState::Failed(error);
            self.ready.notify_all();
        }
    }

    /// Wait for data to become ready and clone it.
    pub fn wait_cloned(&self) -> Result<ComputedTrieData, DeferredTrieDataError> {
        let mut state = self.state.lock();
        loop {
            match &*state {
                DeferredState::Pending => self.ready.wait(&mut state),
                DeferredState::Ready(bundle) => return Ok(bundle.clone()),
                DeferredState::Failed(err) => return Err(err.clone()),
            }
        }
    }
}

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        match &*state {
            DeferredState::Pending => {
                f.debug_struct("DeferredTrieData").field("state", &"pending").finish()
            }
            DeferredState::Ready(_) => {
                f.debug_struct("DeferredTrieData").field("state", &"ready").finish()
            }
            DeferredState::Failed(err) => f
                .debug_struct("DeferredTrieData")
                .field("state", &format_args!("failed({err})"))
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc, Barrier},
        thread,
        time::{Duration, Instant},
    };

    fn empty_bundle() -> ComputedTrieData {
        ComputedTrieData {
            hashed_state: Arc::default(),
            trie_updates: Arc::default(),
            anchor_hash: B256::ZERO,
            trie_input: Arc::new(TrieInputSorted::default()),
        }
    }

    #[test]
    fn resolves_after_set() {
        let deferred = DeferredTrieData::pending();
        let deferred_clone = deferred.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            deferred_clone.set_ready(empty_bundle());
        });

        let result = deferred.wait_cloned().unwrap();
        assert_eq!(result, empty_bundle());
    }

    #[test]
    fn propagates_error() {
        let deferred = DeferredTrieData::pending();
        deferred.set_error(DeferredTrieDataError::new("boom"));

        let err = deferred.wait_cloned().unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    /// Verifies all pending readers block until the deferred data is set, then receive the same
    /// value.
    fn multiple_readers_block_until_ready() {
        let deferred = DeferredTrieData::pending();
        let readers = 5;
        let barrier = Arc::new(Barrier::new(readers + 1));
        let (tx, rx) = mpsc::channel();
        let delay = Duration::from_millis(20);

        for _ in 0..readers {
            let d = deferred.clone();
            let b = barrier.clone();
            let tx = tx.clone();
            thread::spawn(move || {
                // Ensure all readers are queued before any set_ready happens.
                b.wait();
                let start = Instant::now();
                let data = d.wait_cloned().unwrap();
                let elapsed = start.elapsed();
                tx.send((elapsed, data)).unwrap();
            });
        }

        // Let readers reach the barrier, then delay before setting the data.
        barrier.wait();
        thread::sleep(delay);
        deferred.set_ready(empty_bundle());

        for (elapsed, data) in rx.into_iter().take(readers) {
            // Each reader should have blocked for at least `delay`.
            assert!(elapsed >= delay);
            assert_eq!(data, empty_bundle());
        }
    }
}
