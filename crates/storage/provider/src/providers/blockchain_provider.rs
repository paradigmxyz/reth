use crate::{
    providers::BundleStateProvider, AccountReader, BlockHashReader, BlockIdReader, BlockNumReader,
    BlockReader, BlockReaderIdExt, BlockSource, BlockchainTreePendingStateProvider,
    CanonChainTracker, CanonStateNotifications, CanonStateSubscriptions, ChainSpecProvider,
    ChangeSetReader, DatabaseProviderFactory, DatabaseProviderRO, EvmEnvProvider,
    FullExecutionDataProvider, HeaderProvider, ProviderError, ProviderFactory,
    PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt, RequestsProvider,
    StageCheckpointReader, StateProviderBox, StateProviderFactory, StaticFileProviderFactory,
    TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chain_state::CanonicalInMemoryState;
use reth_chainspec::{ChainInfo, ChainSpec};
use reth_db_api::{
    database::Database,
    models::{AccountBeforeTx, StoredBlockBodyIndices},
};
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumber,
    BlockNumberOrTag, BlockWithSenders, Header, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber,
    Withdrawal, Withdrawals, B256, U256,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
    time::Instant,
};
use tracing::trace;

use super::StaticFileProvider;

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[allow(missing_debug_implementations)]
pub struct BlockchainProvider2<DB> {
    /// Provider type used to access the database.
    database: ProviderFactory<DB>,
    /// Tracks the chain info wrt forkchoice updates and in memory canonical
    /// state.
    canonical_in_memory_state: CanonicalInMemoryState,
}

impl<DB> Clone for BlockchainProvider2<DB> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            canonical_in_memory_state: self.canonical_in_memory_state.clone(),
        }
    }
}

impl<DB> BlockchainProvider2<DB> {
    /// Create new provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker.
    pub fn with_latest(database: ProviderFactory<DB>, latest: SealedHeader) -> Self {
        Self { database, canonical_in_memory_state: CanonicalInMemoryState::with_head(latest) }
    }
}

impl<DB> BlockchainProvider2<DB>
where
    DB: Database,
{
    /// Create a new provider using only the database, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ProviderFactory<DB>) -> ProviderResult<Self> {
        let provider = database.provider()?;
        let best: ChainInfo = provider.chain_info()?;
        match provider.header_by_number(best.best_number)? {
            Some(header) => {
                drop(provider);
                Ok(Self::with_latest(database, header.seal(best.best_hash)))
            }
            None => Err(ProviderError::HeaderNotFound(best.best_number.into())),
        }
    }
}

impl<DB> BlockchainProvider2<DB>
where
    DB: Database,
{
    /// Ensures that the given block number is canonical (synced)
    ///
    /// This is a helper for guarding the `HistoricalStateProvider` against block numbers that are
    /// out of range and would lead to invalid results, mainly during initial sync.
    ///
    /// Verifying the `block_number` would be expensive since we need to lookup sync table
    /// Instead, we ensure that the `block_number` is within the range of the
    /// [`Self::best_block_number`] which is updated when a block is synced.
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

impl<DB> DatabaseProviderFactory<DB> for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn database_provider_ro(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        self.database.provider()
    }
}

impl<DB> StaticFileProviderFactory for BlockchainProvider2<DB> {
    fn static_file_provider(&self) -> StaticFileProvider {
        self.database.static_file_provider()
    }
}

impl<DB> HeaderProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.database.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.database.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.database.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        self.database.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.database.headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.database.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.database.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.database.sealed_headers_while(range, predicate)
    }
}

impl<DB> BlockHashReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.database.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.database.canonical_hashes_range(start, end)
    }
}

impl<DB> BlockNumReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(self.canonical_in_memory_state.chain_info())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.canonical_in_memory_state.get_canonical_block_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.database.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.database.block_number(hash)
    }
}

impl<DB> BlockIdReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn pending_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.canonical_in_memory_state.pending_block_num_hash())
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.canonical_in_memory_state.get_safe_num_hash())
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<BlockNumHash>> {
        Ok(self.canonical_in_memory_state.get_finalized_num_hash())
    }
}

