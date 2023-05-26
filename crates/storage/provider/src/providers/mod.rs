use crate::{
    BlockHashProvider, BlockIdProvider, BlockNumProvider, BlockProvider, BlockProviderIdExt,
    BlockchainTreePendingStateProvider, CanonChainTracker, CanonStateNotifications,
    CanonStateSubscriptions, EvmEnvProvider, HeaderProvider, PostStateDataProvider, ProviderError,
    ReceiptProvider, StateProviderBox, StateProviderFactory, TransactionsProvider,
    WithdrawalsProvider,
};
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::ForkchoiceState,
    Error, Result,
};
use reth_primitives::{
    Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag,
    ChainInfo, Header, Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, TransactionMeta,
    TransactionSigned, TxHash, TxNumber, Withdrawal, H256, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};
use std::{
    collections::{BTreeMap, HashSet},
    ops::RangeBounds,
    time::Instant,
};
use tracing::trace;

mod chain_info;
mod database;
mod post_state_provider;
mod state;
use crate::{providers::chain_info::ChainInfoTracker, traits::BlockSource};
pub use database::*;
pub use post_state_provider::PostStateProvider;
use reth_interfaces::blockchain_tree::{error::InsertBlockError, CanonicalOutcome};

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
    /// Tracks the chain info wrt forkchoice updates
    chain_info: ChainInfoTracker,
}

impl<DB, Tree> BlockchainProvider<DB, Tree> {
    /// Create new  provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker.
    pub fn with_latest(database: ShareableDatabase<DB>, tree: Tree, latest: SealedHeader) -> Self {
        Self { database, tree, chain_info: ChainInfoTracker::new(latest) }
    }
}

impl<DB, Tree> BlockchainProvider<DB, Tree>
where
    DB: Database,
{
    /// Create a new provider using only the database and the tree, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ShareableDatabase<DB>, tree: Tree) -> Result<Self> {
        let best = database.chain_info()?;
        match database.header_by_number(best.best_number)? {
            Some(header) => Ok(Self::with_latest(database, tree, header.seal(best.best_hash))),
            None => Err(Error::Provider(ProviderError::HeaderNotFound(best.best_number.into()))),
        }
    }
}

