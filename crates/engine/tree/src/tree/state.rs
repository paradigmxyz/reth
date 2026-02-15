//! Functionality related to tree state.

use crate::engine::EngineApiKind;
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{B256Map, B256Set},
    BlockNumber, B256,
};
use reth_chain_state::{DeferredTrieData, EthPrimitives, ExecutedBlock, LazyOverlay};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives, SealedHeader};
use std::{
    collections::{btree_map, hash_map, BTreeMap, VecDeque},
    ops::Bound,
};
use tracing::{debug, warn};

/// Maximum number of fork blocks allowed at a single block height.
///
/// Prevents unbounded memory growth when many competing blocks are received for the same height
/// without finalization (e.g., from a misconfigured benchmark harness or adversarial peers).
/// When this limit is exceeded, the oldest non-canonical blocks and their subtrees are evicted.
const MAX_SIDECHAINS_PER_BLOCK: usize = 64;

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
    pub(crate) blocks_by_hash: B256Map<ExecutedBlock<N>>,
    /// Executed blocks grouped by their respective block number.
    ///
    /// This maps unique block number to all known blocks for that height.
    ///
    /// Note: there can be multiple blocks at the same height due to forks.
    pub(crate) blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock<N>>>,
    /// Map of any parent block hash to its children.
    pub(crate) parent_to_child: B256Map<B256Set>,
    /// Currently tracked canonical head of the chain.
    pub(crate) current_canonical_head: BlockNumHash,
    /// The engine API variant of this handler
    pub(crate) engine_kind: EngineApiKind,
    /// Pre-computed lazy overlay for the canonical head.
    ///
    /// This is optimistically prepared after the canonical head changes, so that
    /// the next payload building on the canonical head can use it immediately
    /// without recomputing.
    pub(crate) cached_canonical_overlay: Option<PreparedCanonicalOverlay>,
}

impl<N: NodePrimitives> TreeState<N> {
    /// Returns a new, empty tree state that points to the given canonical head.
    pub fn new(current_canonical_head: BlockNumHash, engine_kind: EngineApiKind) -> Self {
        Self {
            blocks_by_hash: B256Map::default(),
            blocks_by_number: BTreeMap::new(),
            current_canonical_head,
            parent_to_child: B256Map::default(),
            engine_kind,
            cached_canonical_overlay: None,
        }
    }

    /// Resets the state and points to the given canonical head.
    pub fn reset(&mut self, current_canonical_head: BlockNumHash) {
        *self = Self::new(current_canonical_head, self.engine_kind);
    }

    /// Returns the number of executed blocks stored.
    pub fn block_count(&self) -> usize {
        self.blocks_by_hash.len()
    }

    /// Returns the [`ExecutedBlock`] by hash.
    pub fn executed_block_by_hash(&self, hash: B256) -> Option<&ExecutedBlock<N>> {
        self.blocks_by_hash.get(&hash)
    }

    /// Returns the sealed block header by hash.
    pub fn sealed_header_by_hash(&self, hash: &B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.blocks_by_hash.get(hash).map(|b| b.sealed_block().sealed_header().clone())
    }

    /// Returns all available blocks for the given hash that lead back to the canonical chain, from
    /// newest to oldest, and the parent hash of the oldest returned block. This parent hash is the
    /// highest persisted block connected to this chain.
    ///
    /// Returns `None` if the block for the given hash is not found.
    pub fn blocks_by_hash(&self, hash: B256) -> Option<(B256, Vec<ExecutedBlock<N>>)> {
        let block = self.blocks_by_hash.get(&hash).cloned()?;
        let mut parent_hash = block.recovered_block().parent_hash();
        let mut blocks = vec![block];
        while let Some(executed) = self.blocks_by_hash.get(&parent_hash) {
            parent_hash = executed.recovered_block().parent_hash();
            blocks.push(executed.clone());
        }

        Some((parent_hash, blocks))
    }