impl<DB> BlockReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        let block = match source {
            BlockSource::Any | BlockSource::Canonical => {
                // check in memory first
                // Note: it's fine to return the unsealed block because the caller already has
                // the hash
                let mut block = self
                    .canonical_in_memory_state
                    .state_by_hash(hash)
                    .map(|block_state| block_state.block().block().clone().unseal());

                if block.is_none() {
                    block = self.database.block_by_hash(hash)?;
                }
                block
            }
            BlockSource::Pending => {
                self.canonical_in_memory_state.pending_block().map(|block| block.unseal())
            }
        };

        Ok(block)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        match id {
            BlockHashOrNumber::Hash(hash) => self.find_block_by_hash(hash, BlockSource::Any),
            BlockHashOrNumber::Number(num) => self.database.block_by_number(num),
        }
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        Ok(self.canonical_in_memory_state.pending_block())
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(self.canonical_in_memory_state.pending_block_with_senders())
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(self.canonical_in_memory_state.pending_block_and_receipts())
    }

    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        self.database.ommers(id)
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.database.block_body_indices(number)
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: If [`TransactionVariant::NoHash`] is provided then the transactions have invalid
    /// hashes, since they would need to be calculated on the spot, and we want fast querying.**
    ///
    /// Returns `None` if block is not found.
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        self.database.block_with_senders(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.database.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.database.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        self.database.block_with_senders_range(range)
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        self.database.sealed_block_with_senders_range(range)
    }
}

impl<DB> TransactionsProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.database.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.database.transaction_by_id(id)
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.database.transaction_by_id_no_hash(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.database.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        self.database.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.database.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        self.database.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        self.database.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.database.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.database.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.database.transaction_sender(id)
    }
}

impl<DB> ReceiptProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.database.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        self.database.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        self.database.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.database.receipts_by_tx_range(range)
    }
}

impl<DB> ReceiptProviderIdExt for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn receipts_by_block_id(&self, block: BlockId) -> ProviderResult<Option<Vec<Receipt>>> {
        match block {
            BlockId::Hash(rpc_block_hash) => {
                let mut receipts = self.receipts_by_block(rpc_block_hash.block_hash.into())?;
                if receipts.is_none() && !rpc_block_hash.require_canonical.unwrap_or(false) {
                    let block_state = self
                        .canonical_in_memory_state
                        .state_by_hash(rpc_block_hash.block_hash)
                        .ok_or(ProviderError::StateForHashNotFound(rpc_block_hash.block_hash))?;
                    receipts = Some(block_state.executed_block_receipts());
                }
                Ok(receipts)
            }
            BlockId::Number(num_tag) => match num_tag {
                BlockNumberOrTag::Pending => Ok(self
                    .canonical_in_memory_state
                    .pending_state()
                    .map(|block_state| block_state.executed_block_receipts())),
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

impl<DB> WithdrawalsProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        self.database.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        self.database.latest_withdrawal()
    }
}

impl<DB> RequestsProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<reth_primitives::Requests>> {
        self.database.requests_by_block(id, timestamp)
    }
}

impl<DB> StageCheckpointReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.database.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.database.provider()?.get_stage_checkpoint_progress(id)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.database.provider()?.get_all_checkpoints()
    }
}

impl<DB> EvmEnvProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.database.provider()?.fill_env_at(cfg, block_env, at, evm_config)
    }

    fn fill_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.database.provider()?.fill_env_with_header(cfg, block_env, header, evm_config)
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.database.provider()?.fill_cfg_env_at(cfg, at, evm_config)
    }

    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        header: &Header,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv,
    {
        self.database.provider()?.fill_cfg_env_with_header(cfg, header, evm_config)
    }
}

impl<DB> PruneCheckpointReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.database.provider()?.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.database.provider()?.get_prune_checkpoints()
    }
}

impl<DB> ChainSpecProvider for BlockchainProvider2<DB>
where
    DB: Send + Sync,
{
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.database.chain_spec()
    }
}

