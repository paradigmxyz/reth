//! Functionality related to tree state.

use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use reth_chain_state::{
    BlockState, CanonStateNotification, CanonicalInMemoryState, CanonicalInMemoryStateWriter,
    EthPrimitives, ExecutedBlock, InMemoryCanonicalBlocksRemoval, InMemoryState,
    InMemoryStateWriter, NewCanonicalChain,
};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_storage_overlay::OverlayManager;
use std::sync::Arc;
use tracing::debug;

/// Keeps track of the state of the tree.
///
/// This type owns the write access to the executed in-memory blocks and to the canonical chain
/// tracking: the canonical head, the safe and the finalized block, the persisted block and the
/// canonical state notifications. Newly executed blocks are tracked in the non-canonical section
/// of the in-memory state, and move to its canonical section when they become canonical.
///
/// ## Invariants
///
/// - The tracked blocks and the canonical chain tracking only change through this type.
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
    /// Write access to the canonical chain tracking and the executed in-memory blocks.
    canonical_in_memory_state_writer: CanonicalInMemoryStateWriter<N>,
    /// Builds overlays for the tracked blocks.
    overlay_manager: OverlayManager<N>,
}

impl<N: NodePrimitives> TreeState<N> {
    /// Creates the tree state that tracks executed blocks through `in_memory_state_writer` and
    /// the canonical chain in `canonical_in_memory_state`.
    ///
    /// # Panics
    ///
    /// - If `canonical_in_memory_state` or `overlay_manager` do not read the in-memory state that
    ///   `in_memory_state_writer` writes to.
    /// - If the in-memory state has canonical blocks and the highest of them is not the canonical
    ///   head of `canonical_in_memory_state`.
    pub fn new(
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        in_memory_state_writer: InMemoryStateWriter<N>,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        assert!(
            in_memory_state_writer.writes_to(overlay_manager.in_memory_state()),
            "the writer does not write to the in-memory state of the overlay manager"
        );
        Self {
            canonical_in_memory_state_writer: CanonicalInMemoryStateWriter::new(
                canonical_in_memory_state,
                in_memory_state_writer,
            ),
            overlay_manager,
        }
    }

