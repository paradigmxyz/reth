//! Wrapper around BlockchainTree that allows for it to be shared.
use parking_lot::RwLock;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::Consensus,
    events::{ChainEventSubscriptions, NewBlockNotifications},
    provider::ProviderError,
    Error,
};
use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlockWithSenders};
use reth_provider::{BlockchainTreePendingStateProvider, ExecutorFactory, PostStateDataProvider};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use super::BlockchainTree;

/// Shareable blockchain tree that is behind tokio::RwLock
#[derive(Clone)]
pub struct ShareableBlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// BlockchainTree
    pub tree: Arc<RwLock<BlockchainTree<DB, C, EF>>>,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> ShareableBlockchainTree<DB, C, EF> {
    /// Create New sharable database.
    pub fn new(tree: BlockchainTree<DB, C, EF>) -> Self {
        Self { tree: Arc::new(RwLock::new(tree)) }
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeEngine
    for ShareableBlockchainTree<DB, C, EF>
{
    fn insert_block_with_senders(
        &self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, Error> {
        self.tree.write().insert_block_with_senders(block)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        self.tree.write().finalize_block(finalized_block)
    }

    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<(), Error> {
        self.tree.write().restore_canonical_hashes(last_finalized_block)
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> Result<(), Error> {
        self.tree.write().make_canonical(block_hash)
    }

    fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error> {
        self.tree.write().unwind(unwind_to)
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeViewer
    for ShareableBlockchainTree<DB, C, EF>
{
    fn pending_blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        self.tree.read().block_indices().index_of_number_to_pending_blocks().clone()
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.read().block_indices().canonical_chain().clone()
    }

    fn canonical_tip(&self) -> BlockNumHash {
        self.tree.read().canonical_tip()
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreePendingStateProvider
    for ShareableBlockchainTree<DB, C, EF>
{
    fn pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Result<Box<dyn PostStateDataProvider>, Error> {
        let post_state = self
            .tree
            .read()
            .post_state_data(block_hash)
            .ok_or(ProviderError::UnknownBlockHash(block_hash))
            .map(Box::new)?;
        Ok(Box::new(post_state))
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> ChainEventSubscriptions
    for ShareableBlockchainTree<DB, C, EF>
{
    fn subscribe_new_blocks(&self) -> NewBlockNotifications {
        self.tree.read().subscribe_new_blocks()
    }
}
