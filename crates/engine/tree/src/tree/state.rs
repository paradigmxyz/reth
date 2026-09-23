//! Functionality related to tree state.

use crate::engine::EngineApiKind;
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use reth_chain_state::{BlockState, CanonicalInMemoryState, EthPrimitives, ExecutedBlock};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives, SealedHeader};
use reth_storage_overlay::OverlayManager;
use std::sync::Arc;
use tracing::debug;

/// Keeps track of the state of the tree.
///
/// The executed blocks themselves are tracked by the node's [`CanonicalInMemoryState`], which this
/// type shares with the providers and the [`OverlayManager`]: newly executed blocks go into its
/// pending section, and the engine moves them to its canonical section on forkchoice updates.
///
/// ## Invariants
///
/// - This only tracks blocks that are connected to the canonical chain.
/// - All executed blocks are valid and have been executed.
#[derive(Debug)]
pub struct TreeState<N: NodePrimitives = EthPrimitives> {
    /// Currently tracked canonical head of the chain.
    pub(crate) current_canonical_head: BlockNumHash,
    /// The engine API variant of this handler
    pub(crate) engine_kind: EngineApiKind,
    /// Manages state trie overlays for in-memory blocks.
    pub(crate) overlay_manager: OverlayManager<N>,
    /// Tracks the executed in-memory blocks, canonical and pending.
    ///
    /// This is the in-memory state of the [`OverlayManager`].
    pub(crate) in_memory_state: CanonicalInMemoryState<N>,
}

impl<N: NodePrimitives> Default for TreeState<N> {
    fn default() -> Self {
        Self::new(Default::default(), Default::default(), Default::default())
    }
}

impl<N: NodePrimitives> TreeState<N> {
    /// Returns a new tree state that points to the given canonical head.
    ///
    /// The executed blocks are tracked by the in-memory state of `overlay_manager`.
    pub fn new(
        current_canonical_head: BlockNumHash,
        engine_kind: EngineApiKind,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        let in_memory_state = overlay_manager.in_memory_state().clone();
        Self { current_canonical_head, engine_kind, overlay_manager, in_memory_state }
    }

    /// Resets the state and points to the given canonical head.
    ///
    /// This removes all executed in-memory blocks, canonical and pending.
    pub fn reset(&mut self, current_canonical_head: BlockNumHash) {
        let removed_hashes = self.in_memory_state.clear_state();
        if !removed_hashes.is_empty() {
            self.overlay_manager.on_blocks_removed(removed_hashes);
        }
        self.current_canonical_head = current_canonical_head;
    }

    /// Returns the engine API variant of this handler.
    pub const fn engine_kind(&self) -> EngineApiKind {
        self.engine_kind
    }

    /// Returns the in-memory state that tracks the executed blocks.
    pub const fn in_memory_state(&self) -> &CanonicalInMemoryState<N> {
        &self.in_memory_state
    }

    /// Returns the number of executed blocks stored.
    pub fn block_count(&self) -> usize {
        self.in_memory_state.canonical_block_count() + self.in_memory_state.pending_block_count()
    }

