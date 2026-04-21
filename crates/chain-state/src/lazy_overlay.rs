//! Lazy overlay computation for trie input.
//!
//! This module provides [`LazyOverlay`], a type that computes the [`TrieInputSorted`]
//! lazily on first access. This allows execution to start before the trie overlay
//! is fully computed.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::B256;
use reth_primitives_traits::{
    dashmap::{self, DashMap},
    AlloyBlockHeader, NodePrimitives,
};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::sync::Arc;
use tracing::{debug, trace};

/// Inputs captured for lazy overlay computation.
#[derive(Clone)]
struct LazyOverlayInputs<N: NodePrimitives = EthPrimitives> {
    /// In-memory blocks from tip to anchor child.
    ///
    /// Blocks must be provided in reverse chain order (newest to oldest).
    blocks: Vec<ExecutedBlock<N>>,
}

/// Lazily computed trie overlay.
///
/// Captures the inputs needed to compute a [`TrieInputSorted`] and defers the actual
/// computation until first access.
///
/// Blocks must be provided in reverse chain order (newest to oldest), so the first block is the
/// chain tip and the last block is the oldest in-memory block in the chain segment.
///
/// # Fast Path vs Slow Path
///
/// - **Fast path**: If the tip block's cached `anchored_trie_input` is ready and its `anchor_hash`
///   matches our expected anchor, we can reuse it directly (O(1)).
/// - **Slow path**: Otherwise, we merge all ancestor blocks' trie data into a new overlay.
#[derive(Clone)]
pub struct LazyOverlay<N: NodePrimitives = EthPrimitives> {
    /// Computed results, cached by requested anchor hash.
    inner: Arc<DashMap<B256, Arc<TrieInputSorted>>>,
    /// Inputs for lazy computation.
    inputs: LazyOverlayInputs<N>,
}

