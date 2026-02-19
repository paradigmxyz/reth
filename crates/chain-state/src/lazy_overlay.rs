//! Lazy overlay computation for trie input.
//!
//! This module provides [`LazyOverlay`], a type that computes the [`TrieInputSorted`]
//! lazily on first access. This allows execution to start before the trie overlay
//! is fully computed.

use crate::DeferredTrieData;
use alloy_primitives::B256;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::sync::{Arc, OnceLock};
use tracing::{debug, trace};

/// Inputs captured for lazy overlay computation.
#[derive(Clone)]
struct LazyOverlayInputs {
    /// The persisted ancestor hash (anchor) this overlay should be built on.
    anchor_hash: B256,
    /// Deferred trie data handles for all in-memory blocks (newest to oldest).
    blocks: Vec<DeferredTrieData>,
}

/// Lazily computed trie overlay.
///
/// Captures the inputs needed to compute a [`TrieInputSorted`] and defers the actual
/// computation until first access. This is conceptually similar to [`DeferredTrieData`]
/// but for overlay computation.
///
/// # Fast Path vs Slow Path
///
/// - **Fast path**: If the tip block's cached `anchored_trie_input` is ready and its `anchor_hash`
///   matches our expected anchor, we can reuse it directly (O(1)).
/// - **Slow path**: Otherwise, we merge all ancestor blocks' trie data into a new overlay.
#[derive(Clone)]
pub struct LazyOverlay {
    /// Computed result, cached after first access.
    inner: Arc<OnceLock<TrieInputSorted>>,
    /// Inputs for lazy computation.
    inputs: LazyOverlayInputs,
}

impl std::fmt::Debug for LazyOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOverlay")
            .field("anchor_hash", &self.inputs.anchor_hash)
            .field("num_blocks", &self.inputs.blocks.len())
            .field("computed", &self.inner.get().is_some())
            .finish()
    }
}

impl LazyOverlay {
    /// Create a new lazy overlay with the given anchor hash and block handles.
    ///
    /// # Arguments
    ///
    /// * `anchor_hash` - The persisted ancestor hash this overlay is built on top of
    /// * `blocks` - Deferred trie data handles for in-memory blocks (newest to oldest)
    pub fn new(anchor_hash: B256, blocks: Vec<DeferredTrieData>) -> Self {
        Self { inner: Arc::new(OnceLock::new()), inputs: LazyOverlayInputs { anchor_hash, blocks } }
    }

    /// Returns the anchor hash this overlay is built on.
    pub const fn anchor_hash(&self) -> B256 {
        self.inputs.anchor_hash
    }

    /// Returns the number of in-memory blocks this overlay covers.
    pub const fn num_blocks(&self) -> usize {
        self.inputs.blocks.len()
    }

    /// Returns true if the overlay has already been computed.
    pub fn is_computed(&self) -> bool {
        self.inner.get().is_some()
    }

    /// Returns the computed trie input, computing it if necessary.
    ///
    /// The first call triggers computation (which may block waiting for deferred data).
    /// Subsequent calls return the cached result immediately.
    pub fn get(&self) -> &TrieInputSorted {
        self.inner.get_or_init(|| self.compute())
    }

    /// Returns the overlay as (nodes, state) tuple for use with `OverlayStateProviderFactory`.
    pub fn as_overlay(&self) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        let input = self.get();
        (Arc::clone(&input.nodes), Arc::clone(&input.state))
    }

    /// Compute the trie input overlay.
    fn compute(&self) -> TrieInputSorted {
        let anchor_hash = self.inputs.anchor_hash;
        let blocks = &self.inputs.blocks;

        if blocks.is_empty() {
            debug!(target: "chain_state::lazy_overlay", "No in-memory blocks, returning empty overlay");
            return TrieInputSorted::default();
        }

        // Fast path: Check if tip block's overlay is ready and anchor matches.
        // The tip block (first in list) has the cumulative overlay from all ancestors.
        if let Some(tip) = blocks.first() {
            let data = tip.wait_cloned();
            if let Some(anchored) = &data.anchored_trie_input {
                if anchored.anchor_hash == anchor_hash {
                    trace!(target: "chain_state::lazy_overlay", %anchor_hash, "Reusing tip block's cached overlay (fast path)");
                    return (*anchored.trie_input).clone();
                }
                debug!(
                    target: "chain_state::lazy_overlay",
                    computed_anchor = %anchored.anchor_hash,
                    %anchor_hash,
                    "Anchor mismatch, falling back to merge"
                );
            }
        }

        // Slow path: Merge all blocks' trie data into a new overlay.
        debug!(target: "chain_state::lazy_overlay", num_blocks = blocks.len(), "Merging blocks (slow path)");
        Self::merge_blocks(blocks)
    }

    /// Merge all blocks' trie data into a single [`TrieInputSorted`].
    ///
    /// Blocks are ordered newest to oldest.
    fn merge_blocks(blocks: &[DeferredTrieData]) -> TrieInputSorted {
        if blocks.is_empty() {
            return TrieInputSorted::default();
        }

        let state =
            HashedPostStateSorted::merge_batch(blocks.iter().map(|b| b.wait_cloned().hashed_state));
        let nodes =
            TrieUpdatesSorted::merge_batch(blocks.iter().map(|b| b.wait_cloned().trie_updates));

        TrieInputSorted { state, nodes, prefix_sets: Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_tasks::LazyHandle;
    use reth_trie::{updates::TrieUpdates, HashedPostState};

    fn empty_deferred(anchor: B256) -> DeferredTrieData {
        DeferredTrieData::pending(
            LazyHandle::ready(HashedPostState::default()),
            Arc::new(TrieUpdates::default()),
            anchor,
            Vec::new(),
        )
    }

    #[test]
    fn empty_blocks_returns_default() {
        let overlay = LazyOverlay::new(B256::ZERO, vec![]);
        let result = overlay.get();
        assert!(result.state.is_empty());
        assert!(result.nodes.is_empty());
    }

    #[test]
    fn single_block_uses_data_directly() {
        let anchor = B256::random();
        let deferred = empty_deferred(anchor);
        let overlay = LazyOverlay::new(anchor, vec![deferred]);

        assert!(!overlay.is_computed());
        let _ = overlay.get();
        assert!(overlay.is_computed());
    }

    #[test]
    fn cached_after_first_access() {
        let overlay = LazyOverlay::new(B256::ZERO, vec![]);

        // First access computes
        let _ = overlay.get();
        assert!(overlay.is_computed());

        // Second access uses cache
        let _ = overlay.get();
        assert!(overlay.is_computed());
    }
}