    /// Prepares a cached lazy overlay for the current canonical head.
    ///
    /// This should be called after the canonical head changes to optimistically
    /// prepare the overlay for the next payload that will likely build on it.
    ///
    /// Returns a clone of the [`LazyOverlay`] so the caller can spawn a background
    /// task to trigger computation via [`LazyOverlay::get`]. This ensures the overlay
    /// is actually computed before the next payload arrives.
    pub(crate) fn prepare_canonical_overlay(&mut self) -> Option<LazyOverlay> {
        let canonical_hash = self.current_canonical_head.hash;

        // Get blocks leading to the canonical head
        let Some((anchor_hash, blocks)) = self.blocks_by_hash(canonical_hash) else {
            // Canonical head not in memory (persisted), no overlay needed
            self.cached_canonical_overlay = None;
            return None;
        };

        // Extract deferred trie data handles from blocks (newest to oldest)
        let handles: Vec<DeferredTrieData> = blocks.iter().map(|b| b.trie_data_handle()).collect();

        let overlay = LazyOverlay::new(anchor_hash, handles);
        self.cached_canonical_overlay = Some(PreparedCanonicalOverlay {
            parent_hash: canonical_hash,
            overlay: overlay.clone(),
            anchor_hash,
        });

        debug!(
            target: "engine::tree",
            %canonical_hash,
            %anchor_hash,
            num_blocks = blocks.len(),
            "Prepared cached canonical overlay"
        );

        Some(overlay)
    }

    /// Returns the cached overlay if it matches the requested parent hash and anchor.
    ///
    /// Both parent hash and anchor hash must match to ensure the overlay is valid.
    /// This prevents using a stale overlay after persistence has advanced the anchor.
    pub(crate) fn get_cached_overlay(
        &self,
        parent_hash: B256,
        expected_anchor: B256,
    ) -> Option<&PreparedCanonicalOverlay> {
        self.cached_canonical_overlay.as_ref().filter(|cached| {
            cached.parent_hash == parent_hash && cached.anchor_hash == expected_anchor
        })
    }

    /// Invalidates the cached overlay.
    ///
    /// Should be called when the anchor changes (e.g., after persistence).
    pub(crate) fn invalidate_cached_overlay(&mut self) {
        self.cached_canonical_overlay = None;
    }

    /// Insert executed block into the state.
    pub fn insert_executed(&mut self, executed: ExecutedBlock<N>) {
        let hash = executed.recovered_block().hash();
        let parent_hash = executed.recovered_block().parent_hash();
        let block_number = executed.recovered_block().number();

        if self.blocks_by_hash.contains_key(&hash) {
            return;
        }

        self.blocks_by_hash.insert(hash, executed.clone());

        self.blocks_by_number.entry(block_number).or_default().push(executed);

        self.parent_to_child.entry(parent_hash).or_default().insert(hash);

        self.enforce_sidechain_limit(block_number);
    }

    /// Removes excess non-canonical fork blocks at the given height when the number of blocks
    /// exceeds [`MAX_SIDECHAINS_PER_BLOCK`].
    ///
    /// Evicts the oldest non-canonical blocks first (front of the insertion-ordered vec),
    /// along with all of their descendant subtrees.
    fn enforce_sidechain_limit(&mut self, block_number: BlockNumber) {
        let Some(blocks_at_height) = self.blocks_by_number.get(&block_number) else {
            return;
        };

        if blocks_at_height.len() <= MAX_SIDECHAINS_PER_BLOCK {
            return;
        }

        // Find the canonical block hash at this height (if any) so we never evict it.
        let canonical_hash_at_height = {
            let mut current = self.current_canonical_head.hash;
            let mut found = None;
            // If the canonical head is at this height, use it directly.
            if self.current_canonical_head.number == block_number {
                found = Some(current);
            } else if self.current_canonical_head.number > block_number {
                // Walk back the canonical chain to find the block at this height.
                while let Some(executed) = self.blocks_by_hash.get(&current) {
                    if executed.recovered_block().number() == block_number {
                        found = Some(current);
                        break;
                    }
                    current = executed.recovered_block().parent_hash();
                }
            }
            found
        };

        // Collect hashes of non-canonical blocks to evict (oldest first from the vec).
        let blocks_at_height = self.blocks_by_number.get(&block_number).unwrap();
        let excess = blocks_at_height.len() - MAX_SIDECHAINS_PER_BLOCK;
        let to_evict: Vec<B256> = blocks_at_height
            .iter()
            .filter(|b| Some(b.recovered_block().hash()) != canonical_hash_at_height)
            .take(excess)
            .map(|b| b.recovered_block().hash())
            .collect();

        if to_evict.is_empty() {
            return;
        }

        warn!(
            target: "engine::tree",
            block_number,
            evicting = to_evict.len(),
            total = self.blocks_by_number.get(&block_number).map_or(0, |b| b.len()),
            "Sidechain fork limit exceeded, evicting non-canonical blocks"
        );

        // BFS removal: evict each block and all its descendants.
        let mut queue: VecDeque<B256> = to_evict.into();
        while let Some(hash) = queue.pop_front() {
            if let Some((_removed, children)) = self.remove_by_hash(hash) {
                queue.extend(children);
            }
        }
    }

