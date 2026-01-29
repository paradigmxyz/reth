//! LRU-based tracking for sparse trie prune retention.

use alloc::collections::VecDeque;
use reth_trie_common::Nibbles;

/// Tracks recently updated leaf keys for LRU-based prune retention.
///
/// During [`crate::SparseTrieExt::prune`], ancestor nodes of recently updated keys are preserved
/// (not converted to hash stubs) even if they exceed `max_depth`. This is useful for
/// cross-block caching scenarios where hot paths should remain revealed.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct PruneLruTracker {
    /// Maximum number of keys to track.
    capacity: usize,
    /// Circular buffer of recently updated leaf key paths.
    recent_keys: VecDeque<Nibbles>,
}

impl PruneLruTracker {
    /// Creates a new tracker with the given capacity.
    pub(crate) fn new(capacity: usize) -> Self {
        Self { capacity, recent_keys: VecDeque::with_capacity(capacity) }
    }

    /// Returns the configured capacity.
    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Sets the capacity. Does not shrink existing entries.
    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        if capacity > self.recent_keys.capacity() {
            self.recent_keys.reserve(capacity - self.recent_keys.capacity());
        }
    }

    /// Records a recently updated key.
    ///
    /// Call this after successful `update_leaf` or `remove_leaf` operations.
    pub(crate) fn record(&mut self, full_path: Nibbles) {
        if self.capacity == 0 {
            return;
        }
        if self.recent_keys.len() == self.capacity {
            self.recent_keys.pop_front();
        }
        self.recent_keys.push_back(full_path);
    }

    /// Clears all tracked keys without changing capacity.
    pub(crate) fn clear(&mut self) {
        self.recent_keys.clear();
    }

    /// Returns an iterator over the recently updated keys.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Nibbles> {
        self.recent_keys.iter()
    }
}