    /// Returns the canonical chain tracking: the chain info and the canonical view of the tracked
    /// blocks.
    pub const fn canonical_in_memory_state(&self) -> &CanonicalInMemoryState<N> {
        self.canonical_in_memory_state_writer.canonical_in_memory_state()
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

    /// Tracks a newly executed block. It becomes the pending block if it is built on the
    /// canonical head.
    ///
    /// A block that is already tracked stays in its section.
    pub fn insert_executed(&mut self, executed: ExecutedBlock<N>) {
        let hash = executed.recovered_block().hash();
        if self.canonical_in_memory_state_writer.insert_executed(executed) {
            self.overlay_manager.precompute_child_execution_overlays(hash);
        }
    }

    /// Makes `new_chain` canonical: its blocks, the canonical head and the chain info change
    /// together.
    ///
    /// The blocks a reorg replaces stay tracked as non-canonical blocks, and are inserted as such
    /// if they are not tracked.
    pub fn update_chain(&mut self, new_chain: NewCanonicalChain<N>) {
        self.canonical_in_memory_state_writer.update_chain(new_chain);
    }

    /// Makes `new_head`, a canonical ancestor of the canonical head, the canonical head.
    ///
    /// The canonical blocks above it stay tracked as non-canonical blocks. If `new_head` is not
    /// tracked as a canonical block, it becomes the only canonical in-memory block.
    pub fn unwind_canonical_head(&mut self, new_head: ExecutedBlock<N>) {
        self.canonical_in_memory_state_writer.unwind_canonical_head(new_head);
    }

    /// Records `last_persisted_block` as persisted, removes the canonical blocks up to
    /// `state_trie_persisted_block` and the non-canonical blocks that conflict with `finalized`.
    ///
    /// The canonical blocks are only removed if `last_persisted_block` is on the in-memory
    /// canonical chain or the block it builds on.
    pub fn on_blocks_persisted(
        &mut self,
        last_persisted_block: BlockNumHash,
        state_trie_persisted_block: BlockNumHash,
        finalized: Option<BlockNumHash>,
    ) {
        self.canonical_in_memory_state_writer.set_persisted(last_persisted_block);
        self.remove_canonical_until(
            state_trie_persisted_block,
            last_persisted_block.hash,
            finalized,
        );
    }

    /// Makes `backfill_head`, the block a backfill run ended at, the canonical head. No in-memory
    /// block is canonical afterwards.
    ///
    /// If the run `unwound` the chain, all in-memory blocks are removed. Otherwise the canonical
    /// blocks up to `backfill_head` are removed if `last_persisted_hash` is on the in-memory
    /// canonical chain or the block it builds on, the non-canonical blocks that conflict with
    /// `backfill_head` are removed, and the remaining blocks stay tracked as non-canonical.
    pub fn on_backfill_finished(
        &mut self,
        backfill_head: SealedHeader<N::BlockHeader>,
        last_persisted_hash: B256,
        unwound: bool,
    ) {
        if unwound {
            self.canonical_in_memory_state_writer.clear(backfill_head);
        } else {
            let backfill_num_hash = backfill_head.num_hash();
            self.remove_canonical_until(
                backfill_num_hash,
                last_persisted_hash,
                Some(backfill_num_hash),
            );
            self.canonical_in_memory_state_writer.demote_canonical_chain(backfill_head);
        }
    }

    /// Sets the safe block.
    pub fn set_safe(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state_writer.set_safe(header);
    }

    /// Sets the finalized block.
    pub fn set_finalized(&mut self, header: SealedHeader<N::BlockHeader>) {
        self.canonical_in_memory_state_writer.set_finalized(header);
    }

    /// Records that a forkchoice update was received now.
    pub fn on_forkchoice_update_received(&mut self) {
        self.canonical_in_memory_state_writer.on_forkchoice_update_received();
    }

    /// Sends `notification` to all subscribers of canonical state notifications.
    pub fn notify_canon_state(&mut self, notification: CanonStateNotification<N>) {
        self.canonical_in_memory_state_writer.notify_canon_state(notification);
    }

    /// Removes the canonical blocks that are persisted and the non-canonical blocks that can no
    /// longer become canonical.
    ///
    /// Canonical blocks up to and including `upper_bound` are only removed if
    /// `last_persisted_hash` is part of the in-memory canonical chain or the block it builds on.
    /// Otherwise canonical blocks were not actually persisted, e.g. because a reorg happened while
    /// persisting, and they stay in memory until the next removal that passes this check, even if
    /// they are below the finalized block.
    ///
    /// Non-canonical blocks are removed either way if they conflict with `finalized`: the blocks
    /// below its height, the other blocks at its height, and every block built on such a block at
    /// its height. Without a finalized block no non-canonical block is removed.
    ///
    /// NOTE: if the finalized block is greater than the upper bound, the only blocks that will be
    /// removed are canonical blocks and sidechains that fork below the `upper_bound`. This is the
    /// same behavior as if the finalized block were `Some(upper_bound)`.
    fn remove_canonical_until(
        &mut self,
        upper_bound: BlockNumHash,
        last_persisted_hash: B256,
        finalized: Option<BlockNumHash>,
    ) {
        debug!(target: "engine::tree", ?upper_bound, ?finalized, "Removing blocks from the tree");

        // If the finalized block is ahead of the upper bound, the only blocks removed are the
        // canonical blocks up to the upper bound and the non-canonical blocks that conflict with
        // the upper bound.
        let finalized = finalized.map(|mut finalized| {
            if upper_bound.number < finalized.number {
                finalized = upper_bound;
                debug!(target: "engine::tree", ?finalized, "Adjusted upper bound");
            }
            finalized
        });

        match self
            .canonical_in_memory_state_writer
            .remove_canonical_until(last_persisted_hash, upper_bound.number)
        {
            InMemoryCanonicalBlocksRemoval::Removed(removed) => {
                debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, removed, "Removed canonical blocks from the tree");
            }
            InMemoryCanonicalBlocksRemoval::PersistedHashNotCanonical => {
                debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, "Skipped removing canonical blocks: the persisted block is not on the in-memory canonical chain");
            }
        }

