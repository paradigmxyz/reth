use crate::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BlockSource, BlockchainTreePendingStateProvider, CanonChainTracker, CanonStateNotifications,
    CanonStateSubscriptions, ChainSpecProvider, ChainStateBlockReader, ChangeSetReader,
    DatabaseProviderFactory, EvmEnvProvider, FullExecutionDataProvider, HeaderProvider,
    ProviderError, PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt, RequestsProvider,
    StageCheckpointReader, StateProviderBox, StateProviderFactory, StaticFileProviderFactory,
    TransactionVariant, TransactionsProvider, TreeViewer, WithdrawalsProvider,
};
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag};
use alloy_primitives::{Address, BlockHash, BlockNumber, Sealable, TxHash, TxNumber, B256, U256};
use reth_blockchain_tree_api::{
    error::{CanonicalError, InsertBlockError},
    BlockValidationKind, BlockchainTreeEngine, BlockchainTreeViewer, CanonicalOutcome,
    InsertPayloadOk,
};
use reth_chain_state::{ChainInfoTracker, ForkChoiceNotifications, ForkChoiceSubscriptions};
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_db_api::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_evm::ConfigureEvmEnv;
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{
    Account, Block, BlockWithSenders, Header, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, TransactionMeta, TransactionSigned, TransactionSignedNoHash, Withdrawal,
    Withdrawals,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    collections::BTreeMap,
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
    time::Instant,
};
use tracing::trace;

mod database;
pub use database::*;

mod static_file;
pub use static_file::{
    StaticFileAccess, StaticFileJarProvider, StaticFileProvider, StaticFileProviderRW,
    StaticFileProviderRWRefMut, StaticFileWriter,
};

mod state;
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};

mod bundle_state_provider;
pub use bundle_state_provider::BundleStateProvider;

mod consistent_view;
use alloy_rpc_types_engine::ForkchoiceState;
pub use consistent_view::{ConsistentDbView, ConsistentViewError};

mod blockchain_provider;
pub use blockchain_provider::BlockchainProvider2;

/// Helper trait keeping common requirements of providers for [`NodeTypesWithDB`].
pub trait ProviderNodeTypes: NodeTypesWithDB<ChainSpec: EthereumHardforks> {}

impl<T> ProviderNodeTypes for T where T: NodeTypesWithDB<ChainSpec: EthereumHardforks> {}

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[allow(missing_debug_implementations)]
pub struct BlockchainProvider<N: NodeTypesWithDB> {
    /// Provider type used to access the database.
    database: ProviderFactory<N>,
    /// The blockchain tree instance.
    tree: Arc<dyn TreeViewer>,
    /// Tracks the chain info wrt forkchoice updates
    chain_info: ChainInfoTracker,
}

impl<N: ProviderNodeTypes> Clone for BlockchainProvider<N> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            tree: self.tree.clone(),
            chain_info: self.chain_info.clone(),
        }
    }
}

impl<N: NodeTypesWithDB> BlockchainProvider<N> {
    /// Sets the treeviewer for the provider.
    #[doc(hidden)]
    pub fn with_tree(mut self, tree: Arc<dyn TreeViewer>) -> Self {
        self.tree = tree;
        self
    }
}

impl<N: ProviderNodeTypes> BlockchainProvider<N> {
    /// Create new provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker, alongside the finalized header
    /// if it exists.
    pub fn with_blocks(
        database: ProviderFactory<N>,
        tree: Arc<dyn TreeViewer>,
        latest: SealedHeader,
        finalized: Option<SealedHeader>,
        safe: Option<SealedHeader>,
    ) -> Self {
        Self { database, tree, chain_info: ChainInfoTracker::new(latest, finalized, safe) }
    }

    /// Create a new provider using only the database and the tree, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ProviderFactory<N>, tree: Arc<dyn TreeViewer>) -> ProviderResult<Self> {
        let provider = database.provider()?;
        let best: ChainInfo = provider.chain_info()?;
        let latest_header = provider
            .header_by_number(best.best_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(best.best_number.into()))?;

