//! [`BlockExecutionDataProvider`] implementations used by the tree.

use reth_primitives::{BlockHash, BlockNumber, ForkBlock};
use reth_provider::{
    BlockExecutionDataProvider, BlockExecutionForkProvider, BlockExecutionOutcome,
};
use std::collections::BTreeMap;

/// Structure that combines references of required data to be a [`BlockExecutionDataProvider`].
#[derive(Clone, Debug)]
pub struct BundleStateDataRef<'a> {
    /// The block execution outcome after execution of one or more transactions and/or blocks.
    pub block_execution_outcome: &'a BlockExecutionOutcome,
    /// The blocks in the sidechain.
    pub sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    pub canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Canonical fork
    pub canonical_fork: ForkBlock,
}

impl<'a> BlockExecutionDataProvider for BundleStateDataRef<'a> {
    fn block_execution_outcome(&self) -> &BlockExecutionOutcome {
        self.block_execution_outcome
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        let block_hash = self.sidechain_block_hashes.get(&block_number).cloned();
        if block_hash.is_some() {
            return block_hash
        }

        self.canonical_block_hashes.get(&block_number).cloned()
    }
}

impl<'a> BlockExecutionForkProvider for BundleStateDataRef<'a> {
    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}

/// Structure that owns the relevant data needs to be a [`BlockExecutionDataProvider`]
#[derive(Clone, Debug)]
pub struct BlockExecutionData {
    /// Block execution outcome.
    pub block_execution_outcome: BlockExecutionOutcome,
    /// Parent block hashes needs for evm BLOCKHASH opcode.
    /// NOTE: it does not mean that all hashes are there but all until finalized are there.
    /// Other hashes can be obtained from provider
    pub parent_block_hashes: BTreeMap<BlockNumber, BlockHash>,
    /// Canonical block where state forked from.
    pub canonical_fork: ForkBlock,
}

impl BlockExecutionDataProvider for BlockExecutionData {
    fn block_execution_outcome(&self) -> &BlockExecutionOutcome {
        &self.block_execution_outcome
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        self.parent_block_hashes.get(&block_number).cloned()
    }
}

impl BlockExecutionForkProvider for BlockExecutionData {
    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}
