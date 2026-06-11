//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use parking_lot::Mutex;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie::updates::TrieUpdates;
use reth_trie_sparse::{ConfigurableSparseTrie, SparseStateTrie};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use tracing::{debug, warn};

const MAX_PUBLISHED_PAYLOAD_BUILDER_SPARSE_TRIES: usize = 64;

/// Type alias for the sparse trie type used in preservation.
pub(super) type SparseTrie = SparseStateTrie<ConfigurableSparseTrie, ConfigurableSparseTrie>;

/// Shared handle to a preserved sparse trie that can be reused across payload validations.
///
/// This is stored in [`PayloadProcessor`](super::PayloadProcessor) and cloned to pass to
/// [`SparseTrieCacheTask`](super::sparse_trie::SparseTrieCacheTask) for trie reuse.
#[derive(Debug, Default, Clone)]
pub(crate) struct SharedPreservedSparseTrie(Arc<Mutex<Option<PreservedSparseTrie>>>);

impl SharedPreservedSparseTrie {
    /// Creates a shared preserved trie handle initialized with the provided trie.
    pub(super) fn new(preserved: Option<PreservedSparseTrie>) -> Self {
        Self(Arc::new(Mutex::new(preserved)))
    }

    /// Takes the preserved trie if present, leaving `None` in its place.
    pub(super) fn take(&self) -> Option<PreservedSparseTrie> {
        self.0.lock().take()
    }

    /// Installs a payload-builder sparse trie as the validation trie for an inserted block.
    ///
    /// The source trie must be anchored at `child_state_root`. If validation's current trie is
    /// anchored at the inserted block's parent, the install advances validation's preserved trie
    /// without forcing the next validation to clear and rebuild from scratch.
    pub(super) fn install_payload_builder_child(
        &self,
        parent_state_root: B256,
        child_state_root: B256,
        source: SharedPreservedSparseTrie,
    ) -> bool {
        let Some(child) = source.take() else {
            debug!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                %child_state_root,
                "payload-builder sparse trie source was not available for fast-path install"
            );
            return false;
        };

        let Some(source_state_root) = child.state_root() else {
            debug!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                %child_state_root,
                "payload-builder sparse trie source was not anchored for fast-path install"
            );
            return false;
        };

        if source_state_root != child_state_root {
            warn!(
                target: "engine::tree::payload_processor",
                %parent_state_root,
                %child_state_root,
                %source_state_root,
                "payload-builder sparse trie source root mismatch during fast-path install"
            );
            return false;
        }

        let mut preserved = self.0.lock();
        match preserved.as_ref().and_then(PreservedSparseTrie::state_root) {
            Some(current_state_root) if current_state_root == child_state_root => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    %child_state_root,
                    "validation sparse trie is already anchored at inserted block"
                );
                true
            }
            Some(current_state_root) if current_state_root == parent_state_root => {
                preserved.replace(child);
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    %child_state_root,
                    "installed payload-builder sparse trie for fast-path inserted block"
                );
                true
            }
            None => {
                preserved.replace(child);
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    %child_state_root,
                    "installed payload-builder sparse trie into empty validation cache"
                );
                true
            }
            Some(current_state_root) => {
                debug!(
                    target: "engine::tree::payload_processor",
                    %parent_state_root,
                    %child_state_root,
                    %current_state_root,
                    "not installing payload-builder sparse trie because validation cache is anchored elsewhere"
                );
                false
            }
        }
    }

    /// Clones the preserved trie into a private handle with trie updates applied.
    ///
    /// If the preserved trie is not anchored at `base_state_root`, this returns `None` so callers
    /// can fall back to a fresh sparse trie rather than cloning unrelated state.
    pub(super) fn clone_with_updates(
        &self,
        base_state_root: B256,
        updated_state_root: B256,
        trie_updates: &TrieUpdates,
    ) -> Option<PreservedSparseTrie> {
        self.0.lock().as_ref().and_then(|preserved| {
            preserved.clone_with_updates(base_state_root, updated_state_root, trie_updates)
        })
    }

    /// Acquires a guard that blocks `take()` until dropped.
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

/// Completed private payload-builder sparse tries that may be installed by fast-path insertion.
#[derive(Debug, Default, Clone)]
pub(super) struct PublishedPayloadBuilderSparseTries(
    Arc<Mutex<PublishedPayloadBuilderSparseTriesInner>>,
);

