//! Wrapper around `BlockchainTree` that allows for it to be shared.

use super::BlockchainTree;
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber};
use parking_lot::RwLock;
use reth_blockchain_tree_api::{
    error::{CanonicalError, InsertBlockError},
    BlockValidationKind, BlockchainTreeEngine, BlockchainTreeViewer, CanonicalOutcome,
    InsertPayloadOk,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader};
use reth_provider::{
    providers::ProviderNodeTypes, BlockchainTreePendingStateProvider, CanonStateSubscriptions,
    FullExecutionDataProvider, ProviderError,
};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::BTreeMap, sync::Arc};
use tracing::trace;

/// Shareable blockchain tree that is behind a `RwLock`
#[derive(Clone, Debug)]
pub struct ShareableBlockchainTree<N: NodeTypesWithDB, E> {
    /// `BlockchainTree`
    pub tree: Arc<RwLock<BlockchainTree<N, E>>>,
}

impl<N: NodeTypesWithDB, E> ShareableBlockchainTree<N, E> {
    /// Create a new shareable database.
    pub fn new(tree: BlockchainTree<N, E>) -> Self {
        Self { tree: Arc::new(RwLock::new(tree)) }
    }
}

impl<N, E> BlockchainTreeEngine for ShareableBlockchainTree<N, E>
where
    N: ProviderNodeTypes,
    E: BlockExecutorProvider,
{
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        let mut tree = self.tree.write();
        // Blockchain tree metrics shouldn't be updated here, see
        // `BlockchainTree::update_chains_metrics` documentation.
        tree.buffer_block(block)
    }

    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
        validation_kind: BlockValidationKind,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        trace!(target: "blockchain_tree", hash = %block.hash(), number = block.number, parent_hash = %block.parent_hash, "Inserting block");
        let mut tree = self.tree.write();
        let res = tree.insert_block(block, validation_kind);
        tree.update_chains_metrics();
        res
    }

    fn finalize_block(&self, finalized_block: BlockNumber) -> ProviderResult<()> {
        trace!(target: "blockchain_tree", finalized_block, "Finalizing block");
        let mut tree = self.tree.write();
        tree.finalize_block(finalized_block)?;
        tree.update_chains_metrics();

        Ok(())
    }

    fn connect_buffered_blocks_to_canonical_hashes_and_finalize(
        &self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), CanonicalError> {
        trace!(target: "blockchain_tree", last_finalized_block, "Connecting buffered blocks to canonical hashes and finalizing the tree");
        let mut tree = self.tree.write();
        let res =
            tree.connect_buffered_blocks_to_canonical_hashes_and_finalize(last_finalized_block);
        tree.update_chains_metrics();
        Ok(res?)
    }

    fn update_block_hashes_and_clear_buffered(
        &self,
    ) -> Result<BTreeMap<BlockNumber, BlockHash>, CanonicalError> {
        let mut tree = self.tree.write();
        let res = tree.update_block_hashes_and_clear_buffered();
        tree.update_chains_metrics();
        Ok(res?)
    }

    fn connect_buffered_blocks_to_canonical_hashes(&self) -> Result<(), CanonicalError> {
        trace!(target: "blockchain_tree", "Connecting buffered blocks to canonical hashes");
        let mut tree = self.tree.write();
        let res = tree.connect_buffered_blocks_to_canonical_hashes();
        tree.update_chains_metrics();
        Ok(res?)
    }

    fn make_canonical(&self, block_hash: BlockHash) -> Result<CanonicalOutcome, CanonicalError> {
        trace!(target: "blockchain_tree", %block_hash, "Making block canonical");
        let mut tree = self.tree.write();
        let res = tree.make_canonical(block_hash);
        tree.update_chains_metrics();
        res
    }
}

impl<N, E> BlockchainTreeViewer for ShareableBlockchainTree<N, E>
where
    N: ProviderNodeTypes,
    E: BlockExecutorProvider,
{
    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader> {
        trace!(target: "blockchain_tree", ?hash, "Returning header by hash");
        self.tree.read().sidechain_block_by_hash(hash).map(|b| b.header.clone())
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", ?block_hash, "Returning block by hash");
        self.tree.read().sidechain_block_by_hash(block_hash).cloned()
    }

    fn block_with_senders_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlockWithSenders> {
        trace!(target: "blockchain_tree", ?block_hash, "Returning block by hash");
        self.tree.read().block_with_senders_by_hash(block_hash).cloned()
    }

    fn buffered_header_by_hash(&self, block_hash: BlockHash) -> Option<SealedHeader> {
        self.tree.read().get_buffered_block(&block_hash).map(|b| b.header.clone())
    }

    fn is_canonical(&self, hash: BlockHash) -> Result<bool, ProviderError> {
        trace!(target: "blockchain_tree", ?hash, "Checking if block is canonical");
        self.tree.read().is_block_hash_canonical(&hash)
    }

    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders> {
        trace!(target: "blockchain_tree", ?hash, "Returning lowest buffered ancestor");
        self.tree.read().lowest_buffered_ancestor(&hash).cloned()
    }

    fn canonical_tip(&self) -> BlockNumHash {
        trace!(target: "blockchain_tree", "Returning canonical tip");
        self.tree.read().block_indices().canonical_tip()
    }

    fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().block_indices().pending_block_num_hash()
    }

    fn pending_block(&self) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().pending_block().cloned()
    }

    fn pending_block_and_receipts(&self) -> Option<(SealedBlock, Vec<Receipt>)> {
        let tree = self.tree.read();
        let pending_block = tree.pending_block()?.clone();
        let receipts =
            tree.receipts_by_block_hash(pending_block.hash())?.into_iter().cloned().collect();
        Some((pending_block, receipts))
    }

    fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<Receipt>> {
        let tree = self.tree.read();
        Some(tree.receipts_by_block_hash(block_hash)?.into_iter().cloned().collect())
    }
}

impl<N, E> BlockchainTreePendingStateProvider for ShareableBlockchainTree<N, E>
where
    N: ProviderNodeTypes,
    E: BlockExecutorProvider,
{
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>> {
        trace!(target: "blockchain_tree", ?block_hash, "Finding pending state provider");
        let provider = self.tree.read().post_state_data(block_hash)?;
        Some(Box::new(provider))
    }
}

impl<N, E> CanonStateSubscriptions for ShareableBlockchainTree<N, E>
where
    N: ProviderNodeTypes,
    E: Send + Sync,
{
    fn subscribe_to_canonical_state(&self) -> reth_provider::CanonStateNotifications {
        trace!(target: "blockchain_tree", "Registered subscriber for canonical state");
        self.tree.read().subscribe_canon_state()
    }
}
