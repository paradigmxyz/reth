//! Wrapper around BlockchainTree that allows for it to be shared.
use parking_lot::RwLock;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::Consensus,
    provider::ProviderError,
    Error,
};
use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlock, SealedBlockWithSenders};
use reth_provider::{
    BlockchainTreePendingStateProvider, CanonStateSubscriptions, ExecutorFactory,
    PostStateDataProvider,
};
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
    /// Recover senders and call [`BlockchainTreeEngine::insert_block_with_senders`].
    fn insert_block(&self, block: SealedBlock) -> Result<BlockStatus, Error> {
        let mut tree = self.tree.write();
        tree.ensure_block_is_in_range(&block)?;
        let block = block
            .seal_with_senders()
            .ok_or(reth_interfaces::executor::Error::SenderRecoveryError)?;
        tree.insert_in_range_block_with_senders(block)
    }

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
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        self.tree.read().block_indices().index_of_number_to_pending_blocks().clone()
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        self.tree.read().block_by_hash(block_hash).cloned()
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.read().block_indices().canonical_chain().clone()
    }

    fn canonical_tip(&self) -> BlockNumHash {
        self.tree.read().block_indices().canonical_tip()
    }

    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        self.tree.read().block_indices().pending_blocks()
    }

    fn pending_block(&self) -> Option<BlockNumHash> {
        let (number, blocks) = self.tree.read().block_indices().pending_blocks();
        blocks.first().map(|&hash| BlockNumHash { number, hash })
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

impl<DB: Database, C: Consensus, EF: ExecutorFactory> CanonStateSubscriptions
    for ShareableBlockchainTree<DB, C, EF>
{
    fn subscribe_canon_state(&self) -> reth_provider::CanonStateNotifications {
        self.tree.read().subscribe_canon_state()
    }
}
