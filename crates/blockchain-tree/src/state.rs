//! Blockchain tree state.

use crate::{AppendableChain, BlockBuffer, BlockIndices};
use alloy_primitives::{BlockHash, BlockNumber};
use reth_primitives::{Receipt, SealedBlock, SealedBlockWithSenders};
use std::collections::{BTreeMap, HashMap};

/// Container to hold the state of the blockchain tree.
#[derive(Debug)]
pub(crate) struct TreeState {
    /// Keeps track of new unique identifiers for chains
    block_chain_id_generator: u64,
    /// The tracked chains and their current data.
    pub(crate) chains: HashMap<SidechainId, AppendableChain>,
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
        buffer_limit: u32,
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

    /// Issues a new unique identifier for a new sidechain.
    #[inline]
    fn next_id(&mut self) -> SidechainId {
        let id = self.block_chain_id_generator;
        self.block_chain_id_generator += 1;
        SidechainId(id)
    }

    /// Expose internal indices of the `BlockchainTree`.
    #[inline]
    pub(crate) const fn block_indices(&self) -> &BlockIndices {
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
        let id = self.block_indices.get_side_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.block_with_senders(block_hash)
    }

    /// Returns the block's receipts with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    pub(crate) fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<&Receipt>> {
        let id = self.block_indices.get_side_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.receipts_by_block_hash(block_hash)
    }

    /// Insert a chain into the tree.
    ///
    /// Inserts a chain into the tree and builds the block indices.
    pub(crate) fn insert_chain(&mut self, chain: AppendableChain) -> Option<SidechainId> {
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
        self.buffered_blocks.block(hash)
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
pub(crate) struct SidechainId(u64);

impl From<SidechainId> for u64 {
    fn from(value: SidechainId) -> Self {
        value.0
    }
}

#[cfg(test)]
impl From<u64> for SidechainId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical_chain::CanonicalChain;
    use alloy_primitives::B256;
    use reth_execution_types::Chain;
    use reth_provider::ExecutionOutcome;

    #[test]
    fn test_tree_state_initialization() {
        // Set up some dummy data for initialization
        let last_finalized_block_number = 10u64;
        let last_canonical_hashes = vec![(9u64, B256::random()), (10u64, B256::random())];
        let buffer_limit = 5;

        // Initialize the tree state
        let tree_state = TreeState::new(
            last_finalized_block_number,
            last_canonical_hashes.clone(),
            buffer_limit,
        );

        // Verify the tree state after initialization
        assert_eq!(tree_state.block_chain_id_generator, 0);
        assert_eq!(tree_state.block_indices().last_finalized_block(), last_finalized_block_number);
        assert_eq!(
            *tree_state.block_indices.canonical_chain().inner(),
            *CanonicalChain::new(last_canonical_hashes.into_iter().collect()).inner()
        );
        assert!(tree_state.chains.is_empty());
        assert!(tree_state.buffered_blocks.lru.is_empty());
    }

    #[test]
    fn test_tree_state_next_id() {
        // Initialize the tree state
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Generate a few sidechain IDs
        let first_id = tree_state.next_id();
        let second_id = tree_state.next_id();

        // Verify the generated sidechain IDs and the updated generator state
        assert_eq!(first_id, SidechainId(0));
        assert_eq!(second_id, SidechainId(1));
        assert_eq!(tree_state.block_chain_id_generator, 2);
    }

    #[test]
    fn test_tree_state_insert_chain() {
        // Initialize tree state
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create a chain with two blocks
        let block: SealedBlockWithSenders = Default::default();
        let block1_hash = B256::random();
        let block2_hash = B256::random();

        let mut block1 = block.clone();
        let mut block2 = block;

        block1.block.set_hash(block1_hash);
        block1.block.set_block_number(9);
        block2.block.set_hash(block2_hash);
        block2.block.set_block_number(10);

        let chain = AppendableChain::new(Chain::new(
            [block1, block2],
            Default::default(),
            Default::default(),
        ));

        // Insert the chain into the TreeState
        let chain_id = tree_state.insert_chain(chain).unwrap();

        // Verify the chain ID and that it was added to the chains collection
        assert_eq!(chain_id, SidechainId(0));
        assert!(tree_state.chains.contains_key(&chain_id));

        // Ensure that the block indices are updated
        assert_eq!(
            tree_state.block_indices.get_side_chain_id(&block1_hash).unwrap(),
            SidechainId(0)
        );
        assert_eq!(
            tree_state.block_indices.get_side_chain_id(&block2_hash).unwrap(),
            SidechainId(0)
        );

        // Ensure that the block chain ID generator was updated
        assert_eq!(tree_state.block_chain_id_generator, 1);

        // Create an empty chain
        let chain_empty = AppendableChain::new(Chain::default());

        // Insert the empty chain into the tree state
        let chain_id = tree_state.insert_chain(chain_empty);

        // Ensure that the empty chain was not inserted
        assert!(chain_id.is_none());

        // Nothing should have changed and no new chain should have been added
        assert!(tree_state.chains.contains_key(&SidechainId(0)));
        assert!(!tree_state.chains.contains_key(&SidechainId(1)));
        assert_eq!(
            tree_state.block_indices.get_side_chain_id(&block1_hash).unwrap(),
            SidechainId(0)
        );
        assert_eq!(
            tree_state.block_indices.get_side_chain_id(&block2_hash).unwrap(),
            SidechainId(0)
        );
        assert_eq!(tree_state.block_chain_id_generator, 1);
    }

    #[test]
    fn test_block_by_hash_side_chain() {
        // Initialize a tree state with some dummy data
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create two side-chain blocks with random hashes
        let block1_hash = B256::random();
        let block2_hash = B256::random();

        let mut block1: SealedBlockWithSenders = Default::default();
        let mut block2: SealedBlockWithSenders = Default::default();

        block1.block.set_hash(block1_hash);
        block1.block.set_block_number(9);
        block2.block.set_hash(block2_hash);
        block2.block.set_block_number(10);

        // Create an chain with these blocks
        let chain = AppendableChain::new(Chain::new(
            vec![block1.clone(), block2.clone()],
            Default::default(),
            Default::default(),
        ));

        // Insert the side chain into the TreeState
        tree_state.insert_chain(chain).unwrap();

        // Retrieve the blocks by their hashes
        let retrieved_block1 = tree_state.block_by_hash(block1_hash);
        assert_eq!(*retrieved_block1.unwrap(), block1.block);

        let retrieved_block2 = tree_state.block_by_hash(block2_hash);
        assert_eq!(*retrieved_block2.unwrap(), block2.block);

        // Test block_by_hash with a random hash that doesn't exist
        let non_existent_hash = B256::random();
        let result = tree_state.block_by_hash(non_existent_hash);

        // Ensure that no block is found
        assert!(result.is_none());
    }

    #[test]
    fn test_block_with_senders_by_hash() {
        // Initialize a tree state with some dummy data
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create two side-chain blocks with random hashes
        let block1_hash = B256::random();
        let block2_hash = B256::random();

        let mut block1: SealedBlockWithSenders = Default::default();
        let mut block2: SealedBlockWithSenders = Default::default();

        block1.block.set_hash(block1_hash);
        block1.block.set_block_number(9);
        block2.block.set_hash(block2_hash);
        block2.block.set_block_number(10);

        // Create a chain with these blocks
        let chain = AppendableChain::new(Chain::new(
            vec![block1.clone(), block2.clone()],
            Default::default(),
            Default::default(),
        ));

        // Insert the side chain into the TreeState
        tree_state.insert_chain(chain).unwrap();

        // Test to retrieve the blocks with senders by their hashes
        let retrieved_block1 = tree_state.block_with_senders_by_hash(block1_hash);
        assert_eq!(*retrieved_block1.unwrap(), block1);

        let retrieved_block2 = tree_state.block_with_senders_by_hash(block2_hash);
        assert_eq!(*retrieved_block2.unwrap(), block2);

        // Test block_with_senders_by_hash with a random hash that doesn't exist
        let non_existent_hash = B256::random();
        let result = tree_state.block_with_senders_by_hash(non_existent_hash);

        // Ensure that no block is found
        assert!(result.is_none());
    }

    #[test]
    fn test_get_buffered_block() {
        // Initialize a tree state with some dummy data
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create a block with a random hash and add it to the buffer
        let block_hash = B256::random();
        let mut block: SealedBlockWithSenders = Default::default();
        block.block.set_hash(block_hash);

        // Add the block to the buffered blocks in the TreeState
        tree_state.buffered_blocks.insert_block(block.clone());

        // Test get_buffered_block to retrieve the block by its hash
        let retrieved_block = tree_state.get_buffered_block(&block_hash);
        assert_eq!(*retrieved_block.unwrap(), block);

        // Test get_buffered_block with a non-existent hash
        let non_existent_hash = B256::random();
        let result = tree_state.get_buffered_block(&non_existent_hash);

        // Ensure that no block is found
        assert!(result.is_none());
    }

    #[test]
    fn test_lowest_buffered_ancestor() {
        // Initialize a tree state with some dummy data
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create blocks with random hashes and set up parent-child relationships
        let ancestor_hash = B256::random();
        let descendant_hash = B256::random();

        let mut ancestor_block: SealedBlockWithSenders = Default::default();
        let mut descendant_block: SealedBlockWithSenders = Default::default();

        ancestor_block.block.set_hash(ancestor_hash);
        descendant_block.block.set_hash(descendant_hash);
        descendant_block.block.set_parent_hash(ancestor_hash);

        // Insert the blocks into the buffer
        tree_state.buffered_blocks.insert_block(ancestor_block.clone());
        tree_state.buffered_blocks.insert_block(descendant_block.clone());

        // Test lowest_buffered_ancestor for the descendant block
        let lowest_ancestor = tree_state.lowest_buffered_ancestor(&descendant_hash);
        assert!(lowest_ancestor.is_some());
        assert_eq!(lowest_ancestor.unwrap().block.hash(), ancestor_hash);

        // Test lowest_buffered_ancestor with a non-existent hash
        let non_existent_hash = B256::random();
        let result = tree_state.lowest_buffered_ancestor(&non_existent_hash);

        // Ensure that no ancestor is found
        assert!(result.is_none());
    }

    #[test]
    fn test_receipts_by_block_hash() {
        // Initialize a tree state with some dummy data
        let mut tree_state = TreeState::new(0, vec![], 5);

        // Create a block with a random hash and receipts
        let block_hash = B256::random();
        let receipt1 = Receipt::default();
        let receipt2 = Receipt::default();

        let mut block: SealedBlockWithSenders = Default::default();
        block.block.set_hash(block_hash);

        let receipts = vec![receipt1, receipt2];

        // Create a chain with the block and its receipts
        let chain = AppendableChain::new(Chain::new(
            vec![block.clone()],
            ExecutionOutcome { receipts: receipts.clone().into(), ..Default::default() },
            Default::default(),
        ));

        // Insert the chain into the TreeState
        tree_state.insert_chain(chain).unwrap();

        // Test receipts_by_block_hash for the inserted block
        let retrieved_receipts = tree_state.receipts_by_block_hash(block_hash);
        assert!(retrieved_receipts.is_some());

        // Check if the correct receipts are returned
        let receipts_ref: Vec<&Receipt> = receipts.iter().collect();
        assert_eq!(retrieved_receipts.unwrap(), receipts_ref);

        // Test receipts_by_block_hash with a non-existent block hash
        let non_existent_hash = B256::random();
        let result = tree_state.receipts_by_block_hash(non_existent_hash);

        // Ensure that no receipts are found
        assert!(result.is_none());
    }
}
