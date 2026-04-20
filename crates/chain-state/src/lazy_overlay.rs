//! Lazy overlay computation for trie input.
//!
//! This module provides [`LazyOverlay`], a type that computes the [`TrieInputSorted`]
//! lazily on first access. This allows execution to start before the trie overlay
//! is fully computed.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::B256;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::sync::{Arc, OnceLock};
use tracing::{debug, trace};

/// Inputs captured for lazy overlay computation.
#[derive(Clone)]
struct LazyOverlayInputs<N: NodePrimitives = EthPrimitives> {
    /// In-memory blocks from tip to anchor child.
    ///
    /// Blocks must be provided in reverse chain order (newest to oldest). The overlay anchor is
    /// derived from the last block's parent hash.
    blocks: Vec<ExecutedBlock<N>>,
}

/// Lazily computed trie overlay.
///
/// Captures the inputs needed to compute a [`TrieInputSorted`] and defers the actual
/// computation until first access.
///
/// Blocks must be provided in reverse chain order (newest to oldest), so the first block is the
/// chain tip and the last block is the child of the persisted anchor. The anchor hash for the
/// overlay is derived from `blocks.last().parent_hash()`.
///
/// # Fast Path vs Slow Path
///
/// - **Fast path**: If the tip block's cached `anchored_trie_input` is ready and its `anchor_hash`
///   matches our expected anchor, we can reuse it directly (O(1)).
/// - **Slow path**: Otherwise, we merge all ancestor blocks' trie data into a new overlay.
#[derive(Clone)]
pub struct LazyOverlay<N: NodePrimitives = EthPrimitives> {
    /// Computed result, cached after first access.
    inner: Arc<OnceLock<TrieInputSorted>>,
    /// Inputs for lazy computation.
    inputs: LazyOverlayInputs<N>,
}

impl<N: NodePrimitives> std::fmt::Debug for LazyOverlay<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOverlay")
            .field("anchor_hash", &self.anchor_hash())
            .field("num_blocks", &self.inputs.blocks.len())
            .field("computed", &self.inner.get().is_some())
            .finish()
    }
}

impl<N: NodePrimitives> LazyOverlay<N> {
    /// Create a new lazy overlay from in-memory blocks.
    ///
    /// # Arguments
    ///
    /// * `blocks` - Executed blocks in reverse chain order (newest to oldest)
    pub fn new(blocks: Vec<ExecutedBlock<N>>) -> Self {
        debug_assert!(
            blocks.windows(2).all(|window| {
                window[0].recovered_block().parent_hash() == window[1].recovered_block().hash()
            }),
            "LazyOverlay blocks must be ordered newest to oldest along a single chain"
        );

        Self { inner: Arc::new(OnceLock::new()), inputs: LazyOverlayInputs { blocks } }
    }

    /// Returns the anchor hash this overlay is built on.
    pub fn anchor_hash(&self) -> Option<B256> {
        self.inputs.blocks.last().map(|block| block.recovered_block().parent_hash())
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
        let blocks = &self.inputs.blocks;

        let Some(anchor_hash) = self.anchor_hash() else {
            debug!(target: "chain_state::lazy_overlay", "No in-memory blocks, returning empty overlay");
            return TrieInputSorted::default();
        };

        // Fast path: Check if tip block's overlay is ready and anchor matches.
        // The tip block (first in list) has the cumulative overlay from all ancestors.
        if let Some(tip) = blocks.first() {
            let data = tip.trie_data();
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
    fn merge_blocks(blocks: &[ExecutedBlock<N>]) -> TrieInputSorted {
        if blocks.is_empty() {
            return TrieInputSorted::default();
        }

        let trie_data: Vec<_> = blocks.iter().map(|block| block.trie_data()).collect();
        let state = HashedPostStateSorted::merge_batch(
            trie_data.iter().map(|trie_data| Arc::clone(&trie_data.hashed_state)),
        );
        let nodes = TrieUpdatesSorted::merge_batch(
            trie_data.iter().map(|trie_data| Arc::clone(&trie_data.trie_updates)),
        );

        TrieInputSorted { state, nodes, prefix_sets: Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, EthPrimitives};

    #[test]
    fn empty_blocks_returns_default() {
        let overlay = LazyOverlay::<EthPrimitives>::new(vec![]);
        let result = overlay.get();
        assert!(result.state.is_empty());
        assert!(result.nodes.is_empty());
    }

    #[test]
    fn single_block_uses_data_directly() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let overlay = LazyOverlay::new(vec![block]);

        assert!(!overlay.is_computed());
        let _ = overlay.get();
        assert!(overlay.is_computed());
    }

    #[test]
    fn cached_after_first_access() {
        let overlay = LazyOverlay::<EthPrimitives>::new(vec![]);

        // First access computes
        let _ = overlay.get();
        assert!(overlay.is_computed());

        // Second access uses cache
        let _ = overlay.get();
        assert!(overlay.is_computed());
    }

    #[test]
    fn anchor_hash_comes_from_oldest_parent() {
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..3).collect();
        let overlay = LazyOverlay::new(blocks.into_iter().rev().collect());

        assert_eq!(overlay.anchor_hash(), Some(B256::ZERO));
    }

    #[test]
    #[should_panic(
        expected = "LazyOverlay blocks must be ordered newest to oldest along a single chain"
    )]
    fn misordered_blocks_panic() {
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..3).collect();
        let _ = LazyOverlay::new(blocks);
    }
}
