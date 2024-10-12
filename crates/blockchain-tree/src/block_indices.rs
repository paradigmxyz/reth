//! Implementation of [`BlockIndices`] related to [`super::BlockchainTree`]

use super::state::SidechainId;
use crate::canonical_chain::CanonicalChain;
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber};
use linked_hash_set::LinkedHashSet;
use reth_execution_types::Chain;
use reth_primitives::SealedBlockWithSenders;
use std::collections::{btree_map, hash_map, BTreeMap, BTreeSet, HashMap, HashSet};

/// Internal indices of the blocks and chains.
///
/// This is main connection between blocks, chains and canonical chain.
///
/// It contains a list of canonical block hashes, forks to child blocks, and a mapping of block hash
/// to chain ID.
#[derive(Debug, Clone)]
pub struct BlockIndices {
    /// Last finalized block.
    last_finalized_block: BlockNumber,
    /// Non-finalized canonical chain. Contains N number (depends on `finalization_depth`) of
    /// blocks. These blocks are found in `fork_to_child` but not inside `blocks_to_chain` or
    /// `number_to_block` as those are sidechain specific indices.
    canonical_chain: CanonicalChain,
    /// Index needed when discarding the chain, so we can remove connected chains from tree.
    ///
    /// This maintains insertion order for all child blocks, so
    /// [`BlockIndices::pending_block_num_hash`] returns always the same block: the first child
    /// block we inserted.
    ///
    /// NOTE: It contains just blocks that are forks as a key and not all blocks.
    fork_to_child: HashMap<BlockHash, LinkedHashSet<BlockHash>>,
    /// Utility index for Block number to block hash(s).
    ///
    /// This maps all blocks with same block number to their hash.
    ///
    /// Can be used for RPC fetch block(s) in chain by its number.
    ///
    /// Note: This is a bijection: at all times `blocks_to_chain` and this map contain the block
    /// hashes.
    block_number_to_block_hashes: BTreeMap<BlockNumber, HashSet<BlockHash>>,
    /// Block hashes to the sidechain IDs they belong to.
    blocks_to_chain: HashMap<BlockHash, SidechainId>,
}

impl BlockIndices {
    /// Create new block indices structure
    pub fn new(
        last_finalized_block: BlockNumber,
        canonical_chain: BTreeMap<BlockNumber, BlockHash>,
    ) -> Self {
        Self {
            last_finalized_block,
            canonical_chain: CanonicalChain::new(canonical_chain),
            fork_to_child: Default::default(),
            blocks_to_chain: Default::default(),
            block_number_to_block_hashes: Default::default(),
        }
    }

    /// Return fork to child indices
    pub const fn fork_to_child(&self) -> &HashMap<BlockHash, LinkedHashSet<BlockHash>> {
        &self.fork_to_child
    }

    /// Return block to sidechain id
    #[allow(dead_code)]
    pub(crate) const fn blocks_to_chain(&self) -> &HashMap<BlockHash, SidechainId> {
        &self.blocks_to_chain
    }

