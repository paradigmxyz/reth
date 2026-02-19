//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use parking_lot::{Condvar, Mutex};
use reth_trie_sparse::SparseStateTrie;
use std::{sync::Arc, time::Instant};
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub(super) type SparseTrie = SparseStateTrie;

/// Inner state protected by the mutex.
#[derive(Debug, Default)]
struct PreservedTrieInner {
    /// Whether the trie is currently being prepared (expensive work in progress).
    /// When true, `take()` and `wait_for_availability()` will block on the condvar.
    preparing: bool,
    /// The preserved trie slot.
    slot: Option<PreservedSparseTrie>,
}

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieCacheTask`](super::sparse_trie::SparseTrieCacheTask) for trie reuse.
///
/// Uses a condvar-backed "preparing" flag to allow expensive trie preparation work
/// (prune + shrink) to happen outside the mutex while still blocking consumers.
#[derive(Debug, Clone)]
pub(super) struct SharedPreservedSparseTrie(Arc<(Mutex<PreservedTrieInner>, Condvar)>);

impl Default for SharedPreservedSparseTrie {
    fn default() -> Self {
        Self(Arc::new((Mutex::new(PreservedTrieInner::default()), Condvar::new())))
    }
}

impl SharedPreservedSparseTrie {
    /// Takes the preserved trie if present, leaving `None` in its place.
    ///
    /// Blocks while the trie is being prepared (preparing flag is set).
    pub(super) fn take(&self) -> Option<PreservedSparseTrie> {
        let (lock, cvar) = &*self.0;
        let mut inner = lock.lock();
        while inner.preparing {
            cvar.wait(&mut inner);
        }
        inner.slot.take()
    }

    /// Acquires a guard that blocks `take()` until dropped.
    /// Use this before sending the state root result to ensure the next block
    /// waits for the trie to be stored.
    pub(super) fn lock(&self) -> PreservedTrieGuard<'_> {
        let (lock, cvar) = &*self.0;
        PreservedTrieGuard { inner: lock.lock(), cvar }
    }

    /// Waits until the sparse trie becomes available (not locked and not preparing).
    ///
    /// This acquires the lock and waits for the preparing flag to clear, ensuring
    /// that any ongoing operations complete before returning. Useful for
    /// synchronization before starting payload processing.
    ///
    /// Returns the time spent waiting.
    pub(super) fn wait_for_availability(&self) -> std::time::Duration {
        let start = Instant::now();
        let (lock, cvar) = &*self.0;
        let mut inner = lock.lock();
        while inner.preparing {
            cvar.wait(&mut inner);
        }
        drop(inner);
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "engine::tree::payload_processor",
                blocked_for=?elapsed,
                "Waited for preserved sparse trie to become available"
            );
        }
        elapsed
    }

    /// Creates a [`PreparingGuard`] after the preparing flag has already been set.
    ///
    /// The returned RAII guard will clear the preparing flag and notify waiters
    /// if dropped without calling [`PreparingGuard::store`] (panic/early-return safety).
    const fn preparing_guard(&self) -> PreparingGuard<'_> {
        PreparingGuard { shared: self, committed: false }
    }
}

/// Guard that holds the lock on the preserved trie.
/// While held, `take()` will block. Call `store()` to save the trie before dropping,
/// or call `mark_preparing_and_release()` to set the preparing flag and release the lock
/// for expensive work.
pub(super) struct PreservedTrieGuard<'a> {
    inner: parking_lot::MutexGuard<'a, PreservedTrieInner>,
    cvar: &'a Condvar,
}

impl<'a> PreservedTrieGuard<'a> {
    /// Stores a preserved trie for later reuse.
    pub(super) fn store(&mut self, trie: PreservedSparseTrie) {
        self.inner.slot = Some(trie);
        self.inner.preparing = false;
        self.cvar.notify_all();
    }

    /// Sets the preparing flag and releases the mutex lock, returning a [`PreparingGuard`].
    ///
    /// After this call the mutex is unlocked but `take()` / `wait_for_availability()` will
    /// still block on the condvar until [`PreparingGuard::store`] is called (or the guard
    /// is dropped, which clears the flag for safety).
    ///
    /// `shared` must be the same [`SharedPreservedSparseTrie`] this guard was obtained from.
    pub(super) fn mark_preparing_and_release(
        mut self,
        shared: &'a SharedPreservedSparseTrie,
    ) -> PreparingGuard<'a> {
        self.inner.preparing = true;
        drop(self); // releases the mutex
        shared.preparing_guard()
    }
}

/// RAII guard that clears the `preparing` flag on drop unless committed via [`store`](Self::store).
///
/// This ensures that if the task panics or returns early after marking `preparing`,
/// the flag is always cleared and waiters are notified.
pub(super) struct PreparingGuard<'a> {
    shared: &'a SharedPreservedSparseTrie,
    committed: bool,
}

impl Drop for PreparingGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let (lock, cvar) = &*self.shared.0;
            let mut inner = lock.lock();
            inner.preparing = false;
            cvar.notify_all();
        }
    }
}

impl PreparingGuard<'_> {
    /// Stores the prepared trie, clears the preparing flag, and notifies waiters.
    ///
    /// Consumes the guard so the Drop impl does not fire.
    pub(super) fn store(mut self, trie: PreservedSparseTrie) {
        let (lock, cvar) = &*self.shared.0;
        let mut inner = lock.lock();
        inner.slot = Some(trie);
        inner.preparing = false;
        self.committed = true;
        cvar.notify_all();
    }
}

/// A preserved sparse trie that can be reused across payload validations.
///
/// The trie exists in one of two states:
/// - **Anchored**: Has a computed state root and can be reused for payloads whose parent state root
///   matches the anchor.
/// - **Cleared**: Trie data has been cleared but allocations are preserved for reuse.
#[derive(Debug)]
pub(super) enum PreservedSparseTrie {
    /// Trie with a computed state root that can be reused for continuation payloads.
    Anchored {
        /// The sparse state trie (pruned after root computation).
        trie: SparseTrie,
        /// The state root this trie represents (computed from the previous block).
        /// Used to verify continuity: new payload's `parent_state_root` must match this.
        state_root: B256,
    },
    /// Cleared trie with preserved allocations, ready for fresh use.
    Cleared {
        /// The sparse state trie with cleared data but preserved allocations.
        trie: SparseTrie,
    },
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    ///
    /// The `state_root` is the computed state root from the trie, which becomes the
    /// anchor for determining if subsequent payloads can reuse this trie.
    pub(super) const fn anchored(trie: SparseTrie, state_root: B256) -> Self {
        Self::Anchored { trie, state_root }
    }

    /// Creates a cleared preserved trie (allocations preserved, data cleared).
    pub(super) const fn cleared(trie: SparseTrie) -> Self {
        Self::Cleared { trie }
    }

    /// Consumes self and returns the trie for reuse.
    ///
    /// If the preserved trie is anchored and the parent state root matches, the pruned
    /// trie structure is reused directly. Otherwise, the trie is cleared but allocations
    /// are preserved to reduce memory overhead.
    pub(super) fn into_trie_for(self, parent_state_root: B256) -> SparseTrie {
        match self {
            Self::Anchored { trie, state_root } if state_root == parent_state_root => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %state_root,
                    "Reusing anchored sparse trie for continuation payload"
                );
                trie
            }
            Self::Anchored { mut trie, state_root } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    anchor_root = %state_root,
                    %parent_state_root,
                    "Clearing anchored sparse trie - parent state root mismatch"
                );
                trie.clear();
                trie
            }
            Self::Cleared { trie } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    "Using cleared sparse trie with preserved allocations"
                );
                trie
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc as StdArc,
        },
        thread,
        time::Duration,
    };

    #[test]
    fn take_blocks_while_preparing_then_unblocks_after_store() {
        let shared = SharedPreservedSparseTrie::default();
        let took = StdArc::new(AtomicBool::new(false));

        // Mark preparing via the guard flow
        {
            let mut guard = shared.lock();
            guard.inner.preparing = true;
        }

        let preparing_guard = shared.preparing_guard();

        // Spawn a thread that tries to take – it should block
        let shared_clone = shared.clone();
        let took_clone = took.clone();
        let handle = thread::spawn(move || {
            let result = shared_clone.take();
            took_clone.store(true, Ordering::SeqCst);
            result
        });

        // Give the thread time to block
        thread::sleep(Duration::from_millis(50));
        assert!(!took.load(Ordering::SeqCst), "take() should be blocked while preparing");

        // Store a cleared trie to unblock
        let trie = SparseStateTrie::new();
        preparing_guard.store(PreservedSparseTrie::cleared(trie));

        let result = handle.join().unwrap();
        assert!(took.load(Ordering::SeqCst), "take() should have completed");
        assert!(result.is_some(), "take() should return the stored trie");
    }

    #[test]
    fn wait_for_availability_blocks_while_preparing() {
        let shared = SharedPreservedSparseTrie::default();
        let available = StdArc::new(AtomicBool::new(false));

        // Set preparing
        {
            let mut guard = shared.lock();
            guard.inner.preparing = true;
        }

        let shared_clone = shared.clone();
        let available_clone = available.clone();
        let handle = thread::spawn(move || {
            shared_clone.wait_for_availability();
            available_clone.store(true, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(50));
        assert!(
            !available.load(Ordering::SeqCst),
            "wait_for_availability should block while preparing"
        );

        // Clear preparing
        {
            let (lock, cvar) = &*shared.0;
            let mut inner = lock.lock();
            inner.preparing = false;
            cvar.notify_all();
        }

        handle.join().unwrap();
        assert!(available.load(Ordering::SeqCst));
    }

    #[test]
    fn preparing_guard_drop_clears_flag_on_panic_path() {
        let shared = SharedPreservedSparseTrie::default();
        let unblocked = StdArc::new(AtomicBool::new(false));

        // Set preparing
        {
            let mut guard = shared.lock();
            guard.inner.preparing = true;
        }

        let preparing_guard = shared.preparing_guard();

        // Spawn a thread that waits on take
        let shared_clone = shared.clone();
        let unblocked_clone = unblocked.clone();
        let handle = thread::spawn(move || {
            let _ = shared_clone.take();
            unblocked_clone.store(true, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(50));
        assert!(!unblocked.load(Ordering::SeqCst), "take() should be blocked");

        // Drop the guard without committing (simulates panic/early-return)
        drop(preparing_guard);

        handle.join().unwrap();
        assert!(
            unblocked.load(Ordering::SeqCst),
            "take() should unblock after PreparingGuard drop"
        );

        // Verify preparing is cleared
        let (lock, _) = &*shared.0;
        let inner = lock.lock();
        assert!(!inner.preparing, "preparing flag should be cleared after guard drop");
    }

    #[test]
    fn mark_preparing_and_release_returns_preparing_guard() {
        let shared = SharedPreservedSparseTrie::default();

        let guard = shared.lock();
        let preparing_guard = guard.mark_preparing_and_release(&shared);

        // Verify preparing is set
        {
            let (lock, _) = &*shared.0;
            let inner = lock.lock();
            assert!(inner.preparing);
        }

        // Store clears it
        preparing_guard.store(PreservedSparseTrie::cleared(SparseStateTrie::new()));

        let (lock, _) = &*shared.0;
        let inner = lock.lock();
        assert!(!inner.preparing);
        assert!(inner.slot.is_some());
    }
}
