//! Contains [Chain], a chain of blocks and their final state.

use crate::PostState;
use reth_primitives::{BlockNumber, SealedBlockWithSenders, TransitionId, BlockHash, ForkBlock};
use std::collections::BTreeMap;

/// A chain of blocks and their final state.
///
/// The chain contains the state of accounts after execution of its blocks,
/// changesets for those blocks (and their transactions), as well as the blocks themselves.
///
/// Used inside the BlockchainTree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// The state of accounts after execution of the blocks in this chain.
    ///
    /// This state also contains the individual changes that lead to the current state.
    state: PostState,
    /// The blocks in this chain.
    blocks: BTreeMap<BlockNumber, SealedBlockWithSenders>,
    /// A mapping of each block number in the chain to the highest transition ID in the chain's
    /// state after execution of the block.
    ///
    /// This is used to revert changes in the state until a certain block number when the chain is
    /// split.
    block_transitions: BTreeMap<BlockNumber, TransitionId>,
}

impl Chain {
    /// Get the blocks in this chain.
    pub fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders> {
        &self.blocks
    }

    /// Get post state of this chain
    pub fn state(&self) -> &PostState {
        &self.state
    }

    /// Return block number of the block hash.
    pub fn block_number(&self, block_hash: BlockHash) -> Option<BlockNumber> {
        self.blocks.iter().find_map(|(num, block)| (block.hash() == block_hash).then_some(*num))
    }

    /// Return post state of the block at the `block_number` or None if block is not known
    pub fn state_at_block(&self, block_number: BlockNumber) -> Option<PostState> {
        let mut state = self.state.clone();
        if self.tip().number == block_number {
            return Some(state);
        }

        if let Some(&transition_id) = self.block_transitions.get(&block_number) {
            state.revert_to(transition_id);
            return Some(state);
        }

        None
    }

    /// Destructure the chain into its inner components, the blocks and the state.
    pub fn into_inner(self) -> (BTreeMap<BlockNumber, SealedBlockWithSenders>, PostState) {
        (self.blocks, self.state)
    }

    /// Get the block at which this chain forked.
    pub fn fork_block(&self) -> ForkBlock {
        let tip = self.first();
        ForkBlock { number: tip.number.saturating_sub(1), hash: tip.parent_hash }
    }

    /// Get the block number at which this chain forked.
    pub fn fork_block_number(&self) -> BlockNumber {
        self.first().number.saturating_sub(1)
    }

    /// Get the block hash at which this chain forked.
    pub fn fork_block_hash(&self) -> BlockHash {
        self.first().parent_hash
    }

    /// Get the first block in this chain.
    pub fn first(&self) -> &SealedBlockWithSenders {
        self.blocks.first_key_value().expect("Chain has at least one block for first").1
    }

    /// Get the tip of the chain.
    ///
    /// # Note
    ///
    /// Chains always have at least one block.
    pub fn tip(&self) -> &SealedBlockWithSenders {
        self.blocks.last_key_value().expect("Chain should have at least one block").1
    }

    /// Create new chain with given blocks and post state.
    pub fn new(blocks: Vec<(SealedBlockWithSenders, PostState)>) -> Self {
        let mut state = PostState::default();
        let mut block_transitions = BTreeMap::new();
        let mut block_num_hash = BTreeMap::new();
        for (block, block_state) in blocks.into_iter() {
            state.extend(block_state);
            block_transitions.insert(block.number, state.transitions_count());
            block_num_hash.insert(block.number, block);
        }

        Self { state, block_transitions, blocks: block_num_hash }
    }
}