impl<DB, Tree> BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer,
{
    /// Ensures that the given block number is canonical (synced)
    ///
    /// This is a helper for guarding the [HistoricalStateProvider] against block numbers that are
    /// out of range and would lead to invalid results, mainly during initial sync.
    ///
    /// Verifying the block_number would be expensive since we need to lookup sync table
    /// Instead, we ensure that the `block_number` is within the range of the
    /// [Self::best_block_number] which is updated when a block is synced.
    #[inline]
    fn ensure_canonical_block(&self, block_number: BlockNumber) -> Result<()> {
        let latest = self.best_block_number()?;
        if block_number > latest {
            Err(ProviderError::HeaderNotFound(block_number.into()).into())
        } else {
            Ok(())
        }
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

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        self.database.sealed_headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> Result<Option<SealedHeader>> {
        self.database.sealed_header(number)
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

impl<DB, Tree> BlockNumProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn chain_info(&self) -> Result<ChainInfo> {
        Ok(self.chain_info.chain_info())
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        Ok(self.chain_info.get_canonical_block_number())
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.database.block_number(hash)
    }
}

impl<DB, Tree> BlockIdProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn pending_block_num_hash(&self) -> Result<Option<BlockNumHash>> {
        Ok(self.tree.pending_block_num_hash())
    }

    fn safe_block_num_hash(&self) -> Result<Option<BlockNumHash>> {
        Ok(self.chain_info.get_safe_num_hash())
    }

    fn finalized_block_num_hash(&self) -> Result<Option<BlockNumHash>> {
        Ok(self.chain_info.get_finalized_num_hash())
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

    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>> {
        match id {
            BlockHashOrNumber::Hash(hash) => self.find_block_by_hash(hash, BlockSource::Any),
            BlockHashOrNumber::Number(num) => self.database.block_by_number(num),
        }
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        Ok(self.tree.pending_block())
    }

    fn ommers(&self, id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
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

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
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

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        self.database.receipts_by_block(block)
    }
}

impl<DB, Tree> WithdrawalsProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> Result<Option<Vec<Withdrawal>>> {
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
    fn fill_env_at(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> Result<()> {
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

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockHashOrNumber) -> Result<()> {
        self.database.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        self.database.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> Result<()> {
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
        self.ensure_canonical_block(block_number)?;
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

        if let Some(block) = self.tree.pending_block_num_hash() {
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
    fn buffer_block(
        &self,
        block: SealedBlockWithSenders,
    ) -> std::result::Result<(), InsertBlockError> {
        self.tree.buffer_block(block)
    }

    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
    ) -> std::result::Result<BlockStatus, InsertBlockError> {
        self.tree.insert_block(block)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        self.tree.finalize_block(finalized_block)
    }

    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<()> {
        self.tree.restore_canonical_hashes(last_finalized_block)
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> Result<CanonicalOutcome> {
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

    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader> {
        self.tree.header_by_hash(hash)
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        self.tree.block_by_hash(block_hash)
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.canonical_blocks()
    }

    fn find_canonical_ancestor(&self, hash: BlockHash) -> Option<BlockHash> {
        self.tree.find_canonical_ancestor(hash)
    }

    fn canonical_tip(&self) -> BlockNumHash {
        self.tree.canonical_tip()
    }

    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        self.tree.pending_blocks()
    }

    fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        self.tree.pending_block_num_hash()
    }
}

impl<DB, Tree> CanonChainTracker for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: Send + Sync,
    Self: BlockProvider,
{
    fn on_forkchoice_update_received(&self, _update: &ForkchoiceState) {
        // update timestamp
        self.chain_info.on_forkchoice_update_received();
    }

    fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.chain_info.last_forkchoice_update_received_at()
    }

    fn set_canonical_head(&self, header: SealedHeader) {
        self.chain_info.set_canonical_head(header);
    }

    fn set_safe(&self, header: SealedHeader) {
        self.chain_info.set_safe(header);
    }

    fn set_finalized(&self, header: SealedHeader) {
        self.chain_info.set_finalized(header);
    }
}

impl<DB, Tree> BlockProviderIdExt for BlockchainProvider<DB, Tree>
where
    Self: BlockProvider + BlockIdProvider,
    Tree: BlockchainTreeEngine,
{
    fn block_by_id(&self, id: BlockId) -> Result<Option<Block>> {
        match id {
            BlockId::Number(num) => self.block_by_number_or_tag(num),
            BlockId::Hash(hash) => {
                // TODO: should we only apply this for the RPCs that are listed in EIP-1898?
                // so not at the provider level?
                // if we decide to do this at a higher level, then we can make this an automatic
                // trait impl
                if Some(true) == hash.require_canonical {
                    // check the database, canonical blocks are only stored in the database
                    self.find_block_by_hash(hash.block_hash, BlockSource::Database)
                } else {
                    self.block_by_hash(hash.block_hash)
                }
            }
        }
    }

    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<Header>> {
        match id {
            BlockNumberOrTag::Latest => Ok(Some(self.chain_info.get_canonical_head().unseal())),
            BlockNumberOrTag::Finalized => {
                Ok(self.chain_info.get_finalized_header().map(|h| h.unseal()))
            }
            BlockNumberOrTag::Safe => Ok(self.chain_info.get_safe_header().map(|h| h.unseal())),
            BlockNumberOrTag::Earliest => self.header_by_number(0),
            BlockNumberOrTag::Pending => Ok(self.tree.pending_header().map(|h| h.unseal())),
            BlockNumberOrTag::Number(num) => self.header_by_number(num),
        }
    }

    fn sealed_header_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<SealedHeader>> {
        match id {
            BlockNumberOrTag::Latest => Ok(Some(self.chain_info.get_canonical_head())),
            BlockNumberOrTag::Finalized => Ok(self.chain_info.get_finalized_header()),
            BlockNumberOrTag::Safe => Ok(self.chain_info.get_safe_header()),
            BlockNumberOrTag::Earliest => {
                self.header_by_number(0)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
            }
            BlockNumberOrTag::Pending => Ok(self.tree.pending_header()),
            BlockNumberOrTag::Number(num) => {
                self.header_by_number(num)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
            }
        }
    }

    fn sealed_header_by_id(&self, id: BlockId) -> Result<Option<SealedHeader>> {
        match id {
            BlockId::Number(num) => self.sealed_header_by_number_or_tag(num),
            BlockId::Hash(hash) => {
                self.header(&hash.block_hash)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
            }
        }
    }

    fn header_by_id(&self, id: BlockId) -> Result<Option<Header>> {
        match id {
            BlockId::Number(num) => self.header_by_number_or_tag(num),
            BlockId::Hash(hash) => self.header(&hash.block_hash),
        }
    }

    fn ommers_by_id(&self, id: BlockId) -> Result<Option<Vec<Header>>> {
        match id {
            BlockId::Number(num) => self.ommers_by_number_or_tag(num),
            BlockId::Hash(hash) => {
                // TODO: EIP-1898 question, see above
                // here it is not handled
                self.ommers(BlockHashOrNumber::Hash(hash.block_hash))
            }
        }
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