#[derive(Debug, Default)]
struct PublishedPayloadBuilderSparseTriesInner {
    order: VecDeque<B256>,
    tries: HashMap<B256, SharedPreservedSparseTrie>,
}

impl PublishedPayloadBuilderSparseTries {
    /// Publishes a completed private sparse trie keyed by its computed state root.
    pub(super) fn publish(&self, state_root: B256, trie: SharedPreservedSparseTrie) {
        let mut inner = self.0.lock();
        if inner.tries.insert(state_root, trie).is_some() {
            inner.order.retain(|root| *root != state_root);
        }
        inner.order.push_back(state_root);

        while inner.order.len() > MAX_PUBLISHED_PAYLOAD_BUILDER_SPARSE_TRIES {
            if let Some(evicted) = inner.order.pop_front() {
                inner.tries.remove(&evicted);
            }
        }

        debug!(
            target: "engine::tree::payload_processor",
            %state_root,
            published_sparse_tries = inner.tries.len(),
            "published payload-builder sparse trie for fast-path reuse"
        );
    }

    /// Takes a published private sparse trie for the given state root.
    pub(super) fn take(&self, state_root: B256) -> Option<SharedPreservedSparseTrie> {
        let mut inner = self.0.lock();
        inner.order.retain(|root| *root != state_root);
        inner.tries.remove(&state_root)
    }
}

/// Guard that holds the lock on the preserved trie.
/// While held, `take()` will block. Call `store()` to save the trie before dropping.
pub(super) struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, Option<PreservedSparseTrie>>);

