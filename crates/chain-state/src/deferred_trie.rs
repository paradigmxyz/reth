use alloy_primitives::B256;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{
    fmt,
    sync::{Arc, OnceLock},
};

/// Sorted trie data computed for an executed block.
/// These represent the complete set of sorted trie data required to persist
/// block state for, and generate proofs on top of a block.
#[derive(Clone, Debug, Default)]
pub struct ComputedTrieData {
    /// Sorted hashed post-state produced by execution.
    pub hashed_state: Arc<HashedPostStateSorted>,
    /// Sorted trie updates produced by state root computation.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// The persisted ancestor hash this trie input is anchored to.
    pub anchor_hash: Option<B256>,
    /// Trie input constructed from in-memory overlays.
    pub trie_input: Option<Arc<TrieInputSorted>>,
}

impl ComputedTrieData {
    /// Construct a bundle that includes trie input anchored to a persisted ancestor.
    pub const fn with_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
        anchor_hash: B256,
        trie_input: Arc<TrieInputSorted>,
    ) -> Self {
        Self {
            hashed_state,
            trie_updates,
            anchor_hash: Some(anchor_hash),
            trie_input: Some(trie_input),
        }
    }

    /// Construct a bundle without trie input or anchor information.
    pub const fn without_trie_input(
        hashed_state: Arc<HashedPostStateSorted>,
        trie_updates: Arc<TrieUpdatesSorted>,
    ) -> Self {
        Self { hashed_state, trie_updates, anchor_hash: None, trie_input: None }
    }

    /// Returns the anchor hash, if present.
    pub const fn anchor_hash(&self) -> Option<B256> {
        self.anchor_hash
    }

    /// Returns the trie input, if present.
    pub const fn trie_input(&self) -> Option<&Arc<TrieInputSorted>> {
        self.trie_input.as_ref()
    }

    /// Set the trie input and anchor hash.
    pub fn set_trie_input(&mut self, anchor_hash: B256, trie_input: Arc<TrieInputSorted>) {
        self.anchor_hash = Some(anchor_hash);
        self.trie_input = Some(trie_input);
    }

    /// Remove trie input and anchor hash.
    pub fn clear_trie_input(&mut self) {
        self.anchor_hash = None;
        self.trie_input = None;
    }
}

/// Shared handle to asynchronously populated trie data.
///
/// A thin wrapper over `Arc<OnceLock<ComputedTrieData>>` that lets producers call
/// [`DeferredTrieData::set_ready`] once, and consumers block with [`DeferredTrieData::wait_cloned`]
/// until the trie data is available.
#[derive(Clone)]
pub struct DeferredTrieData(Arc<OnceLock<ComputedTrieData>>);

impl Default for DeferredTrieData {
    fn default() -> Self {
        Self::pending()
    }
}

impl DeferredTrieData {
    /// Create a new pending handle that will be completed later via [`Self::set_ready`].
    pub fn pending() -> Self {
        Self(Arc::new(OnceLock::new()))
    }

    /// Create a handle that is already populated with the given [`ComputedTrieData`].
    ///
    /// Useful when trie data is available immediately; [`Self::wait_cloned`] will return without
    /// blocking.
    pub fn ready(bundle: ComputedTrieData) -> Self {
        let data = OnceLock::new();
        data.set(bundle).unwrap(); // Safe: newly created OnceLock
        Self(Arc::new(data))
    }

    /// Populate the handle with the computed trie data.
    ///
    /// Safe to call multiple times; only the first value is stored.
    pub fn set_ready(&self, bundle: ComputedTrieData) {
        let _ = self.0.set(bundle);
    }

    /// Block until data is available and return an owned clone.
    ///
    /// If the value is already set, returns immediately. Multiple callers can wait concurrently and
    /// all receive the same `ComputedTrieData`.
    ///
    /// # Blocking Behavior
    ///
    /// This method blocks the calling thread indefinitely until [`Self::set_ready`] is called.
    /// There is no timeout - if the value is never set, this will block forever.
    ///
    /// # Panics
    ///
    /// If the background task computing trie data panics before calling [`Self::set_ready`],
    /// all waiters will block forever. This is intentional - trie computation failures are
    /// considered unrecoverable and should crash the node.
    ///
    /// # Concurrency
    ///
    /// Multiple threads can wait concurrently. All waiters wake when the value is set,
    /// and each receives a cloned `ComputedTrieData` (Arc clones are cheap).
    pub fn wait_cloned(&self) -> ComputedTrieData {
        self.0.wait().clone()
    }
}