    /// Remove single executed block by its hash.
    ///
    /// ## Returns
    ///
    /// The removed block and the block hashes of its children.
    fn remove_by_hash(&mut self, hash: B256) -> Option<(ExecutedBlock<N>, B256Set)> {
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
                entry.get_mut().remove(index);

                // If there are no blocks left then remove the entry for this block
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }

        Some((executed, children))
    }

    /// Returns whether or not the hash is part of the canonical chain.
    pub fn is_canonical(&self, hash: B256) -> bool {
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
    pub fn remove_canonical_until(&mut self, upper_bound: BlockNumber, last_persisted_hash: B256) {
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
                self.remove_by_hash(executed.recovered_block().hash());
            }
        }
        debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, "Removed canonical blocks from the tree");
    }

    /// Removes all blocks that are below the finalized block, as well as removing non-canonical
    /// sidechains that fork from below the finalized block.
    pub fn prune_finalized_sidechains(&mut self, finalized_num_hash: BlockNumHash) {
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

        // Invalidate the cached overlay since blocks were removed and the anchor may have changed
        self.invalidate_cached_overlay();
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
}

/// Pre-computed lazy overlay for the canonical head block.
///
/// This is prepared **optimistically** when the canonical head changes, allowing
/// the next payload (which typically builds on the canonical head) to reuse
/// the pre-computed overlay immediately without re-traversing in-memory blocks.
///
/// The overlay captures deferred trie data handles from all in-memory blocks
/// between the canonical head and the persisted anchor. When a new payload
/// arrives building on the canonical head, this cached overlay can be used
/// directly instead of calling `blocks_by_hash` and collecting handles again.
///
/// # Invalidation
///
/// The cached overlay is invalidated when:
/// - Persistence completes (anchor changes)
/// - The canonical head changes to a different block
#[derive(Debug, Clone)]
pub struct PreparedCanonicalOverlay {
    /// The block hash for which this overlay is prepared as a parent.
    ///
    /// When a payload arrives with this parent hash, the overlay can be reused.
    pub parent_hash: B256,
    /// The pre-computed lazy overlay containing deferred trie data handles.
    ///
    /// This is computed optimistically after `set_canonical_head` so subsequent
    /// payloads don't need to re-collect the handles.
    pub overlay: LazyOverlay,
    /// The anchor hash (persisted ancestor) this overlay is based on.
    ///
    /// Used to verify the overlay is still valid (anchor hasn't changed due to persistence).
    pub anchor_hash: B256,
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
            Some(&B256Set::from_iter([blocks[1].recovered_block().hash()]))
        );

        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].recovered_block().hash()));

        tree_state.insert_executed(blocks[2].clone());

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[1].recovered_block().hash()),
            Some(&B256Set::from_iter([blocks[2].recovered_block().hash()]))
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
            Some(&B256Set::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&B256Set::from_iter([blocks[4].recovered_block().hash()]))
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
            Some(&B256Set::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&B256Set::from_iter([blocks[4].recovered_block().hash()]))
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
            Some(&B256Set::from_iter([blocks[3].recovered_block().hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].recovered_block().hash()),
            Some(&B256Set::from_iter([blocks[4].recovered_block().hash()]))
        );
    }

    #[tokio::test]
    async fn test_sidechain_limit_evicts_oldest_non_canonical() {
        let mut tree_state = TreeState::new(BlockNumHash::default(), EngineApiKind::Ethereum);
        let mut test_block_builder = TestBlockBuilder::eth();

        // Insert a canonical chain: block 1 -> block 2
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..3).collect();
        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        // Set canonical head to block 2
        tree_state.set_canonical_head(blocks[1].recovered_block().num_hash());

        let parent_hash = blocks[0].recovered_block().hash();

        // Insert MAX_SIDECHAINS_PER_BLOCK + 10 fork blocks at height 2, all parented to block 1
        let mut fork_blocks = Vec::new();
        for _ in 0..(MAX_SIDECHAINS_PER_BLOCK + 10) {
            let fork = test_block_builder.get_executed_block_with_number(2, parent_hash);
            fork_blocks.push(fork);
        }

        for fork in &fork_blocks {
            tree_state.insert_executed(fork.clone());
        }

        // Should be capped: the canonical block + MAX_SIDECHAINS_PER_BLOCK - 1 forks
        assert!(
            tree_state.blocks_by_number[&2].len() <= MAX_SIDECHAINS_PER_BLOCK,
            "blocks at height 2 should be at most {MAX_SIDECHAINS_PER_BLOCK}, got {}",
            tree_state.blocks_by_number[&2].len()
        );

        // Canonical block must survive eviction
        assert!(
            tree_state.blocks_by_hash.contains_key(&blocks[1].recovered_block().hash()),
            "canonical block must not be evicted"
        );

        // Oldest non-canonical forks should have been evicted
        assert!(
            !tree_state.blocks_by_hash.contains_key(&fork_blocks[0].recovered_block().hash()),
            "oldest fork block should be evicted"
        );
    }

    #[tokio::test]
    async fn test_sidechain_limit_evicts_descendant_subtrees() {
        let mut tree_state = TreeState::new(BlockNumHash::default(), EngineApiKind::Ethereum);
        let mut test_block_builder = TestBlockBuilder::eth();

        // Build canonical chain: 1 -> 2 -> 3
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..4).collect();
        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        tree_state.set_canonical_head(blocks[2].recovered_block().num_hash());

        let parent_hash = blocks[0].recovered_block().hash();

        // Insert MAX_SIDECHAINS_PER_BLOCK - 1 fork blocks at height 2
        // (together with the canonical block, this fills the limit exactly)
        let mut fork_blocks_h2 = Vec::new();
        for _ in 0..(MAX_SIDECHAINS_PER_BLOCK - 1) {
            let fork = test_block_builder.get_executed_block_with_number(2, parent_hash);
            fork_blocks_h2.push(fork);
        }
        for fork in &fork_blocks_h2 {
            tree_state.insert_executed(fork.clone());
        }

        // Add a child (height 3) to the first fork block at height 2.
        // Inserted while fork_blocks_h2[0] is still in the tree.
        let child_of_first_fork = test_block_builder
            .get_executed_block_with_number(3, fork_blocks_h2[0].recovered_block().hash());
        tree_state.insert_executed(child_of_first_fork.clone());

        // Now insert one more fork at height 2 to exceed the limit and trigger eviction.
        let extra_fork = test_block_builder.get_executed_block_with_number(2, parent_hash);
        tree_state.insert_executed(extra_fork);

        // The first fork block should have been evicted (oldest non-canonical)
        assert!(
            !tree_state.blocks_by_hash.contains_key(&fork_blocks_h2[0].recovered_block().hash()),
            "first fork block should be evicted"
        );

        // Its child at height 3 should also have been evicted via BFS subtree removal.
        assert!(
            !tree_state.blocks_by_hash.contains_key(&child_of_first_fork.recovered_block().hash()),
            "child of evicted fork block should also be evicted"
        );

        // Canonical blocks must survive
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[1].recovered_block().hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].recovered_block().hash()));
    }
}
