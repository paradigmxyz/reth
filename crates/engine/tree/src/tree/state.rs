//! Functionality related to tree state.

use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use reth_chain_state::{
    BlockState, CanonicalInMemoryState, EthPrimitives, ExecutedBlock, InMemoryState,
};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_storage_overlay::OverlayManager;
use std::sync::Arc;

/// Keeps track of the state of the tree.
///
/// This type reads the executed in-memory blocks and the canonical chain tracking: the canonical
/// head, the safe and the finalized block, the persisted block and the canonical state
/// notifications. Newly executed blocks are tracked in the non-canonical section of the in-memory
/// state, and move to its canonical section when they become canonical.
///
/// ## Invariants
///
/// - The tracked blocks and the canonical chain tracking only change through the
///   [`CanonicalInMemoryStateWriter`](reth_chain_state::CanonicalInMemoryStateWriter) of the
///   engine.
/// - All tracked blocks are valid. The engine either validated them, or loaded them from disk as
///   canonical blocks: the blocks a disk reorg moves off the canonical chain, and the canonical
///   block the head is reset to when the engine unwinds to a block on disk.
/// - The canonical head is the canonical head of the chain info, see [`Self::canonical_head`].
/// - The canonical section holds the canonical chain above the persisted state/trie frontier. The
///   non-canonical section holds blocks that can still become canonical: payloads that await a
///   forkchoice update, fork blocks, reorged-out blocks and blocks demoted after a backfill run.
///   They are pruned once they are below the finalized block or fork off at its height.
#[derive(Debug)]
pub struct TreeState<N: NodePrimitives = EthPrimitives> {
    /// The canonical chain tracking and the canonical view of the tracked blocks.
    canonical_in_memory_state: CanonicalInMemoryState<N>,
    /// Builds overlays for the tracked blocks.
    overlay_manager: OverlayManager<N>,
}

impl<N: NodePrimitives> TreeState<N> {
    /// Creates the tree state that reads the canonical chain tracking of
    /// `canonical_in_memory_state` and the executed blocks of its in-memory state.
    pub const fn new(
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        Self { canonical_in_memory_state, overlay_manager }
    }

    /// Returns the canonical chain tracking: the chain info and the canonical view of the tracked
    /// blocks.
    pub const fn canonical_in_memory_state(&self) -> &CanonicalInMemoryState<N> {
        &self.canonical_in_memory_state
    }

    /// Returns the in-memory state that tracks the executed blocks, canonical or not.
    pub fn in_memory_state(&self) -> &InMemoryState<N> {
        self.canonical_in_memory_state().in_memory_state()
    }

    /// Returns the overlay manager that builds overlays for the tracked blocks.
    pub const fn overlay_manager(&self) -> &OverlayManager<N> {
        &self.overlay_manager
    }

    /// Returns the canonical head, which is the canonical head of the chain info.
    pub fn canonical_head(&self) -> BlockNumHash {
        self.canonical_in_memory_state().get_canonical_num_hash()
    }

    /// Returns the block hash of the canonical head.
    pub fn canonical_block_hash(&self) -> B256 {
        self.canonical_head().hash
    }

    /// Returns the block number of the canonical head.
    pub fn canonical_block_number(&self) -> BlockNumber {
        self.canonical_head().number
    }

    /// Returns `true` if a block with the given hash is tracked, canonical or not.
    pub fn contains_hash(&self, hash: &B256) -> bool {
        self.in_memory_state().contains(hash)
    }

