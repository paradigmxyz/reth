//! Functionality related to tree state.

use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use reth_chain_state::{
    BlockState, EthPrimitives, ExecutedBlock, InMemoryState, InMemoryStateBlockCounts,
};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_storage_overlay::OverlayManager;

/// Keeps track of the state of the tree.
///
/// The executed blocks are tracked by the in-memory state of the [`OverlayManager`], which this
/// type reads and the engine writes through its
/// [`CanonicalInMemoryState`](reth_chain_state::CanonicalInMemoryState). Newly executed blocks are
/// tracked in its non-canonical section, and move to its canonical section when they become
/// canonical.
///
/// ## Invariants
///
/// - All executed blocks are valid and have been executed.
#[derive(Debug, Default)]
pub struct TreeState<N: NodePrimitives = EthPrimitives> {
    /// Currently tracked canonical head of the chain.
    pub(crate) current_canonical_head: BlockNumHash,
    /// Manages state trie overlays for in-memory blocks.
    pub(crate) overlay_manager: OverlayManager<N>,
}

impl<N: NodePrimitives> TreeState<N> {
    /// Returns a new tree state that points to the given canonical head and reads the executed
    /// blocks of the in-memory state of `overlay_manager`.
    pub const fn new(
        current_canonical_head: BlockNumHash,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        Self { current_canonical_head, overlay_manager }
    }

    /// Returns the number of executed blocks stored.
    pub fn block_count(&self) -> usize {
        let InMemoryStateBlockCounts { canonical, non_canonical } =
            self.in_memory_state().block_counts();
        canonical + non_canonical
    }

    /// Returns the [`ExecutedBlock`] by hash.
    pub fn executed_block_by_hash(&self, hash: B256) -> Option<ExecutedBlock<N>> {
        self.in_memory_state().state_by_hash(hash).map(|state| state.block())
    }

    /// Returns `true` if a block with the given hash exists in memory.
    pub fn contains_hash(&self, hash: &B256) -> bool {
        self.in_memory_state().contains(hash)
    }

    /// Returns the sealed block header by hash.
    pub fn sealed_header_by_hash(&self, hash: &B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.in_memory_state()
            .state_by_hash(*hash)
            .map(|state| state.block_ref().sealed_block().sealed_header().clone())
    }

