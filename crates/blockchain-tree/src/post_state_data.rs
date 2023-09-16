//! Substate for blockchain trees

use reth_primitives::{BlockHash, BlockNumber, ForkBlock};
use reth_provider::{BundleStateDataProvider, BundleStateWithReceipts};
use std::collections::BTreeMap;

/// Structure that bundles references of data needs to implement [`BundleStateDataProvider`]
#[derive(Clone, Debug)]
pub struct BundleStateDataRef<'a> {
    /// The wrapped state after execution of one or more transactions and/or blocks.
    pub state: &'a BundleStateWithReceipts,
    /// The blocks in the sidechain.
    pub sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    pub canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Canonical fork
    pub canonical_fork: ForkBlock,
}

impl<'a> BundleStateDataProvider for BundleStateDataRef<'a> {
    fn state(&self) -> &BundleStateWithReceipts {
        self.state
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        let block_hash = self.sidechain_block_hashes.get(&block_number).cloned();
        if block_hash.is_some() {
            return block_hash
        }

        self.canonical_block_hashes.get(&block_number).cloned()
    }

    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}

/// Structure that contains data needs to implement [`BundleStateDataProvider`]
#[derive(Clone, Debug)]
pub struct BundleStateData {
    /// Post state with changes
    pub state: BundleStateWithReceipts,
    /// Parent block hashes needs for evm BLOCKHASH opcode.
    /// NOTE: it does not mean that all hashes are there but all until finalized are there.
    /// Other hashes can be obtained from provider
    pub parent_block_hashed: BTreeMap<BlockNumber, BlockHash>,
    /// Canonical block where state forked from.
    pub canonical_fork: ForkBlock,
}

impl BundleStateDataProvider for BundleStateData {
    fn state(&self) -> &BundleStateWithReceipts {
        &self.state
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        self.parent_block_hashed.get(&block_number).cloned()
    }

    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}