        let finalized_header = provider
            .last_finalized_block_number()?
            .map(|num| provider.sealed_header(num))
            .transpose()?
            .flatten();

        let safe_header = provider
            .last_safe_block_number()?
            .map(|num| provider.sealed_header(num))
            .transpose()?
            .flatten();

        Ok(Self::with_blocks(
            database,
            tree,
            SealedHeader::new(latest_header, best.best_hash),
            finalized_header,
            safe_header,
        ))
    }

    /// Ensures that the given block number is canonical (synced)
    ///
    /// This is a helper for guarding the [`HistoricalStateProvider`] against block numbers that are
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

impl<N> BlockchainProvider<N>
where
    Self: StateProviderFactory,
    N: NodeTypesWithDB,
{
    /// Return a [`StateProviderBox`] that contains bundle state data provider.
    /// Used to inspect or execute transaction on the pending state.
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

impl<N: ProviderNodeTypes> DatabaseProviderFactory for BlockchainProvider<N> {
    type DB = N::DB;
    type Provider = <ProviderFactory<N> as DatabaseProviderFactory>::Provider;
    type ProviderRW = <ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        self.database.database_provider_ro()
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        self.database.database_provider_rw()
    }
}

impl<N: ProviderNodeTypes> StaticFileProviderFactory for BlockchainProvider<N> {
    fn static_file_provider(&self) -> StaticFileProvider {
        self.database.static_file_provider()
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> BlockHashReader for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> BlockNumReader for BlockchainProvider<N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(self.chain_info.chain_info())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.chain_info.get_canonical_block_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.database.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.database.block_number(hash)
    }
}