    /// Returns all available blocks for the given hash that lead back to the canonical chain, from
    /// newest to oldest, and the parent hash of the oldest returned block. This parent hash is the
    /// highest persisted block connected to this chain.
    ///
    /// Returns `None` if the block for the given hash is not found.
    pub fn blocks_by_hash(&self, hash: B256) -> Option<(B256, Vec<ExecutedBlock<N>>)> {
        let state = self.in_memory_state().state_by_hash(hash)?;
        Some((state.anchor().hash, state.chain().map(BlockState::block).collect()))
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

    /// Returns the in-memory state that tracks the executed blocks, canonical or not.
    fn in_memory_state(&self) -> &InMemoryState<N> {
        self.overlay_manager.in_memory_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip1898::BlockWithParent;
    use reth_chain_state::{
        test_utils::TestBlockBuilder, CanonicalInMemoryState, ChainInfoHeaders, NewCanonicalChain,
    };
    use reth_primitives_traits::AlloyBlockHeader;

    /// Creates a tree state without blocks and the canonical state that writes the blocks it
    /// reads. Its canonical head is the default header.
    fn tree_state() -> (TreeState, CanonicalInMemoryState) {
        let overlay_manager = OverlayManager::default();
        let canonical_in_memory_state = CanonicalInMemoryState::with_head(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            overlay_manager.in_memory_state().clone(),
        );
        (TreeState::new(BlockNumHash::default(), overlay_manager), canonical_in_memory_state)
    }

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Returns the hashes of the chain of the tracked block `block`, followed by the hash of the
    /// block the chain builds on.
    fn chain_hashes(tree_state: &TreeState, block: &ExecutedBlock) -> Vec<B256> {
        tree_state.in_memory_state().state_by_hash(hash(block)).unwrap().chain_hashes().collect()
    }

    /// Determines if the second block is a descendant of the first block.
    ///
    /// If the two blocks are the same, this returns `false`.
    fn is_descendant(tree_state: &TreeState, first: BlockNumHash, second: BlockWithParent) -> bool {
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

        // The first block is an ancestor if it is on the in-memory chain of the second block's
        // parent, or the block that chain builds on.
        tree_state
            .in_memory_state()
            .state_by_hash(second.parent)
            .is_some_and(|parent| parent.chain_hashes().any(|hash| hash == first.hash))
    }

    /// Removes the blocks the engine removes once `persisted` is persisted while `finalized` is the
    /// finalized block.
    fn on_blocks_persisted(
        canonical_in_memory_state: &CanonicalInMemoryState,
        persisted: BlockNumHash,
        finalized: Option<BlockNumHash>,
    ) {
        canonical_in_memory_state.remove_persisted_blocks_until(persisted, persisted.number);
        if let Some(finalized) = finalized {
            canonical_in_memory_state.prune_conflicting_with_finalized(
                if persisted.number < finalized.number { persisted } else { finalized },
            );
        }
    }

    #[test]
    fn test_tree_state_normal_descendant() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        canonical_in_memory_state.insert_executed(blocks[0].clone());
        assert!(is_descendant(
            &tree_state,
            blocks[0].recovered_block().num_hash(),
            blocks[1].recovered_block().block_with_parent()
        ));

        canonical_in_memory_state.insert_executed(blocks[1].clone());

        assert!(is_descendant(
            &tree_state,
            blocks[0].recovered_block().num_hash(),
            blocks[2].recovered_block().block_with_parent()
        ));
        assert!(is_descendant(
            &tree_state,
            blocks[1].recovered_block().num_hash(),
            blocks[2].recovered_block().block_with_parent()
        ));
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        canonical_in_memory_state.insert_executed(blocks[0].clone());
        canonical_in_memory_state.insert_executed(blocks[1].clone());

        // Block 2 is linked to block 1, and nothing else is tracked.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[1]),
            [hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );
        assert_eq!(tree_state.block_count(), 2);

        canonical_in_memory_state.insert_executed(blocks[2].clone());

        // Block 3 is linked to block 2, and nothing else is tracked.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[2]),
            [hash(&blocks[2]), hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );

        // Executed blocks are not canonical until a forkchoice update makes them canonical.
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 3 }
        );
        for block in &blocks {
            assert!(tree_state.contains_hash(&hash(block)));
            assert_eq!(tree_state.executed_block_by_hash(hash(block)).as_ref(), Some(block));
            assert_eq!(tree_state.sealed_header_by_hash(&hash(block)).unwrap().hash(), hash(block));
        }
        let (anchor_hash, chain) = tree_state.blocks_by_hash(hash(&blocks[2])).unwrap();
        assert_eq!(anchor_hash, persisted_hash);
        assert_eq!(chain, blocks.iter().rev().cloned().collect::<Vec<_>>());
        assert!(tree_state.blocks_by_hash(B256::random()).is_none());
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed_with_reorg() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks = test_block_builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        canonical_in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 0 }
        );

        let fork_block_3 = test_block_builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let fork_block_4 =
            test_block_builder.get_executed_block_with_number(4, hash(&fork_block_3));
        let fork_block_5 =
            test_block_builder.get_executed_block_with_number(5, hash(&fork_block_4));

        canonical_in_memory_state.insert_executed(fork_block_3.clone());
        canonical_in_memory_state.insert_executed(fork_block_4.clone());
        canonical_in_memory_state.insert_executed(fork_block_5.clone());

        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );
        // Two blocks at height 3 (original and fork).
        assert_eq!(canonical_in_memory_state.hash_by_number(3), Some(hash(&blocks[2])));
        assert!(tree_state.contains_hash(&hash(&fork_block_3)));
        // The fork block is built on block 2.
        assert_eq!(
            chain_hashes(&tree_state, &fork_block_3),
            [hash(&fork_block_3), hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );

        // Verify that we can insert the same block again without issues.
        canonical_in_memory_state.insert_executed(fork_block_4.clone());
        assert_eq!(tree_state.block_count(), 8);

        // The fork blocks are linked to each other.
        assert_eq!(
            chain_hashes(&tree_state, &fork_block_5),
            [
                hash(&fork_block_5),
                hash(&fork_block_4),
                hash(&fork_block_3),
                hash(&blocks[1]),
                hash(&blocks[0]),
                persisted_hash
            ]
        );

        // Two blocks at the heights 4 and 5.
        for (number, canonical, fork) in
            [(4, &blocks[3], &fork_block_4), (5, &blocks[4], &fork_block_5)]
        {
            assert_eq!(canonical_in_memory_state.hash_by_number(number), Some(hash(canonical)));
            assert!(tree_state.contains_hash(&hash(fork)));
        }

        // Reorging to the fork moves the replaced canonical blocks to the non-canonical section.
        canonical_in_memory_state.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork_block_3.clone(), fork_block_4, fork_block_5],
            old: blocks[2..].to_vec(),
        });
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );
        assert_eq!(canonical_in_memory_state.hash_by_number(3), Some(hash(&fork_block_3)));
        assert!(tree_state.contains_hash(&hash(&blocks[4])));
        // The replaced block 3 is a non-canonical block built on block 2.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[2]),
            [hash(&blocks[2]), hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );
    }

    /// Asserts that blocks 1 and 2 were removed, and blocks 3 to 5 kept.
    fn assert_removed_through_block_2(
        tree_state: &TreeState,
        canonical_in_memory_state: &CanonicalInMemoryState,
        blocks: &[ExecutedBlock],
    ) {
        for (number, block) in (1..).zip(&blocks[..2]) {
            assert!(!tree_state.contains_hash(&hash(block)));
            assert!(canonical_in_memory_state.hash_by_number(number).is_none());
        }
        for (number, block) in (3..).zip(&blocks[2..]) {
            assert!(tree_state.contains_hash(&hash(block)));
            assert_eq!(canonical_in_memory_state.hash_by_number(number), Some(hash(block)));
        }

        // The remaining chain starts right after the removed blocks.
        let (anchor_hash, chain) = tree_state.blocks_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(anchor_hash, hash(&blocks[1]));
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        canonical_in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        // Inclusive bound, so we should remove anything up to and including 2.
        on_blocks_persisted(
            &canonical_in_memory_state,
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &canonical_in_memory_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_finalized() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        canonical_in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        // We should still remove everything up to and including 2.
        on_blocks_persisted(
            &canonical_in_memory_state,
            BlockNumHash::new(2, hash(&blocks[1])),
            None,
        );

        assert_removed_through_block_2(&tree_state, &canonical_in_memory_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_lower_finalized() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        canonical_in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        // We have no forks so we should still remove anything up to and including 2.
        on_blocks_persisted(
            &canonical_in_memory_state,
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[0].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &canonical_in_memory_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_prunes_finalized_sidechains() {
        let (tree_state, canonical_in_memory_state) = tree_state();
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        for block in &blocks {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        canonical_in_memory_state.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        // A fork off block 1 can never become canonical once block 2 is finalized, a fork off
        // block 3 still can.
        let dead_fork = builder.get_executed_block_with_number(2, hash(&blocks[0]));
        let dead_fork_child = builder.get_executed_block_with_number(3, hash(&dead_fork));
        let live_fork = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        for block in [&dead_fork, &dead_fork_child, &live_fork] {
            canonical_in_memory_state.insert_executed(block.clone());
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );

        on_blocks_persisted(
            &canonical_in_memory_state,
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &canonical_in_memory_state, &blocks);
        assert!(!tree_state.contains_hash(&hash(&dead_fork)));
        assert!(!tree_state.contains_hash(&hash(&dead_fork_child)));
        assert!(tree_state.contains_hash(&hash(&live_fork)));
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 1 }
        );

        // The chain of the surviving fork starts right after the removed blocks.
        let (anchor_hash, chain) = tree_state.blocks_by_hash(hash(&live_fork)).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(anchor_hash, hash(&blocks[1]));
    }
}
