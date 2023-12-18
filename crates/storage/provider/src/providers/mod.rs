use crate::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BlockchainTreePendingStateProvider, BundleStateDataProvider, CanonChainTracker,
    CanonStateNotifications, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    EvmEnvProvider, HeaderProvider, ProviderError, PruneCheckpointReader, ReceiptProvider,
    ReceiptProviderIdExt, StageCheckpointReader, StateProviderBox, StateProviderFactory,
    TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use reth_db::{database::Database, models::StoredBlockBodyIndices};
use reth_interfaces::{
    blockchain_tree::{BlockchainTreeEngine, BlockchainTreeViewer},
    consensus::ForkchoiceState,
    provider::ProviderResult,
    RethError, RethResult,
};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumber,
    BlockNumberOrTag, BlockWithSenders, ChainInfo, ChainSpec, Header, PruneCheckpoint,
    PruneSegment, Receipt, SealedBlock, SealedBlockWithSenders, SealedHeader, TransactionMeta,
    TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, B256, U256,
};
use revm::primitives::{BlockEnv, CfgEnv};
use std::{
    collections::{BTreeMap, HashSet},
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
    time::Instant,
};
use tracing::trace;

pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};

mod bundle_state_provider;
mod chain_info;
mod database;
mod snapshot;
pub use snapshot::{SnapshotJarProvider, SnapshotProvider};
mod state;
use crate::{providers::chain_info::ChainInfoTracker, traits::BlockSource};
pub use bundle_state_provider::BundleStateProvider;
pub use database::*;
use reth_db::models::AccountBeforeTx;
use reth_interfaces::blockchain_tree::{
    error::InsertBlockError, BlockValidationKind, CanonicalOutcome, InsertPayloadOk,
};

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Clone, Debug)]
pub struct BlockchainProvider<DB, Tree> {
    /// Provider type used to access the database.
    database: ProviderFactory<DB>,
    /// The blockchain tree instance.
    tree: Tree,
    /// Tracks the chain info wrt forkchoice updates
    chain_info: ChainInfoTracker,
}

impl<DB, Tree> BlockchainProvider<DB, Tree> {
    /// Create new  provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker.
    pub fn with_latest(database: ProviderFactory<DB>, tree: Tree, latest: SealedHeader) -> Self {
        Self { database, tree, chain_info: ChainInfoTracker::new(latest) }
    }
}

impl<DB, Tree> BlockchainProvider<DB, Tree>
where
    DB: Database,
{
    /// Create a new provider using only the database and the tree, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ProviderFactory<DB>, tree: Tree) -> ProviderResult<Self> {
        let provider = database.provider()?;
        let best: ChainInfo = provider.chain_info()?;
        match provider.header_by_number(best.best_number)? {
            Some(header) => {
                drop(provider);
                Ok(Self::with_latest(database, tree, header.seal(best.best_hash)))
            }
            None => Err(ProviderError::HeaderNotFound(best.best_number.into())),
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
    fn ensure_canonical_block(&self, block_number: BlockNumber) -> ProviderResult<()> {
        let latest = self.best_block_number()?;
        if block_number > latest {
            Err(ProviderError::HeaderNotFound(block_number.into()))
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
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.database.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.database.provider()?.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.database.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        self.database.provider()?.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.database.provider()?.headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.database.provider()?.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.database.provider()?.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.database.provider()?.sealed_headers_while(range, predicate)
    }
}

impl<DB, Tree> BlockHashReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.database.provider()?.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.database.provider()?.canonical_hashes_range(start, end)
    }
}

impl<DB, Tree> BlockNumReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(self.chain_info.chain_info())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.chain_info.get_canonical_block_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.database.provider()?.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.database.provider()?.block_number(hash)
    }
}

impl<DB, Tree> BlockIdReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn pending_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.tree.pending_block_num_hash())
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.chain_info.get_safe_num_hash())
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.chain_info.get_finalized_num_hash())
    }
}

impl<DB, Tree> BlockReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        let block = match source {
            BlockSource::Any => {
                // check database first
                let mut block = self.database.provider()?.block_by_hash(hash)?;
                if block.is_none() {
                    // Note: it's fine to return the unsealed block because the caller already has
                    // the hash
                    block = self.tree.block_by_hash(hash).map(|block| block.unseal());
                }
                block
            }
            BlockSource::Pending => self.tree.block_by_hash(hash).map(|block| block.unseal()),
            BlockSource::Database => self.database.provider()?.block_by_hash(hash)?,
        };

        Ok(block)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        match id {
            BlockHashOrNumber::Hash(hash) => self.find_block_by_hash(hash, BlockSource::Any),
            BlockHashOrNumber::Number(num) => self.database.provider()?.block_by_number(num),
        }
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        Ok(self.tree.pending_block())
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(self.tree.pending_block_with_senders())
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(self.tree.pending_block_and_receipts())
    }

    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        self.database.provider()?.ommers(id)
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.database.provider()?.block_body_indices(number)
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: If [TransactionVariant::NoHash] is provided then the transactions have invalid
    /// hashes, since they would need to be calculated on the spot, and we want fast querying.**
    ///
    /// Returns `None` if block is not found.
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        self.database.provider()?.block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.database.provider()?.block_range(range)
    }
}

