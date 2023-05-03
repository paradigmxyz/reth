//! Wrapper around BlockchainTree that allows for it to be shared.
use super::BlockchainTree;
use parking_lot::RwLock;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::Consensus,
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
use tracing::trace;

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
    fn insert_block_without_senders(&self, block: SealedBlock) -> Result<BlockStatus, Error> {
        let mut tree = self.tree.write();
        // check if block is known before recovering all senders.
        if let Some(status) = tree.is_block_known(block.num_hash())? {
            return Ok(status)
        }
        let block = block
            .seal_with_senders()
            .ok_or(reth_interfaces::executor::Error::SenderRecoveryError)?;
        tree.insert_block_inner(block, true)
    }

    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), Error> {
        self.tree.write().buffer_block(block)
    }

    fn insert_block(&self, block: SealedBlockWithSenders) -> Result<BlockStatus, Error> {
        trace!(target: "blockchain_tree", ?block, "Inserting block");
        self.tree.write().insert_block(block)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        trace!(target: "blockchain_tree", ?finalized_block, "Finalizing block");
        self.tree.write().finalize_block(finalized_block)
    }

    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<(), Error> {
        trace!(target: "blockchain_tree", ?last_finalized_block, "Restoring canonical hashes for last finalized block");
        self.tree.write().restore_canonical_hashes(last_finalized_block)
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> Result<(), Error> {
        trace!(target: "blockchain_tree", ?block_hash, "Making block canonical");
        self.tree.write().make_canonical(block_hash)
    }

    fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error> {
        trace!(target: "blockchain_tree", ?unwind_to, "Unwinding to block number");
        self.tree.write().unwind(unwind_to)
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeViewer
    for ShareableBlockchainTree<DB, C, EF>
{
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        trace!(target: "blockchain_tree", "Returning all blocks in blockchain tree");
        self.tree.read().block_indices().index_of_number_to_pending_blocks().clone()
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", ?block_hash, "Returning block by hash");
        self.tree.read().block_by_hash(block_hash).cloned()
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        trace!(target: "blockchain_tree", "Returning canonical blocks in tree");
        self.tree.read().block_indices().canonical_chain().clone()
    }

    fn find_canonical_ancestor(&self, hash: BlockHash) -> Option<BlockHash> {
        let mut parent = hash;
        let tree = self.tree.read();

        // walk up the tree and check if the parent is in the sidechain
        while let Some(block) = tree.block_by_hash(parent) {
            parent = block.parent_hash;
        }

        if tree.block_indices().is_block_hash_canonical(&parent) {
            return Some(parent)
        }

        None
    }

    fn canonical_tip(&self) -> BlockNumHash {
        trace!(target: "blockchain_tree", "Returning canonical tip");
        self.tree.read().block_indices().canonical_tip()
    }

    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        trace!(target: "blockchain_tree", "Returning all pending blocks");
        self.tree.read().block_indices().pending_blocks()
    }

    fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().block_indices().pending_block_num_hash()
    }

    fn pending_block(&self) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().pending_block().cloned()
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreePendingStateProvider
    for ShareableBlockchainTree<DB, C, EF>
{
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn PostStateDataProvider>> {
        trace!(target: "blockchain_tree", ?block_hash, "Finding pending state provider");
        let provider = self.tree.read().post_state_data(block_hash)?;
        Some(Box::new(provider))
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> CanonStateSubscriptions
    for ShareableBlockchainTree<DB, C, EF>
{
    fn subscribe_to_canonical_state(&self) -> reth_provider::CanonStateNotifications {
        trace!(target: "blockchain_tree", "Registered subscriber for canonical state");
        self.tree.read().subscribe_canon_state()
    }
}
