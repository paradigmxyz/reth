//! Substate for blockchain trees

use crate::blockchain_tree::chain::ForkBlock;
use reth_primitives::{BlockHash, BlockNumber};
use reth_provider::{post_state::PostState, PostStateDataProvider};
use std::collections::BTreeMap;

/// Structure that bundles references of data needs to implement [`PostStateDataProvider`]
#[derive(Clone, Debug)]
pub struct PostStateDataRef<'a> {
    /// The wrapped state after execution of one or more transactions and/or blocks.
    pub state: &'a PostState,
    /// The blocks in the sidechain.
    pub sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    pub canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Canonical fork
    pub canonical_fork: ForkBlock,
}

impl<'a> PostStateDataProvider for PostStateDataRef<'a> {
    fn state(&self) -> &PostState {
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

/// Structure that contains data needs to implement [`PostStateDataProvider`]
#[derive(Clone, Debug)]
pub struct PostStateData {
    /// Post state with changes
    pub state: PostState,
    /// Parent block hashes needs for evm BLOCKHASH opcode.
    /// NOTE: it does not mean that all hashes are there but all until finalized are there.
    /// Other hashes can be obtained from provider
    pub parent_block_hashed: BTreeMap<BlockNumber, BlockHash>,
    /// Canonical block where state forked from.
    pub canonical_fork: ForkBlock,
}

impl PostStateDataProvider for PostStateData {
    fn state(&self) -> &PostState {
        &self.state
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        self.parent_block_hashed.get(&block_number).cloned()
    }

    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}
