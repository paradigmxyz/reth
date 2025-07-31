//! Functionality related to tree state.

use crate::engine::EngineApiKind;
use alloy_eips::{eip1898::BlockWithParent, merge::EPOCH_SLOTS, BlockNumHash};
use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockNumber, B256,
};
use reth_chain_state::{EthPrimitives, ExecutedBlockWithTrieUpdates};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives, SealedBlock};
use reth_trie::updates::TrieUpdates;
use std::{
    collections::{btree_map, hash_map, BTreeMap, VecDeque},
    ops::Bound,
    sync::Arc,
};
use tracing::debug;

/// Default number of blocks to retain persisted trie updates
const DEFAULT_PERSISTED_TRIE_UPDATES_RETENTION: u64 = EPOCH_SLOTS * 2;

/// Number of blocks to retain persisted trie updates for OP Stack chains
/// OP Stack chains only need `EPOCH_BLOCKS` as reorgs are relevant only when
/// op-node reorgs to the same chain twice
const OPSTACK_PERSISTED_TRIE_UPDATES_RETENTION: u64 = EPOCH_SLOTS;

/// Keeps track of the state of the tree.
///
/// ## Invariants
///
/// - This only stores blocks that are connected to the canonical chain.
/// - All executed blocks are valid and have been executed.
#[derive(Debug, Default)]
pub struct TreeState<N: NodePrimitives = EthPrimitives> {
    /// __All__ unique executed blocks by block hash that are connected to the canonical chain.
    ///
    /// This includes blocks of all forks.
    pub(crate) blocks_by_hash: HashMap<B256, ExecutedBlockWithTrieUpdates<N>>,
    /// Executed blocks grouped by their respective block number.
    ///
    /// This maps unique block number to all known blocks for that height.
    ///
    /// Note: there can be multiple blocks at the same height due to forks.
    pub(crate) blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlockWithTrieUpdates<N>>>,
    /// Map of any parent block hash to its children.
    pub(crate) parent_to_child: HashMap<B256, HashSet<B256>>,
    /// Map of hash to trie updates for canonical blocks that are persisted but not finalized.
    ///
    /// Contains the block number for easy removal.
    pub(crate) persisted_trie_updates: HashMap<B256, (BlockNumber, Arc<TrieUpdates>)>,
    /// Currently tracked canonical head of the chain.
    pub(crate) current_canonical_head: BlockNumHash,
    /// The engine API variant of this handler
    pub(crate) engine_kind: EngineApiKind,
}

impl<N: NodePrimitives> TreeState<N> {
    /// Returns a new, empty tree state that points to the given canonical head.
    pub(crate) fn new(current_canonical_head: BlockNumHash, engine_kind: EngineApiKind) -> Self {
        Self {
            blocks_by_hash: HashMap::default(),
            blocks_by_number: BTreeMap::new(),
            current_canonical_head,
            parent_to_child: HashMap::default(),
            persisted_trie_updates: HashMap::default(),
            engine_kind,
        }
    }

    /// Resets the state and points to the given canonical head.
    pub(crate) fn reset(&mut self, current_canonical_head: BlockNumHash) {
        *self = Self::new(current_canonical_head, self.engine_kind);
    }

    /// Returns the number of executed blocks stored.
    pub(crate) fn block_count(&self) -> usize {
        self.blocks_by_hash.len()
    }

    /// Returns the [`ExecutedBlockWithTrieUpdates`] by hash.
    pub(crate) fn executed_block_by_hash(
        &self,
        hash: B256,
    ) -> Option<&ExecutedBlockWithTrieUpdates<N>> {
        self.blocks_by_hash.get(&hash)
    }

    /// Returns the block by hash.
    pub(crate) fn block_by_hash(&self, hash: B256) -> Option<Arc<SealedBlock<N::Block>>> {
        self.blocks_by_hash.get(&hash).map(|b| Arc::new(b.recovered_block().sealed_block().clone()))
    }

    /// Returns all available blocks for the given hash that lead back to the canonical chain, from
    /// newest to oldest. And the parent hash of the oldest block that is missing from the buffer.
    ///
    /// Returns `None` if the block for the given hash is not found.
    pub(crate) fn blocks_by_hash(
        &self,
        hash: B256,
    ) -> Option<(B256, Vec<ExecutedBlockWithTrieUpdates<N>>)> {
        let block = self.blocks_by_hash.get(&hash).cloned()?;
        let mut parent_hash = block.recovered_block().parent_hash();
        let mut blocks = vec![block];
        while let Some(executed) = self.blocks_by_hash.get(&parent_hash) {
            parent_hash = executed.recovered_block().parent_hash();
            blocks.push(executed.clone());
        }

        Some((parent_hash, blocks))
    }