impl<DB, Tree> TransactionsProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.database.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.database.provider()?.transaction_by_id(id)
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.database.provider()?.transaction_by_id_no_hash(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.database.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        self.database.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.database.provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        self.database.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        self.database.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.database.provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.database.provider()?.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.database.provider()?.transaction_sender(id)
    }
}

impl<DB, Tree> ReceiptProvider for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.database.provider()?.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        self.database.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        self.database.provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.database.provider()?.receipts_by_tx_range(range)
    }
}
impl<DB, Tree> ReceiptProviderIdExt for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreeViewer + Send + Sync,
{
    fn receipts_by_block_id(&self, block: BlockId) -> ProviderResult<Option<Vec<Receipt>>> {
        match block {
            BlockId::Hash(rpc_block_hash) => {
                let mut receipts = self.receipts_by_block(rpc_block_hash.block_hash.into())?;
                if receipts.is_none() && !rpc_block_hash.require_canonical.unwrap_or(false) {
                    receipts = self.tree.receipts_by_block_hash(rpc_block_hash.block_hash);
                }
                Ok(receipts)
            }
            BlockId::Number(num_tag) => match num_tag {
                BlockNumberOrTag::Pending => Ok(self.tree.pending_receipts()),
                _ => {
                    if let Some(num) = self.convert_block_number(num_tag)? {
                        self.receipts_by_block(num.into())
                    } else {
                        Ok(None)
                    }
                }
            },
        }
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
    ) -> ProviderResult<Option<Vec<Withdrawal>>> {
        self.database.provider()?.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        self.database.provider()?.latest_withdrawal()
    }
}

impl<DB, Tree> StageCheckpointReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.database.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.database.provider()?.get_stage_checkpoint_progress(id)
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
    ) -> ProviderResult<()> {
        self.database.provider()?.fill_env_at(cfg, block_env, at)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> ProviderResult<()> {
        self.database.provider()?.fill_env_with_header(cfg, block_env, header)
    }

    fn fill_block_env_at(
        &self,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> ProviderResult<()> {
        self.database.provider()?.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(
        &self,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> ProviderResult<()> {
        self.database.provider()?.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> ProviderResult<()> {
        self.database.provider()?.fill_cfg_env_at(cfg, at)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> ProviderResult<()> {
        self.database.provider()?.fill_cfg_env_with_header(cfg, header)
    }
}

impl<DB, Tree> PruneCheckpointReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Send + Sync,
{
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.database.provider()?.get_prune_checkpoint(segment)
    }
}

impl<DB, Tree> ChainSpecProvider for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: Send + Sync,
{
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.database.chain_spec()
    }
}

impl<DB, Tree> StateProviderFactory for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreePendingStateProvider + BlockchainTreeViewer,
{
    /// Storage provider for latest block
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", "Getting latest block state provider");
        self.database.latest()
    }

    fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_number, "Getting history by block number");
        self.ensure_canonical_block(block_number)?;
        self.database.history_by_block_number(block_number)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");
        self.database.history_by_block_hash(block_hash)
    }

    fn state_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block, "Getting state by block hash");
        let mut state = self.history_by_block_hash(block);

        // we failed to get the state by hash, from disk, hash block be the pending block
        if state.is_err() {
            if let Ok(Some(pending)) = self.pending_state_by_hash(block) {
                // we found pending block by hash
                state = Ok(pending)
            }
        }

        state
    }

    /// Returns the state provider for pending state.
    ///
    /// If there's no pending block available then the latest state provider is returned:
    /// [Self::latest]
    fn pending(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", "Getting provider for pending state");

        if let Some(block) = self.tree.pending_block_num_hash() {
            if let Ok(pending) = self.tree.pending_state_provider(block.hash) {
                return self.pending_with_provider(pending)
            }
        }

        // fallback to latest state if the pending block is not available
        self.latest()
    }

    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        if let Some(state) = self.tree.find_pending_state_provider(block_hash) {
            return Ok(Some(self.pending_with_provider(state)?))
        }
        Ok(None)
    }

    fn pending_with_provider(
        &self,
        bundle_state_data: Box<dyn BundleStateDataProvider>,
    ) -> ProviderResult<StateProviderBox> {
        let canonical_fork = bundle_state_data.canonical_fork();
        trace!(target: "providers::blockchain", ?canonical_fork, "Returning post state provider");

        let state_provider = self.history_by_block_hash(canonical_fork.hash)?;
        let bundle_state_provider = BundleStateProvider::new(state_provider, bundle_state_data);
        Ok(Box::new(bundle_state_provider))
    }
}

