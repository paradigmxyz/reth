//! [`ExecutionDataProvider`] implementations used by the tree.

use alloy_eips::ForkBlock;
use alloy_primitives::{BlockHash, BlockNumber};
use reth_provider::{BlockExecutionForkProvider, ExecutionDataProvider, ExecutionOutcome};
use std::collections::BTreeMap;

/// Structure that combines references of required data to be a [`ExecutionDataProvider`].
#[derive(Clone, Debug)]
pub struct BundleStateDataRef<'a> {
    /// The execution outcome after execution of one or more transactions and/or blocks.
    pub execution_outcome: &'a ExecutionOutcome,
    /// The blocks in the sidechain.
    pub sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    pub canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Canonical fork
    pub canonical_fork: ForkBlock,
}

impl ExecutionDataProvider for BundleStateDataRef<'_> {
    fn execution_outcome(&self) -> &ExecutionOutcome {
        self.execution_outcome
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        let block_hash = self.sidechain_block_hashes.get(&block_number).copied();
        if block_hash.is_some() {
            return block_hash;
        }

        self.canonical_block_hashes.get(&block_number).copied()
    }
}

impl BlockExecutionForkProvider for BundleStateDataRef<'_> {
    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}

/// Structure that owns the relevant data needs to be a [`ExecutionDataProvider`]
#[derive(Clone, Debug)]
pub struct ExecutionData {
    /// Execution outcome.
    pub execution_outcome: ExecutionOutcome,
    /// Parent block hashes needs for evm BLOCKHASH opcode.
    /// NOTE: it does not mean that all hashes are there but all until finalized are there.
    /// Other hashes can be obtained from provider
    pub parent_block_hashes: BTreeMap<BlockNumber, BlockHash>,
    /// Canonical block where state forked from.
    pub canonical_fork: ForkBlock,
}

impl ExecutionDataProvider for ExecutionData {
    fn execution_outcome(&self) -> &ExecutionOutcome {
        &self.execution_outcome
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        self.parent_block_hashes.get(&block_number).copied()
    }
}

impl BlockExecutionForkProvider for ExecutionData {
    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}