impl<DB> StateProviderFactory for BlockchainProvider2<DB>
where
    DB: Database,
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
    /// [`Self::latest`]
    fn pending(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", "Getting provider for pending state");

        // TODO: check in memory overlay https://github.com/paradigmxyz/reth/issues/9614

        // fallback to latest state if the pending block is not available
        self.latest()
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        // TODO: check in memory overlay https://github.com/paradigmxyz/reth/issues/9614

        Ok(None)
    }

    fn pending_with_provider(
        &self,
        bundle_state_data: Box<dyn FullExecutionDataProvider>,
    ) -> ProviderResult<StateProviderBox> {
        let canonical_fork = bundle_state_data.canonical_fork();
        trace!(target: "providers::blockchain", ?canonical_fork, "Returning post state provider");

        let state_provider = self.history_by_block_hash(canonical_fork.hash)?;
        let bundle_state_provider = BundleStateProvider::new(state_provider, bundle_state_data);
        Ok(Box::new(bundle_state_provider))
    }
}

impl<DB> CanonChainTracker for BlockchainProvider2<DB>
where
    DB: Send + Sync,
    Self: BlockReader,
{
    fn on_forkchoice_update_received(&self, _update: &ForkchoiceState) {
        // update timestamp
        self.canonical_in_memory_state.on_forkchoice_update_received();
    }

    fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.canonical_in_memory_state.last_received_update_timestamp()
    }

    fn on_transition_configuration_exchanged(&self) {
        self.canonical_in_memory_state.on_transition_configuration_exchanged();
    }

    fn last_exchanged_transition_configuration_timestamp(&self) -> Option<Instant> {
        self.canonical_in_memory_state.last_exchanged_transition_configuration_timestamp()
    }

    fn set_canonical_head(&self, header: SealedHeader) {
        self.canonical_in_memory_state.set_canonical_head(header);
    }

    fn set_safe(&self, header: SealedHeader) {
        self.canonical_in_memory_state.set_safe(header);
    }

    fn set_finalized(&self, header: SealedHeader) {
        self.canonical_in_memory_state.set_finalized(header);
    }
}

impl<DB> BlockReaderIdExt for BlockchainProvider2<DB>
where
    Self: BlockReader + BlockIdReader + ReceiptProviderIdExt,
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
                    self.find_block_by_hash(hash.block_hash, BlockSource::Canonical)
                } else {
                    self.block_by_hash(hash.block_hash)
                }
            }
        }
    }

    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<Header>> {
        Ok(match id {
            BlockNumberOrTag::Latest => {
                Some(self.canonical_in_memory_state.get_canonical_head().unseal())
            }
            BlockNumberOrTag::Finalized => {
                self.canonical_in_memory_state.get_finalized_header().map(|h| h.unseal())
            }
            BlockNumberOrTag::Safe => {
                self.canonical_in_memory_state.get_safe_header().map(|h| h.unseal())
            }
            BlockNumberOrTag::Earliest => self.header_by_number(0)?,
            BlockNumberOrTag::Pending => self.canonical_in_memory_state.pending_header(),

            BlockNumberOrTag::Number(num) => self.header_by_number(num)?,
        })
    }

    fn sealed_header_by_number_or_tag(
        &self,
        id: BlockNumberOrTag,
    ) -> ProviderResult<Option<SealedHeader>> {
        match id {
            BlockNumberOrTag::Latest => {
                Ok(Some(self.canonical_in_memory_state.get_canonical_head()))
            }
            BlockNumberOrTag::Finalized => {
                Ok(self.canonical_in_memory_state.get_finalized_header())
            }
            BlockNumberOrTag::Safe => Ok(self.canonical_in_memory_state.get_safe_header()),
            BlockNumberOrTag::Earliest => {
                self.header_by_number(0)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
            }
            BlockNumberOrTag::Pending => Ok(self.canonical_in_memory_state.pending_sealed_header()),
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

impl<DB> BlockchainTreePendingStateProvider for BlockchainProvider2<DB>
where
    DB: Send + Sync,
{
    fn find_pending_state_provider(
        &self,
        _block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>> {
        // TODO: check in memory overlay https://github.com/paradigmxyz/reth/issues/9614
        None
    }
}

impl<DB> CanonStateSubscriptions for BlockchainProvider2<DB>
where
    DB: Send + Sync,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        self.canonical_in_memory_state.subscribe_canon_state()
    }
}

impl<DB> ChangeSetReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        self.database.provider()?.account_block_changeset(block_number)
    }
}

impl<DB> AccountReader for BlockchainProvider2<DB>
where
    DB: Database + Sync + Send,
{
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.database.provider()?.basic_account(address)
    }
}