    /// Returns the [`BlockState`] of the executed block with the given hash.
    pub fn executed_state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.in_memory_state.executed_state_by_hash(hash)
    }

    /// Returns the [`ExecutedBlock`] by hash.
    pub fn executed_block_by_hash(&self, hash: B256) -> Option<ExecutedBlock<N>> {
        self.executed_state_by_hash(hash).map(|state| state.block())
    }

    /// Returns `true` if a block with the given hash exists in memory.
    pub fn contains_hash(&self, hash: &B256) -> bool {
        self.executed_state_by_hash(*hash).is_some()
    }

    /// Returns the sealed block header by hash.
    pub fn sealed_header_by_hash(&self, hash: &B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.executed_state_by_hash(*hash)
            .map(|state| state.block_ref().sealed_block().sealed_header().clone())
    }

    /// Returns all available blocks for the given hash that lead back to the canonical chain, from
    /// newest to oldest, and the parent hash of the oldest returned block. This parent hash is the
    /// highest persisted block connected to this chain.
    ///
    /// Returns `None` if the block for the given hash is not found.
    pub fn blocks_by_hash(&self, hash: B256) -> Option<(B256, Vec<ExecutedBlock<N>>)> {
        let state = self.executed_state_by_hash(hash)?;
        let blocks = state.chain().map(BlockState::block).collect::<Vec<_>>();
        let parent_hash = state.anchor().hash;
        Some((parent_hash, blocks))
    }

    /// Insert executed block into the state.
    ///
    /// The block is added to the pending section of the in-memory state until a forkchoice
    /// update makes it canonical.
    pub fn insert_executed(&mut self, executed: ExecutedBlock<N>) {
        let hash = executed.recovered_block().hash();
        let parent_hash = executed.recovered_block().parent_hash();

        if self.contains_hash(&hash) {
            return;
        }

        self.in_memory_state.insert_pending(executed);
        self.overlay_manager.on_block_inserted(hash, parent_hash);
    }

    /// Returns whether or not the hash is part of the canonical chain.
    pub fn is_canonical(&self, hash: B256) -> bool {
        let mut current_block = self.current_canonical_head.hash;
        if current_block == hash {
            return true
        }

        while let Some(executed) = self.executed_state_by_hash(current_block) {
            current_block = executed.block_ref().recovered_block().parent_hash();
            if current_block == hash {
                return true
            }
        }

        false
    }

    /// Remove all blocks up to __and including__ the given block number.
    ///
    /// If a finalized hash is provided, the only non-canonical blocks which will be removed are
    /// those which have a fork point at or below the finalized hash.
    ///
    /// Canonical blocks below the upper bound will still be removed, but only if the last
    /// persisted hash is part of the canonical chain.
    ///
    /// NOTE: if the finalized block is greater than the upper bound, the only blocks that will be
    /// removed are canonical blocks and sidechains that fork below the `upper_bound`. This is the
    /// same behavior as if the `finalized_num` were `Some(upper_bound)`.
    pub fn remove_until(
        &mut self,
        upper_bound: BlockNumHash,
        last_persisted_hash: B256,
        finalized_num_hash: Option<BlockNumHash>,
    ) {
        debug!(target: "engine::tree", ?upper_bound, ?finalized_num_hash, "Removing blocks from the tree");

        // If the finalized num is ahead of the upper bound, and exists, we need to instead ensure
        // that the only blocks removed, are canonical blocks less than the upper bound
        let finalized_num_hash = finalized_num_hash.map(|mut finalized| {
            if upper_bound.number < finalized.number {
                finalized = upper_bound;
                debug!(target: "engine::tree", ?finalized, "Adjusted upper bound");
            }
            finalized
        });

        // We want to do two things:
        // * remove canonical blocks that are persisted
        // * remove forks whose root are below the finalized block
        let mut removed_hashes = self
            .in_memory_state
            .remove_canonical_blocks_until(last_persisted_hash, upper_bound.number);
        debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, removed = removed_hashes.len(), "Removed canonical blocks from the tree");

        if let Some(finalized_num_hash) = finalized_num_hash {
            let pruned = self.in_memory_state.prune_pending_below(finalized_num_hash);
            debug!(target: "engine::tree", ?finalized_num_hash, pruned = pruned.len(), "Removed finalized sidechain blocks");
            removed_hashes.extend(pruned);
        }

        if !removed_hashes.is_empty() {
            self.overlay_manager.on_blocks_removed(removed_hashes);
        }
    }

    /// Updates the canonical head to the given block.
    pub const fn set_canonical_head(&mut self, new_head: BlockNumHash) {
        self.current_canonical_head = new_head;
    }

    /// Returns the tracked canonical head.
    pub const fn canonical_head(&self) -> &BlockNumHash {
        &self.current_canonical_head
    }

    /// Returns the block hash of the canonical head.
    pub const fn canonical_block_hash(&self) -> B256 {
        self.canonical_head().hash
    }

    /// Returns the block number of the canonical head.
    pub const fn canonical_block_number(&self) -> BlockNumber {
        self.canonical_head().number
    }
}

