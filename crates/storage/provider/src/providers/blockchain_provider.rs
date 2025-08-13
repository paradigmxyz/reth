#![allow(unused)]
use crate::{
    providers::{ConsistentProvider, ProviderNodeTypes, StaticFileProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BlockSource, CanonChainTracker, CanonStateNotifications, CanonStateSubscriptions,
    ChainSpecProvider, ChainStateBlockReader, ChangeSetReader, DatabaseProvider,
    DatabaseProviderFactory, FullProvider, HashedPostStateProvider, HeaderProvider, ProviderError,
    ProviderFactory, PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt,
    StageCheckpointReader, StateProviderBox, StateProviderFactory, StateReader,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider,
};
use alloy_consensus::{transaction::TransactionMeta, Header};
use alloy_eips::{
    eip4895::{Withdrawal, Withdrawals},
    BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag,
};
use alloy_primitives::{Address, BlockHash, BlockNumber, Sealable, TxHash, TxNumber, B256, U256};
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chain_state::{
    BlockState, CanonicalInMemoryState, ForkChoiceNotifications, ForkChoiceSubscriptions,
    MemoryOverlayStateProvider,
};
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_db_api::{
    models::{AccountBeforeTx, BlockNumberAddress, StoredBlockBodyIndices},
    transaction::DbTx,
    Database,
};
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{ConfigureEvm, EvmEnv};
use reth_execution_types::ExecutionOutcome;
use reth_node_types::{BlockTy, HeaderTy, NodeTypesWithDB, ReceiptTy, TxTy};
use reth_primitives_traits::{
    Account, BlockBody, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader, StorageEntry,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockBodyIndicesProvider, DBProvider, NodePrimitivesProvider, StateCommitmentProvider,
    StorageChangeSetReader,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::HashedPostState;
use reth_trie_db::StateCommitment;
use revm_database::BundleState;
use std::{
    ops::{Add, RangeBounds, RangeInclusive, Sub},
    sync::Arc,
    time::Instant,
};
use tracing::trace;

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Debug)]
pub struct BlockchainProvider<N: NodeTypesWithDB> {
    /// Provider factory used to access the database.
    pub(crate) database: ProviderFactory<N>,
    /// Tracks the chain info wrt forkchoice updates and in memory canonical
    /// state.
    pub(crate) canonical_in_memory_state: CanonicalInMemoryState<N::Primitives>,
}

impl<N: NodeTypesWithDB> Clone for BlockchainProvider<N> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            canonical_in_memory_state: self.canonical_in_memory_state.clone(),
        }
    }
}

impl<N: ProviderNodeTypes> BlockchainProvider<N> {
    /// Create a new [`BlockchainProvider`] using only the storage, fetching the latest
    /// header from the database to initialize the provider.
    pub fn new(storage: ProviderFactory<N>) -> ProviderResult<Self> {
        let provider = storage.provider()?;
        let best = provider.chain_info()?;
        match provider.header_by_number(best.best_number)? {
            Some(header) => {
                drop(provider);
                Ok(Self::with_latest(storage, SealedHeader::new(header, best.best_hash))?)
            }
            None => Err(ProviderError::HeaderNotFound(best.best_number.into())),
        }
    }

    /// Create new provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker.
    ///
    /// This returns a `ProviderResult` since it tries the retrieve the last finalized header from
    /// `database`.
    pub fn with_latest(
        storage: ProviderFactory<N>,
        latest: SealedHeader<HeaderTy<N>>,
    ) -> ProviderResult<Self> {
        let provider = storage.provider()?;
        let finalized_header = provider
            .last_finalized_block_number()?
            .map(|num| provider.sealed_header(num))
            .transpose()?
            .flatten();
        let safe_header = provider
            .last_safe_block_number()?
            .or_else(|| {
                // for the purpose of this we can also use the finalized block if we don't have the
                // safe block
                provider.last_finalized_block_number().ok().flatten()
            })
            .map(|num| provider.sealed_header(num))
            .transpose()?
            .flatten();
        Ok(Self {
            database: storage,
            canonical_in_memory_state: CanonicalInMemoryState::with_head(
                latest,
                finalized_header,
                safe_header,
            ),
        })
    }

    /// Gets a clone of `canonical_in_memory_state`.
    pub fn canonical_in_memory_state(&self) -> CanonicalInMemoryState<N::Primitives> {
        self.canonical_in_memory_state.clone()
    }

    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    #[track_caller]
    pub fn consistent_provider(&self) -> ProviderResult<ConsistentProvider<N>> {
        ConsistentProvider::new(self.database.clone(), self.canonical_in_memory_state())
    }

    /// This uses a given [`BlockState`] to initialize a state provider for that block.
    fn block_state_provider(
        &self,
        state: &BlockState<N::Primitives>,
    ) -> ProviderResult<MemoryOverlayStateProvider<N::Primitives>> {
        let anchor_hash = state.anchor().hash;
        let latest_historical = self.database.history_by_block_hash(anchor_hash)?;
        Ok(state.state_provider(latest_historical))
    }

    /// Return the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// If the range is empty, or there are no blocks for the given range, then this returns `None`.
    pub fn get_state(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<ExecutionOutcome<ReceiptTy<N>>>> {
        self.consistent_provider()?.get_state(range)
    }
}

impl<N: NodeTypesWithDB> NodePrimitivesProvider for BlockchainProvider<N> {
    type Primitives = N::Primitives;
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

impl<N: ProviderNodeTypes> StateCommitmentProvider for BlockchainProvider<N> {
    type StateCommitment = N::StateCommitment;
}

impl<N: ProviderNodeTypes> StaticFileProviderFactory for BlockchainProvider<N> {
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        self.database.static_file_provider()
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for BlockchainProvider<N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        self.consistent_provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.consistent_provider()?.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.consistent_provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        self.consistent_provider()?.header_td_by_number(number)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.consistent_provider()?.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.consistent_provider()?.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.consistent_provider()?.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.consistent_provider()?.sealed_headers_while(range, predicate)
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for BlockchainProvider<N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.consistent_provider()?.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.consistent_provider()?.canonical_hashes_range(start, end)
    }
}

impl<N: ProviderNodeTypes> BlockNumReader for BlockchainProvider<N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(self.canonical_in_memory_state.chain_info())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self.canonical_in_memory_state.get_canonical_block_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.database.last_block_number()
    }

    fn earliest_block_number(&self) -> ProviderResult<BlockNumber> {
        self.database.earliest_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.consistent_provider()?.block_number(hash)
    }
}

impl<N: ProviderNodeTypes> BlockIdReader for BlockchainProvider<N> {
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

impl<N: ProviderNodeTypes> BlockReader for BlockchainProvider<N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        self.consistent_provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        self.consistent_provider()?.block(id)
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(self.canonical_in_memory_state.pending_recovered_block())
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        Ok(self.canonical_in_memory_state.pending_block_and_receipts())
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: If [`TransactionVariant::NoHash`] is provided then the transactions have invalid
    /// hashes, since they would need to be calculated on the spot, and we want fast querying.**
    ///
    /// Returns `None` if block is not found.
    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.consistent_provider()?.recovered_block(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.consistent_provider()?.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.consistent_provider()?.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.consistent_provider()?.block_with_senders_range(range)
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.consistent_provider()?.recovered_block_range(range)
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for BlockchainProvider<N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.consistent_provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.consistent_provider()?.transaction_by_id(id)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.consistent_provider()?.transaction_by_id_unhashed(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        self.consistent_provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        self.consistent_provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.consistent_provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        self.consistent_provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        self.consistent_provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.consistent_provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.consistent_provider()?.senders_by_tx_range(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.consistent_provider()?.transaction_sender(id)
    }
}

impl<N: ProviderNodeTypes> ReceiptProvider for BlockchainProvider<N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.consistent_provider()?.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        self.consistent_provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.consistent_provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.consistent_provider()?.receipts_by_tx_range(range)
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        self.consistent_provider()?.receipts_by_block_range(block_range)
    }
}

impl<N: ProviderNodeTypes> ReceiptProviderIdExt for BlockchainProvider<N> {
    fn receipts_by_block_id(&self, block: BlockId) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.consistent_provider()?.receipts_by_block_id(block)
    }
}

