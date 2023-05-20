//! Implementation of [`BlockIndices`] related to [`super::BlockchainTree`]

use super::chain::BlockChainId;
use crate::canonical_chain::CanonicalChain;
use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlockWithSenders};
use reth_provider::Chain;
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
    /// Canonical chain. Contains N number (depends on `finalization_depth`) of blocks.
    /// These blocks are found in fork_to_child but not inside `blocks_to_chain` or
    /// `number_to_block` as those are chain specific indices.
    canonical_chain: CanonicalChain,
    /// Index needed when discarding the chain, so we can remove connected chains from tree.
    /// NOTE: It contains just blocks that are forks as a key and not all blocks.
    fork_to_child: HashMap<BlockHash, HashSet<BlockHash>>,
    /// Utility index for Block number to block hash(s).
    ///
    /// This maps all blocks with same block number to their hash.
    ///
    /// Can be used for RPC fetch block(s) in chain by its number.
    ///
    /// Note: This is a bijection: at all times `blocks_to_chain` and this map contain the block
    /// hashes.
    block_number_to_block_hashes: BTreeMap<BlockNumber, HashSet<BlockHash>>,
    /// Block hashes and side chain they belong
    blocks_to_chain: HashMap<BlockHash, BlockChainId>,
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

    /// Return internal index that maps all pending block number to their hashes.
    ///
    /// This essentially contains all possible branches. Given a parent block, then the child block
    /// number as the key has all possible block hashes as the value.
    pub fn block_number_to_block_hashes(&self) -> &BTreeMap<BlockNumber, HashSet<BlockHash>> {
        &self.block_number_to_block_hashes
    }

    /// Return fork to child indices
    pub fn fork_to_child(&self) -> &HashMap<BlockHash, HashSet<BlockHash>> {
        &self.fork_to_child
    }

    /// Return block to chain id
    pub fn blocks_to_chain(&self) -> &HashMap<BlockHash, BlockChainId> {
        &self.blocks_to_chain
    }

    /// Returns the hash and number of the pending block (the first block in the
    /// [Self::pending_blocks]) set.
    pub(crate) fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        let canonical_tip = self.canonical_tip();
        let hash = self.fork_to_child.get(&canonical_tip.hash)?.iter().next().copied()?;
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

    /// Returns the block number of the canonical block with the given hash.
    ///
    /// Returns `None` if no block could be found in the canonical chain.
    #[inline]
    pub(crate) fn get_canonical_block_number(&self, block_hash: &BlockHash) -> Option<BlockNumber> {
        self.canonical_chain.get_canonical_block_number(self.last_finalized_block, block_hash)
    }

    /// Check if block hash belongs to canonical chain.
    pub(crate) fn is_block_hash_canonical(&self, block_hash: &BlockHash) -> bool {
        self.get_canonical_block_number(block_hash).is_some()
    }

    /// Last finalized block
    pub fn last_finalized_block(&self) -> BlockNumber {
        self.last_finalized_block
    }

    /// Insert non fork block.
    pub(crate) fn insert_non_fork_block(
        &mut self,
        block_number: BlockNumber,
        block_hash: BlockHash,
        chain_id: BlockChainId,
    ) {
        self.block_number_to_block_hashes.entry(block_number).or_default().insert(block_hash);
        self.blocks_to_chain.insert(block_hash, chain_id);
    }

    /// Insert block to chain and fork child indices of the new chain
    pub(crate) fn insert_chain(&mut self, chain_id: BlockChainId, chain: &Chain) {
        for (number, block) in chain.blocks().iter() {
            // add block -> chain_id index
            self.blocks_to_chain.insert(block.hash(), chain_id);
            // add number -> block
            self.block_number_to_block_hashes.entry(*number).or_default().insert(block.hash());
        }
        let first = chain.first();
        // add parent block -> block index
        self.fork_to_child.entry(first.parent_hash).or_default().insert(first.hash());
    }

    /// Get the chain ID the block belongs to
    pub(crate) fn get_blocks_chain_id(&self, block: &BlockHash) -> Option<BlockChainId> {
        self.blocks_to_chain.get(block).cloned()
    }

    /// Update all block hashes. iterate over present and new list of canonical hashes and compare
    /// them. Remove all missmatches, disconnect them and return all chains that needs to be
    /// removed.
    pub(crate) fn update_block_hashes(
        &mut self,
        hashes: BTreeMap<u64, BlockHash>,
    ) -> (BTreeSet<BlockChainId>, Vec<BlockNumHash>) {
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
                // end of old_hashes canonical chain. New chain has more block then old chain.
                while let Some(new) = new_hash {
                    // add new blocks to added list.
                    added.push(new.into());
                    new_hash = new_hashes.next();
                }
                break
            };
            let Some(new_block_value) = new_hash else  {
                // Old canonical chain had more block than new chain.
                // remove all present block.
                // this is mostly not going to happen as reorg should make new chain in Tree.
                while let Some(rem) = old_hash {
                    removed.push(rem);
                    old_hash = old_hashes.next();
                }
                break;
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
                        added.push(new_block_value.into());
                    }
                    new_hash = new_hashes.next();
                    old_hash = old_hashes.next();
                }
                std::cmp::Ordering::Greater => {
                    // old chain has more past blocks that new chain
                    removed.push(old_block_value);
                    old_hash = old_hashes.next()
                }
            }
        }

        // remove childs of removed blocks
        (
            removed.into_iter().fold(BTreeSet::new(), |mut fold, (number, hash)| {
                fold.extend(self.remove_block(number, hash));
                fold
            }),
            added,
        )
    }

    /// Remove chain from indices and return dependent chains that needs to be removed.
    /// Does the cleaning of the tree and removing blocks from the chain.
    pub fn remove_chain(&mut self, chain: &Chain) -> BTreeSet<BlockChainId> {
        let mut lose_chains = BTreeSet::new();
        for (block_number, block) in chain.blocks().iter() {
            let block_hash = block.hash();
            lose_chains.extend(self.remove_block(*block_number, block_hash))
        }
        lose_chains
    }

    /// Remove Blocks from indices.
    fn remove_block(
        &mut self,
        block_number: BlockNumber,
        block_hash: BlockHash,
    ) -> BTreeSet<BlockChainId> {
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
    /// blocks that depends on unwinded canonical chain. And should be
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
    ) -> BTreeSet<BlockChainId> {
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

        for block_hash in finalized_blocks.into_iter() {
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
    pub fn canonical_number(&self, block_hash: BlockHash) -> Option<BlockNumber> {
        self.canonical_chain.canonical_number(block_hash)
    }

    /// get canonical tip
    #[inline]
    pub fn canonical_tip(&self) -> BlockNumHash {
        self.canonical_chain.tip()
    }

    /// Canonical chain needed for execution of EVM. It should contains last 256 block hashes.
    #[inline]
    pub(crate) fn canonical_chain(&self) -> &CanonicalChain {
        &self.canonical_chain
    }
}