    /// Returns the [`BlockState`] of the tracked block with the given hash, canonical or not.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.in_memory_state().state_by_hash(hash)
    }

    /// Returns the sealed header of the tracked block with the given hash, canonical or not.
    pub fn sealed_header_by_hash(&self, hash: &B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.state_by_hash(*hash)
            .map(|state| state.block_ref().sealed_block().sealed_header().clone())
    }

    /// Returns the block with the given hash and its linked in-memory ancestors, newest first.
    ///
    /// Returns `None` if the block is not tracked.
    pub fn blocks_by_hash(&self, hash: B256) -> Option<Vec<ExecutedBlock<N>>> {
        let state = self.state_by_hash(hash)?;
        Some(state.chain().map(BlockState::block).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip1898::BlockWithParent;
    use reth_chain_state::{
        test_utils::TestBlockBuilder, CanonicalInMemoryStateWriter, ChainInfoHeaders,
        InMemoryStateBlockCounts, NewCanonicalChain,
    };
    use reth_primitives_traits::AlloyBlockHeader;

    /// Creates a tree state without blocks and the writer of the state it reads. Its canonical
    /// head is the default header.
    fn tree_state() -> (TreeState, CanonicalInMemoryStateWriter) {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            in_memory_state.clone(),
        );
        let canonical_in_memory_state_writer = CanonicalInMemoryStateWriter::new(
            canonical_in_memory_state.clone(),
            in_memory_state_writer,
        );
        (
            TreeState::new(canonical_in_memory_state, OverlayManager::new(in_memory_state)),
            canonical_in_memory_state_writer,
        )
    }

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Makes `blocks` canonical the way the engine does on a forkchoice update.
    fn make_canonical(
        canonical_in_memory_state_writer: &mut CanonicalInMemoryStateWriter,
        blocks: &[ExecutedBlock],
    ) {
        canonical_in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks.to_vec() });
    }

    /// Returns the hashes of the chain of the tracked block `block`, followed by the hash of the
    /// block the chain builds on.
    fn chain_hashes(tree_state: &TreeState, block: &ExecutedBlock) -> Vec<B256> {
        tree_state.state_by_hash(hash(block)).unwrap().chain_hashes().collect()
    }

    fn is_canonical(tree_state: &TreeState, block: &ExecutedBlock) -> bool {
        tree_state.canonical_in_memory_state().state_by_hash(hash(block)).is_some()
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
            .state_by_hash(second.parent)
            .is_some_and(|parent| parent.chain_hashes().any(|hash| hash == first.hash))
    }

    #[test]
    fn test_tree_state_normal_descendant() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        canonical_in_memory_state_writer.insert_executed(blocks[0].clone());
        assert!(is_descendant(
            &tree_state,
            blocks[0].recovered_block().num_hash(),
            blocks[1].recovered_block().block_with_parent()
        ));

        canonical_in_memory_state_writer.insert_executed(blocks[1].clone());

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
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        canonical_in_memory_state_writer.insert_executed(blocks[0].clone());
        canonical_in_memory_state_writer.insert_executed(blocks[1].clone());

        // Block 2 is linked to block 1, and nothing else is tracked.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[1]),
            [hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 2 }
        );

        canonical_in_memory_state_writer.insert_executed(blocks[2].clone());

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
            assert!(!is_canonical(&tree_state, block));
        }
        let chain = tree_state.blocks_by_hash(hash(&blocks[2])).unwrap();
        assert_eq!(chain, blocks.iter().rev().cloned().collect::<Vec<_>>());
        assert_eq!(
            tree_state.state_by_hash(hash(&blocks[2])).unwrap().anchor().hash,
            persisted_hash
        );
        assert!(tree_state.blocks_by_hash(B256::random()).is_none());
    }

    #[test]
    fn insert_executed_makes_a_block_built_on_the_canonical_head_pending() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        // A block that is not built on the canonical head does not become the pending block.
        canonical_in_memory_state_writer.insert_executed(blocks[0].clone());
        assert!(tree_state.canonical_in_memory_state().pending_state().is_none());
        make_canonical(&mut canonical_in_memory_state_writer, &blocks[..1]);

        canonical_in_memory_state_writer.insert_executed(blocks[1].clone());
        let pending = tree_state.canonical_in_memory_state().pending_state().unwrap();
        assert_eq!(pending.hash(), hash(&blocks[1]));
        assert_eq!(
            pending.chain_hashes().collect::<Vec<_>>(),
            chain_hashes(&tree_state, &blocks[1])
        );
        assert!(!is_canonical(&tree_state, &blocks[1]));
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 1 }
        );

        // A block built on the pending block is tracked, and the pending block stays.
        canonical_in_memory_state_writer.insert_executed(blocks[2].clone());
        assert_eq!(
            tree_state.canonical_in_memory_state().pending_state().unwrap().hash(),
            hash(&blocks[1])
        );
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 2 }
        );
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed_with_reorg() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks = test_block_builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        make_canonical(&mut canonical_in_memory_state_writer, &blocks);
        assert_eq!(tree_state.canonical_head(), blocks[4].recovered_block().num_hash());
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 0 }
        );

        let fork_block_3 = test_block_builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let fork_block_4 =
            test_block_builder.get_executed_block_with_number(4, hash(&fork_block_3));
        let fork_block_5 =
            test_block_builder.get_executed_block_with_number(5, hash(&fork_block_4));

        canonical_in_memory_state_writer.insert_executed(fork_block_3.clone());
        canonical_in_memory_state_writer.insert_executed(fork_block_4.clone());
        canonical_in_memory_state_writer.insert_executed(fork_block_5.clone());

        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );
        // Two blocks at height 3 (original and fork).
        assert_eq!(
            tree_state.canonical_in_memory_state().hash_by_number(3),
            Some(hash(&blocks[2]))
        );
        assert_eq!(tree_state.state_by_hash(hash(&fork_block_3)).unwrap().number(), 3);
        assert!(!is_canonical(&tree_state, &fork_block_3));
        // The fork block is built on block 2.
        assert_eq!(
            chain_hashes(&tree_state, &fork_block_3),
            [hash(&fork_block_3), hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );

        // Verify that we can insert the same block again without issues.
        canonical_in_memory_state_writer.insert_executed(fork_block_4.clone());
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );

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
            assert_eq!(
                tree_state.canonical_in_memory_state().hash_by_number(number),
                Some(hash(canonical))
            );
            assert_eq!(tree_state.state_by_hash(hash(fork)).unwrap().number(), number);
            assert!(!is_canonical(&tree_state, fork));
        }

        // Reorging to the fork moves the replaced canonical blocks to the non-canonical section.
        canonical_in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork_block_3.clone(), fork_block_4, fork_block_5.clone()],
            old: blocks[2..].to_vec(),
        });
        assert_eq!(tree_state.canonical_head(), fork_block_5.recovered_block().num_hash());
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );
        assert!(is_canonical(&tree_state, &fork_block_3));
        assert!(!is_canonical(&tree_state, &blocks[2]));
        assert!(tree_state.contains_hash(&hash(&blocks[4])));
        assert_eq!(
            tree_state.canonical_in_memory_state().hash_by_number(3),
            Some(hash(&fork_block_3))
        );
        // The replaced block 3 is a non-canonical block built on block 2.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[2]),
            [hash(&blocks[2]), hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );
    }

    /// Asserts that blocks 1 and 2 were removed, and blocks 3 to 5 kept.
    fn assert_removed_through_block_2(tree_state: &TreeState, blocks: &[ExecutedBlock]) {
        let canonical_in_memory_state = tree_state.canonical_in_memory_state();
        for (number, block) in (1..).zip(&blocks[..2]) {
            assert!(!tree_state.contains_hash(&hash(block)));
            assert!(canonical_in_memory_state.hash_by_number(number).is_none());
        }
        for (number, block) in (3..).zip(&blocks[2..]) {
            assert!(tree_state.contains_hash(&hash(block)));
            assert_eq!(canonical_in_memory_state.hash_by_number(number), Some(hash(block)));
        }

        // The remaining chain starts right after the removed blocks.
        let chain = tree_state.blocks_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(
            tree_state.state_by_hash(hash(&blocks[4])).unwrap().anchor().hash,
            hash(&blocks[1])
        );
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        make_canonical(&mut canonical_in_memory_state_writer, &blocks);

        // Inclusive bound, so we should remove anything up to and including 2.
        canonical_in_memory_state_writer.on_blocks_persisted(
            BlockNumHash::new(2, hash(&blocks[1])),
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_finalized() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        make_canonical(&mut canonical_in_memory_state_writer, &blocks);

        // We should still remove everything up to and including 2.
        canonical_in_memory_state_writer.on_blocks_persisted(
            BlockNumHash::new(2, hash(&blocks[1])),
            BlockNumHash::new(2, hash(&blocks[1])),
            None,
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_lower_finalized() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();

        for block in &blocks {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        make_canonical(&mut canonical_in_memory_state_writer, &blocks);

        // We have no forks so we should still remove anything up to and including 2.
        canonical_in_memory_state_writer.on_blocks_persisted(
            BlockNumHash::new(2, hash(&blocks[1])),
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[0].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_prunes_finalized_sidechains() {
        let (tree_state, mut canonical_in_memory_state_writer) = tree_state();
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        for block in &blocks {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        make_canonical(&mut canonical_in_memory_state_writer, &blocks);

        // A fork off block 1 can never become canonical once block 2 is finalized, a fork off
        // block 3 still can.
        let dead_fork = builder.get_executed_block_with_number(2, hash(&blocks[0]));
        let dead_fork_child = builder.get_executed_block_with_number(3, hash(&dead_fork));
        let live_fork = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        for block in [&dead_fork, &dead_fork_child, &live_fork] {
            canonical_in_memory_state_writer.insert_executed(block.clone());
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );

        canonical_in_memory_state_writer.on_blocks_persisted(
            BlockNumHash::new(2, hash(&blocks[1])),
            BlockNumHash::new(2, hash(&blocks[1])),
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
        assert!(!tree_state.contains_hash(&hash(&dead_fork)));
        assert!(!tree_state.contains_hash(&hash(&dead_fork_child)));
        assert!(tree_state.contains_hash(&hash(&live_fork)));
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 1 }
        );

        // The chain of the surviving fork starts right after the removed blocks.
        let chain = tree_state.blocks_by_hash(hash(&live_fork)).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(
            tree_state.state_by_hash(hash(&live_fork)).unwrap().anchor().hash,
            hash(&blocks[1])
        );
    }
}
