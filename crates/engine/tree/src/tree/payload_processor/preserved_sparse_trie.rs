//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_trie_sparse::SparseStateTrie;
use std::{collections::VecDeque, sync::Arc, time::Instant};
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub(super) type SparseTrie = SparseStateTrie;

/// Maximum number of preserved tries to cache.
const PRESERVED_TRIE_CACHE_CAPACITY: usize = 3;

/// Inner storage for preserved sparse tries.
#[derive(Debug, Default)]
struct PreservedTrieCache {
    /// Anchored tries, keyed by their computed state root, most-recent-first.
    anchored: VecDeque<(B256, SparseTrie)>,
    /// A single cleared trie with preserved allocations as fallback.
    cleared: Option<SparseTrie>,
}

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieCacheTask`](super::sparse_trie::SparseTrieCacheTask) for trie reuse.
///
/// Maintains a bounded cache of anchored tries keyed by state root, enabling trie reuse
/// across short reorgs and parallel payload validations.
#[derive(Debug, Default, Clone)]
pub(super) struct SharedPreservedSparseTrie(Arc<Mutex<PreservedTrieCache>>);

impl SharedPreservedSparseTrie {
    /// Takes the best matching trie for the given parent state root.
    ///
    /// Priority:
    /// 1. Exact anchored match (`state_root` == `parent_state_root`)
    /// 2. Cleared trie with preserved allocations
    /// 3. Evict oldest anchored trie and clear it for allocation reuse
    pub(super) fn take_for_parent(&self, parent_state_root: B256) -> Option<SparseTrie> {
        let mut cache = self.0.lock();

        // 1) Exact match
        if let Some(idx) = cache.anchored.iter().position(|(root, _)| *root == parent_state_root) {
            let (_, trie) = cache.anchored.remove(idx).unwrap();
            debug!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                "Reusing anchored sparse trie for continuation payload"
            );
            return Some(trie);
        }

        // 2) Cleared fallback
        if let Some(trie) = cache.cleared.take() {
            debug!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                "Using cleared sparse trie with preserved allocations"
            );
            return Some(trie);
        }

        // 3) Evict oldest and clear
        if let Some((evicted_root, mut trie)) = cache.anchored.pop_back() {
            debug!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                %evicted_root,
                "Clearing evicted anchored sparse trie - parent state root mismatch"
            );
            trie.clear();
            return Some(trie);
        }

        None
    }

    /// Acquires a guard that blocks `take_for_parent()` until dropped.
    /// Use this before sending the state root result to ensure the next block
    /// waits for the trie to be stored.
    pub(super) fn lock(&self) -> PreservedTrieGuard<'_> {
        PreservedTrieGuard(self.0.lock())
    }

    /// Waits until the sparse trie lock becomes available.
    ///
    /// This acquires and immediately releases the lock, ensuring that any
    /// ongoing operations complete before returning. Useful for synchronization
    /// before starting payload processing.
    ///
    /// Returns the time spent waiting for the lock.
    pub(super) fn wait_for_availability(&self) -> std::time::Duration {
        let start = Instant::now();
        let _guard = self.0.lock();
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
}

/// Guard that holds the lock on the preserved trie cache.
/// While held, `take_for_parent()` will block. Call `store_anchored()` or `store_cleared()`
/// to save a trie before dropping.
pub(super) struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, PreservedTrieCache>);

impl PreservedTrieGuard<'_> {
    /// Stores an anchored trie for future reuse.
    ///
    /// Returns any evicted tries that should be dropped outside the lock.
    pub(super) fn store_anchored(&mut self, state_root: B256, trie: SparseTrie) -> Vec<SparseTrie> {
        let mut evicted = Vec::new();
        // Remove any existing entry for this root
        if let Some(idx) = self.0.anchored.iter().position(|(root, _)| *root == state_root) {
            let (_, old_trie) = self.0.anchored.remove(idx).unwrap();
            evicted.push(old_trie);
        }
        // Push to front (most recent)
        self.0.anchored.push_front((state_root, trie));
        // Evict oldest if over capacity
        while self.0.anchored.len() > PRESERVED_TRIE_CACHE_CAPACITY {
            if let Some((_, evicted_trie)) = self.0.anchored.pop_back() {
                evicted.push(evicted_trie);
            }
        }
        evicted
    }

    /// Stores a cleared trie with preserved allocations.
    pub(super) fn store_cleared(&mut self, trie: SparseTrie) {
        self.0.cleared = Some(trie);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_default_trie() -> SparseTrie {
        SparseStateTrie::new()
    }

    #[test]
    fn take_exact_anchored_match() {
        let shared = SharedPreservedSparseTrie::default();
        let root1 = B256::from([1u8; 32]);
        let root2 = B256::from([2u8; 32]);
        let root3 = B256::from([3u8; 32]);

        {
            let mut guard = shared.lock();
            guard.store_anchored(root1, new_default_trie());
            guard.store_anchored(root2, new_default_trie());
            guard.store_anchored(root3, new_default_trie());
        }

        // Should find exact match for root2
        let trie = shared.take_for_parent(root2);
        assert!(trie.is_some(), "expected exact match for root2");

        // root1 and root3 should still be present
        let trie1 = shared.take_for_parent(root1);
        assert!(trie1.is_some(), "expected root1 still present");
        let trie3 = shared.take_for_parent(root3);
        assert!(trie3.is_some(), "expected root3 still present");
    }

    #[test]
    fn fallback_to_cleared_trie() {
        let shared = SharedPreservedSparseTrie::default();
        let root = B256::from([1u8; 32]);
        let query = B256::from([99u8; 32]);

        {
            let mut guard = shared.lock();
            guard.store_anchored(root, new_default_trie());
            guard.store_cleared(new_default_trie());
        }

        // Query for non-existent root should prefer cleared trie over evicting anchored
        let trie = shared.take_for_parent(query);
        assert!(trie.is_some(), "expected cleared fallback");

        // Anchored should still be present
        let trie1 = shared.take_for_parent(root);
        assert!(trie1.is_some(), "expected anchored trie still present");
    }

    #[test]
    fn evict_oldest_when_no_match_and_no_cleared() {
        let shared = SharedPreservedSparseTrie::default();
        let root1 = B256::from([1u8; 32]);
        let root2 = B256::from([2u8; 32]);
        let query = B256::from([99u8; 32]);

        {
            let mut guard = shared.lock();
            guard.store_anchored(root1, new_default_trie());
            guard.store_anchored(root2, new_default_trie());
        }

        // No cleared trie, no exact match — should evict oldest (root1)
        let trie = shared.take_for_parent(query);
        assert!(trie.is_some(), "expected evicted trie");

        // root2 should still be present, root1 evicted
        let trie2 = shared.take_for_parent(root2);
        assert!(trie2.is_some(), "expected root2 still present");

        let trie1 = shared.take_for_parent(root1);
        assert!(trie1.is_none(), "expected root1 to be evicted");
    }

    #[test]
    fn capacity_bound_respected() {
        let shared = SharedPreservedSparseTrie::default();
        let roots: Vec<B256> = (0..5).map(|i| B256::from([i as u8; 32])).collect();

        {
            let mut guard = shared.lock();
            for root in &roots {
                guard.store_anchored(*root, new_default_trie());
            }
        }

        // Only the most recent PRESERVED_TRIE_CACHE_CAPACITY entries should remain
        // Stored in order: root0, root1, root2, root3, root4
        // Most recent (front): root4, root3, root2
        // Evicted: root0, root1
        let trie4 = shared.take_for_parent(roots[4]);
        assert!(trie4.is_some(), "expected root4 present");

        let trie3 = shared.take_for_parent(roots[3]);
        assert!(trie3.is_some(), "expected root3 present");

        let trie2 = shared.take_for_parent(roots[2]);
        assert!(trie2.is_some(), "expected root2 present");

        // root0 and root1 were evicted — no more entries left
        let trie1 = shared.take_for_parent(roots[1]);
        assert!(trie1.is_none(), "expected root1 evicted");
    }

    #[test]
    fn empty_cache_returns_none() {
        let shared = SharedPreservedSparseTrie::default();
        let query = B256::from([42u8; 32]);

        let trie = shared.take_for_parent(query);
        assert!(trie.is_none(), "expected None from empty cache");
    }

    #[test]
    fn store_anchored_deduplicates() {
        let shared = SharedPreservedSparseTrie::default();
        let root = B256::from([1u8; 32]);

        {
            let mut guard = shared.lock();
            guard.store_anchored(root, new_default_trie());
            guard.store_anchored(root, new_default_trie());
            guard.store_anchored(root, new_default_trie());
        }

        // Take the one entry
        let trie = shared.take_for_parent(root);
        assert!(trie.is_some());

        // Should be empty now (no duplicates)
        let trie2 = shared.take_for_parent(root);
        assert!(trie2.is_none(), "expected no duplicates");
    }
}
