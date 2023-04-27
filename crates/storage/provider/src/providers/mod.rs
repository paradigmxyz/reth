use crate::{
    BlockHashProvider, BlockIdProvider, BlockProvider, BlockchainTreePendingStateProvider,
    CanonStateNotifications, CanonStateSubscriptions, EvmEnvProvider, HeaderProvider,
    PostStateDataProvider, ReceiptProvider, StateProviderBox, StateProviderFactory,
    TransactionsProvider, WithdrawalsProvider,
};
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    Result,
};
use reth_primitives::{
    Block, BlockHash, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag, ChainInfo, Header,
    Receipt, SealedBlock, SealedBlockWithSenders, TransactionMeta, TransactionSigned, TxHash,
    TxNumber, Withdrawal, H256, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};
use std::{
    collections::{BTreeMap, HashSet},
    ops::RangeBounds,
};
use tracing::trace;

mod database;
mod post_state_provider;
mod state;
use crate::traits::BlockSource;
pub use database::*;
pub use post_state_provider::PostStateProvider;

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Clone)]
pub struct BlockchainProvider<DB, Tree> {
    /// Provider type used to access the database.
    database: ShareableDatabase<DB>,
    /// The blockchain tree instance.
    tree: Tree,
}

impl<DB, Tree> BlockchainProvider<DB, Tree> {
    /// Create new  provider instance that wraps the database and the blockchain tree.
    pub fn new(database: ShareableDatabase<DB>, tree: Tree) -> Self {
        Self { database, tree }
    }
}

impl<DB, Tree> HeaderProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        self.database.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        self.database.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        self.database.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        self.database.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        self.database.headers_range(range)
    }
}

impl<DB, Tree> BlockHashProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.database.block_hash(number)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        self.database.canonical_hashes_range(start, end)
    }
}

impl<DB, Tree> BlockIdProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn chain_info(&self) -> Result<ChainInfo> {
        self.database.chain_info()
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        self.database.best_block_number()
    }

    fn convert_block_number(&self, num: BlockNumberOrTag) -> Result<Option<BlockNumber>> {
        let num = match num {
            BlockNumberOrTag::Latest => self.chain_info()?.best_number,
            BlockNumberOrTag::Number(num) => num,
            BlockNumberOrTag::Pending => return Ok(self.tree.pending_block().map(|b| b.number)),
            BlockNumberOrTag::Finalized => return Ok(self.chain_info()?.last_finalized),
            BlockNumberOrTag::Safe => return Ok(self.chain_info()?.safe_finalized),
            BlockNumberOrTag::Earliest => 0,
        };
        Ok(Some(num))
    }

    fn block_hash_for_id(&self, block_id: BlockId) -> Result<Option<H256>> {
        match block_id {
            BlockId::Hash(hash) => Ok(Some(hash.into())),
            BlockId::Number(num) => match num {
                BlockNumberOrTag::Latest => Ok(Some(self.chain_info()?.best_hash)),
                BlockNumberOrTag::Pending => Ok(self.tree.pending_block().map(|b| b.hash)),
                _ => self
                    .convert_block_number(num)?
                    .map(|num| self.block_hash(num))
                    .transpose()
                    .map(|maybe_hash| maybe_hash.flatten()),
            },
        }
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.database.block_number(hash)
    }
}

impl<DB, Tree> BlockProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn find_block_by_hash(&self, hash: H256, source: BlockSource) -> Result<Option<Block>> {
        let block = match source {
            BlockSource::Any => {
                // check pending source first
                // Note: it's fine to return the unsealed block because the caller already has the
                // hash
                let mut block = self.tree.block_by_hash(hash).map(|block| block.unseal());
                if block.is_none() {
                    block = self.database.block_by_hash(hash)?;
                }
                block
            }
            BlockSource::Pending => self.tree.block_by_hash(hash).map(|block| block.unseal()),
            BlockSource::Database => self.database.block_by_hash(hash)?,
        };

        Ok(block)
    }

    fn block(&self, id: BlockId) -> Result<Option<Block>> {
        self.database.block(id)
    }

    fn ommers(&self, id: BlockId) -> Result<Option<Vec<Header>>> {
        self.database.ommers(id)
    }
}