impl<DB, Tree> BlockchainTreeEngine for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: BlockchainTreeEngine,
{
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        self.tree.buffer_block(block)
    }

    fn insert_block(
        &self,
        block: SealedBlockWithSenders,
        validation_kind: BlockValidationKind,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        self.tree.insert_block(block, validation_kind)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        self.tree.finalize_block(finalized_block)
    }

    fn connect_buffered_blocks_to_canonical_hashes_and_finalize(
        &self,
        last_finalized_block: BlockNumber,
    ) -> RethResult<()> {
        self.tree.connect_buffered_blocks_to_canonical_hashes_and_finalize(last_finalized_block)
    }

    fn connect_buffered_blocks_to_canonical_hashes(&self) -> RethResult<()> {
        self.tree.connect_buffered_blocks_to_canonical_hashes()
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> RethResult<CanonicalOutcome> {
        self.tree.make_canonical(block_hash)
    }

    fn unwind(&self, unwind_to: BlockNumber) -> RethResult<()> {
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

    fn block_with_senders_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlockWithSenders> {
        self.tree.block_with_senders_by_hash(block_hash)
    }

    fn buffered_block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        self.tree.buffered_block_by_hash(block_hash)
    }

    fn buffered_header_by_hash(&self, block_hash: BlockHash) -> Option<SealedHeader> {
        self.tree.buffered_header_by_hash(block_hash)
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        self.tree.canonical_blocks()
    }

    fn find_canonical_ancestor(&self, hash: BlockHash) -> Option<BlockHash> {
        self.tree.find_canonical_ancestor(hash)
    }

    fn is_canonical(&self, hash: BlockHash) -> Result<bool, RethError> {
        self.tree.is_canonical(hash)
    }

    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders> {
        self.tree.lowest_buffered_ancestor(hash)
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

    fn pending_block_and_receipts(&self) -> Option<(SealedBlock, Vec<Receipt>)> {
        self.tree.pending_block_and_receipts()
    }

    fn receipts_by_block_hash(&self, block_hash: BlockHash) -> Option<Vec<Receipt>> {
        self.tree.receipts_by_block_hash(block_hash)
    }
}

impl<DB, Tree> CanonChainTracker for BlockchainProvider<DB, Tree>
where
    DB: Send + Sync,
    Tree: Send + Sync,
    Self: BlockReader,
{
    fn on_forkchoice_update_received(&self, _update: &ForkchoiceState) {
        // update timestamp
        self.chain_info.on_forkchoice_update_received();
    }

    fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.chain_info.last_forkchoice_update_received_at()
    }

    fn on_transition_configuration_exchanged(&self) {
        self.chain_info.on_transition_configuration_exchanged();
    }

    fn last_exchanged_transition_configuration_timestamp(&self) -> Option<Instant> {
        self.chain_info.last_transition_configuration_exchanged_at()
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

impl<DB, Tree> BlockReaderIdExt for BlockchainProvider<DB, Tree>
where
    Self: BlockReader + BlockIdReader + ReceiptProviderIdExt,
    Tree: BlockchainTreeEngine,
{
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Block>> {
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

    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<Header>> {
        Ok(match id {
            BlockNumberOrTag::Latest => Some(self.chain_info.get_canonical_head().unseal()),
            BlockNumberOrTag::Finalized => {
                self.chain_info.get_finalized_header().map(|h| h.unseal())
            }
            BlockNumberOrTag::Safe => self.chain_info.get_safe_header().map(|h| h.unseal()),
            BlockNumberOrTag::Earliest => self.header_by_number(0)?,
            BlockNumberOrTag::Pending => self.tree.pending_header().map(|h| h.unseal()),
            BlockNumberOrTag::Number(num) => self.header_by_number(num)?,
        })
    }

    fn sealed_header_by_number_or_tag(
        &self,
        id: BlockNumberOrTag,
    ) -> ProviderResult<Option<SealedHeader>> {
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

    fn sealed_header_by_id(&self, id: BlockId) -> ProviderResult<Option<SealedHeader>> {
        Ok(match id {
            BlockId::Number(num) => self.sealed_header_by_number_or_tag(num)?,
            BlockId::Hash(hash) => self.header(&hash.block_hash)?.map(|h| h.seal_slow()),
        })
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Header>> {
        Ok(match id {
            BlockId::Number(num) => self.header_by_number_or_tag(num)?,
            BlockId::Hash(hash) => self.header(&hash.block_hash)?,
        })
    }

    fn ommers_by_id(&self, id: BlockId) -> ProviderResult<Option<Vec<Header>>> {
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
    ) -> Option<Box<dyn BundleStateDataProvider>> {
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

impl<DB, Tree> ChangeSetReader for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: Sync + Send,
{
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        self.database.provider()?.account_block_changeset(block_number)
    }
}

impl<DB, Tree> AccountReader for BlockchainProvider<DB, Tree>
where
    DB: Database + Sync + Send,
    Tree: Sync + Send,
{
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.database.provider()?.basic_account(address)
    }
}