    /// Returns the hash and number of the pending block.
    ///
    /// It is possible that multiple child blocks for the canonical tip exist.
    /// This will always return the _first_ child we recorded for the canonical tip.
    pub(crate) fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        let canonical_tip = self.canonical_tip();
        let hash = self.fork_to_child.get(&canonical_tip.hash)?.front().copied()?;
        Some(BlockNumHash { number: canonical_tip.number + 1, hash })
    }

    /// Returns all pending block hashes.
    ///
    /// Pending blocks are considered blocks that are extending the canonical tip by one block
    /// number and have their parent hash set to the canonical tip.
    pub fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        let canonical_tip = self.canonical_tip();
        let pending_blocks = self
            .fork_to_child
            .get(&canonical_tip.hash)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        (canonical_tip.number + 1, pending_blocks)
    }

    /// Last finalized block
    pub const fn last_finalized_block(&self) -> BlockNumber {
        self.last_finalized_block
    }

    /// Insert non fork block.
    pub(crate) fn insert_non_fork_block(
        &mut self,
        block_number: BlockNumber,
        block_hash: BlockHash,
        chain_id: SidechainId,
    ) {
        self.block_number_to_block_hashes.entry(block_number).or_default().insert(block_hash);
        self.blocks_to_chain.insert(block_hash, chain_id);
    }

    /// Insert block to chain and fork child indices of the new chain
    pub(crate) fn insert_chain(&mut self, chain_id: SidechainId, chain: &Chain) {
        for (number, block) in chain.blocks() {
            // add block -> chain_id index
            self.blocks_to_chain.insert(block.hash(), chain_id);
            // add number -> block
            self.block_number_to_block_hashes.entry(*number).or_default().insert(block.hash());
        }
        let first = chain.first();
        // add parent block -> block index
        self.fork_to_child.entry(first.parent_hash).or_default().insert_if_absent(first.hash());
    }

    /// Get the [`SidechainId`] for the given block hash if it exists.
    pub(crate) fn get_side_chain_id(&self, block: &BlockHash) -> Option<SidechainId> {
        self.blocks_to_chain.get(block).copied()
    }

    /// Update all block hashes. iterate over present and new list of canonical hashes and compare
    /// them. Remove all mismatches, disconnect them and return all chains that needs to be
    /// removed.
    pub(crate) fn update_block_hashes(
        &mut self,
        hashes: BTreeMap<u64, BlockHash>,
    ) -> (BTreeSet<SidechainId>, Vec<BlockNumHash>) {
        // set new canonical hashes.
        self.canonical_chain.replace(hashes.clone());

        let mut new_hashes = hashes.into_iter();
        let mut old_hashes = self.canonical_chain().clone().into_iter();

        let mut removed = Vec::new();
        let mut added = Vec::new();

        let mut new_hash = new_hashes.next();
        let mut old_hash = old_hashes.next();

        loop {
            let Some(old_block_value) = old_hash else {
                // end of old_hashes canonical chain. New chain has more blocks than old chain.
                while let Some(new) = new_hash {
                    // add new blocks to added list.
                    added.push(new.into());
                    new_hash = new_hashes.next();
                }
                break
            };
            let Some(new_block_value) = new_hash else {
                // Old canonical chain had more block than new chain.
                // remove all present block.
                // this is mostly not going to happen as reorg should make new chain in Tree.
                while let Some(rem) = old_hash {
                    removed.push(rem);
                    old_hash = old_hashes.next();
                }
                break
            };
            // compare old and new canonical block number
            match new_block_value.0.cmp(&old_block_value.0) {
                std::cmp::Ordering::Less => {
                    // new chain has more past blocks than old chain
                    added.push(new_block_value.into());
                    new_hash = new_hashes.next();
                }
                std::cmp::Ordering::Equal => {
                    if new_block_value.1 != old_block_value.1 {
                        // remove block hash as it is different
                        removed.push(old_block_value);
                        added.push(new_block_value.into())
                    }
                    new_hash = new_hashes.next();
                    old_hash = old_hashes.next();
                }
                std::cmp::Ordering::Greater => {
                    // old chain has more past blocks than new chain
                    removed.push(old_block_value);
                    old_hash = old_hashes.next()
                }
            }
        }

        // remove children of removed blocks
        (
            removed.into_iter().fold(BTreeSet::new(), |mut fold, (number, hash)| {
                fold.extend(self.remove_block(number, hash));
                fold
            }),
            added,
        )
    }

    /// Remove chain from indices and return dependent chains that need to be removed.
    /// Does the cleaning of the tree and removing blocks from the chain.
    pub(crate) fn remove_chain(&mut self, chain: &Chain) -> BTreeSet<SidechainId> {
        chain
            .blocks()
            .iter()
            .flat_map(|(block_number, block)| {
                let block_hash = block.hash();
                self.remove_block(*block_number, block_hash)
            })
            .collect()
    }

    /// Remove Blocks from indices.
    fn remove_block(
        &mut self,
        block_number: BlockNumber,
        block_hash: BlockHash,
    ) -> BTreeSet<SidechainId> {
        // rm number -> block
        if let btree_map::Entry::Occupied(mut entry) =
            self.block_number_to_block_hashes.entry(block_number)
        {
            let set = entry.get_mut();
            set.remove(&block_hash);
            // remove set if empty
            if set.is_empty() {
                entry.remove();
            }
        }

        // rm block -> chain_id
        self.blocks_to_chain.remove(&block_hash);

        // rm fork -> child
        let removed_fork = self.fork_to_child.remove(&block_hash);
        removed_fork
            .map(|fork_blocks| {
                fork_blocks
                    .into_iter()
                    .filter_map(|fork_child| self.blocks_to_chain.remove(&fork_child))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remove all blocks from canonical list and insert new blocks to it.
    ///
    /// It is assumed that blocks are interconnected and that they connect to canonical chain
    pub fn canonicalize_blocks(&mut self, blocks: &BTreeMap<BlockNumber, SealedBlockWithSenders>) {
        if blocks.is_empty() {
            return
        }

        // Remove all blocks from canonical chain
        let first_number = *blocks.first_key_value().unwrap().0;

        // this will remove all blocks numbers that are going to be replaced.
        self.canonical_chain.retain(|&number, _| number < first_number);

        // remove them from block to chain_id index
        blocks.iter().map(|(_, b)| (b.number, b.hash(), b.parent_hash)).for_each(
            |(number, hash, parent_hash)| {
                // rm block -> chain_id
                self.blocks_to_chain.remove(&hash);

                // rm number -> block
                if let btree_map::Entry::Occupied(mut entry) =
                    self.block_number_to_block_hashes.entry(number)
                {
                    let set = entry.get_mut();
                    set.remove(&hash);
                    // remove set if empty
                    if set.is_empty() {
                        entry.remove();
                    }
                }
                // rm fork block -> hash
                if let hash_map::Entry::Occupied(mut entry) = self.fork_to_child.entry(parent_hash)
                {
                    let set = entry.get_mut();
                    set.remove(&hash);
                    // remove set if empty
                    if set.is_empty() {
                        entry.remove();
                    }
                }
            },
        );

        // insert new canonical
        self.canonical_chain.extend(blocks.iter().map(|(number, block)| (*number, block.hash())))
    }

    /// this is function that is going to remove N number of last canonical hashes.
    ///
    /// NOTE: This is not safe standalone, as it will not disconnect
    /// blocks that depend on unwinded canonical chain. And should be
    /// used when canonical chain is reinserted inside Tree.
    pub(crate) fn unwind_canonical_chain(&mut self, unwind_to: BlockNumber) {
        // this will remove all blocks numbers that are going to be replaced.
        self.canonical_chain.retain(|num, _| *num <= unwind_to);
    }

    /// Used for finalization of block.
    ///
    /// Return list of chains for removal that depend on finalized canonical chain.
    pub(crate) fn finalize_canonical_blocks(
        &mut self,
        finalized_block: BlockNumber,
        num_of_additional_canonical_hashes_to_retain: u64,
    ) -> BTreeSet<SidechainId> {
        // get finalized chains. blocks between [self.last_finalized,finalized_block).
        // Dont remove finalized_block, as sidechain can point to it.
        let finalized_blocks: Vec<BlockHash> = self
            .canonical_chain
            .iter()
            .filter(|(number, _)| *number >= self.last_finalized_block && *number < finalized_block)
            .map(|(_, hash)| hash)
            .collect();

        // remove unneeded canonical hashes.
        let remove_until =
            finalized_block.saturating_sub(num_of_additional_canonical_hashes_to_retain);
        self.canonical_chain.retain(|&number, _| number >= remove_until);

        let mut lose_chains = BTreeSet::new();

        for block_hash in finalized_blocks {
            // there is a fork block.
            if let Some(fork_blocks) = self.fork_to_child.remove(&block_hash) {
                lose_chains = fork_blocks.into_iter().fold(lose_chains, |mut fold, fork_child| {
                    if let Some(lose_chain) = self.blocks_to_chain.remove(&fork_child) {
                        fold.insert(lose_chain);
                    }
                    fold
                });
            }
        }

        // set last finalized block.
        self.last_finalized_block = finalized_block;

        lose_chains
    }

    /// Returns the block hash of the canonical block with the given number.
    #[inline]
    pub fn canonical_hash(&self, block_number: &BlockNumber) -> Option<BlockHash> {
        self.canonical_chain.canonical_hash(block_number)
    }

    /// Returns the block number of the canonical block with the given hash.
    #[inline]
    pub fn canonical_number(&self, block_hash: &BlockHash) -> Option<BlockNumber> {
        self.canonical_chain.canonical_number(block_hash)
    }

    /// get canonical tip
    #[inline]
    pub fn canonical_tip(&self) -> BlockNumHash {
        self.canonical_chain.tip()
    }

    /// Canonical chain needed for execution of EVM. It should contain last 256 block hashes.
    #[inline]
    pub(crate) const fn canonical_chain(&self) -> &CanonicalChain {
        &self.canonical_chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_primitives::{Header, SealedBlock, SealedHeader};

    #[test]
    fn pending_block_num_hash_returns_none_if_no_fork() {
        // Create a new canonical chain with a single block (represented by its number and hash).
        let canonical_chain = BTreeMap::from([(0, B256::from_slice(&[1; 32]))]);

        let block_indices = BlockIndices::new(0, canonical_chain);

        // No fork to child blocks, so there is no pending block.
        assert_eq!(block_indices.pending_block_num_hash(), None);
    }

    #[test]
    fn pending_block_num_hash_works() {
        // Create a canonical chain with multiple blocks at heights 1, 2, and 3.
        let canonical_chain = BTreeMap::from([
            (1, B256::from_slice(&[1; 32])),
            (2, B256::from_slice(&[2; 32])),
            (3, B256::from_slice(&[3; 32])),
        ]);

        let mut block_indices = BlockIndices::new(3, canonical_chain);

        // Define the hash of the parent block (the block at height 3 in the canonical chain).
        let parent_hash = B256::from_slice(&[3; 32]);

        // Define the hashes of two child blocks that extend the canonical chain.
        let child_hash_1 = B256::from_slice(&[2; 32]);
        let child_hash_2 = B256::from_slice(&[3; 32]);

        // Create a set to store both child block hashes.
        let mut child_set = LinkedHashSet::new();
        child_set.insert(child_hash_1);
        child_set.insert(child_hash_2);

        // Associate the parent block hash with its children in the fork_to_child mapping.
        block_indices.fork_to_child.insert(parent_hash, child_set);

        // Pending block should be the first child block.
        assert_eq!(
            block_indices.pending_block_num_hash(),
            Some(BlockNumHash { number: 4, hash: child_hash_1 })
        );
    }

    #[test]
    fn pending_blocks_returns_empty_if_no_fork() {
        // Create a canonical chain with a single block at height 10.
        let canonical_chain = BTreeMap::from([(10, B256::from_slice(&[1; 32]))]);
        let block_indices = BlockIndices::new(0, canonical_chain);

        // No child blocks are associated with the canonical tip.
        assert_eq!(block_indices.pending_blocks(), (11, Vec::new()));
    }

    #[test]
    fn pending_blocks_returns_multiple_children() {
        // Define the hash of the parent block (the block at height 5 in the canonical chain).
        let parent_hash = B256::from_slice(&[3; 32]);

        // Create a canonical chain with a block at height 5.
        let canonical_chain = BTreeMap::from([(5, parent_hash)]);
        let mut block_indices = BlockIndices::new(0, canonical_chain);

        // Define the hashes of two child blocks.
        let child_hash_1 = B256::from_slice(&[4; 32]);
        let child_hash_2 = B256::from_slice(&[5; 32]);

        // Create a set to store both child block hashes.
        let mut child_set = LinkedHashSet::new();
        child_set.insert(child_hash_1);
        child_set.insert(child_hash_2);

        // Associate the parent block hash with its children.
        block_indices.fork_to_child.insert(parent_hash, child_set);

        // Pending blocks should be the two child blocks.
        assert_eq!(block_indices.pending_blocks(), (6, vec![child_hash_1, child_hash_2]));
    }

    #[test]
    fn pending_blocks_with_multiple_forked_chains() {
        // Define hashes for parent blocks and child blocks.
        let parent_hash_1 = B256::from_slice(&[6; 32]);
        let parent_hash_2 = B256::from_slice(&[7; 32]);

        // Create a canonical chain with blocks at heights 1 and 2.
        let canonical_chain = BTreeMap::from([(1, parent_hash_1), (2, parent_hash_2)]);

        let mut block_indices = BlockIndices::new(2, canonical_chain);

        // Define hashes for child blocks.
        let child_hash_1 = B256::from_slice(&[8; 32]);
        let child_hash_2 = B256::from_slice(&[9; 32]);

        // Create sets to store child blocks for each parent block.
        let mut child_set_1 = LinkedHashSet::new();
        let mut child_set_2 = LinkedHashSet::new();
        child_set_1.insert(child_hash_1);
        child_set_2.insert(child_hash_2);

        // Associate parent block hashes with their child blocks.
        block_indices.fork_to_child.insert(parent_hash_1, child_set_1);
        block_indices.fork_to_child.insert(parent_hash_2, child_set_2);

        // Check that the pending blocks are only those extending the canonical tip.
        assert_eq!(block_indices.pending_blocks(), (3, vec![child_hash_2]));
    }

    #[test]
    fn insert_non_fork_block_adds_block_correctly() {
        // Create a new BlockIndices instance with an empty state.
        let mut block_indices = BlockIndices::new(0, BTreeMap::new());

        // Define test parameters.
        let block_number = 1;
        let block_hash = B256::from_slice(&[1; 32]);
        let chain_id = SidechainId::from(42);

        // Insert the block into the BlockIndices instance.
        block_indices.insert_non_fork_block(block_number, block_hash, chain_id);

        // Check that the block number to block hashes mapping includes the new block hash.
        assert_eq!(
            block_indices.block_number_to_block_hashes.get(&block_number),
            Some(&HashSet::from([block_hash]))
        );

        // Check that the block hash to chain ID mapping includes the new entry.
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash), Some(&chain_id));
    }

    #[test]
    fn insert_non_fork_block_combined_tests() {
        // Create a new BlockIndices instance with an empty state.
        let mut block_indices = BlockIndices::new(0, BTreeMap::new());

        // Define test parameters.
        let block_number_1 = 2;
        let block_hash_1 = B256::from_slice(&[1; 32]);
        let block_hash_2 = B256::from_slice(&[2; 32]);
        let chain_id_1 = SidechainId::from(84);

        let block_number_2 = 4;
        let block_hash_3 = B256::from_slice(&[3; 32]);
        let chain_id_2 = SidechainId::from(200);

        // Insert multiple hashes for the same block number.
        block_indices.insert_non_fork_block(block_number_1, block_hash_1, chain_id_1);
        block_indices.insert_non_fork_block(block_number_1, block_hash_2, chain_id_1);

        // Insert blocks with different numbers.
        block_indices.insert_non_fork_block(block_number_2, block_hash_3, chain_id_2);

        // Block number 1 should have two block hashes associated with it.
        let mut expected_hashes_for_block_1 = HashSet::default();
        expected_hashes_for_block_1.insert(block_hash_1);
        expected_hashes_for_block_1.insert(block_hash_2);
        assert_eq!(
            block_indices.block_number_to_block_hashes.get(&block_number_1),
            Some(&expected_hashes_for_block_1)
        );

        // Check that the block hashes for block_number_1 are associated with the correct chain ID.
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash_1), Some(&chain_id_1));
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash_2), Some(&chain_id_1));

        // Block number 2 should have a single block hash associated with it.
        assert_eq!(
            block_indices.block_number_to_block_hashes.get(&block_number_2),
            Some(&HashSet::from([block_hash_3]))
        );

        // Block hash 3 should be associated with the correct chain ID.
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash_3), Some(&chain_id_2));
    }

    #[test]
    fn insert_chain_validates_insertion() {
        // Create a new BlockIndices instance with an empty state.
        let mut block_indices = BlockIndices::new(0, BTreeMap::new());

        // Define test parameters.
        let chain_id = SidechainId::from(42);

        // Define some example blocks and their hashes.
        let block_hash_1 = B256::from_slice(&[1; 32]);
        let block_hash_2 = B256::from_slice(&[2; 32]);
        let parent_hash = B256::from_slice(&[0; 32]);

        // Define blocks with their numbers and parent hashes.
        let block_1 = SealedBlockWithSenders {
            block: SealedBlock {
                header: SealedHeader::new(
                    Header { parent_hash, number: 1, ..Default::default() },
                    block_hash_1,
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let block_2 = SealedBlockWithSenders {
            block: SealedBlock {
                header: SealedHeader::new(
                    Header { parent_hash: block_hash_1, number: 2, ..Default::default() },
                    block_hash_2,
                ),
                ..Default::default()
            },
            ..Default::default()
        };

        // Define a chain containing the blocks.
        let chain = Chain::new(vec![block_1, block_2], Default::default(), Default::default());

        // Insert the chain into the BlockIndices.
        block_indices.insert_chain(chain_id, &chain);

        // Check that the blocks are correctly mapped to the chain ID.
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash_1), Some(&chain_id));
        assert_eq!(block_indices.blocks_to_chain.get(&block_hash_2), Some(&chain_id));

        // Check that block numbers map to their respective hashes.
        let mut expected_hashes_1 = HashSet::default();
        expected_hashes_1.insert(block_hash_1);
        assert_eq!(block_indices.block_number_to_block_hashes.get(&1), Some(&expected_hashes_1));

        let mut expected_hashes_2 = HashSet::default();
        expected_hashes_2.insert(block_hash_2);
        assert_eq!(block_indices.block_number_to_block_hashes.get(&2), Some(&expected_hashes_2));

        // Check that the fork_to_child mapping contains the correct parent-child relationship.
        // We take the first block of the chain.
        let mut expected_children = LinkedHashSet::new();
        expected_children.insert(block_hash_1);
        assert_eq!(block_indices.fork_to_child.get(&parent_hash), Some(&expected_children));
    }
}
