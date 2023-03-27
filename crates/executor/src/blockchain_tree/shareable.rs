//! Wrapper around BlockchainTree that allows for it to be shared.
use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::Consensus,
    Error,
};
use reth_primitives::{BlockHash, BlockNumber, SealedBlockWithSenders};
use reth_provider::{BlockchainTreePendingStateProvider, ExecutorFactory, PostStateDataProvider};
use tokio::sync::RwLock;

use super::BlockchainTree;

/// Shareable blockchain tree that is behind tokio::RwLock
pub struct ShareableBlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// BlockchainTree
    tree: RwLock<BlockchainTree<DB, C, EF>>,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> ShareableBlockchainTree<DB, C, EF> {
    /// Create New sharable database.
    pub fn new() {}
}

#[async_trait]
impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeEngine
    for ShareableBlockchainTree<DB, C, EF>
{
    async fn insert_block_with_senders(
        &self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, Error> {
        self.tree.write().await.insert_block_with_senders(block)
    }

    async fn finalize_block(&self, finalized_block: BlockNumber) {
        self.tree.write().await.finalize_block(finalized_block)
    }

    async fn restore_canonical_hashes(
        &self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error> {
        self.tree.write().await.restore_canonical_hashes(last_finalized_block)
    }

    async fn make_canonical(&self, block_hash: &BlockHash) -> Result<(), Error> {
        self.tree.write().await.make_canonical(block_hash)
    }

    async fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error> {
        self.tree.write().await.unwind(unwind_to)
    }
}

#[async_trait]
impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeViewer
    for ShareableBlockchainTree<DB, C, EF>
{
    async fn pending_blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        self.tree.read().await.block_indices().index_number_to_pending_blocks().clone()
    }

    async fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.read().await.block_indices().canonical_chain().clone()
    }
}

#[async_trait]
impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreePendingStateProvider
    for ShareableBlockchainTree<DB, C, EF>
{
    async fn pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Result<Box<dyn PostStateDataProvider>, Error> {
        let Some(post_state) = self.tree.read().await.post_state_data(block_hash) else { panic!("")};
        Ok(Box::new(post_state))
    }
}