impl<N: ProviderNodeTypes> BlockBodyIndicesProvider for BlockchainProvider<N> {
    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.consistent_provider()?.block_body_indices(number)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        self.consistent_provider()?.block_body_indices_range(range)
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for BlockchainProvider<N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.consistent_provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.consistent_provider()?.get_stage_checkpoint_progress(id)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.consistent_provider()?.get_all_checkpoints()
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for BlockchainProvider<N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.consistent_provider()?.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.consistent_provider()?.get_prune_checkpoints()
    }
}

impl<N: NodeTypesWithDB> ChainSpecProvider for BlockchainProvider<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<N::ChainSpec> {
        self.database.chain_spec()
    }
}

impl<N: ProviderNodeTypes> StateProviderFactory for BlockchainProvider<N> {
    /// Storage provider for latest block
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", "Getting latest block state provider");
        // use latest state provider if the head state exists
        if let Some(state) = self.canonical_in_memory_state.head_state() {
            trace!(target: "providers::blockchain", "Using head state for latest state provider");
            Ok(self.block_state_provider(&state)?.boxed())
        } else {
            trace!(target: "providers::blockchain", "Using database state for latest state provider");
            self.database.latest()
        }
    }

    fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_number, "Getting history by block number");
        let provider = self.consistent_provider()?;
        provider.ensure_canonical_block(block_number)?;
        let hash = provider
            .block_hash(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        provider.into_state_provider_at_block_hash(hash)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");
        self.consistent_provider()?.into_state_provider_at_block_hash(block_hash)
    }

    fn state_by_block_hash(&self, hash: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?hash, "Getting state by block hash");
        if let Ok(state) = self.history_by_block_hash(hash) {
            // This could be tracked by a historical block
            Ok(state)
        } else if let Ok(Some(pending)) = self.pending_state_by_hash(hash) {
            // .. or this could be the pending state
            Ok(pending)
        } else {
            // if we couldn't find it anywhere, then we should return an error
            Err(ProviderError::StateForHashNotFound(hash))
        }
    }

    /// Returns the state provider for pending state.
    ///
    /// If there's no pending block available then the latest state provider is returned:
    /// [`Self::latest`]
    fn pending(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", "Getting provider for pending state");

        if let Some(pending) = self.canonical_in_memory_state.pending_state() {
            // we have a pending block
            return Ok(Box::new(self.block_state_provider(&pending)?));
        }

        // fallback to latest state if the pending block is not available
        self.latest()
    }

    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        if let Some(pending) = self.canonical_in_memory_state.pending_state() {
            if pending.hash() == block_hash {
                return Ok(Some(Box::new(self.block_state_provider(&pending)?)));
            }
        }
        Ok(None)
    }

    /// Returns a [`StateProviderBox`] indexed by the given block number or tag.
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
                self.state_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;
                self.state_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => {
                self.history_by_block_number(self.earliest_block_number()?)
            }
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => {
                let hash = self
                    .block_hash(num)?
                    .ok_or_else(|| ProviderError::HeaderNotFound(num.into()))?;
                self.state_by_block_hash(hash)
            }
        }
    }
}

impl<N: NodeTypesWithDB> HashedPostStateProvider for BlockchainProvider<N> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<<N::StateCommitment as StateCommitment>::KeyHasher>(
            bundle_state.state(),
        )
    }
}

impl<N: ProviderNodeTypes> CanonChainTracker for BlockchainProvider<N> {
    type Header = HeaderTy<N>;

    fn on_forkchoice_update_received(&self, _update: &ForkchoiceState) {
        // update timestamp
        self.canonical_in_memory_state.on_forkchoice_update_received();
    }

    fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.canonical_in_memory_state.last_received_update_timestamp()
    }

    fn set_canonical_head(&self, header: SealedHeader<Self::Header>) {
        self.canonical_in_memory_state.set_canonical_head(header);
    }

    fn set_safe(&self, header: SealedHeader<Self::Header>) {
        self.canonical_in_memory_state.set_safe(header);
    }

    fn set_finalized(&self, header: SealedHeader<Self::Header>) {
        self.canonical_in_memory_state.set_finalized(header);
    }
}

impl<N: ProviderNodeTypes> BlockReaderIdExt for BlockchainProvider<N>
where
    Self: ReceiptProviderIdExt,
{
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Block>> {
        self.consistent_provider()?.block_by_id(id)
    }

    fn header_by_number_or_tag(
        &self,
        id: BlockNumberOrTag,
    ) -> ProviderResult<Option<Self::Header>> {
        self.consistent_provider()?.header_by_number_or_tag(id)
    }

    fn sealed_header_by_number_or_tag(
        &self,
        id: BlockNumberOrTag,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.consistent_provider()?.sealed_header_by_number_or_tag(id)
    }

    fn sealed_header_by_id(
        &self,
        id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.consistent_provider()?.sealed_header_by_id(id)
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Header>> {
        self.consistent_provider()?.header_by_id(id)
    }
}

impl<N: ProviderNodeTypes> CanonStateSubscriptions for BlockchainProvider<N> {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<Self::Primitives> {
        self.canonical_in_memory_state.subscribe_canon_state()
    }
}

impl<N: ProviderNodeTypes> ForkChoiceSubscriptions for BlockchainProvider<N> {
    type Header = HeaderTy<N>;

    fn subscribe_safe_block(&self) -> ForkChoiceNotifications<Self::Header> {
        let receiver = self.canonical_in_memory_state.subscribe_safe_block();
        ForkChoiceNotifications(receiver)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications<Self::Header> {
        let receiver = self.canonical_in_memory_state.subscribe_finalized_block();
        ForkChoiceNotifications(receiver)
    }
}

impl<N: ProviderNodeTypes> StorageChangeSetReader for BlockchainProvider<N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        self.consistent_provider()?.storage_changeset(block_number)
    }
}

impl<N: ProviderNodeTypes> ChangeSetReader for BlockchainProvider<N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        self.consistent_provider()?.account_block_changeset(block_number)
    }
}

impl<N: ProviderNodeTypes> AccountReader for BlockchainProvider<N> {
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.consistent_provider()?.basic_account(address)
    }
}

impl<N: ProviderNodeTypes> StateReader for BlockchainProvider<N> {
    type Receipt = ReceiptTy<N>;

    /// Re-constructs the [`ExecutionOutcome`] from in-memory and database state, if necessary.
    ///
    /// If data for the block does not exist, this will return [`None`].
    ///
    /// NOTE: This cannot be called safely in a loop outside of the blockchain tree thread. This is
    /// because the [`CanonicalInMemoryState`] could change during a reorg, causing results to be
    /// inconsistent. Currently this can safely be called within the blockchain tree thread,
    /// because the tree thread is responsible for modifying the [`CanonicalInMemoryState`] in the
    /// first place.
    fn get_state(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>> {
        StateReader::get_state(&self.consistent_provider()?, block)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        providers::BlockchainProvider,
        test_utils::{
            create_test_provider_factory, create_test_provider_factory_with_chain_spec,
            MockNodeTypesWithDB,
        },
        writer::UnifiedStorageWriter,
        BlockWriter, CanonChainTracker, ProviderFactory, StaticFileProviderFactory,
        StaticFileWriter,
    };
    use alloy_eips::{BlockHashOrNumber, BlockNumHash, BlockNumberOrTag};
    use alloy_primitives::{BlockNumber, TxNumber, B256};
    use itertools::Itertools;
    use rand::Rng;
    use reth_chain_state::{
        test_utils::TestBlockBuilder, CanonStateNotification, CanonStateSubscriptions,
        CanonicalInMemoryState, ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates,
        NewCanonicalChain,
    };
    use reth_chainspec::{
        ChainSpec, ChainSpecBuilder, ChainSpecProvider, EthereumHardfork, MAINNET,
    };
    use reth_db_api::{
        cursor::DbCursorRO,
        models::{AccountBeforeTx, StoredBlockBodyIndices},
        tables,
        transaction::DbTx,
    };
    use reth_errors::ProviderError;
    use reth_ethereum_primitives::{Block, EthPrimitives, Receipt};
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_primitives_traits::{
        BlockBody, RecoveredBlock, SealedBlock, SignedTransaction, SignerRecoverable,
    };
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::{
        BlockBodyIndicesProvider, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader,
        BlockReaderIdExt, BlockSource, ChangeSetReader, DatabaseProviderFactory, HeaderProvider,
        ReceiptProvider, ReceiptProviderIdExt, StateProviderFactory, TransactionVariant,
        TransactionsProvider,
    };
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_changeset_range, random_eoa_accounts,
        random_receipt, BlockParams, BlockRangeParams,
    };
    use revm_database::BundleState;
    use std::{
        ops::{Bound, Deref, Range, RangeBounds},
        sync::Arc,
        time::Instant,
    };