#[cfg(test)]
impl<N: NodePrimitives> TreeState<N> {
    /// Determines if the second block is a descendant of the first block.
    ///
    /// If the two blocks are the same, this returns `false`.
    pub fn is_descendant(
        &self,
        first: BlockNumHash,
        second: alloy_eips::eip1898::BlockWithParent,
    ) -> bool {
        // If the second block's parent is the first block's hash, then it is a direct child
        // and we can return early.
        if second.parent == first.hash {
            return true
        }

        // If the second block is lower than, or has the same block number, they are not
        // descendants.
        if second.block.number <= first.number {
            return false
        }

        // iterate through parents of the second until we reach the number
        let Some(mut current_block) = self.executed_block_by_hash(second.parent) else {
            // If we can't find its parent in the tree, we can't continue, so return false
            return false
        };

        while current_block.recovered_block().number() > first.number + 1 {
            let Some(block) =
                self.executed_block_by_hash(current_block.recovered_block().parent_hash())
            else {
                // If we can't find its parent in the tree, we can't continue, so return false
                return false
            };

            current_block = block;
        }

        // Now the block numbers should be equal, so we compare hashes.
        current_block.recovered_block().parent_hash() == first.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::B256Set;
    use reth_chain_state::{test_utils::TestBlockBuilder, NewCanonicalChain};

    fn tree_state(head: BlockNumHash) -> TreeState {
        TreeState::new(head, EngineApiKind::Ethereum, OverlayManager::default())
    }

    /// Makes `blocks` canonical the way the engine does on a forkchoice update.
    fn make_canonical(tree_state: &mut TreeState, blocks: &[ExecutedBlock]) {
        tree_state.set_canonical_head(blocks.last().unwrap().recovered_block().num_hash());
        tree_state.in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.to_vec() });
    }

    fn pending_children(tree_state: &TreeState, parent: &ExecutedBlock) -> B256Set {
        tree_state
            .in_memory_state
            .pending_children(parent.recovered_block().hash())
            .iter()
            .map(|state| state.hash())
            .collect()
    }

    #[test]
    fn test_tree_state_normal_descendant() {
        let mut tree_state = tree_state(BlockNumHash::default());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..4).collect();

        tree_state.insert_executed(blocks[0].clone());
        assert!(tree_state.is_descendant(
            blocks[0].recovered_block().num_hash(),
            blocks[1].recovered_block().block_with_parent()
        ));

        tree_state.insert_executed(blocks[1].clone());

        assert!(tree_state.is_descendant(
            blocks[0].recovered_block().num_hash(),
            blocks[2].recovered_block().block_with_parent()
        ));
        assert!(tree_state.is_descendant(
            blocks[1].recovered_block().num_hash(),
            blocks[2].recovered_block().block_with_parent()
        ));
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed() {
        let mut tree_state = tree_state(BlockNumHash::default());
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..4).collect();

        tree_state.insert_executed(blocks[0].clone());
        tree_state.insert_executed(blocks[1].clone());

        assert_eq!(
            pending_children(&tree_state, &blocks[0]),
            B256Set::from_iter([blocks[1].recovered_block().hash()])
        );
        assert!(pending_children(&tree_state, &blocks[1]).is_empty());

        tree_state.insert_executed(blocks[2].clone());

        assert_eq!(
            pending_children(&tree_state, &blocks[1]),
            B256Set::from_iter([blocks[2].recovered_block().hash()])
        );
        assert!(pending_children(&tree_state, &blocks[2]).is_empty());

        // Executed blocks are pending until a forkchoice update makes them canonical, and they
        // share the in-memory state of the overlay manager.
        assert_eq!(tree_state.in_memory_state.pending_block_count(), 3);
        assert_eq!(tree_state.in_memory_state.canonical_block_count(), 0);
        assert!(tree_state.in_memory_state.ptr_eq(tree_state.overlay_manager.in_memory_state()));
        let (anchor, chain) =
            tree_state.blocks_by_hash(blocks[2].recovered_block().hash()).unwrap();
        assert_eq!(anchor, blocks[0].recovered_block().parent_hash());
        assert_eq!(chain, blocks.iter().rev().cloned().collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed_with_reorg() {
        let mut tree_state = tree_state(BlockNumHash::default());
        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);
        assert_eq!(tree_state.block_count(), 5);

        let fork_block_3 = test_block_builder
            .get_executed_block_with_number(3, blocks[1].recovered_block().hash());
        let fork_block_4 = test_block_builder
            .get_executed_block_with_number(4, fork_block_3.recovered_block().hash());
        let fork_block_5 = test_block_builder
            .get_executed_block_with_number(5, fork_block_4.recovered_block().hash());

        tree_state.insert_executed(fork_block_3.clone());
        tree_state.insert_executed(fork_block_4.clone());
        tree_state.insert_executed(fork_block_5.clone());

        assert_eq!(tree_state.block_count(), 8);
        // two blocks at height 3 (original and fork)
        assert_eq!(tree_state.in_memory_state.blocks_at_number(3).len(), 2);
        assert_eq!(pending_children(&tree_state, &blocks[1]).len(), 1); // the fork block

        // verify that we can insert the same block again without issues
        tree_state.insert_executed(fork_block_4.clone());
        assert_eq!(tree_state.block_count(), 8);

        assert!(pending_children(&tree_state, &fork_block_3)
            .contains(&fork_block_4.recovered_block().hash()));
        assert!(pending_children(&tree_state, &fork_block_4)
            .contains(&fork_block_5.recovered_block().hash()));

        assert_eq!(tree_state.in_memory_state.blocks_at_number(4).len(), 2);
        assert_eq!(tree_state.in_memory_state.blocks_at_number(5).len(), 2);

        // Reorging to the fork moves the replaced canonical blocks to the pending section.
        tree_state.set_canonical_head(fork_block_5.recovered_block().num_hash());
        tree_state.in_memory_state.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork_block_3.clone(), fork_block_4, fork_block_5],
            old: blocks[2..].to_vec(),
        });
        assert_eq!(tree_state.block_count(), 8);
        assert_eq!(tree_state.in_memory_state.canonical_block_count(), 5);
        assert!(tree_state.is_canonical(fork_block_3.recovered_block().hash()));
        assert!(!tree_state.is_canonical(blocks[2].recovered_block().hash()));
        assert!(tree_state.contains_hash(&blocks[4].recovered_block().hash()));
        assert_eq!(
            pending_children(&tree_state, &blocks[1]),
            B256Set::from_iter([blocks[2].recovered_block().hash()])
        );
    }

    /// Asserts that blocks 1 and 2 were removed, and blocks 3 to 5 kept.
    fn assert_removed_through_block_2(tree_state: &TreeState, blocks: &[ExecutedBlock]) {
        assert!(!tree_state.contains_hash(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.contains_hash(&blocks[1].recovered_block().hash()));
        assert!(tree_state.in_memory_state.blocks_at_number(1).is_empty());
        assert!(tree_state.in_memory_state.blocks_at_number(2).is_empty());

        assert!(tree_state.contains_hash(&blocks[2].recovered_block().hash()));
        assert!(tree_state.contains_hash(&blocks[3].recovered_block().hash()));
        assert!(tree_state.contains_hash(&blocks[4].recovered_block().hash()));
        assert!(!tree_state.in_memory_state.blocks_at_number(3).is_empty());
        assert!(!tree_state.in_memory_state.blocks_at_number(4).is_empty());
        assert!(!tree_state.in_memory_state.blocks_at_number(5).is_empty());

        // The remaining chain starts right after the removed blocks.
        let (anchor, chain) =
            tree_state.blocks_by_hash(blocks[4].recovered_block().hash()).unwrap();
        assert_eq!(anchor, blocks[1].recovered_block().hash());
        assert_eq!(chain.len(), 3);
    }

    #[tokio::test]
    async fn test_tree_state_remove_before() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = tree_state(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // inclusive bound, so we should remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = tree_state(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // we should still remove everything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            None,
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_lower_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = tree_state(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // we have no forks so we should still remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            Some(blocks[0].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_until_prunes_finalized_sidechains() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = tree_state(start_num_hash);
        let mut builder = TestBlockBuilder::eth();
        let blocks: Vec<_> = builder.get_executed_blocks(1..6).collect();
        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // A fork off block 1 can never become canonical once block 2 is finalized, a fork off
        // block 3 still can.
        let dead_fork =
            builder.get_executed_block_with_number(2, blocks[0].recovered_block().hash());
        let dead_fork_child =
            builder.get_executed_block_with_number(3, dead_fork.recovered_block().hash());
        let live_fork =
            builder.get_executed_block_with_number(4, blocks[2].recovered_block().hash());
        for block in [&dead_fork, &dead_fork_child, &live_fork] {
            tree_state.insert_executed(block.clone());
        }
        assert_eq!(tree_state.block_count(), 8);

        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
        assert!(!tree_state.contains_hash(&dead_fork.recovered_block().hash()));
        assert!(!tree_state.contains_hash(&dead_fork_child.recovered_block().hash()));
        assert!(tree_state.contains_hash(&live_fork.recovered_block().hash()));
        assert_eq!(tree_state.block_count(), 4);

        // The surviving fork is re-linked to the remaining canonical chain.
        let (anchor, chain) =
            tree_state.blocks_by_hash(live_fork.recovered_block().hash()).unwrap();
        assert_eq!(anchor, blocks[1].recovered_block().hash());
        assert_eq!(chain.len(), 2);
    }
}