impl fmt::Debug for DeferredTrieData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.0.get().is_some() { "ready" } else { "pending" };
        f.debug_struct("DeferredTrieData").field("state", &state).finish()
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
            anchor_hash: None,
            trie_input: None,
        }
    }

    #[test]
    /// Verifies that `wait_cloned` blocks until `set_ready` is called from another thread.
    fn resolves_after_set() {
        let deferred = DeferredTrieData::pending();
        let deferred_clone = deferred.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            deferred_clone.set_ready(empty_bundle());
        });

        let result = deferred.wait_cloned();
        let expected = empty_bundle();
        assert_eq!(result.hashed_state, expected.hashed_state);
        assert_eq!(result.trie_updates, expected.trie_updates);
        assert_eq!(result.anchor_hash, expected.anchor_hash);
    }

    #[test]
    /// Ensures `ready` returns immediately without blocking.
    fn ready_returns_immediately() {
        let bundle = empty_bundle();
        let deferred = DeferredTrieData::ready(bundle.clone());

        let start = Instant::now();
        let result = deferred.wait_cloned();
        let elapsed = start.elapsed();

        assert_eq!(result.hashed_state, bundle.hashed_state);
        assert_eq!(result.trie_updates, bundle.trie_updates);
        assert_eq!(result.anchor_hash, bundle.anchor_hash);
        // Should return essentially immediately; allow some slack to avoid flakiness.
        assert!(elapsed < Duration::from_millis(20));
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
                let data = d.wait_cloned();
                let elapsed = start.elapsed();
                tx.send((elapsed, data)).unwrap();
            });
        }

        // Let readers reach the barrier, then delay before setting the data.
        barrier.wait();
        thread::sleep(delay);
        deferred.set_ready(empty_bundle());

        let expected = empty_bundle();
        for (elapsed, data) in rx.into_iter().take(readers) {
            // Each reader should have blocked for at least `delay`.
            assert!(elapsed >= delay);
            assert_eq!(data.hashed_state, expected.hashed_state);
            assert_eq!(data.trie_updates, expected.trie_updates);
            assert_eq!(data.anchor_hash, expected.anchor_hash);
        }
    }

    #[test]
    /// Confirms only the first `set_ready` value is stored.
    fn multiple_set_ready_takes_first() {
        let deferred = DeferredTrieData::pending();
        let first = ComputedTrieData {
            // Use `with_last_byte` for distinct, deterministic anchors in tests.
            anchor_hash: Some(B256::with_last_byte(1)),
            ..empty_bundle()
        };
        let second =
            ComputedTrieData { anchor_hash: Some(B256::with_last_byte(2)), ..empty_bundle() };

        deferred.set_ready(first.clone());
        deferred.set_ready(second);

        assert_eq!(deferred.wait_cloned().anchor_hash, first.anchor_hash);
    }

    #[test]
    /// Verifies clones share readiness across handles.
    fn clones_share_state() {
        let deferred = DeferredTrieData::pending();
        let setter = deferred.clone();
        let bundle =
            ComputedTrieData { anchor_hash: Some(B256::with_last_byte(3)), ..empty_bundle() };

        thread::spawn(move || setter.set_ready(bundle));

        assert_eq!(deferred.wait_cloned().anchor_hash, Some(B256::with_last_byte(3)));
    }

    #[test]
    /// Ensures default initialization creates a pending handle that requires `set_ready`.
    fn default_is_pending() {
        let deferred: DeferredTrieData = Default::default();
        let setter = deferred.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            setter.set_ready(empty_bundle());
        });

        let result = deferred.wait_cloned();
        let expected = empty_bundle();
        assert_eq!(result.hashed_state, expected.hashed_state);
        assert_eq!(result.trie_updates, expected.trie_updates);
        assert_eq!(result.anchor_hash, expected.anchor_hash);
    }

    #[test]
    /// Ensures fast path when data is set before any waiter calls `wait_cloned`.
    fn set_before_wait() {
        let deferred = DeferredTrieData::pending();
        let bundle =
            ComputedTrieData { anchor_hash: Some(B256::with_last_byte(4)), ..empty_bundle() };

        deferred.set_ready(bundle.clone());

        let start = Instant::now();
        let result = deferred.wait_cloned();
        let elapsed = start.elapsed();

        assert_eq!(result.anchor_hash, bundle.anchor_hash);
        assert!(elapsed < Duration::from_millis(20));
    }
}