    const TEST_BLOCKS_COUNT: usize = 5;

    const TEST_TRANSACTIONS_COUNT: u8 = 4;

    fn random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        requests_count: Option<Range<u8>>,
        withdrawals_count: Option<Range<u8>>,
        tx_count: impl RangeBounds<u8>,
    ) -> (Vec<SealedBlock<Block>>, Vec<SealedBlock<Block>>) {
        let block_range = (database_blocks + in_memory_blocks - 1) as u64;

        let tx_start = match tx_count.start_bound() {
            Bound::Included(&n) | Bound::Excluded(&n) => n,
            Bound::Unbounded => u8::MIN,
        };
        let tx_end = match tx_count.end_bound() {
            Bound::Included(&n) | Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => u8::MAX,
        };

        let blocks = random_block_range(
            rng,
            0..=block_range,
            BlockRangeParams {
                parent: Some(B256::ZERO),
                tx_count: tx_start..tx_end,
                requests_count,
                withdrawals_count,
            },
        );
        let (database_blocks, in_memory_blocks) = blocks.split_at(database_blocks);
        (database_blocks.to_vec(), in_memory_blocks.to_vec())
    }

    #[expect(clippy::type_complexity)]
    fn provider_with_chain_spec_and_random_blocks(
        rng: &mut impl Rng,
        chain_spec: Arc<ChainSpec>,
        database_blocks: usize,
        in_memory_blocks: usize,
        block_range_params: BlockRangeParams,
    ) -> eyre::Result<(
        BlockchainProvider<MockNodeTypesWithDB>,
        Vec<SealedBlock<Block>>,
        Vec<SealedBlock<Block>>,
        Vec<Vec<Receipt>>,
    )> {
        let (database_blocks, in_memory_blocks) = random_blocks(
            rng,
            database_blocks,
            in_memory_blocks,
            block_range_params.requests_count,
            block_range_params.withdrawals_count,
            block_range_params.tx_count,
        );

        let receipts: Vec<Vec<_>> = database_blocks
            .iter()
            .chain(in_memory_blocks.iter())
            .map(|block| block.body().transactions.iter())
            .map(|tx| tx.map(|tx| random_receipt(rng, tx, Some(2), None)).collect())
            .collect();

        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let provider_rw = factory.database_provider_rw()?;
        let static_file_provider = factory.static_file_provider();

        // Write transactions to static files with the right `tx_num``
        let mut tx_num = provider_rw
            .block_body_indices(database_blocks.first().as_ref().unwrap().number.saturating_sub(1))?
            .map(|indices| indices.next_tx_num())
            .unwrap_or_default();

        // Insert blocks into the database
        for (block, receipts) in database_blocks.iter().zip(&receipts) {
            // TODO: this should be moved inside `insert_historical_block`: <https://github.com/paradigmxyz/reth/issues/11524>
            let mut transactions_writer =
                static_file_provider.latest_writer(StaticFileSegment::Transactions)?;
            let mut receipts_writer =
                static_file_provider.latest_writer(StaticFileSegment::Receipts)?;
            transactions_writer.increment_block(block.number)?;
            receipts_writer.increment_block(block.number)?;

            for (tx, receipt) in block.body().transactions().zip(receipts) {
                transactions_writer.append_transaction(tx_num, tx)?;
                receipts_writer.append_receipt(tx_num, receipt)?;
                tx_num += 1;
            }

            provider_rw.insert_historical_block(
                block.clone().try_recover().expect("failed to seal block with senders"),
            )?;
        }

        // Commit to both storages: database and static files
        UnifiedStorageWriter::commit(provider_rw)?;

        let provider = BlockchainProvider::new(factory)?;

        // Insert the rest of the blocks and receipts into the in-memory state
        let chain = NewCanonicalChain::Commit {
            new: in_memory_blocks
                .iter()
                .map(|block| {
                    let senders = block.senders().expect("failed to recover senders");
                    let block_receipts = receipts.get(block.number as usize).unwrap().clone();
                    let execution_outcome =
                        ExecutionOutcome { receipts: vec![block_receipts], ..Default::default() };

                    ExecutedBlockWithTrieUpdates::new(
                        Arc::new(RecoveredBlock::new_sealed(block.clone(), senders)),
                        execution_outcome.into(),
                        Default::default(),
                        ExecutedTrieUpdates::empty(),
                    )
                })
                .collect(),
        };
        provider.canonical_in_memory_state.update_chain(chain);

        // Get canonical, safe, and finalized blocks
        let blocks = database_blocks.iter().chain(in_memory_blocks.iter()).collect::<Vec<_>>();
        let block_count = blocks.len();
        let canonical_block = blocks.get(block_count - 1).unwrap();
        let safe_block = blocks.get(block_count - 2).unwrap();
        let finalized_block = blocks.get(block_count - 3).unwrap();

        // Set the canonical head, safe, and finalized blocks
        provider.set_canonical_head(canonical_block.clone_sealed_header());
        provider.set_safe(safe_block.clone_sealed_header());
        provider.set_finalized(finalized_block.clone_sealed_header());

        Ok((provider, database_blocks.clone(), in_memory_blocks.clone(), receipts))
    }

    #[expect(clippy::type_complexity)]
    fn provider_with_random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        block_range_params: BlockRangeParams,
    ) -> eyre::Result<(
        BlockchainProvider<MockNodeTypesWithDB>,
        Vec<SealedBlock<Block>>,
        Vec<SealedBlock<Block>>,
        Vec<Vec<Receipt>>,
    )> {
        provider_with_chain_spec_and_random_blocks(
            rng,
            MAINNET.clone(),
            database_blocks,
            in_memory_blocks,
            block_range_params,
        )
    }

    /// This will persist the last block in-memory and delete it from
    /// `canonical_in_memory_state` right after a database read transaction is created.
    ///
    /// This simulates a RPC method having a different view than when its database transaction was
    /// created.
    fn persist_block_after_db_tx_creation(
        provider: BlockchainProvider<MockNodeTypesWithDB>,
        block_number: BlockNumber,
    ) {
        let hook_provider = provider.clone();
        provider.database.db_ref().set_post_transaction_hook(Box::new(move || {
            if let Some(state) = hook_provider.canonical_in_memory_state.head_state() {
                if state.anchor().number + 1 == block_number {
                    let mut lowest_memory_block =
                        state.parent_state_chain().last().expect("qed").block();
                    let num_hash = lowest_memory_block.recovered_block().num_hash();

                    let mut execution_output = (*lowest_memory_block.execution_output).clone();
                    execution_output.first_block = lowest_memory_block.recovered_block().number;
                    lowest_memory_block.execution_output = Arc::new(execution_output);

                    // Push to disk
                    let provider_rw = hook_provider.database_provider_rw().unwrap();
                    UnifiedStorageWriter::from(&provider_rw, &hook_provider.static_file_provider())
                        .save_blocks(vec![lowest_memory_block])
                        .unwrap();
                    UnifiedStorageWriter::commit(provider_rw).unwrap();

                    // Remove from memory
                    hook_provider.canonical_in_memory_state.remove_persisted_blocks(num_hash);
                }
            }
        }));
    }

    #[test]
    fn test_block_reader_find_block_by_hash() -> eyre::Result<()> {
        // Initialize random number generator and provider factory
        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks and split into database and in-memory blocks
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        let (database_blocks, in_memory_blocks) = blocks.split_at(5);

        // Insert first 5 blocks into the database
        let provider_rw = factory.provider_rw()?;
        for block in database_blocks {
            provider_rw.insert_historical_block(
                block.clone().try_recover().expect("failed to seal block with senders"),
            )?;
        }
        provider_rw.commit()?;

        // Create a new provider
        let provider = BlockchainProvider::new(factory)?;

        // Useful blocks
        let first_db_block = database_blocks.first().unwrap();
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let last_in_mem_block = in_memory_blocks.last().unwrap();

        // No block in memory before setting in memory state
        assert_eq!(provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Any)?, None);
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Canonical)?,
            None
        );
        // No pending block in memory
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Pending)?,
            None
        );

        // Insert first block into the in-memory state
        let in_memory_block_senders =
            first_in_mem_block.senders().expect("failed to recover senders");
        let chain = NewCanonicalChain::Commit {
            new: vec![ExecutedBlockWithTrieUpdates::new(
                Arc::new(RecoveredBlock::new_sealed(
                    first_in_mem_block.clone(),
                    in_memory_block_senders,
                )),
                Default::default(),
                Default::default(),
                ExecutedTrieUpdates::empty(),
            )],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        // Now the block should be found in memory
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Any)?,
            Some(first_in_mem_block.clone().into_block())
        );
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Canonical)?,
            Some(first_in_mem_block.clone().into_block())
        );

        // Find the first block in database by hash
        assert_eq!(
            provider.find_block_by_hash(first_db_block.hash(), BlockSource::Any)?,
            Some(first_db_block.clone().into_block())
        );
        assert_eq!(
            provider.find_block_by_hash(first_db_block.hash(), BlockSource::Canonical)?,
            Some(first_db_block.clone().into_block())
        );

        // No pending block in database
        assert_eq!(provider.find_block_by_hash(first_db_block.hash(), BlockSource::Pending)?, None);

        // Insert the last block into the pending state
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(
                    last_in_mem_block.clone(),
                    Default::default(),
                )),
                execution_output: Default::default(),
                hashed_state: Default::default(),
            },
            trie: ExecutedTrieUpdates::empty(),
        });

        // Now the last block should be found in memory
        assert_eq!(
            provider.find_block_by_hash(last_in_mem_block.hash(), BlockSource::Pending)?,
            Some(last_in_mem_block.clone().into_block())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_block() -> eyre::Result<()> {
        // Initialize random number generator and provider factory
        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks and split into database and in-memory blocks
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        let (database_blocks, in_memory_blocks) = blocks.split_at(5);

        // Insert first 5 blocks into the database
        let provider_rw = factory.provider_rw()?;
        for block in database_blocks {
            provider_rw.insert_historical_block(
                block.clone().try_recover().expect("failed to seal block with senders"),
            )?;
        }
        provider_rw.commit()?;

        // Create a new provider
        let provider = BlockchainProvider::new(factory)?;

        // First in memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        // First database block
        let first_db_block = database_blocks.first().unwrap();

        // First in memory block should not be found yet as not integrated to the in-memory state
        assert_eq!(provider.block(BlockHashOrNumber::Hash(first_in_mem_block.hash()))?, None);
        assert_eq!(provider.block(BlockHashOrNumber::Number(first_in_mem_block.number))?, None);

        // Insert first block into the in-memory state
        let in_memory_block_senders =
            first_in_mem_block.senders().expect("failed to recover senders");
        let chain = NewCanonicalChain::Commit {
            new: vec![ExecutedBlockWithTrieUpdates::new(
                Arc::new(RecoveredBlock::new_sealed(
                    first_in_mem_block.clone(),
                    in_memory_block_senders,
                )),
                Default::default(),
                Default::default(),
                ExecutedTrieUpdates::empty(),
            )],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        // First in memory block should be found
        assert_eq!(
            provider.block(BlockHashOrNumber::Hash(first_in_mem_block.hash()))?,
            Some(first_in_mem_block.clone().into_block())
        );
        assert_eq!(
            provider.block(BlockHashOrNumber::Number(first_in_mem_block.number))?,
            Some(first_in_mem_block.clone().into_block())
        );

        // First database block should be found
        assert_eq!(
            provider.block(BlockHashOrNumber::Hash(first_db_block.hash()))?,
            Some(first_db_block.clone().into_block())
        );
        assert_eq!(
            provider.block(BlockHashOrNumber::Number(first_db_block.number))?,
            Some(first_db_block.clone().into_block())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_pending_block() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Generate a random block
        let mut rng = generators::rng();
        let block = random_block(
            &mut rng,
            0,
            BlockParams { parent: Some(B256::ZERO), ..Default::default() },
        );

        // Set the block as pending
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(
                    block.clone(),
                    block.senders().unwrap(),
                )),
                execution_output: Default::default(),
                hashed_state: Default::default(),
            },
            trie: ExecutedTrieUpdates::empty(),
        });

        // Assertions related to the pending block

        assert_eq!(
            provider.pending_block()?,
            Some(RecoveredBlock::new_sealed(block.clone(), block.senders().unwrap()))
        );

        assert_eq!(
            provider.pending_block_and_receipts()?,
            Some((RecoveredBlock::new_sealed(block.clone(), block.senders().unwrap()), vec![]))
        );

        Ok(())
    }

    #[test]
    fn test_block_body_indices() -> eyre::Result<()> {
        // Create a new provider
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams {
                tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                ..Default::default()
            },
        )?;

        let first_in_mem_block = in_memory_blocks.first().unwrap();

        // Insert the first block into the in-memory state
        let in_memory_block_senders =
            first_in_mem_block.senders().expect("failed to recover senders");
        let chain = NewCanonicalChain::Commit {
            new: vec![ExecutedBlockWithTrieUpdates::new(
                Arc::new(RecoveredBlock::new_sealed(
                    first_in_mem_block.clone(),
                    in_memory_block_senders,
                )),
                Default::default(),
                Default::default(),
                ExecutedTrieUpdates::empty(),
            )],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        let first_db_block = database_blocks.first().unwrap().clone();
        let first_in_mem_block = in_memory_blocks.first().unwrap().clone();

        // First database block body indices should be found
        assert_eq!(
            provider.block_body_indices(first_db_block.number)?.unwrap(),
            StoredBlockBodyIndices { first_tx_num: 0, tx_count: 4 }
        );

        // First in-memory block body indices should be found with the first tx after the database
        // blocks
        assert_eq!(
            provider.block_body_indices(first_in_mem_block.number)?.unwrap(),
            StoredBlockBodyIndices { first_tx_num: 20, tx_count: 4 }
        );

        // A random block number should return None as the block is not found
        let mut rng = rand::rng();
        let random_block_number: u64 = rng.random();
        assert_eq!(provider.block_body_indices(random_block_number)?, None);

        Ok(())
    }

    #[test]
    fn test_block_hash_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        assert_eq!(provider.block_hash(database_block.number)?, Some(database_block.hash()));
        assert_eq!(provider.block_hash(in_memory_block.number)?, Some(in_memory_block.hash()));

        assert_eq!(
            provider.canonical_hashes_range(0, 10)?,
            [database_blocks, in_memory_blocks]
                .concat()
                .iter()
                .map(|block| block.hash())
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_header_provider() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();
        // make sure that the finalized block is on db
        let finalized_block = database_blocks.get(database_blocks.len() - 3).unwrap();
        provider.set_finalized(finalized_block.clone_sealed_header());

        let blocks = [database_blocks, in_memory_blocks].concat();

        assert_eq!(
            provider.header_td_by_number(database_block.number)?,
            Some(database_block.difficulty)
        );

        assert_eq!(
            provider.header_td_by_number(in_memory_block.number)?,
            Some(in_memory_block.difficulty)
        );

        assert_eq!(
            provider.sealed_headers_while(0..=10, |header| header.number <= 8)?,
            blocks
                .iter()
                .take_while(|header| header.number <= 8)
                .map(|b| b.clone_sealed_header())
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_canon_state_subscriptions() -> eyre::Result<()> {
        let factory = create_test_provider_factory();

        // Generate a random block to initialise the blockchain provider.
        let mut test_block_builder = TestBlockBuilder::eth();
        let block_1 = test_block_builder.generate_random_block(0, B256::ZERO);
        let block_hash_1 = block_1.hash();

        // Insert and commit the block.
        let provider_rw = factory.provider_rw()?;
        provider_rw.insert_historical_block(block_1)?;
        provider_rw.commit()?;

        let provider = BlockchainProvider::new(factory)?;

        // Subscribe twice for canonical state updates.
        let in_memory_state = provider.canonical_in_memory_state();
        let mut rx_1 = provider.subscribe_to_canonical_state();
        let mut rx_2 = provider.subscribe_to_canonical_state();

        // Send and receive commit notifications.
        let block_2 = test_block_builder.generate_random_block(1, block_hash_1);
        let chain = Chain::new(vec![block_2], ExecutionOutcome::default(), None);
        let commit = CanonStateNotification::Commit { new: Arc::new(chain.clone()) };
        in_memory_state.notify_canon_state(commit.clone());
        let (notification_1, notification_2) = tokio::join!(rx_1.recv(), rx_2.recv());
        assert_eq!(notification_1, Ok(commit.clone()));
        assert_eq!(notification_2, Ok(commit.clone()));

        // Send and receive re-org notifications.
        let block_3 = test_block_builder.generate_random_block(1, block_hash_1);
        let block_4 = test_block_builder.generate_random_block(2, block_3.hash());
        let new_chain = Chain::new(vec![block_3, block_4], ExecutionOutcome::default(), None);
        let re_org =
            CanonStateNotification::Reorg { old: Arc::new(chain), new: Arc::new(new_chain) };
        in_memory_state.notify_canon_state(re_org.clone());
        let (notification_1, notification_2) = tokio::join!(rx_1.recv(), rx_2.recv());
        assert_eq!(notification_1, Ok(re_org.clone()));
        assert_eq!(notification_2, Ok(re_org.clone()));

        Ok(())
    }

    #[test]
    fn test_block_num_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        assert_eq!(provider.best_block_number()?, in_memory_blocks.last().unwrap().number);
        assert_eq!(provider.last_block_number()?, database_blocks.last().unwrap().number);

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.first().unwrap().clone();
        assert_eq!(provider.block_number(database_block.hash())?, Some(database_block.number));
        assert_eq!(provider.block_number(in_memory_block.hash())?, Some(in_memory_block.number));

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_block_by_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.hash();

        assert_eq!(
            provider.block_by_id(block_number.into()).unwrap(),
            Some(database_block.clone().into_block())
        );
        assert_eq!(
            provider.block_by_id(block_hash.into()).unwrap(),
            Some(database_block.into_block())
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.hash();
        assert_eq!(
            provider.block_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.clone().into_block())
        );
        assert_eq!(
            provider.block_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.into_block())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_header_by_number_or_tag() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();

        let in_memory_block_count = in_memory_blocks.len();
        let canonical_block = in_memory_blocks.get(in_memory_block_count - 1).unwrap().clone();
        let safe_block = in_memory_blocks.get(in_memory_block_count - 2).unwrap().clone();
        let finalized_block = in_memory_blocks.get(in_memory_block_count - 3).unwrap().clone();

        let block_number = database_block.number;
        assert_eq!(
            provider.header_by_number_or_tag(block_number.into()).unwrap(),
            Some(database_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(block_number.into())?,
            Some(database_block.clone_sealed_header())
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Latest).unwrap(),
            Some(canonical_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest).unwrap(),
            Some(canonical_block.clone_sealed_header())
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Safe).unwrap(),
            Some(safe_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Safe).unwrap(),
            Some(safe_block.clone_sealed_header())
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Finalized).unwrap(),
            Some(finalized_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Finalized).unwrap(),
            Some(finalized_block.clone_sealed_header())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_header_by_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.hash();

        assert_eq!(
            provider.header_by_id(block_number.into()).unwrap(),
            Some(database_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_number.into()).unwrap(),
            Some(database_block.clone_sealed_header())
        );

        assert_eq!(
            provider.header_by_id(block_hash.into()).unwrap(),
            Some(database_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_hash.into()).unwrap(),
            Some(database_block.clone_sealed_header())
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.hash();

        assert_eq!(
            provider.header_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.clone_sealed_header())
        );

        assert_eq!(
            provider.header_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.header().clone())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.clone_sealed_header())
        );

        Ok(())
    }

    #[test]
    fn test_receipt_provider_id_ext_receipts_by_block_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams { tx_count: 1..3, ..Default::default() },
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.hash();

        assert!(!receipts.get(database_block.number as usize).unwrap().is_empty());
        assert!(!provider
            .receipts_by_number_or_tag(database_block.number.into())?
            .unwrap()
            .is_empty());

        assert_eq!(
            provider.receipts_by_block_id(block_number.into())?.unwrap(),
            receipts.get(block_number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_block_id(block_hash.into())?.unwrap(),
            receipts.get(block_number as usize).unwrap().clone()
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.hash();

        assert_eq!(
            provider.receipts_by_block_id(block_number.into())?.unwrap(),
            receipts.get(block_number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_block_id(block_hash.into())?.unwrap(),
            receipts.get(block_number as usize).unwrap().clone()
        );

        Ok(())
    }

    #[test]
    fn test_receipt_provider_id_ext_receipts_by_block_number_or_tag() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams { tx_count: 1..3, ..Default::default() },
        )?;

        let database_block = database_blocks.first().unwrap().clone();

        let in_memory_block_count = in_memory_blocks.len();
        let canonical_block = in_memory_blocks.get(in_memory_block_count - 1).unwrap().clone();
        let safe_block = in_memory_blocks.get(in_memory_block_count - 2).unwrap().clone();
        let finalized_block = in_memory_blocks.get(in_memory_block_count - 3).unwrap().clone();

        assert!(!receipts.get(database_block.number as usize).unwrap().is_empty());
        assert!(!provider
            .receipts_by_number_or_tag(database_block.number.into())?
            .unwrap()
            .is_empty());

        assert_eq!(
            provider.receipts_by_number_or_tag(database_block.number.into())?.unwrap(),
            receipts.get(database_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap(),
            receipts.get(canonical_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Safe)?.unwrap(),
            receipts.get(safe_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Finalized)?.unwrap(),
            receipts.get(finalized_block.number as usize).unwrap().clone()
        );

        Ok(())
    }

    #[test]
    fn test_changeset_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let (database_blocks, in_memory_blocks) =
            random_blocks(&mut rng, TEST_BLOCKS_COUNT, 1, None, None, 0..1);

        let first_database_block = database_blocks.first().map(|block| block.number).unwrap();
        let last_database_block = database_blocks.last().map(|block| block.number).unwrap();
        let first_in_memory_block = in_memory_blocks.first().map(|block| block.number).unwrap();

        let accounts = random_eoa_accounts(&mut rng, 2);

        let (database_changesets, database_state) = random_changeset_range(
            &mut rng,
            &database_blocks,
            accounts.into_iter().map(|(address, account)| (address, (account, Vec::new()))),
            0..0,
            0..0,
        );
        let (in_memory_changesets, in_memory_state) = random_changeset_range(
            &mut rng,
            &in_memory_blocks,
            database_state
                .iter()
                .map(|(address, (account, storage))| (*address, (*account, storage.clone()))),
            0..0,
            0..0,
        );

        let factory = create_test_provider_factory();

        let provider_rw = factory.provider_rw()?;
        provider_rw.append_blocks_with_state(
            database_blocks
                .into_iter()
                .map(|b| b.try_recover().expect("failed to seal block with senders"))
                .collect(),
            &ExecutionOutcome {
                bundle: BundleState::new(
                    database_state.into_iter().map(|(address, (account, _))| {
                        (address, None, Some(account.into()), Default::default())
                    }),
                    database_changesets
                        .iter()
                        .map(|block_changesets| {
                            block_changesets.iter().map(|(address, account, _)| {
                                (*address, Some(Some((*account).into())), [])
                            })
                        })
                        .collect::<Vec<_>>(),
                    Vec::new(),
                ),
                first_block: first_database_block,
                ..Default::default()
            },
            Default::default(),
            Default::default(),
        )?;
        provider_rw.commit()?;

        let provider = BlockchainProvider::new(factory)?;

        let in_memory_changesets = in_memory_changesets.into_iter().next().unwrap();
        let chain = NewCanonicalChain::Commit {
            new: vec![in_memory_blocks
                .first()
                .map(|block| {
                    let senders = block.senders().expect("failed to recover senders");
                    ExecutedBlockWithTrieUpdates::new(
                        Arc::new(RecoveredBlock::new_sealed(block.clone(), senders)),
                        Arc::new(ExecutionOutcome {
                            bundle: BundleState::new(
                                in_memory_state.into_iter().map(|(address, (account, _))| {
                                    (address, None, Some(account.into()), Default::default())
                                }),
                                [in_memory_changesets.iter().map(|(address, account, _)| {
                                    (*address, Some(Some((*account).into())), Vec::new())
                                })],
                                [],
                            ),
                            first_block: first_in_memory_block,
                            ..Default::default()
                        }),
                        Default::default(),
                        ExecutedTrieUpdates::empty(),
                    )
                })
                .unwrap()],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        assert_eq!(
            provider.account_block_changeset(last_database_block).unwrap(),
            database_changesets
                .into_iter()
                .next_back()
                .unwrap()
                .into_iter()
                .sorted_by_key(|(address, _, _)| *address)
                .map(|(address, account, _)| AccountBeforeTx { address, info: Some(account) })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            provider.account_block_changeset(first_in_memory_block).unwrap(),
            in_memory_changesets
                .into_iter()
                .sorted_by_key(|(address, _, _)| *address)
                .map(|(address, account, _)| AccountBeforeTx { address, info: Some(account) })
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_state_provider_factory() -> eyre::Result<()> {
        let mut rng = generators::rng();

        // test in-memory state use-cases
        let (in_memory_provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // test database state use-cases
        let (only_database_provider, database_blocks, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            0,
            BlockRangeParams::default(),
        )?;

        let blocks = [database_blocks.clone(), in_memory_blocks.clone()].concat();
        let first_in_memory_block = in_memory_blocks.first().unwrap();
        let first_db_block = database_blocks.first().unwrap();

        // test latest state
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider.latest().unwrap().block_hash(first_in_memory_block.number)?.unwrap()
        );
        // test latest falls back to database state when there's no in-memory block
        assert_eq!(
            first_db_block.hash(),
            only_database_provider.latest().unwrap().block_hash(first_db_block.number)?.unwrap()
        );

        // test history by block number
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .history_by_block_number(first_in_memory_block.number)?
                .block_hash(first_in_memory_block.number)?
                .unwrap()
        );
        assert_eq!(
            first_db_block.hash(),
            only_database_provider
                .history_by_block_number(first_db_block.number)?
                .block_hash(first_db_block.number)?
                .unwrap()
        );
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .history_by_block_hash(first_in_memory_block.hash())?
                .block_hash(first_in_memory_block.number)?
                .unwrap()
        );
        assert!(only_database_provider.history_by_block_hash(B256::random()).is_err());

        // test state by block hash
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .state_by_block_hash(first_in_memory_block.hash())?
                .block_hash(first_in_memory_block.number)?
                .unwrap()
        );
        assert_eq!(
            first_db_block.hash(),
            only_database_provider
                .state_by_block_hash(first_db_block.hash())?
                .block_hash(first_db_block.number)?
                .unwrap()
        );
        assert!(only_database_provider.state_by_block_hash(B256::random()).is_err());

        // test pending without pending state- falls back to latest
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .pending()
                .unwrap()
                .block_hash(first_in_memory_block.number)
                .unwrap()
                .unwrap()
        );

        // adding a pending block to state can test pending() and  pending_state_by_hash() function
        let pending_block = database_blocks[database_blocks.len() - 1].clone();
        only_database_provider.canonical_in_memory_state.set_pending_block(
            ExecutedBlockWithTrieUpdates {
                block: ExecutedBlock {
                    recovered_block: Arc::new(RecoveredBlock::new_sealed(
                        pending_block.clone(),
                        Default::default(),
                    )),
                    execution_output: Default::default(),
                    hashed_state: Default::default(),
                },
                trie: ExecutedTrieUpdates::empty(),
            },
        );

        assert_eq!(
            pending_block.hash(),
            only_database_provider
                .pending()
                .unwrap()
                .block_hash(pending_block.number)
                .unwrap()
                .unwrap()
        );

        assert_eq!(
            pending_block.hash(),
            only_database_provider
                .pending_state_by_hash(pending_block.hash())?
                .unwrap()
                .block_hash(pending_block.number)?
                .unwrap()
        );

        // test state by block number or tag
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Number(
                    first_in_memory_block.number
                ))?
                .block_hash(first_in_memory_block.number)?
                .unwrap()
        );
        assert_eq!(
            first_in_memory_block.hash(),
            in_memory_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Latest)?
                .block_hash(first_in_memory_block.number)?
                .unwrap()
        );
        // test state by block tag for safe block
        let safe_block = in_memory_blocks[in_memory_blocks.len() - 2].clone();
        in_memory_provider.canonical_in_memory_state.set_safe(safe_block.clone_sealed_header());
        assert_eq!(
            safe_block.hash(),
            in_memory_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Safe)?
                .block_hash(safe_block.number)?
                .unwrap()
        );
        // test state by block tag for finalized block
        let finalized_block = in_memory_blocks[in_memory_blocks.len() - 3].clone();
        in_memory_provider
            .canonical_in_memory_state
            .set_finalized(finalized_block.clone_sealed_header());
        assert_eq!(
            finalized_block.hash(),
            in_memory_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Finalized)?
                .block_hash(finalized_block.number)?
                .unwrap()
        );
        // test state by block tag for earliest block
        let earliest_block = blocks.first().unwrap().clone();
        assert_eq!(
            earliest_block.hash(),
            only_database_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Earliest)?
                .block_hash(earliest_block.number)?
                .unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_block_id_reader() -> eyre::Result<()> {
        // Create a new provider
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Set the pending block in memory
        let pending_block = in_memory_blocks.last().unwrap();
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(
                    pending_block.clone(),
                    Default::default(),
                )),
                execution_output: Default::default(),
                hashed_state: Default::default(),
            },
            trie: ExecutedTrieUpdates::empty(),
        });

        // Set the safe block in memory
        let safe_block = in_memory_blocks[in_memory_blocks.len() - 2].clone();
        provider.canonical_in_memory_state.set_safe(safe_block.clone_sealed_header());

        // Set the finalized block in memory
        let finalized_block = in_memory_blocks[in_memory_blocks.len() - 3].clone();
        provider.canonical_in_memory_state.set_finalized(finalized_block.clone_sealed_header());

        // Verify the pending block number and hash
        assert_eq!(
            provider.pending_block_num_hash()?,
            Some(BlockNumHash { number: pending_block.number, hash: pending_block.hash() })
        );

        // Verify the safe block number and hash
        assert_eq!(
            provider.safe_block_num_hash()?,
            Some(BlockNumHash { number: safe_block.number, hash: safe_block.hash() })
        );

        // Verify the finalized block number and hash
        assert_eq!(
            provider.finalized_block_num_hash()?,
            Some(BlockNumHash { number: finalized_block.number, hash: finalized_block.hash() })
        );

        Ok(())
    }

    macro_rules! test_by_tx_range {
        ([$(($method:ident, $data_extractor:expr)),* $(,)?]) => {{

            // Get the number methods being tested.
            // Since each method tested will move a block from memory to storage, this ensures we have enough.
            let extra_blocks = [$(stringify!($method)),*].len();

            let mut rng = generators::rng();
            let (provider, mut database_blocks, mut in_memory_blocks, receipts) = provider_with_random_blocks(
                &mut rng,
                TEST_BLOCKS_COUNT,
                TEST_BLOCKS_COUNT + extra_blocks,
                BlockRangeParams {
                    tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                    ..Default::default()
                },
            )?;

            $(
                // Since data moves for each tried method, need to recalculate everything
                let db_tx_count =
                    database_blocks.iter().map(|b| b.transaction_count()).sum::<usize>() as u64;
                let in_mem_tx_count =
                    in_memory_blocks.iter().map(|b| b.transaction_count()).sum::<usize>() as u64;

                let db_range = 0..=(db_tx_count - 1);
                let in_mem_range = db_tx_count..=(in_mem_tx_count + db_range.end());

                // Retrieve the expected database data
                let database_data =
                    database_blocks.iter().flat_map(|b| $data_extractor(b, &receipts)).collect::<Vec<_>>();
                assert_eq!(provider.$method(db_range.clone())?, database_data, "full db data");

                // Retrieve the expected in-memory data
                let in_memory_data =
                    in_memory_blocks.iter().flat_map(|b| $data_extractor(b, &receipts)).collect::<Vec<_>>();
                assert_eq!(provider.$method(in_mem_range.clone())?, in_memory_data, "full mem data");

                // Test partial in-memory range
                assert_eq!(
                    &provider.$method(in_mem_range.start() + 1..=in_mem_range.end() - 1)?,
                    &in_memory_data[1..in_memory_data.len() - 1],
                    "partial mem data"
                );

                // Test range in in-memory to unbounded end
                assert_eq!(provider.$method(in_mem_range.start() + 1..)?, &in_memory_data[1..], "unbounded mem data");

                // Test last element in-memory
                assert_eq!(provider.$method(in_mem_range.end()..)?, &in_memory_data[in_memory_data.len() -1 ..], "last mem data");

                // Test range that spans database and in-memory with unbounded end
                assert_eq!(
                    provider.$method(in_mem_range.start() - 2..)?,
                    database_data[database_data.len() - 2..]
                        .iter()
                        .chain(&in_memory_data[..])
                        .cloned()
                        .collect::<Vec<_>>(),
                    "unbounded span data"
                );

                // Test range that spans database and in-memory
                {
                    // This block will be persisted to disk and removed from memory AFTER the first database query. This ensures that we query the in-memory state before the database avoiding any race condition.
                    persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[0].number);

                    assert_eq!(
                        provider.$method(in_mem_range.start() - 2..=in_mem_range.end() - 1)?,
                        database_data[database_data.len() - 2..]
                            .iter()
                            .chain(&in_memory_data[..in_memory_data.len() - 1])
                            .cloned()
                            .collect::<Vec<_>>(),
                        "span data"
                    );

                    // Adjust our blocks accordingly
                    database_blocks.push(in_memory_blocks.remove(0));
                }

                // Test invalid range
                let start_tx_num = u64::MAX;
                let end_tx_num = u64::MAX;
                let result = provider.$method(start_tx_num..end_tx_num)?;
                assert!(result.is_empty(), "No data should be found for an invalid transaction range");

                // Test empty range
                let result = provider.$method(in_mem_range.end()+10..in_mem_range.end()+20)?;
                assert!(result.is_empty(), "No data should be found for an empty transaction range");
            )*
        }};
    }

    #[test]
    fn test_methods_by_tx_range() -> eyre::Result<()> {
        test_by_tx_range!([
            (senders_by_tx_range, |block: &SealedBlock<Block>, _: &Vec<Vec<Receipt>>| block
                .senders()
                .unwrap()),
            (transactions_by_tx_range, |block: &SealedBlock<Block>, _: &Vec<Vec<Receipt>>| block
                .body()
                .transactions
                .clone()),
            (receipts_by_tx_range, |block: &SealedBlock<Block>, receipts: &Vec<Vec<Receipt>>| {
                receipts[block.number as usize].clone()
            })
        ]);

        Ok(())
    }

    macro_rules! test_by_block_range {
        ([$(($method:ident, $data_extractor:expr)),* $(,)?]) => {{
            // Get the number methods being tested.
            // Since each method tested will move a block from memory to storage, this ensures we have enough.
            let extra_blocks = [$(stringify!($method)),*].len();

            let mut rng = generators::rng();
            let (provider, mut database_blocks, mut in_memory_blocks, _) = provider_with_random_blocks(
                &mut rng,
                TEST_BLOCKS_COUNT,
                TEST_BLOCKS_COUNT + extra_blocks,
                BlockRangeParams {
                    tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                    ..Default::default()
                },
            )?;

            $(
                // Since data moves for each tried method, need to recalculate everything
                let db_block_count = database_blocks.len() as u64;
                let in_mem_block_count = in_memory_blocks.len() as u64;

                let db_range = 0..=db_block_count - 1;
                let in_mem_range = db_block_count..=(in_mem_block_count + db_range.end());

                // Retrieve the expected database data
                let database_data =
                    database_blocks.iter().map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!(provider.$method(db_range.clone())?, database_data);

                // Retrieve the expected in-memory data
                let in_memory_data =
                    in_memory_blocks.iter().map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!(provider.$method(in_mem_range.clone())?, in_memory_data);

                // Test partial in-memory range
                assert_eq!(
                    &provider.$method(in_mem_range.start() + 1..=in_mem_range.end() - 1)?,
                    &in_memory_data[1..in_memory_data.len() - 1]
                );

                // Test range that spans database and in-memory
                {

                    // This block will be persisted to disk and removed from memory AFTER the first database query. This ensures that we query the in-memory state before the database avoiding any race condition.
                    persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[0].number);

                    assert_eq!(
                        provider.$method(in_mem_range.start() - 2..=in_mem_range.end() - 1)?,
                        database_data[database_data.len() - 2..]
                            .iter()
                            .chain(&in_memory_data[..in_memory_data.len() - 1])
                            .cloned()
                            .collect::<Vec<_>>()
                    );

                    // Adjust our blocks accordingly
                    database_blocks.push(in_memory_blocks.remove(0));
                }

                // Test invalid range
                let start_block_num = u64::MAX;
                let end_block_num = u64::MAX;
                let result = provider.$method(start_block_num..=end_block_num-1)?;
                assert!(result.is_empty(), "No data should be found for an invalid block range");

                // Test valid range with empty results
                let result = provider.$method(in_mem_range.end() + 10..=in_mem_range.end() + 20)?;
                assert!(result.is_empty(), "No data should be found for an empty block range");
            )*
        }};
    }

    #[test]
    fn test_methods_by_block_range() -> eyre::Result<()> {
        // todo(joshie) add canonical_hashes_range below after changing its interface into range
        // instead start end
        test_by_block_range!([
            (headers_range, |block: &SealedBlock<Block>| block.header().clone()),
            (sealed_headers_range, |block: &SealedBlock<Block>| block.clone_sealed_header()),
            (block_range, |block: &SealedBlock<Block>| block.clone().into_block()),
            (block_with_senders_range, |block: &SealedBlock<Block>| block
                .clone()
                .try_recover()
                .unwrap()),
            (recovered_block_range, |block: &SealedBlock<Block>| block
                .clone()
                .try_recover()
                .unwrap()),
            (transactions_by_block_range, |block: &SealedBlock<Block>| block
                .body()
                .transactions
                .clone()),
        ]);

        Ok(())
    }

    /// Helper macro to call a provider method based on argument count and check its result
    macro_rules! call_method {
        ($provider:expr, $method:ident, ($($args:expr),*), $expected_item:expr) => {{
            let result = $provider.$method($($args),*)?;
            assert_eq!(
                result,
                $expected_item,
                "{}: item does not match the expected item for arguments {:?}",
                stringify!($method),
                ($($args),*)
            );
        }};

        // Handle valid or invalid arguments for one argument
        (ONE, $provider:expr, $method:ident, $item_extractor:expr, $txnum:expr, $txhash:expr, $block:expr, $receipts:expr) => {{
            let (arg, expected_item) = $item_extractor($block, $txnum($block), $txhash($block), $receipts);
            call_method!($provider, $method, (arg), expected_item);
        }};

        // Handle valid or invalid arguments for two arguments
        (TWO, $provider:expr, $method:ident, $item_extractor:expr, $txnum:expr, $txhash:expr, $block:expr, $receipts:expr) => {{
            let ((arg1, arg2), expected_item) = $item_extractor($block, $txnum($block), $txhash($block), $receipts);
            call_method!($provider, $method, (arg1, arg2), expected_item);
        }};
    }

    /// Macro to test non-range methods.
    ///
    /// ( `NUMBER_ARGUMENTS`, METHOD, FN -> ((`METHOD_ARGUMENT(s)`,...), `EXPECTED_RESULT`),
    /// `INVALID_ARGUMENTS`)
    macro_rules! test_non_range {
    ([$(($arg_count:ident, $method:ident, $item_extractor:expr, $invalid_args:expr)),* $(,)?]) => {{

        // Get the number methods being tested.
        // Since each method tested will move a block from memory to storage, this ensures we have enough.
        let extra_blocks = [$(stringify!($arg_count)),*].len();

        let mut rng = generators::rng();
        let (provider, mut database_blocks, in_memory_blocks, receipts) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT + extra_blocks,
            BlockRangeParams {
                tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                ..Default::default()
            },
        )?;

        let mut in_memory_blocks: std::collections::VecDeque<_> = in_memory_blocks.into();

        $(
            let tx_hash = |block: &SealedBlock<Block>| *block.body().transactions[0].tx_hash();
            let tx_num = |block: &SealedBlock<Block>| {
                database_blocks
                    .iter()
                    .chain(in_memory_blocks.iter())
                    .take_while(|b| b.number < block.number)
                    .map(|b| b.transaction_count())
                    .sum::<usize>() as u64
            };

            // Ensure that the first generated in-memory block exists
            {
                // This block will be persisted to disk and removed from memory AFTER the first database query. This ensures that we query the in-memory state before the database avoiding any race condition.
                persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[0].number);

                call_method!($arg_count, provider, $method, $item_extractor, tx_num, tx_hash, &in_memory_blocks[0], &receipts);

                // Move the block as well in our own structures
                database_blocks.push(in_memory_blocks.pop_front().unwrap());
            }

            // database_blocks is changed above
            let tx_num = |block: &SealedBlock<Block>| {
                database_blocks
                    .iter()
                    .chain(in_memory_blocks.iter())
                    .take_while(|b| b.number < block.number)
                    .map(|b| b.transaction_count())
                    .sum::<usize>() as u64
            };

            // Invalid/Non-existent argument should return `None`
            {
                call_method!($arg_count, provider, $method, |_,_,_,_| ( ($invalid_args, None)), tx_num, tx_hash, &in_memory_blocks[0], &receipts);
            }

            // Check that the item is only in memory and not in database
            {
                let last_mem_block = &in_memory_blocks[in_memory_blocks.len() - 1];

                let (args, expected_item) = $item_extractor(last_mem_block, tx_num(last_mem_block), tx_hash(last_mem_block), &receipts);
                call_method!($arg_count, provider, $method, |_,_,_,_| (args.clone(), expected_item), tx_num, tx_hash, last_mem_block, &receipts);

                // Ensure the item is not in storage
                call_method!($arg_count, provider.database, $method, |_,_,_,_| ( (args, None)), tx_num, tx_hash, last_mem_block, &receipts);
            }
        )*
    }};
}

    #[test]
    fn test_non_range_methods() -> eyre::Result<()> {
        let test_tx_index = 0;

        test_non_range!([
            // TODO: header should use B256 like others instead of &B256
            // (
            //     ONE,
            //     header,
            //     |block: &SealedBlock, tx_num: TxNumber, tx_hash: B256, receipts: &Vec<Vec<Receipt>>| (&block.hash(), Some(block.header.header().clone())),
            //     (&B256::random())
            // ),
            (
                ONE,
                header_by_number,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    block.number,
                    Some(block.header().clone())
                ),
                u64::MAX
            ),
            (
                ONE,
                sealed_header,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    block.number,
                    Some(block.clone_sealed_header())
                ),
                u64::MAX
            ),
            (
                ONE,
                block_hash,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    block.number,
                    Some(block.hash())
                ),
                u64::MAX
            ),
            (
                ONE,
                block_number,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    block.hash(),
                    Some(block.number)
                ),
                B256::random()
            ),
            (
                ONE,
                block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Hash(block.hash()),
                    Some(block.clone().into_block())
                ),
                BlockHashOrNumber::Hash(B256::random())
            ),
            (
                ONE,
                block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Number(block.number),
                    Some(block.clone().into_block())
                ),
                BlockHashOrNumber::Number(u64::MAX)
            ),
            (
                ONE,
                block_body_indices,
                |block: &SealedBlock<Block>, tx_num: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    block.number,
                    Some(StoredBlockBodyIndices {
                        first_tx_num: tx_num,
                        tx_count: block.transaction_count() as u64
                    })
                ),
                u64::MAX
            ),
            (
                TWO,
                recovered_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    (BlockHashOrNumber::Number(block.number), TransactionVariant::WithHash),
                    block.clone().try_recover().ok()
                ),
                (BlockHashOrNumber::Number(u64::MAX), TransactionVariant::WithHash)
            ),
            (
                TWO,
                recovered_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    (BlockHashOrNumber::Hash(block.hash()), TransactionVariant::WithHash),
                    block.clone().try_recover().ok()
                ),
                (BlockHashOrNumber::Hash(B256::random()), TransactionVariant::WithHash)
            ),
            (
                TWO,
                sealed_block_with_senders,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    (BlockHashOrNumber::Number(block.number), TransactionVariant::WithHash),
                    block.clone().try_recover().ok()
                ),
                (BlockHashOrNumber::Number(u64::MAX), TransactionVariant::WithHash)
            ),
            (
                TWO,
                sealed_block_with_senders,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    (BlockHashOrNumber::Hash(block.hash()), TransactionVariant::WithHash),
                    block.clone().try_recover().ok()
                ),
                (BlockHashOrNumber::Hash(B256::random()), TransactionVariant::WithHash)
            ),
            (
                ONE,
                transaction_id,
                |_: &SealedBlock<Block>, tx_num: TxNumber, tx_hash: B256, _: &Vec<Vec<Receipt>>| (
                    tx_hash,
                    Some(tx_num)
                ),
                B256::random()
            ),
            (
                ONE,
                transaction_by_id,
                |block: &SealedBlock<Block>, tx_num: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    tx_num,
                    Some(block.body().transactions[test_tx_index].clone())
                ),
                u64::MAX
            ),
            (
                ONE,
                transaction_by_id_unhashed,
                |block: &SealedBlock<Block>, tx_num: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    tx_num,
                    Some(block.body().transactions[test_tx_index].clone())
                ),
                u64::MAX
            ),
            (
                ONE,
                transaction_by_hash,
                |block: &SealedBlock<Block>, _: TxNumber, tx_hash: B256, _: &Vec<Vec<Receipt>>| (
                    tx_hash,
                    Some(block.body().transactions[test_tx_index].clone())
                ),
                B256::random()
            ),
            (
                ONE,
                transaction_block,
                |block: &SealedBlock<Block>, tx_num: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    tx_num,
                    Some(block.number)
                ),
                u64::MAX
            ),
            (
                ONE,
                transactions_by_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Number(block.number),
                    Some(block.body().transactions.clone())
                ),
                BlockHashOrNumber::Number(u64::MAX)
            ),
            (
                ONE,
                transactions_by_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Hash(block.hash()),
                    Some(block.body().transactions.clone())
                ),
                BlockHashOrNumber::Number(u64::MAX)
            ),
            (
                ONE,
                transaction_sender,
                |block: &SealedBlock<Block>, tx_num: TxNumber, _: B256, _: &Vec<Vec<Receipt>>| (
                    tx_num,
                    block.body().transactions[test_tx_index].recover_signer().ok()
                ),
                u64::MAX
            ),
            (
                ONE,
                receipt,
                |block: &SealedBlock<Block>,
                 tx_num: TxNumber,
                 _: B256,
                 receipts: &Vec<Vec<Receipt>>| (
                    tx_num,
                    Some(receipts[block.number as usize][test_tx_index].clone())
                ),
                u64::MAX
            ),
            (
                ONE,
                receipt_by_hash,
                |block: &SealedBlock<Block>,
                 _: TxNumber,
                 tx_hash: B256,
                 receipts: &Vec<Vec<Receipt>>| (
                    tx_hash,
                    Some(receipts[block.number as usize][test_tx_index].clone())
                ),
                B256::random()
            ),
            (
                ONE,
                receipts_by_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, receipts: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Number(block.number),
                    Some(receipts[block.number as usize].clone())
                ),
                BlockHashOrNumber::Number(u64::MAX)
            ),
            (
                ONE,
                receipts_by_block,
                |block: &SealedBlock<Block>, _: TxNumber, _: B256, receipts: &Vec<Vec<Receipt>>| (
                    BlockHashOrNumber::Hash(block.hash()),
                    Some(receipts[block.number as usize].clone())
                ),
                BlockHashOrNumber::Hash(B256::random())
            ),
            // TODO: withdrawals, requests, ommers
        ]);

        Ok(())
    }

    #[test]
    fn test_race() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT - 1,
            TEST_BLOCKS_COUNT + 1,
            BlockRangeParams {
                tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                ..Default::default()
            },
        )?;

        // Old implementation was querying the database first. This is problematic, if there are
        // changes AFTER the database transaction is created.
        let old_transaction_hash_fn =
            |hash: B256,
             canonical_in_memory_state: CanonicalInMemoryState,
             factory: ProviderFactory<MockNodeTypesWithDB>| {
                assert!(factory.transaction_by_hash(hash)?.is_none(), "should not be in database");
                Ok::<_, ProviderError>(canonical_in_memory_state.transaction_by_hash(hash))
            };

        // Correct implementation queries in-memory first
        let correct_transaction_hash_fn =
            |hash: B256,
             canonical_in_memory_state: CanonicalInMemoryState,
             _factory: ProviderFactory<MockNodeTypesWithDB>| {
                if let Some(tx) = canonical_in_memory_state.transaction_by_hash(hash) {
                    return Ok::<_, ProviderError>(Some(tx));
                }
                panic!("should not be in database");
                // _factory.transaction_by_hash(hash)
            };

        // OLD BEHAVIOUR
        {
            // This will persist block 1 AFTER a database is created. Moving it from memory to
            // storage.
            persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[0].number);
            let to_be_persisted_tx = in_memory_blocks[0].body().transactions[0].clone();

            // Even though the block exists, given the order of provider queries done in the method
            // above, we do not see it.
            assert!(matches!(
                old_transaction_hash_fn(
                    *to_be_persisted_tx.tx_hash(),
                    provider.canonical_in_memory_state(),
                    provider.database.clone()
                ),
                Ok(None)
            ));
        }

        // CORRECT BEHAVIOUR
        {
            // This will persist block 1 AFTER a database is created. Moving it from memory to
            // storage.
            persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[1].number);
            let to_be_persisted_tx = in_memory_blocks[1].body().transactions[0].clone();

            assert!(matches!(
                correct_transaction_hash_fn(
                    *to_be_persisted_tx.tx_hash(),
                    provider.canonical_in_memory_state(),
                    provider.database
                ),
                Ok(Some(to_be_persisted_tx))
            ));
        }

        Ok(())
    }
}