        if let Some(finalized) = finalized {
            let pruned =
                self.canonical_in_memory_state_writer.prune_conflicting_with_finalized(finalized);
            debug!(target: "engine::tree", ?finalized, pruned, "Removed finalized sidechain blocks");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip1898::BlockWithParent;
    use reth_chain_state::{
        test_utils::TestBlockBuilder, ChainInfoHeaders, InMemoryStateBlockCounts,
    };
    use reth_primitives_traits::AlloyBlockHeader;

    /// Creates a tree state without blocks. Its canonical head is the default header.
    fn tree_state() -> TreeState {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            in_memory_state.clone(),
        );
        TreeState::new(
            canonical_in_memory_state,
            in_memory_state_writer,
            OverlayManager::new(in_memory_state),
        )
    }

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Makes `blocks` canonical the way the engine does on a forkchoice update.
    fn make_canonical(tree_state: &mut TreeState, blocks: &[ExecutedBlock]) {
        tree_state.update_chain(NewCanonicalChain::Commit { new: blocks.to_vec() });
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
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        tree_state.insert_executed(blocks[0].clone());
        assert!(is_descendant(
            &tree_state,
            blocks[0].recovered_block().num_hash(),
            blocks[1].recovered_block().block_with_parent()
        ));

        tree_state.insert_executed(blocks[1].clone());

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
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        tree_state.insert_executed(blocks[0].clone());
        tree_state.insert_executed(blocks[1].clone());

        // Block 2 is linked to block 1, and nothing else is tracked.
        assert_eq!(
            chain_hashes(&tree_state, &blocks[1]),
            [hash(&blocks[1]), hash(&blocks[0]), persisted_hash]
        );
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 2 }
        );

        tree_state.insert_executed(blocks[2].clone());

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
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        // A block that is not built on the canonical head does not become the pending block.
        tree_state.insert_executed(blocks[0].clone());
        assert!(tree_state.canonical_in_memory_state().pending_state().is_none());
        make_canonical(&mut tree_state, &blocks[..1]);

        tree_state.insert_executed(blocks[1].clone());
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
        tree_state.insert_executed(blocks[2].clone());
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
        let mut tree_state = tree_state();
        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks = test_block_builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);
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

        tree_state.insert_executed(fork_block_3.clone());
        tree_state.insert_executed(fork_block_4.clone());
        tree_state.insert_executed(fork_block_5.clone());

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
        tree_state.insert_executed(fork_block_4.clone());
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
        tree_state.update_chain(NewCanonicalChain::Reorg {
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
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // Inclusive bound, so we should remove anything up to and including 2.
        tree_state.remove_canonical_until(
            BlockNumHash::new(2, hash(&blocks[1])),
            persisted_hash,
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_finalized() {
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // We should still remove everything up to and including 2.
        tree_state.remove_canonical_until(
            BlockNumHash::new(2, hash(&blocks[1])),
            persisted_hash,
            None,
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_lower_finalized() {
        let mut tree_state = tree_state();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // We have no forks so we should still remove anything up to and including 2.
        tree_state.remove_canonical_until(
            BlockNumHash::new(2, hash(&blocks[1])),
            persisted_hash,
            Some(blocks[0].recovered_block().num_hash()),
        );

        assert_removed_through_block_2(&tree_state, &blocks);
    }

    #[tokio::test]
    async fn test_tree_state_remove_canonical_until_prunes_finalized_sidechains() {
        let mut tree_state = tree_state();
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();
        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        make_canonical(&mut tree_state, &blocks);

        // A fork off block 1 can never become canonical once block 2 is finalized, a fork off
        // block 3 still can.
        let dead_fork = builder.get_executed_block_with_number(2, hash(&blocks[0]));
        let dead_fork_child = builder.get_executed_block_with_number(3, hash(&dead_fork));
        let live_fork = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        for block in [&dead_fork, &dead_fork_child, &live_fork] {
            tree_state.insert_executed(block.clone());
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 }
        );

        tree_state.remove_canonical_until(
            BlockNumHash::new(2, hash(&blocks[1])),
            persisted_hash,
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

    #[test]
    #[should_panic(
        expected = "the writer does not write to the in-memory state of the overlay manager"
    )]
    fn new_panics_if_the_overlay_manager_reads_another_in_memory_state() {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::<EthPrimitives>::new();
        let other_in_memory_state = InMemoryState::read_only();
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            in_memory_state,
        );

        let _ = TreeState::new(
            canonical_in_memory_state,
            in_memory_state_writer,
            OverlayManager::new(other_in_memory_state),
        );
    }

    #[test]
    #[should_panic(
        expected = "the writer does not write to the in-memory state of the canonical in-memory state"
    )]
    fn new_panics_if_the_canonical_in_memory_state_reads_another_in_memory_state() {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::<EthPrimitives>::new();
        let other_in_memory_state = InMemoryState::read_only();
        let canonical_in_memory_state = CanonicalInMemoryState::new(
            ChainInfoHeaders {
                canonical_head: SealedHeader::default(),
                safe: None,
                finalized: None,
            },
            other_in_memory_state,
        );

        let _ = TreeState::new(
            canonical_in_memory_state,
            in_memory_state_writer,
            OverlayManager::new(in_memory_state),
        );
    }

    #[test]
    fn on_blocks_persisted_keeps_canonical_blocks_if_the_persisted_block_is_not_canonical() {
        let mut tree_state = tree_state();
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        make_canonical(&mut tree_state, &blocks);

        // The persisted chain forked off block 1 and is not canonical in memory. A fork off
        // block 2 can never become canonical once block 3 is finalized.
        let persisted_fork = builder.get_executed_block_with_number(2, hash(&blocks[0]));
        let persisted = persisted_fork.recovered_block().num_hash();
        let dead_fork = builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let live_fork = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        for block in [&dead_fork, &live_fork] {
            tree_state.insert_executed(block.clone());
        }

        tree_state.on_blocks_persisted(
            persisted,
            persisted,
            Some(blocks[2].recovered_block().num_hash()),
        );

        // The persisted block is recorded and no canonical block is removed. The finalized block
        // is above the upper bound, so only non-canonical blocks that conflict with the upper
        // bound are removed, and there is none.
        assert_eq!(
            tree_state.canonical_in_memory_state().get_persisted_num_hash(),
            Some(persisted)
        );
        assert_eq!(tree_state.canonical_head(), blocks[4].recovered_block().num_hash());
        for block in &blocks {
            assert!(is_canonical(&tree_state, block));
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 2 }
        );

        // With the finalized block at the upper bound the conflicting fork is removed, and the
        // canonical blocks still stay.
        let upper_bound = BlockNumHash::new(3, hash(&blocks[2]));
        tree_state.on_blocks_persisted(persisted, upper_bound, Some(upper_bound));
        assert!(!tree_state.contains_hash(&hash(&dead_fork)));
        assert!(tree_state.contains_hash(&hash(&live_fork)));
        for block in &blocks {
            assert!(is_canonical(&tree_state, block));
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 1 }
        );

        // Once the persisted block is on the canonical chain, the canonical blocks are removed.
        let persisted = blocks[2].recovered_block().num_hash();
        tree_state.on_blocks_persisted(persisted, persisted, None);
        assert_eq!(
            tree_state.canonical_in_memory_state().get_persisted_num_hash(),
            Some(persisted)
        );
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 2, non_canonical: 1 }
        );
    }

    #[test]
    fn update_chain_tracks_untracked_reorged_blocks_as_non_canonical() {
        let mut tree_state = tree_state();
        let mut builder = TestBlockBuilder::eth();
        // Blocks 1 to 3 are the persisted canonical chain and not tracked. The head is block 3.
        let old = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        tree_state.on_backfill_finished(
            old[2].recovered_block().clone_sealed_header(),
            hash(&old[2]),
            false,
        );
        assert_eq!(tree_state.canonical_head(), old[2].recovered_block().num_hash());

        let fork_block_2 = builder.get_executed_block_with_number(2, hash(&old[0]));
        let fork_block_3 = builder.get_executed_block_with_number(3, hash(&fork_block_2));
        let fork_block_4 = builder.get_executed_block_with_number(4, hash(&fork_block_3));
        let new = vec![fork_block_2, fork_block_3, fork_block_4];
        for block in &new {
            tree_state.insert_executed(block.clone());
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 3 }
        );

        // The reorg replaces the blocks 2 and 3, which were loaded from disk.
        tree_state
            .update_chain(NewCanonicalChain::Reorg { new: new.clone(), old: old[1..].to_vec() });

        assert_eq!(tree_state.canonical_head(), new[2].recovered_block().num_hash());
        for block in &new {
            assert!(is_canonical(&tree_state, block));
        }
        for block in &old[1..] {
            assert!(tree_state.contains_hash(&hash(block)));
            assert!(!is_canonical(&tree_state, block));
        }
        assert!(!tree_state.contains_hash(&hash(&old[0])));
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 2 }
        );
        assert_eq!(
            chain_hashes(&tree_state, &old[2]),
            [hash(&old[2]), hash(&old[1]), hash(&old[0])]
        );
        assert!(tree_state.canonical_in_memory_state().pending_state().is_none());
    }

    #[test]
    fn unwind_canonical_head_keeps_the_unwound_blocks_as_non_canonical() {
        let mut tree_state = tree_state();
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        make_canonical(&mut tree_state, &blocks);

        // Unwinding to a canonical in-memory block keeps the canonical blocks below it.
        tree_state.unwind_canonical_head(blocks[2].clone());
        assert_eq!(tree_state.canonical_head(), blocks[2].recovered_block().num_hash());
        for (block, canonical) in blocks.iter().zip([true, true, true, false, false]) {
            assert!(tree_state.contains_hash(&hash(block)));
            assert_eq!(is_canonical(&tree_state, block), canonical);
        }

        // A block built on the new head becomes the pending block.
        let child = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        tree_state.insert_executed(child.clone());
        assert_eq!(
            tree_state.canonical_in_memory_state().pending_state().unwrap().hash(),
            hash(&child)
        );

        // Unwinding to a block that is not tracked makes it the only canonical in-memory block.
        let mut tree_state = self::tree_state();
        make_canonical(&mut tree_state, &blocks[2..]);
        tree_state.unwind_canonical_head(blocks[0].clone());
        assert_eq!(tree_state.canonical_head(), blocks[0].recovered_block().num_hash());
        for (block, canonical) in blocks.iter().zip([true, false, false, false, false]) {
            assert_eq!(is_canonical(&tree_state, block), canonical);
        }
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 3 }
        );
    }

    #[test]
    fn on_backfill_finished_makes_the_backfill_block_the_head_and_no_block_canonical() {
        let mut builder = TestBlockBuilder::eth();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        let persisted_hash = blocks[0].recovered_block().parent_hash();
        // A fork off block 2 conflicts with a backfill run that ended at block 3.
        let dead_fork = builder.get_executed_block_with_number(3, hash(&blocks[1]));
        let backfill_head = blocks[2].recovered_block().clone_sealed_header();

        // The run persisted the blocks up to block 3: they are removed with the conflicting fork,
        // and the blocks above stay tracked as non-canonical blocks.
        let mut tree_state = tree_state();
        make_canonical(&mut tree_state, &blocks);
        tree_state.insert_executed(dead_fork.clone());
        tree_state.on_backfill_finished(backfill_head.clone(), persisted_hash, false);
        assert_eq!(tree_state.canonical_head(), backfill_head.num_hash());
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 2 }
        );
        assert!(!tree_state.contains_hash(&hash(&dead_fork)));
        assert_eq!(
            chain_hashes(&tree_state, &blocks[4]),
            [hash(&blocks[4]), hash(&blocks[3]), hash(&blocks[2])]
        );

        // A run that unwound the chain removes every block.
        let mut tree_state = self::tree_state();
        make_canonical(&mut tree_state, &blocks);
        tree_state.insert_executed(dead_fork);
        tree_state.on_backfill_finished(backfill_head.clone(), persisted_hash, true);
        assert_eq!(tree_state.canonical_head(), backfill_head.num_hash());
        assert_eq!(
            tree_state.in_memory_state().block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 0 }
        );
    }
}