impl PreservedTrieGuard<'_> {
    /// Stores a preserved trie for later reuse.
    pub(super) fn store(&mut self, trie: PreservedSparseTrie) {
        self.0.replace(trie);
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

    pub(super) const fn state_root(&self) -> Option<B256> {
        match self {
            Self::Anchored { state_root, .. } => Some(*state_root),
            Self::Cleared { .. } => None,
        }
    }

    /// Clones this preserved trie if it can safely represent the updated root.
    ///
    /// This is used by speculative payload building: validation keeps ownership of the shared
    /// preserved trie while the builder receives a private copy for the speculative parent root.
    ///
    /// Trie updates only describe intermediate trie node changes, not account/storage leaf
    /// post-state. Applying non-empty updates to a cloned base-root sparse trie would make the
    /// clone appear anchored at `updated_state_root` while still containing `base_state_root` leaf
    /// values. If the preserved trie is already anchored at `updated_state_root`, it represents the
    /// validated post-state and can be cloned directly.
    fn clone_with_updates(
        &self,
        base_state_root: B256,
        updated_state_root: B256,
        trie_updates: &TrieUpdates,
    ) -> Option<Self> {
        match self {
            Self::Anchored { trie, state_root } if *state_root == updated_state_root => {
                Some(Self::Anchored {
                    trie: trie.clone_for_reuse(),
                    state_root: updated_state_root,
                })
            }
            Self::Anchored { trie, state_root } if *state_root == base_state_root => {
                if updated_state_root != base_state_root || !trie_updates.is_empty() {
                    debug!(
                        target: "engine::tree::payload_processor",
                        %base_state_root,
                        %updated_state_root,
                        root_changed = updated_state_root != base_state_root,
                        account_nodes = trie_updates.account_nodes.len(),
                        removed_nodes = trie_updates.removed_nodes.len(),
                        storage_tries = trie_updates.storage_tries.len(),
                        "not cloning preserved sparse trie because parent update requires leaf post-state"
                    );
                    return None;
                }

                Some(Self::Anchored {
                    trie: trie.clone_for_reuse(),
                    state_root: updated_state_root,
                })
            }
            Self::Anchored { state_root, .. } => {
                debug!(
                    target: "engine::tree::payload_processor",
                    anchor_root = %state_root,
                    %base_state_root,
                    %updated_state_root,
                    "not cloning preserved sparse trie because anchor root matches neither update base nor updated root"
                );
                None
            }
            Self::Cleared { trie } => Some(Self::Cleared { trie: trie.clone_for_reuse() }),
        }
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
    use reth_trie_common::Nibbles;

    #[test]
    fn clone_with_updates_reuses_anchored_trie_for_empty_updates() {
        let base_state_root = B256::with_last_byte(1);
        let updated_state_root = base_state_root;
        let preserved = PreservedSparseTrie::anchored(SparseTrie::default(), base_state_root);

        let cloned = preserved
            .clone_with_updates(base_state_root, updated_state_root, &TrieUpdates::default())
            .expect("empty updates can reuse anchored sparse trie");

        assert!(matches!(
            cloned,
            PreservedSparseTrie::Anchored { state_root, .. } if state_root == updated_state_root
        ));
    }

    #[test]
    fn clone_with_updates_rejects_changed_root_without_updates_for_anchored_trie() {
        let base_state_root = B256::with_last_byte(1);
        let updated_state_root = B256::with_last_byte(2);
        let preserved = PreservedSparseTrie::anchored(SparseTrie::default(), base_state_root);

        assert!(preserved
            .clone_with_updates(base_state_root, updated_state_root, &TrieUpdates::default())
            .is_none());
    }

    #[test]
    fn clone_with_updates_rejects_non_empty_updates_for_anchored_trie() {
        let base_state_root = B256::with_last_byte(1);
        let updated_state_root = B256::with_last_byte(2);
        let preserved = PreservedSparseTrie::anchored(SparseTrie::default(), base_state_root);
        let mut trie_updates = TrieUpdates::default();
        trie_updates.removed_nodes.insert(Nibbles::from_nibbles_unchecked([0x01]));

        assert!(preserved
            .clone_with_updates(base_state_root, updated_state_root, &trie_updates)
            .is_none());
    }

    #[test]
    fn clone_with_updates_reuses_trie_already_anchored_at_updated_root() {
        let base_state_root = B256::with_last_byte(1);
        let updated_state_root = B256::with_last_byte(2);
        let preserved = PreservedSparseTrie::anchored(SparseTrie::default(), updated_state_root);
        let mut trie_updates = TrieUpdates::default();
        trie_updates.removed_nodes.insert(Nibbles::from_nibbles_unchecked([0x01]));

        let cloned = preserved
            .clone_with_updates(base_state_root, updated_state_root, &trie_updates)
            .expect("validated post-state trie can be cloned for speculative builder");

        assert!(matches!(
            cloned,
            PreservedSparseTrie::Anchored { state_root, .. } if state_root == updated_state_root
        ));
    }

    #[test]
    fn clone_with_updates_rejects_unrelated_anchor() {
        let base_state_root = B256::with_last_byte(1);
        let updated_state_root = B256::with_last_byte(2);
        let unrelated_state_root = B256::with_last_byte(3);
        let preserved = PreservedSparseTrie::anchored(SparseTrie::default(), unrelated_state_root);

        assert!(preserved
            .clone_with_updates(base_state_root, updated_state_root, &TrieUpdates::default())
            .is_none());
    }

    #[test]
    fn install_payload_builder_child_advances_parent_anchor() {
        let parent_state_root = B256::with_last_byte(1);
        let child_state_root = B256::with_last_byte(2);
        let validation = SharedPreservedSparseTrie::new(Some(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            parent_state_root,
        )));
        let source = SharedPreservedSparseTrie::new(Some(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            child_state_root,
        )));

        assert!(validation.install_payload_builder_child(
            parent_state_root,
            child_state_root,
            source
        ));

        assert_eq!(
            validation.0.lock().as_ref().and_then(PreservedSparseTrie::state_root),
            Some(child_state_root)
        );
    }

    #[test]
    fn install_payload_builder_child_rejects_unrelated_anchor() {
        let parent_state_root = B256::with_last_byte(1);
        let child_state_root = B256::with_last_byte(2);
        let unrelated_state_root = B256::with_last_byte(3);
        let validation = SharedPreservedSparseTrie::new(Some(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            unrelated_state_root,
        )));
        let source = SharedPreservedSparseTrie::new(Some(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            child_state_root,
        )));

        assert!(!validation.install_payload_builder_child(
            parent_state_root,
            child_state_root,
            source
        ));

        assert_eq!(
            validation.0.lock().as_ref().and_then(PreservedSparseTrie::state_root),
            Some(unrelated_state_root)
        );
    }

    #[test]
    fn published_payload_builder_sparse_tries_take_removes_entry() {
        let state_root = B256::with_last_byte(1);
        let published = PublishedPayloadBuilderSparseTries::default();
        let source = SharedPreservedSparseTrie::new(Some(PreservedSparseTrie::anchored(
            SparseTrie::default(),
            state_root,
        )));

        published.publish(state_root, source);

        assert!(published.take(state_root).is_some());
        assert!(published.take(state_root).is_none());
    }
}
