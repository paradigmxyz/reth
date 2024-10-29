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
        self.sidechain_block_hashes
            .get(&block_number)
            .copied()
            .or_else(|| self.canonical_block_hashes.get(&block_number).copied())
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_bundle_state_data_ref_block_hash() {
        // Generate a default execution outcome
        let execution_outcome = ExecutionOutcome::default();

        // Generate sidechain and canonical block hashes
        let mut sidechain_block_hashes = BTreeMap::new();
        sidechain_block_hashes.insert(1, B256::from_slice(&[1; 32]));
        let mut canonical_block_hashes = BTreeMap::new();
        canonical_block_hashes.insert(2, B256::from_slice(&[2; 32]));

        // Generate a default canonical fork
        let canonical_fork = ForkBlock::default();

        // Group the data into a bundle state data reference
        let bundle_state_data_ref = BundleStateDataRef {
            execution_outcome: &execution_outcome,
            sidechain_block_hashes: &sidechain_block_hashes,
            canonical_block_hashes: &canonical_block_hashes,
            canonical_fork,
        };

        // Test the bundle state data reference
        assert_eq!(bundle_state_data_ref.execution_outcome(), &execution_outcome);
        assert_eq!(bundle_state_data_ref.canonical_fork(), canonical_fork);

        // Test the block hashes
        assert_eq!(bundle_state_data_ref.block_hash(1), Some(B256::from_slice(&[1; 32])));
        assert_eq!(bundle_state_data_ref.block_hash(2), Some(B256::from_slice(&[2; 32])));
        assert!(bundle_state_data_ref.block_hash(3).is_none());
    }
}