impl<N: ProviderNodeTypes> BlockIdReader for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> BlockReader for BlockchainProvider<N> {
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        let block = match source {
            BlockSource::Any => {
                // check database first
                let mut block = self.database.block_by_hash(hash)?;
                if block.is_none() {
                    // Note: it's fine to return the unsealed block because the caller already has
                    // the hash
                    block = self.tree.block_by_hash(hash).map(|block| block.unseal());
                }
                block
            }
            BlockSource::Pending => self.tree.block_by_hash(hash).map(|block| block.unseal()),
            BlockSource::Canonical => self.database.block_by_hash(hash)?,
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
        Ok(self.tree.pending_block())
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(self.tree.pending_block_with_senders())
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(self.tree.pending_block_and_receipts())
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

impl<N: ProviderNodeTypes> TransactionsProvider for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> ReceiptProvider for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> ReceiptProviderIdExt for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> WithdrawalsProvider for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> RequestsProvider for BlockchainProvider<N> {
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<reth_primitives::Requests>> {
        self.database.requests_by_block(id, timestamp)
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> EvmEnvProvider for BlockchainProvider<N> {
    fn fill_env_at<EvmConfig>(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
        evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        self.database.provider()?.fill_cfg_env_with_header(cfg, header, evm_config)
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> ChainSpecProvider for BlockchainProvider<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.database.chain_spec()
    }
}

impl<N: ProviderNodeTypes> StateProviderFactory for BlockchainProvider<N> {
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

    /// Returns a [`StateProviderBox`] indexed by the given block number or tag.
    ///
    /// Note: if a number is provided this will only look at historical(canonical) state.
    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash =
                    self.finalized_block_hash()?.ok_or(ProviderError::FinalizedBlockNotFound)?;

                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => {
                // Note: The `BlockchainProvider` could also lookup the tree for the given block number, if for example the block number is `latest + 1`, however this should only support canonical state: <https://github.com/paradigmxyz/reth/issues/4515>
                self.history_by_block_number(num)
            }
        }
    }

    /// Returns the state provider for pending state.
    ///
    /// If there's no pending block available then the latest state provider is returned:
    /// [`Self::latest`]
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
}

impl<N: ProviderNodeTypes> BlockchainTreeEngine for BlockchainProvider<N> {
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

    fn finalize_block(&self, finalized_block: BlockNumber) -> ProviderResult<()> {
        self.tree.finalize_block(finalized_block)
    }

    fn connect_buffered_blocks_to_canonical_hashes_and_finalize(
        &self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), CanonicalError> {
        self.tree.connect_buffered_blocks_to_canonical_hashes_and_finalize(last_finalized_block)
    }

    fn update_block_hashes_and_clear_buffered(
        &self,
    ) -> Result<BTreeMap<BlockNumber, B256>, CanonicalError> {
        self.tree.update_block_hashes_and_clear_buffered()
    }

    fn connect_buffered_blocks_to_canonical_hashes(&self) -> Result<(), CanonicalError> {
        self.tree.connect_buffered_blocks_to_canonical_hashes()
    }

    fn make_canonical(&self, block_hash: BlockHash) -> Result<CanonicalOutcome, CanonicalError> {
        self.tree.make_canonical(block_hash)
    }
}

impl<N: ProviderNodeTypes> BlockchainTreeViewer for BlockchainProvider<N> {
    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader> {
        self.tree.header_by_hash(hash)
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        self.tree.block_by_hash(block_hash)
    }

    fn block_with_senders_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlockWithSenders> {
        self.tree.block_with_senders_by_hash(block_hash)
    }

    fn buffered_header_by_hash(&self, block_hash: BlockHash) -> Option<SealedHeader> {
        self.tree.buffered_header_by_hash(block_hash)
    }

    fn is_canonical(&self, hash: BlockHash) -> Result<bool, ProviderError> {
        self.tree.is_canonical(hash)
    }

    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders> {
        self.tree.lowest_buffered_ancestor(hash)
    }

    fn canonical_tip(&self) -> BlockNumHash {
        self.tree.canonical_tip()
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

impl<N: ProviderNodeTypes> CanonChainTracker for BlockchainProvider<N>
where
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

impl<N: ProviderNodeTypes> BlockReaderIdExt for BlockchainProvider<N>
where
    Self: BlockReader + ReceiptProviderIdExt,
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
                    BlockReader::block_by_hash(self, hash.block_hash)
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
            BlockNumberOrTag::Earliest => self.header_by_number(0)?.map_or_else(
                || Ok(None),
                |h| {
                    let sealed = h.seal_slow();
                    let (header, seal) = sealed.into_parts();
                    Ok(Some(SealedHeader::new(header, seal)))
                },
            ),
            BlockNumberOrTag::Pending => Ok(self.tree.pending_header()),
            BlockNumberOrTag::Number(num) => self.header_by_number(num)?.map_or_else(
                || Ok(None),
                |h| {
                    let sealed = h.seal_slow();
                    let (header, seal) = sealed.into_parts();
                    Ok(Some(SealedHeader::new(header, seal)))
                },
            ),
        }
    }

    fn sealed_header_by_id(&self, id: BlockId) -> ProviderResult<Option<SealedHeader>> {
        Ok(match id {
            BlockId::Number(num) => self.sealed_header_by_number_or_tag(num)?,
            BlockId::Hash(hash) => self.header(&hash.block_hash)?.map(|h| {
                let sealed = h.seal_slow();
                let (header, seal) = sealed.into_parts();
                SealedHeader::new(header, seal)
            }),
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

impl<N: ProviderNodeTypes> BlockchainTreePendingStateProvider for BlockchainProvider<N> {
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>> {
        self.tree.find_pending_state_provider(block_hash)
    }
}

impl<N: ProviderNodeTypes> CanonStateSubscriptions for BlockchainProvider<N> {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        self.tree.subscribe_to_canonical_state()
    }
}

impl<N: ProviderNodeTypes> ForkChoiceSubscriptions for BlockchainProvider<N> {
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications {
        let receiver = self.chain_info.subscribe_safe_block();
        ForkChoiceNotifications(receiver)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications {
        let receiver = self.chain_info.subscribe_finalized_block();
        ForkChoiceNotifications(receiver)
    }
}

impl<N: ProviderNodeTypes> ChangeSetReader for BlockchainProvider<N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        self.database.provider()?.account_block_changeset(block_number)
    }
}

impl<N: ProviderNodeTypes> AccountReader for BlockchainProvider<N> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        self.database.provider()?.basic_account(address)
    }
}