impl<DB, Tree> TransactionsProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn transaction_id(&self, tx_hash: TxHash) -> Result<Option<TxNumber>> {
        self.database.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        self.database.transaction_by_id(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        self.database.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        self.database.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> Result<Option<BlockNumber>> {
        self.database.transaction_block(id)
    }

    fn transactions_by_block(&self, id: BlockId) -> Result<Option<Vec<TransactionSigned>>> {
        self.database.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        self.database.transactions_by_block_range(range)
    }
}

impl<DB, Tree> ReceiptProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        self.database.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        self.database.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockId) -> Result<Option<Vec<Receipt>>> {
        self.database.receipts_by_block(block)
    }
}

impl<DB, Tree> WithdrawalsProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn withdrawals_by_block(&self, id: BlockId, timestamp: u64) -> Result<Option<Vec<Withdrawal>>> {
        self.database.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
        self.database.latest_withdrawal()
    }
}

impl<DB, Tree> EvmEnvProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn fill_env_at(&self, cfg: &mut CfgEnv, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        self.database.fill_env_at(cfg, block_env, at)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        self.database.fill_env_with_header(cfg, block_env, header)
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        self.database.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        self.database.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockId) -> Result<()> {
        self.database.fill_cfg_env_at(cfg, at)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        self.database.fill_cfg_env_with_header(cfg, header)
    }
}

impl<DB, Tree> StateProviderFactory for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreePendingStateProvider + BlockchainTreeViewer,
{
    /// Storage provider for latest block
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::blockchain", "Getting latest block state provider");
        self.database.latest()
    }

    fn history_by_block_number(&self, block_number: BlockNumber) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::blockchain", ?block_number, "Getting history by block number");
        self.database.history_by_block_number(block_number)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");
        self.database.history_by_block_hash(block_hash)
    }

    fn state_by_block_hash(&self, block: BlockHash) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::blockchain", ?block, "Getting state by block hash");

        // check tree first
        if let Some(pending) = self.tree.find_pending_state_provider(block) {
            trace!(target: "providers::blockchain", "Returning pending state provider");
            return self.pending_with_provider(pending)
        }
        // not found in tree, check database
        self.history_by_block_hash(block)
    }

    /// Storage provider for pending state.
    fn pending(&self) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::blockchain", "Getting provider for pending state");

        if let Some(block) = self.tree.pending_block() {
            let pending = self.tree.pending_state_provider(block.hash)?;
            return self.pending_with_provider(pending)
        }
        self.latest()
    }

    fn pending_with_provider(
        &self,
        post_state_data: Box<dyn PostStateDataProvider>,
    ) -> Result<StateProviderBox<'_>> {
        let canonical_fork = post_state_data.canonical_fork();
        trace!(target: "providers::blockchain", ?canonical_fork, "Returning post state provider");

        let state_provider = self.history_by_block_hash(canonical_fork.hash)?;
        let post_state_provider = PostStateProvider::new(state_provider, post_state_data);
        Ok(Box::new(post_state_provider))
    }
}

impl<DB, Tree> BlockchainTreeEngine for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: BlockchainTreeEngine,
{
    fn insert_block_with_senders(&self, block: SealedBlockWithSenders) -> Result<BlockStatus> {
        self.tree.insert_block_with_senders(block)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        self.tree.finalize_block(finalized_block)
    }

    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<()> {
        self.tree.restore_canonical_hashes(last_finalized_block)
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> Result<()> {
        self.tree.make_canonical(block_hash)
    }

    fn unwind(&self, unwind_to: BlockNumber) -> Result<()> {
        self.tree.unwind(unwind_to)
    }
}

impl<DB, Tree> BlockchainTreeViewer for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: BlockchainTreeViewer,
{
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        self.tree.blocks()
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        self.tree.block_by_hash(block_hash)
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.canonical_blocks()
    }

    fn canonical_tip(&self) -> BlockNumHash {
        self.tree.canonical_tip()
    }

    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        self.tree.pending_blocks()
    }

    fn pending_block(&self) -> Option<BlockNumHash> {
        self.tree.pending_block()
    }
}

impl<DB, Tree> BlockchainTreePendingStateProvider for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: BlockchainTreePendingStateProvider,
{
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn PostStateDataProvider>> {
        self.tree.find_pending_state_provider(block_hash)
    }
}

impl<DB, Tree> CanonStateSubscriptions for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: CanonStateSubscriptions,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        self.tree.subscribe_to_canonical_state()
    }
}