    /// Insert executed block into the state.
    pub(crate) fn insert_executed(&mut self, executed: ExecutedBlockWithTrieUpdates<N>) {
        let hash = executed.recovered_block().hash();
        let parent_hash = executed.recovered_block().parent_hash();
        let block_number = executed.recovered_block().number();

        if self.blocks_by_hash.contains_key(&hash) {
            return;
        }

        self.blocks_by_hash.insert(hash, executed.clone());

        self.blocks_by_number.entry(block_number).or_default().push(executed);

        self.parent_to_child.entry(parent_hash).or_default().insert(hash);

        for children in self.parent_to_child.values_mut() {
            children.retain(|child| self.blocks_by_hash.contains_key(child));
        }
    }

    /// Remove single executed block by its hash.
    ///
    /// ## Returns
    ///
    /// The removed block and the block hashes of its children.
    fn remove_by_hash(
        &mut self,
        hash: B256,
    ) -> Option<(ExecutedBlockWithTrieUpdates<N>, HashSet<B256>)> {
        let executed = self.blocks_by_hash.remove(&hash)?;

        // Remove this block from collection of children of its parent block.
        let parent_entry = self.parent_to_child.entry(executed.recovered_block().parent_hash());
        if let hash_map::Entry::Occupied(mut entry) = parent_entry {
            entry.get_mut().remove(&hash);

            if entry.get().is_empty() {
                entry.remove();
            }
        }

        // Remove point to children of this block.
        let children = self.parent_to_child.remove(&hash).unwrap_or_default();

        // Remove this block from `blocks_by_number`.
        let block_number_entry = self.blocks_by_number.entry(executed.recovered_block().number());
        if let btree_map::Entry::Occupied(mut entry) = block_number_entry {
            // We have to find the index of the block since it exists in a vec
            if let Some(index) = entry.get().iter().position(|b| b.recovered_block().hash() == hash)
            {
                entry.get_mut().swap_remove(index);

                // If there are no blocks left then remove the entry for this block
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }

        Some((executed, children))
    }

    /// Returns whether or not the hash is part of the canonical chain.
    pub(crate) fn is_canonical(&self, hash: B256) -> bool {
        let mut current_block = self.current_canonical_head.hash;
        if current_block == hash {
            return true
        }

        while let Some(executed) = self.blocks_by_hash.get(&current_block) {
            current_block = executed.recovered_block().parent_hash();
            if current_block == hash {
                return true
            }
        }

        false
    }

    /// Removes canonical blocks below the upper bound, only if the last persisted hash is
    /// part of the canonical chain.
    pub(crate) fn remove_canonical_until(
        &mut self,
        upper_bound: BlockNumber,
        last_persisted_hash: B256,
    ) {
        debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, "Removing canonical blocks from the tree");

        // If the last persisted hash is not canonical, then we don't want to remove any canonical
        // blocks yet.
        if !self.is_canonical(last_persisted_hash) {
            return
        }

        // First, let's walk back the canonical chain and remove canonical blocks lower than the
        // upper bound
        let mut current_block = self.current_canonical_head.hash;
        while let Some(executed) = self.blocks_by_hash.get(&current_block) {
            current_block = executed.recovered_block().parent_hash();
            if executed.recovered_block().number() <= upper_bound {
                let num_hash = executed.recovered_block().num_hash();
                debug!(target: "engine::tree", ?num_hash, "Attempting to remove block walking back from the head");
                if let Some((mut removed, _)) =
                    self.remove_by_hash(executed.recovered_block().hash())
                {
                    debug!(target: "engine::tree", ?num_hash, "Removed block walking back from the head");
                    // finally, move the trie updates
                    let Some(trie_updates) = removed.trie.take_present() else {
                        debug!(target: "engine::tree", ?num_hash, "No trie updates found for persisted block");
                        continue;
                    };
                    self.persisted_trie_updates.insert(
                        removed.recovered_block().hash(),
                        (removed.recovered_block().number(), trie_updates),
                    );
                }
            }
        }
        debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, "Removed canonical blocks from the tree");
    }

    /// Prunes old persisted trie updates based on the current block number
    /// and chain type (OP Stack or regular)
    pub(crate) fn prune_persisted_trie_updates(&mut self) {
        let retention_blocks = if self.engine_kind.is_opstack() {
            OPSTACK_PERSISTED_TRIE_UPDATES_RETENTION
        } else {
            DEFAULT_PERSISTED_TRIE_UPDATES_RETENTION
        };

        let earliest_block_to_retain =
            self.current_canonical_head.number.saturating_sub(retention_blocks);

        self.persisted_trie_updates
            .retain(|_, (block_number, _)| *block_number > earliest_block_to_retain);
    }

    /// Removes all blocks that are below the finalized block, as well as removing non-canonical
    /// sidechains that fork from below the finalized block.
    pub(crate) fn prune_finalized_sidechains(&mut self, finalized_num_hash: BlockNumHash) {
        let BlockNumHash { number: finalized_num, hash: finalized_hash } = finalized_num_hash;

        // We remove disconnected sidechains in three steps:
        // * first, remove everything with a block number __below__ the finalized block.
        // * next, we populate a vec with parents __at__ the finalized block.
        // * finally, we iterate through the vec, removing children until the vec is empty
        // (BFS).

        // We _exclude_ the finalized block because we will be dealing with the blocks __at__
        // the finalized block later.
        let blocks_to_remove = self
            .blocks_by_number
            .range((Bound::Unbounded, Bound::Excluded(finalized_num)))
            .flat_map(|(_, blocks)| blocks.iter().map(|b| b.recovered_block().hash()))
            .collect::<Vec<_>>();
        for hash in blocks_to_remove {
            if let Some((removed, _)) = self.remove_by_hash(hash) {
                debug!(target: "engine::tree", num_hash=?removed.recovered_block().num_hash(), "Removed finalized sidechain block");
            }
        }

        self.prune_persisted_trie_updates();

        // The only block that should remain at the `finalized` number now, is the finalized
        // block, if it exists.
        //
        // For all other blocks, we  first put their children into this vec.
        // Then, we will iterate over them, removing them, adding their children, etc,
        // until the vec is empty.
        let mut blocks_to_remove = self.blocks_by_number.remove(&finalized_num).unwrap_or_default();

        // re-insert the finalized hash if we removed it
        if let Some(position) =
            blocks_to_remove.iter().position(|b| b.recovered_block().hash() == finalized_hash)
        {
            let finalized_block = blocks_to_remove.swap_remove(position);
            self.blocks_by_number.insert(finalized_num, vec![finalized_block]);
        }

        let mut blocks_to_remove = blocks_to_remove
            .into_iter()
            .map(|e| e.recovered_block().hash())
            .collect::<VecDeque<_>>();
        while let Some(block) = blocks_to_remove.pop_front() {
            if let Some((removed, children)) = self.remove_by_hash(block) {
                debug!(target: "engine::tree", num_hash=?removed.recovered_block().num_hash(), "Removed finalized sidechain child block");
                blocks_to_remove.extend(children);
            }
        }
    }

    /// Remove all blocks up to __and including__ the given block number.
    ///
    /// If a finalized hash is provided, the only non-canonical blocks which will be removed are
    /// those which have a fork point at or below the finalized hash.
    ///
    /// Canonical blocks below the upper bound will still be removed.
    ///
    /// NOTE: if the finalized block is greater than the upper bound, the only blocks that will be
    /// removed are canonical blocks and sidechains that fork below the `upper_bound`. This is the
    /// same behavior as if the `finalized_num` were `Some(upper_bound)`.
    pub(crate) fn remove_until(
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
        // We can do this in 2 steps:
        // * remove all canonical blocks below the upper bound
        // * fetch the number of the finalized hash, removing any sidechains that are __below__ the
        // finalized block
        self.remove_canonical_until(upper_bound.number, last_persisted_hash);

        // Now, we have removed canonical blocks (assuming the upper bound is above the finalized
        // block) and only have sidechains below the finalized block.
        if let Some(finalized_num_hash) = finalized_num_hash {
            self.prune_finalized_sidechains(finalized_num_hash);
        }
    }

    /// Determines if the second block is a direct descendant of the first block.
    ///
    /// If the two blocks are the same, this returns `false`.
    pub(crate) fn is_descendant(&self, first: BlockNumHash, second: BlockWithParent) -> bool {
        // If the second block's parent is the first block's hash, then it is a direct descendant
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
        let Some(mut current_block) = self.blocks_by_hash.get(&second.parent) else {
            // If we can't find its parent in the tree, we can't continue, so return false
            return false
        };

        while current_block.recovered_block().number() > first.number + 1 {
            let Some(block) =
                self.blocks_by_hash.get(&current_block.recovered_block().parent_hash())
            else {
                // If we can't find its parent in the tree, we can't continue, so return false
                return false
            };

            current_block = block;
        }

        // Now the block numbers should be equal, so we compare hashes.
        current_block.recovered_block().parent_hash() == first.hash
    }

    /// Updates the canonical head to the given block.
    pub(crate) const fn set_canonical_head(&mut self, new_head: BlockNumHash) {
        self.current_canonical_head = new_head;
    }

    /// Returns the tracked canonical head.
    pub(crate) const fn canonical_head(&self) -> &BlockNumHash {
        &self.current_canonical_head
    }

    /// Returns the block hash of the canonical head.
    pub(crate) const fn canonical_block_hash(&self) -> B256 {
        self.canonical_head().hash
    }

    /// Returns the block number of the canonical head.
    pub(crate) const fn canonical_block_number(&self) -> BlockNumber {
        self.canonical_head().number
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chain_state::test_utils::TestBlockBuilder;

    #[test]
    fn test_tree_state_normal_descendant() {
        let mut tree_state = TreeState::new(BlockNumHash::default(), EngineApiKind::Ethereum);
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
        let mut tree_state = TreeState::new(BlockNumHash::default(), EngineApiKind::Ethereum);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..4).collect();

        tree_state.insert_executed(blocks[0].clone());
        tree_state.insert_executed(blocks[1].clone());

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[0].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[1].recovered_block().hash()]))
        );

        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));

        tree_state.insert_executed(blocks[2].clone());

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[1].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[2].recovered_block().hash()]))
        );
        assert!(tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[2].recovered_block().hash()));
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed_with_reorg() {
        let mut tree_state = TreeState::new(BlockNumHash::default(), EngineApiKind::Ethereum);
        let mut test_block_builder = TestBlockBuilder::eth();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        assert_eq!(tree_state.blocks_by_hash.len(), 5);

        let fork_block_3 = test_block_builder
            .get_executed_block_with_number(3, blocks[1].recovered_block().hash());
        let fork_block_4 = test_block_builder
            .get_executed_block_with_number(4, fork_block_3.recovered_block().hash());
        let fork_block_5 = test_block_builder
            .get_executed_block_with_number(5, fork_block_4.recovered_block().hash());

        tree_state.insert_executed(fork_block_3.clone());
        tree_state.insert_executed(fork_block_4.clone());
        tree_state.insert_executed(fork_block_5.clone());

        assert_eq!(tree_state.blocks_by_hash.len(), 8);
        assert_eq!(tree_state.blocks_by_number[&3].len(), 2); // two blocks at height 3 (original and fork)
        assert_eq!(tree_state.parent_to_child[&blocks[1].recovered_block().hash()].len(), 2); // block 2 should have two children

        // verify that we can insert the same block again without issues
        tree_state.insert_executed(fork_block_4.clone());
        assert_eq!(tree_state.blocks_by_hash.len(), 8);

        assert!(tree_state.parent_to_child[&fork_block_3.recovered_block().hash()]
            .contains(&fork_block_4.recovered_block().hash()));
        assert!(tree_state.parent_to_child[&fork_block_4.recovered_block().hash()]
            .contains(&fork_block_5.recovered_block().hash()));

        assert_eq!(tree_state.blocks_by_number[&4].len(), 2);
        assert_eq!(tree_state.blocks_by_number[&5].len(), 2);
    }

    #[tokio::test]
    async fn test_tree_state_remove_before() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash, EngineApiKind::Ethereum);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.recovered_block().num_hash());

        // inclusive bound, so we should remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            Some(blocks[1].recovered_block().num_hash()),
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].recovered_block().hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].recovered_block().hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].recovered_block().hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[4].recovered_block().hash()]))
        );
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash, EngineApiKind::Ethereum);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.recovered_block().num_hash());

        // we should still remove everything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            None,
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].recovered_block().hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].recovered_block().hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].recovered_block().hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[4].recovered_block().hash()]))
        );
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_lower_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash, EngineApiKind::Ethereum);
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.recovered_block().num_hash());

        // we have no forks so we should still remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].recovered_block().hash()),
            start_num_hash.hash,
            Some(blocks[0].recovered_block().num_hash()),
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].recovered_block().hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].recovered_block().hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].recovered_block().hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].recovered_block().hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].recovered_block().hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&HashSet::from_iter([blocks[4].recovered_block().hash()]))
        );
    }
}