impl<N: NodePrimitives> std::fmt::Debug for LazyOverlay<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOverlay")
            .field(
                "oldest_block_parent_hash",
                &self.inputs.blocks.last().map(|block| block.recovered_block().parent_hash()),
            )
            .field("num_blocks", &self.inputs.blocks.len())
            .field("cached_anchors", &self.inner.len())
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

        Self { inner: Default::default(), inputs: LazyOverlayInputs { blocks } }
    }

    /// Returns the number of in-memory blocks this overlay covers.
    pub const fn num_blocks(&self) -> usize {
        self.inputs.blocks.len()
    }

    /// Returns the oldest anchor hash this overlay can serve.
    ///
    /// This is the parent hash of the oldest block in the stored newest-to-oldest chain segment.
    pub fn anchor_hash(&self) -> Option<B256> {
        self.inputs.blocks.last().map(|block| block.recovered_block().parent_hash())
    }

    /// Returns true if there are no blocks in the overlay, or if one of the blocks has the given
    /// hash as a parent hash.
    pub fn has_anchor_hash(&self, hash: B256) -> bool {
        self.inputs.blocks.is_empty() ||
            self.inputs.blocks.iter().any(|b| b.recovered_block().parent_hash() == hash)
    }

    #[cfg(test)]
    /// Returns true if the overlay has already been computed for the requested anchor.
    pub fn is_computed(&self, anchor_hash: B256) -> bool {
        self.inner.contains_key(&anchor_hash)
    }

    /// Returns the computed trie input for the requested anchor, computing it if necessary.
    ///
    /// The first call triggers computation (which may block waiting for deferred data).
    /// Subsequent calls for the same anchor return the cached result immediately.
    pub fn get(&self, anchor_hash: B256) -> Arc<TrieInputSorted> {
        match self.inner.entry(anchor_hash) {
            dashmap::Entry::Occupied(entry) => Arc::clone(entry.get()),
            dashmap::Entry::Vacant(entry) => {
                let input = self.compute(anchor_hash);
                entry.insert(Arc::clone(&input));
                input
            }
        }
    }

    /// Returns the overlay as (nodes, state) tuple for use with `OverlayStateProviderFactory`.
    pub fn as_overlay(
        &self,
        anchor_hash: B256,
    ) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        let input = self.get(anchor_hash);
        (Arc::clone(&input.nodes), Arc::clone(&input.state))
    }

    /// Compute the trie input overlay.
    fn compute(&self, anchor_hash: B256) -> Arc<TrieInputSorted> {
        let blocks = &self.inputs.blocks;
        if blocks.is_empty() {
            return Default::default()
        }

        let Some(last_index) =
            blocks.iter().position(|block| block.recovered_block().parent_hash() == anchor_hash)
        else {
            panic!(
                "LazyOverlay does not contain a block whose parent hash matches requested anchor {anchor_hash}"
            );
        };
        let blocks = &blocks[..=last_index];

        // Fast path: Check if tip block's overlay is ready and anchor matches.
        // The tip block (first in list) has the cumulative overlay from all ancestors up to the
        // requested anchor.
        if let Some(tip) = blocks.first() {
            let data = tip.trie_data();
            if let Some(anchored) = &data.anchored_trie_input {
                if anchored.anchor_hash == anchor_hash {
                    trace!(target: "chain_state::lazy_overlay", %anchor_hash, "Reusing tip block's cached overlay (fast path)");
                    return Arc::clone(&anchored.trie_input);
                }
                debug!(
                    target: "chain_state::lazy_overlay",
                    computed_anchor = %anchored.anchor_hash,
                    %anchor_hash,
                    "Anchor mismatch, falling back to merge"
                );
            }
        }

        // Slow path: Merge the prefix of blocks from the tip back to the requested anchor.
        debug!(
            target: "chain_state::lazy_overlay",
            %anchor_hash,
            num_blocks = blocks.len(),
            "Merging blocks (slow path)"
        );
        Arc::new(Self::merge_blocks(blocks))
    }

    /// Merge all blocks' trie data into a single [`TrieInputSorted`].
    ///
    /// Blocks are ordered newest to oldest.
    fn merge_blocks(blocks: &[ExecutedBlock<N>]) -> TrieInputSorted {
        if blocks.is_empty() {
            return TrieInputSorted::default();
        }

        let state = HashedPostStateSorted::merge_batch(
            blocks.iter().map(|block| block.trie_data().hashed_state),
        );
        let nodes = TrieUpdatesSorted::merge_batch(
            blocks.iter().map(|block| block.trie_data().trie_updates),
        );

        TrieInputSorted { state, nodes, prefix_sets: Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, ComputedTrieData, EthPrimitives, ExecutedBlock};
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage};
    use std::sync::Arc;

    fn with_unique_state(
        block: &ExecutedBlock<EthPrimitives>,
        id: u8,
    ) -> ExecutedBlock<EthPrimitives> {
        let hashed_address = B256::with_last_byte(id);
        let hashed_slot = B256::with_last_byte(id.saturating_add(32));
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::without_trie_input(
                Arc::new(hashed_state),
                Arc::new(TrieUpdatesSorted::default()),
            ),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    #[test]
    fn single_block_uses_data_directly() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let anchor_hash = block.recovered_block().parent_hash();
        let overlay = LazyOverlay::new(vec![block]);

        assert!(!overlay.is_computed(anchor_hash));
        let _ = overlay.get(anchor_hash);
        assert!(overlay.is_computed(anchor_hash));
    }

    #[test]
    fn caches_results_per_anchor() {
        let blocks = test_blocks();
        let prefix_anchor = blocks[2].recovered_block().hash();
        let full_anchor = blocks[2].recovered_block().parent_hash();
        let overlay = LazyOverlay::new(blocks);

        let prefix = overlay.get(prefix_anchor);
        let full = overlay.get(full_anchor);

        assert!(overlay.is_computed(prefix_anchor));
        assert!(overlay.is_computed(full_anchor));
        assert!(!Arc::ptr_eq(&prefix, &full));
        assert!(Arc::ptr_eq(&prefix, &overlay.get(prefix_anchor)));
        assert!(Arc::ptr_eq(&full, &overlay.get(full_anchor)));
    }

    #[test]
    fn requested_anchor_limits_the_merged_prefix() {
        let blocks = test_blocks();
        let prefix_anchor = blocks[2].recovered_block().hash();
        let expected = LazyOverlay::merge_blocks(&blocks[..2]);
        let overlay = LazyOverlay::new(blocks);
        let actual = overlay.get(prefix_anchor);

        assert_eq!(actual.nodes.as_ref(), expected.nodes.as_ref());
        assert_eq!(actual.state.as_ref(), expected.state.as_ref());
    }

    #[test]
    fn anchor_hash_returns_oldest_served_anchor() {
        let blocks = test_blocks();
        let expected_anchor = blocks.last().unwrap().recovered_block().parent_hash();
        let overlay = LazyOverlay::new(blocks);

        assert_eq!(overlay.anchor_hash(), Some(expected_anchor));
    }

    #[test]
    fn reuses_tip_overlay_when_anchor_matches() {
        let mut blocks = test_blocks();
        let prefix_anchor = blocks[2].recovered_block().hash();
        let tip_overlay = Arc::new(LazyOverlay::merge_blocks(&blocks[..2]));
        let tip_data = blocks[0].trie_data();

        blocks[0] = ExecutedBlock::new(
            Arc::clone(&blocks[0].recovered_block),
            Arc::clone(&blocks[0].execution_output),
            ComputedTrieData::with_trie_input(
                tip_data.hashed_state,
                tip_data.trie_updates,
                prefix_anchor,
                Arc::clone(&tip_overlay),
            ),
        );

        let overlay = LazyOverlay::new(blocks);
        let actual = overlay.get(prefix_anchor);

        assert!(Arc::ptr_eq(&actual, &tip_overlay));
    }

    #[test]
    #[should_panic(
        expected = "LazyOverlay does not contain a block whose parent hash matches requested anchor"
    )]
    fn missing_anchor_panics() {
        let blocks = test_blocks();
        let missing_anchor = blocks[0].recovered_block().hash();
        let overlay = LazyOverlay::new(blocks);

        let _ = overlay.get(missing_anchor);
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
