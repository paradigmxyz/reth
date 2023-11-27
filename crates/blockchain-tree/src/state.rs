//! Blockchain tree state.

use crate::{AppendableChain, BlockBuffer, BlockIndices};
use reth_primitives::{BlockHash, BlockNumber, Receipt, SealedBlock, SealedBlockWithSenders};
use std::collections::{BTreeMap, HashMap};

/// Container to hold the state of the blockchain tree.
#[derive(Debug)]
pub(crate) struct TreeState {
    /// Keeps track of new unique identifiers for chains
    block_chain_id_generator: u64,
    /// The tracked chains and their current data.
    pub(crate) chains: HashMap<BlockChainId, AppendableChain>,
    /// Indices to block and their connection to the canonical chain.
    ///
    /// This gets modified by the tree itself and is read from engine API/RPC to access the pending
    /// block for example.
    pub(crate) block_indices: BlockIndices,
    /// Unconnected block buffer.
    pub(crate) buffered_blocks: BlockBuffer,
}

impl TreeState {
    /// Initializes the tree state with the given last finalized block number and last canonical
    /// hashes.
    pub(crate) fn new(
        last_finalized_block_number: BlockNumber,
        last_canonical_hashes: impl IntoIterator<Item = (BlockNumber, BlockHash)>,
        buffer_limit: usize,
    ) -> Self {
        Self {
            block_chain_id_generator: 0,
            chains: Default::default(),
            block_indices: BlockIndices::new(
                last_finalized_block_number,
                BTreeMap::from_iter(last_canonical_hashes),
            ),
            buffered_blocks: BlockBuffer::new(buffer_limit),
        }
    }

    /// Issues a new unique identifier for a new chain.
    #[inline]
    fn next_id(&mut self) -> BlockChainId {
        let id = self.block_chain_id_generator;
        self.block_chain_id_generator += 1;
        BlockChainId(id)
    }

    /// Expose internal indices of the BlockchainTree.
    #[inline]
    pub(crate) fn block_indices(&self) -> &BlockIndices {
        &self.block_indices
    }

    /// Returns the block with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    #[inline]
    pub(crate) fn block_by_hash(&self, block_hash: BlockHash) -> Option<&SealedBlock> {
        self.block_with_senders_by_hash(block_hash).map(|block| &block.block)
    }
    /// Returns the block with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    #[inline]
    pub(crate) fn block_with_senders_by_hash(
        &self,
        block_hash: BlockHash,
    ) -> Option<&SealedBlockWithSenders> {
        let id = self.block_indices.get_blocks_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.block_with_senders(block_hash)
    }

    /// Returns the block's receipts with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    pub(crate) fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<&Receipt>> {
        let id = self.block_indices.get_blocks_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.receipts_by_block_hash(block_hash)
    }

    /// Insert a chain into the tree.
    ///
    /// Inserts a chain into the tree and builds the block indices.
    pub(crate) fn insert_chain(&mut self, chain: AppendableChain) -> Option<BlockChainId> {
        if chain.is_empty() {
            return None
        }
        let chain_id = self.next_id();

        self.block_indices.insert_chain(chain_id, &chain);
        // add chain_id -> chain index
        self.chains.insert(chain_id, chain);
        Some(chain_id)
    }

    /// Checks the block buffer for the given block.
    pub(crate) fn get_buffered_block(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        self.buffered_blocks.block_by_hash(hash)
    }

    /// Gets the lowest ancestor for the given block in the block buffer.
    pub(crate) fn lowest_buffered_ancestor(
        &self,
        hash: &BlockHash,
    ) -> Option<&SealedBlockWithSenders> {
        self.buffered_blocks.lowest_ancestor(hash)
    }
}

/// The ID of a sidechain internally in a [`BlockchainTree`][super::BlockchainTree].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct BlockChainId(u64);

impl From<BlockChainId> for u64 {
    fn from(value: BlockChainId) -> Self {
        value.0
    }
}

#[cfg(test)]
impl From<u64> for BlockChainId {
    fn from(value: u64) -> Self {
        BlockChainId(value)
    }
}
