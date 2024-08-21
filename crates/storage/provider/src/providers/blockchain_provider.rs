use crate::{
    providers::StaticFileProvider, AccountReader, BlockHashReader, BlockIdReader, BlockNumReader,
    BlockReader, BlockReaderIdExt, BlockSource, CanonChainTracker, CanonStateNotifications,
    CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderRO, EvmEnvProvider, FinalizedBlockReader, HeaderProvider, ProviderError,
    ProviderFactory, PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt,
    RequestsProvider, StageCheckpointReader, StateProviderBox, StateProviderFactory,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chain_state::{
    BlockState, CanonicalInMemoryState, ForkChoiceNotifications, ForkChoiceSubscriptions,
    MemoryOverlayStateProvider,
};
use reth_chainspec::{ChainInfo, ChainSpec};
use reth_db_api::{
    database::Database,
    models::{AccountBeforeTx, StoredBlockBodyIndices},
};
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumHash, BlockNumber,
    BlockNumberOrTag, BlockWithSenders, EthereumHardforks, Header, Receipt, SealedBlock,
    SealedBlockWithSenders, SealedHeader, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, Withdrawals, B256, U256,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::ProviderResult;
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use std::{
    ops::{Add, Bound, RangeBounds, RangeInclusive, Sub},
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
pub struct BlockchainProvider2<DB> {
    /// Provider type used to access the database.
    database: ProviderFactory<DB>,
    /// Tracks the chain info wrt forkchoice updates and in memory canonical
    /// state.
    pub(super) canonical_in_memory_state: CanonicalInMemoryState,
}

impl<DB> Clone for BlockchainProvider2<DB> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            canonical_in_memory_state: self.canonical_in_memory_state.clone(),
        }
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
                Ok(Self::with_latest(database, header.seal(best.best_hash))?)
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
        database: ProviderFactory<DB>,
        latest: SealedHeader,
    ) -> ProviderResult<Self> {
        let provider = database.provider()?;
        let finalized_header = provider
            .last_finalized_block_number()?
            .map(|num| provider.sealed_header(num))
            .transpose()?
            .flatten();
        Ok(Self {
            database,
            canonical_in_memory_state: CanonicalInMemoryState::with_head(latest, finalized_header),
        })
    }

    /// Gets a clone of `canonical_in_memory_state`.
    pub fn canonical_in_memory_state(&self) -> CanonicalInMemoryState {
        self.canonical_in_memory_state.clone()
    }

    // Helper function to convert range bounds
    fn convert_range_bounds<T>(
        &self,
        range: impl RangeBounds<T>,
        end_unbounded: impl FnOnce() -> T,
    ) -> (T, T)
    where
        T: Copy + Add<Output = T> + Sub<Output = T> + From<u8>,
    {
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + T::from(1u8),
            Bound::Unbounded => T::from(0u8),
        };

        let end = match range.end_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n - T::from(1u8),
            Bound::Unbounded => end_unbounded(),
        };

        (start, end)
    }

    /// This uses a given [`BlockState`] to initialize a state provider for that block.
    fn block_state_provider(
        &self,
        state: impl AsRef<BlockState>,
    ) -> ProviderResult<MemoryOverlayStateProvider> {
        let state = state.as_ref();
        let anchor_hash = state.anchor().hash;
        let latest_historical = self.database.history_by_block_hash(anchor_hash)?;
        Ok(self.canonical_in_memory_state.state_provider(state.hash(), latest_historical))
    }

    /// Returns:
    /// 1. The block state as [`Some`] if the block is in memory, and [`None`] if the block is in
    ///    database.
    /// 2. The in-block transaction index.
    fn block_state_by_tx_id(
        &self,
        provider: &DatabaseProviderRO<DB>,
        id: TxNumber,
    ) -> ProviderResult<Option<(Option<Arc<BlockState>>, usize)>> {
        // Get the last block number stored in the database
        let last_database_block_number = provider.last_block_number()?;

        // Get the next tx number for the last block stored in the database and consider it the
        // first tx number of the in-memory state
        let Some(last_block_body_index) =
            provider.block_body_indices(last_database_block_number)?
        else {
            return Ok(None);
        };
        let mut in_memory_tx_num = last_block_body_index.next_tx_num();

        if id < in_memory_tx_num {
            // If the transaction number is less than the first in-memory transaction number, make a
            // database lookup
            let Some(block_number) = provider.transaction_block(id)? else { return Ok(None) };
            let Some(body_index) = provider.block_body_indices(block_number)? else {
                return Ok(None)
            };
            let tx_index = id - body_index.last_tx_num();
            Ok(Some((None, tx_index as usize)))
        } else {
            // Otherwise, iterate through in-memory blocks and find the transaction with the
            // matching number

            let first_in_memory_block_number = last_database_block_number.saturating_add(1);
            let last_in_memory_block_number =
                self.canonical_in_memory_state.get_canonical_block_number();

            for block_number in first_in_memory_block_number..=last_in_memory_block_number {
                let Some(block_state) =
                    self.canonical_in_memory_state.state_by_number(block_number)
                else {
                    return Ok(None);
                };

                let executed_block = block_state.block();
                let block = executed_block.block();

                for tx_index in 0..block.body.len() {
                    if id == in_memory_tx_num {
                        return Ok(Some((Some(block_state), tx_index)))
                    }

                    in_memory_tx_num += 1;
                }
            }

            Ok(None)
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
        if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(*block_hash) {
            return Ok(Some(block_state.block().block().header.header().clone()));
        }

        self.database.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
            return Ok(Some(block_state.block().block().header.header().clone()));
        }

        self.database.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        if let Some(num) = self.block_number(*hash)? {
            self.header_td_by_number(num)
        } else {
            Ok(None)
        }
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        // If the TD is recorded on disk, we can just return that
        if let Some(td) = self.database.header_td_by_number(number)? {
            Ok(Some(td))
        } else if self.canonical_in_memory_state.hash_by_number(number).is_some() {
            // Otherwise, if the block exists in memory, we should return a TD for it.
            //
            // The canonical in memory state should only store post-merge blocks. Post-merge blocks
            // have zero difficulty. This means we can use the total difficulty for the last
            // persisted block number.
            let last_persisted_block_number = self.database.last_block_number()?;
            self.database.header_td_by_number(last_persisted_block_number)
        } else {
            // If the block does not exist in memory, and does not exist on-disk, we should not
            // return a TD for it.
            Ok(None)
        }
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        let (start, end) = self.convert_range_bounds(range, || {
            self.canonical_in_memory_state.get_canonical_block_number()
        });
        let mut range = start..=end;
        let mut headers = Vec::with_capacity((end - start + 1) as usize);

        // First, fetch the headers from the database
        let mut db_headers = self.database.headers_range(range.clone())?;

        // Advance the range iterator by the number of headers fetched from the database
        range.nth(db_headers.len() - 1);

        headers.append(&mut db_headers);

        // Fetch the remaining headers from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                headers.push(block_state.block().block().header.header().clone());
            } else {
                break
            }
        }

        Ok(headers)
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        if let Some(block_state) = self.canonical_in_memory_state.state_by_number(number) {
            return Ok(Some(block_state.block().block().header.clone()));
        }

        self.database.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        let (start, end) = self.convert_range_bounds(range, || {
            self.canonical_in_memory_state.get_canonical_block_number()
        });
        let mut range = start..=end;
        let mut sealed_headers = Vec::with_capacity((end - start + 1) as usize);

        // First, fetch the headers from the database
        let mut db_headers = self.database.sealed_headers_range(range.clone())?;

        // Advance the range iterator by the number of headers fetched from the database
        range.nth(db_headers.len() - 1);

        sealed_headers.append(&mut db_headers);

        // Fetch the remaining headers from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                sealed_headers.push(block_state.block().block().header.clone());
            } else {
                break
            }
        }

        Ok(sealed_headers)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        let (start, end) = self.convert_range_bounds(range, || {
            self.canonical_in_memory_state.get_canonical_block_number()
        });
        let mut range = start..=end;
        let mut sealed_headers = Vec::with_capacity((end - start + 1) as usize);

        // First, fetch the headers from the database
        let mut db_headers = self.database.sealed_headers_while(range.clone(), &mut predicate)?;

        // Advance the range iterator by the number of headers fetched from the database
        range.nth(db_headers.len() - 1);

        sealed_headers.append(&mut db_headers);

        // Fetch the remaining headers from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                let header = block_state.block().block().header.clone();
                if !predicate(&header) {
                    break
                }
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                sealed_headers.push(header);
            } else {
                break
            }
        }

        Ok(sealed_headers)
    }
}

impl<DB> BlockHashReader for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        if let Some(block_state) = self.canonical_in_memory_state.state_by_number(number) {
            return Ok(Some(block_state.hash()));
        }

        self.database.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let mut range = start..=end;

        let mut hashes = Vec::with_capacity((end - start + 1) as usize);

        // First, fetch the hashes from the database
        let mut db_hashes = self.database.canonical_hashes_range(start, end)?;

        // Advance the range iterator by the number of blocks fetched from the database
        range.nth(db_hashes.len() - 1);

        hashes.append(&mut db_hashes);

        // Fetch the remaining blocks from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                hashes.push(block_state.hash());
            } else {
                break
            }
        }

        Ok(hashes)
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
        if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
            return Ok(Some(block_state.number()));
        }

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
        match source {
            BlockSource::Any | BlockSource::Canonical => {
                // check in memory first
                // Note: it's fine to return the unsealed block because the caller already has
                // the hash
                if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
                    return Ok(Some(block_state.block().block().clone().unseal()));
                }
                self.database.find_block_by_hash(hash, source)
            }
            BlockSource::Pending => {
                Ok(self.canonical_in_memory_state.pending_block().map(|block| block.unseal()))
            }
        }
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        match id {
            BlockHashOrNumber::Hash(hash) => self.find_block_by_hash(hash, BlockSource::Any),
            BlockHashOrNumber::Number(num) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                    return Ok(Some(block_state.block().block().clone().unseal()));
                }
                self.database.block_by_number(num)
            }
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
        match self.convert_hash_or_number(id)? {
            Some(number) => {
                // If the Paris (Merge) hardfork block is known and block is after it, return empty
                // ommers.
                if self.database.chain_spec().final_paris_total_difficulty(number).is_some() {
                    return Ok(Some(Vec::new()));
                }

                // Check in-memory state first
                self.canonical_in_memory_state
                    .state_by_number(number)
                    .map(|o| o.block().block().ommers.clone())
                    .map_or_else(|| self.database.ommers(id), |ommers| Ok(Some(ommers)))
            }
            None => self.database.ommers(id),
        }
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        if let Some(indices) = self.database.block_body_indices(number)? {
            Ok(Some(indices))
        } else if let Some(state) = self.canonical_in_memory_state.state_by_number(number) {
            // we have to construct the stored indices for the in memory blocks
            //
            // To calculate this we will fetch the anchor block and walk forward from all parents
            let mut parent_chain = state.parent_state_chain();
            parent_chain.reverse();
            let anchor_num = state.anchor().number;
            let mut stored_indices = self
                .database
                .block_body_indices(anchor_num)?
                .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(anchor_num))?;
            stored_indices.first_tx_num = stored_indices.next_tx_num();

            for state in parent_chain {
                let txs = state.block().block.body.len() as u64;
                if state.block().block().number == number {
                    stored_indices.tx_count = txs;
                } else {
                    stored_indices.first_tx_num += txs;
                }
            }

            Ok(Some(stored_indices))
        } else {
            Ok(None)
        }
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
        match id {
            BlockHashOrNumber::Hash(hash) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
                    let block = block_state.block().block().clone();
                    let senders = block_state.block().senders().clone();
                    return Ok(Some(BlockWithSenders { block: block.unseal(), senders }));
                }
            }
            BlockHashOrNumber::Number(num) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                    let block = block_state.block().block().clone();
                    let senders = block_state.block().senders().clone();
                    return Ok(Some(BlockWithSenders { block: block.unseal(), senders }));
                }
            }
        }
        self.database.block_with_senders(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        match id {
            BlockHashOrNumber::Hash(hash) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
                    let block = block_state.block().block().clone();
                    let senders = block_state.block().senders().clone();
                    return Ok(Some(SealedBlockWithSenders { block, senders }));
                }
            }
            BlockHashOrNumber::Number(num) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                    let block = block_state.block().block().clone();
                    let senders = block_state.block().senders().clone();
                    return Ok(Some(SealedBlockWithSenders { block, senders }));
                }
            }
        }
        self.database.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, mut range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        let capacity = (range.end() - range.start() + 1) as usize;
        let mut blocks = Vec::with_capacity(capacity);

        // First, fetch the blocks from the database
        let mut database_blocks = self.database.block_range(range.clone())?;
        blocks.append(&mut database_blocks);

        // Advance the range iterator by the number of blocks fetched from the database
        range.nth(blocks.len() - 1);

        // Fetch the remaining blocks from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                blocks.push(block_state.block().block().clone().unseal());
            } else {
                break
            }
        }

        Ok(blocks)
    }

    fn block_with_senders_range(
        &self,
        mut range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        let capacity = (range.end() - range.start() + 1) as usize;
        let mut blocks = Vec::with_capacity(capacity);

        // First, fetch the blocks from the database
        let mut database_blocks = self.database.block_with_senders_range(range.clone())?;
        blocks.append(&mut database_blocks);

        // Advance the range iterator by the number of blocks fetched from the database
        range.nth(database_blocks.len() - 1);

        // Fetch the remaining blocks from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                let block = block_state.block().block().clone();
                let senders = block_state.block().senders().clone();
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                blocks.push(BlockWithSenders { block: block.unseal(), senders });
            } else {
                break
            }
        }

        Ok(blocks)
    }

    fn sealed_block_with_senders_range(
        &self,
        mut range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        let capacity = (range.end() - range.start() + 1) as usize;
        let mut blocks = Vec::with_capacity(capacity);

        // First, fetch the blocks from the database
        let mut database_blocks = self.database.sealed_block_with_senders_range(range.clone())?;
        blocks.append(&mut database_blocks);

        // Advance the range iterator by the number of blocks fetched from the database
        range.nth(database_blocks.len() - 1);

        // Fetch the remaining blocks from the in-memory state
        for num in range {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(num) {
                let block = block_state.block().block().clone();
                let senders = block_state.block().senders().clone();
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                blocks.push(SealedBlockWithSenders { block, senders });
            } else {
                break
            }
        }

        Ok(blocks)
    }
}

impl<DB> TransactionsProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        // First, check the database
        if let Some(id) = self.database.transaction_id(tx_hash)? {
            return Ok(Some(id))
        }

        // If the transaction is not found in the database, check the in-memory state

        // Get the last transaction number stored in the database
        let last_database_block_number = self.database.last_block_number()?;
        let last_database_tx_id = self
            .database
            .block_body_indices(last_database_block_number)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(last_database_block_number))?
            .last_tx_num();

        // Find the transaction in the in-memory state with the matching hash, and return its
        // number
        let mut in_memory_tx_id = last_database_tx_id + 1;
        for block_number in last_database_block_number.saturating_add(1)..=
            self.canonical_in_memory_state.get_canonical_block_number()
        {
            // TODO: there might be an update between loop iterations, we
            // need to handle that situation.
            let block_state = self
                .canonical_in_memory_state
                .state_by_number(block_number)
                .ok_or(ProviderError::StateForNumberNotFound(block_number))?;
            for tx in &block_state.block().block().body {
                if tx.hash() == tx_hash {
                    return Ok(Some(in_memory_tx_id))
                }

                in_memory_tx_id += 1;
            }
        }

        Ok(None)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        let provider = self.database.provider()?;
        let Some((block_state, tx_index)) = self.block_state_by_tx_id(&provider, id)? else {
            return Ok(None)
        };

        if let Some(block_state) = block_state {
            let transaction = block_state.block().block().body.get(tx_index).cloned();
            Ok(transaction)
        } else {
            provider.transaction_by_id(id)
        }
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        let provider = self.database.provider()?;
        let Some((block_state, tx_index)) = self.block_state_by_tx_id(&provider, id)? else {
            return Ok(None)
        };

        if let Some(block_state) = block_state {
            let transaction =
                block_state.block().block().body.get(tx_index).cloned().map(Into::into);
            Ok(transaction)
        } else {
            provider.transaction_by_id_no_hash(id)
        }
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        if let Some(tx) = self.canonical_in_memory_state.transaction_by_hash(hash) {
            return Ok(Some(tx))
        }

        self.database.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        if let Some((tx, meta)) =
            self.canonical_in_memory_state.transaction_by_hash_with_meta(tx_hash)
        {
            return Ok(Some((tx, meta)))
        }

        self.database.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        let provider = self.database.provider()?;
        Ok(self
            .block_state_by_tx_id(&provider, id)?
            .and_then(|(block_state, _)| block_state)
            .map(|block_state| block_state.block().block().number))
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        match id {
            BlockHashOrNumber::Hash(hash) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
                    return Ok(Some(block_state.block().block().body.clone()));
                }
            }
            BlockHashOrNumber::Number(number) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_number(number) {
                    return Ok(Some(block_state.block().block().body.clone()));
                }
            }
        }
        self.database.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        let (start, end) = self.convert_range_bounds(range, || {
            self.canonical_in_memory_state.get_canonical_block_number()
        });

        let mut transactions = Vec::new();
        let mut last_in_memory_block = None;

        for number in start..=end {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(number) {
                // TODO: there might be an update between loop iterations, we
                // need to handle that situation.
                transactions.push(block_state.block().block().body.clone());
                last_in_memory_block = Some(number);
            } else {
                break
            }
        }

        if let Some(last_block) = last_in_memory_block {
            if last_block < end {
                let mut db_transactions =
                    self.database.transactions_by_block_range((last_block + 1)..=end)?;
                transactions.append(&mut db_transactions);
            }
        } else {
            transactions = self.database.transactions_by_block_range(start..=end)?;
        }

        Ok(transactions)
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
        let provider = self.database.provider()?;
        let Some((block_state, tx_index)) = self.block_state_by_tx_id(&provider, id)? else {
            return Ok(None)
        };

        if let Some(block_state) = block_state {
            let sender = block_state
                .block()
                .block()
                .body
                .get(tx_index)
                .and_then(|transaction| transaction.recover_signer());
            Ok(sender)
        } else {
            provider.transaction_sender(id)
        }
    }
}

impl<DB> ReceiptProvider for BlockchainProvider2<DB>
where
    DB: Database,
{
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        let provider = self.database.provider()?;
        let Some((block_state, tx_index)) = self.block_state_by_tx_id(&provider, id)? else {
            return Ok(None)
        };

        if let Some(block_state) = block_state {
            let receipt = block_state.executed_block_receipts().get(tx_index).cloned();
            Ok(receipt)
        } else {
            provider.receipt(id)
        }
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        for block_state in self.canonical_in_memory_state.canonical_chain() {
            let executed_block = block_state.block();
            let block = executed_block.block();
            let receipts = block_state.executed_block_receipts();

            // assuming 1:1 correspondence between transactions and receipts
            debug_assert_eq!(
                block.body.len(),
                receipts.len(),
                "Mismatch between transaction and receipt count"
            );

            if let Some(tx_index) = block.body.iter().position(|tx| tx.hash() == hash) {
                // safe to use tx_index for receipts due to 1:1 correspondence
                return Ok(receipts.get(tx_index).cloned());
            }
        }

        self.database.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        match block {
            BlockHashOrNumber::Hash(hash) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_hash(hash) {
                    return Ok(Some(block_state.executed_block_receipts()));
                }
            }
            BlockHashOrNumber::Number(number) => {
                if let Some(block_state) = self.canonical_in_memory_state.state_by_number(number) {
                    return Ok(Some(block_state.executed_block_receipts()));
                }
            }
        }

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
        if !self.database.chain_spec().is_shanghai_active_at_timestamp(timestamp) {
            return Ok(None)
        }

        let Some(number) = self.convert_hash_or_number(id)? else { return Ok(None) };

        if let Some(block) = self.canonical_in_memory_state.state_by_number(number) {
            Ok(block.block().block().withdrawals.clone())
        } else {
            self.database.withdrawals_by_block(id, timestamp)
        }
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        let best_block_num = self.best_block_number()?;

        // If the best block is in memory, use that. Otherwise, use the latest withdrawal in the
        // database.
        if let Some(block) = self.canonical_in_memory_state.state_by_number(best_block_num) {
            Ok(block.block().block().withdrawals.clone().and_then(|mut w| w.pop()))
        } else {
            self.database.latest_withdrawal()
        }
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
        if !self.database.chain_spec().is_prague_active_at_timestamp(timestamp) {
            return Ok(None)
        }
        let Some(number) = self.convert_hash_or_number(id)? else { return Ok(None) };
        if let Some(block) = self.canonical_in_memory_state.state_by_number(number) {
            Ok(block.block().block().requests.clone())
        } else {
            self.database.requests_by_block(id, timestamp)
        }
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
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_env_with_header(cfg, block_env, &header, evm_config)
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
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_and_block_env(
            cfg,
            block_env,
            &self.database.chain_spec(),
            header,
            total_difficulty,
        );
        Ok(())
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
        let hash = self.convert_number(at)?.ok_or(ProviderError::HeaderNotFound(at))?;
        let header = self.header(&hash)?.ok_or(ProviderError::HeaderNotFound(at))?;
        self.fill_cfg_env_with_header(cfg, &header, evm_config)
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
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_env(cfg, &self.database.chain_spec(), header, total_difficulty);
        Ok(())
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
        // use latest state provider if the head state exists
        if let Some(state) = self.canonical_in_memory_state.head_state() {
            trace!(target: "providers::blockchain", "Using head state for latest state provider");
            Ok(self.block_state_provider(state)?.boxed())
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
        self.ensure_canonical_block(block_number)?;
        let hash = self
            .block_hash(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        self.history_by_block_hash(hash)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");
        if let Ok(state) = self.database.history_by_block_hash(block_hash) {
            // This could be tracked by a block in the database block
            Ok(state)
        } else if let Some(state) = self.canonical_in_memory_state.state_by_hash(block_hash) {
            // ... or this could be tracked by the in memory state
            let state_provider = self.block_state_provider(state)?;
            Ok(Box::new(state_provider))
        } else {
            // if we couldn't find it anywhere, then we should return an error
            Err(ProviderError::StateForHashNotFound(block_hash))
        }
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

        if let Some(block) = self.canonical_in_memory_state.pending_block_num_hash() {
            let historical = self.database.history_by_block_hash(block.hash)?;
            let pending_provider =
                self.canonical_in_memory_state.state_provider(block.hash, historical);

            return Ok(Box::new(pending_provider));
        }

        // fallback to latest state if the pending block is not available
        self.latest()
    }

    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        let historical = self.database.history_by_block_hash(block_hash)?;
        if let Some(block) = self.canonical_in_memory_state.pending_block_num_hash() {
            if block.hash == block_hash {
                let pending_provider =
                    self.canonical_in_memory_state.state_provider(block_hash, historical);

                return Ok(Some(Box::new(pending_provider)))
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
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
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

impl<DB> CanonStateSubscriptions for BlockchainProvider2<DB>
where
    DB: Send + Sync,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        self.canonical_in_memory_state.subscribe_canon_state()
    }
}

impl<DB> ForkChoiceSubscriptions for BlockchainProvider2<DB>
where
    DB: Send + Sync,
{
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications {
        let receiver = self.canonical_in_memory_state.subscribe_safe_block();
        ForkChoiceNotifications(receiver)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications {
        let receiver = self.canonical_in_memory_state.subscribe_finalized_block();
        ForkChoiceNotifications(receiver)
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
        if let Some(state) = self.canonical_in_memory_state.state_by_number(block_number) {
            let changesets = state
                .block()
                .execution_output
                .bundle
                .reverts
                .clone()
                .into_plain_state_reverts()
                .accounts
                .into_iter()
                .flatten()
                .map(|(address, info)| AccountBeforeTx { address, info: info.map(Into::into) })
                .collect();
            Ok(changesets)
        } else {
            self.database.provider()?.account_block_changeset(block_number)
        }
    }
}

impl<DB> AccountReader for BlockchainProvider2<DB>
where
    DB: Database + Sync + Send,
{
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        // use latest state provider
        let state_provider = self.latest()?;
        state_provider.basic_account(address)
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::Range, sync::Arc};

    use itertools::Itertools;
    use rand::Rng;
    use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
    use reth_chainspec::{
        ChainSpec, ChainSpecBuilder, ChainSpecProvider, EthereumHardfork, MAINNET,
    };
    use reth_db::{models::AccountBeforeTx, test_utils::TempDatabase, DatabaseEnv};
    use reth_execution_types::ExecutionOutcome;
    use reth_primitives::{
        BlockHashOrNumber, BlockNumberOrTag, Receipt, SealedBlock, StaticFileSegment, B256,
    };
    use reth_storage_api::{
        BlockHashReader, BlockNumReader, BlockReader, BlockReaderIdExt, BlockSource,
        ChangeSetReader, HeaderProvider, ReceiptProviderIdExt, RequestsProvider,
    };
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_changeset_range, random_eoa_accounts,
        random_receipt,
    };
    use revm::db::BundleState;

    use crate::{
        providers::BlockchainProvider2,
        test_utils::{create_test_provider_factory, create_test_provider_factory_with_chain_spec},
        BlockWriter, CanonChainTracker, StaticFileWriter,
    };

    const TEST_BLOCKS_COUNT: usize = 5;

    fn random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        requests_count: Option<Range<u8>>,
    ) -> (Vec<SealedBlock>, Vec<SealedBlock>) {
        let block_range = (database_blocks + in_memory_blocks - 1) as u64;
        let blocks = random_block_range(rng, 0..=block_range, B256::ZERO, 0..1, requests_count);
        let (database_blocks, in_memory_blocks) = blocks.split_at(database_blocks);
        (database_blocks.to_vec(), in_memory_blocks.to_vec())
    }

    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn provider_with_chain_spec_and_random_blocks(
        rng: &mut impl Rng,
        chain_spec: Arc<ChainSpec>,
        database_blocks: usize,
        in_memory_blocks: usize,
        requests_count: Option<Range<u8>>,
    ) -> eyre::Result<(
        BlockchainProvider2<Arc<TempDatabase<DatabaseEnv>>>,
        Vec<SealedBlock>,
        Vec<SealedBlock>,
        Vec<Vec<Receipt>>,
    )> {
        let (database_blocks, in_memory_blocks) =
            random_blocks(rng, database_blocks, in_memory_blocks, requests_count);
        let receipts: Vec<Vec<_>> = database_blocks
            .iter()
            .chain(in_memory_blocks.iter())
            .map(|block| block.body.iter())
            .map(|tx| tx.map(|tx| random_receipt(rng, tx, Some(2))).collect())
            .collect();

        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let provider_rw = factory.provider_rw()?;

        // Insert blocks and receipts into the database
        for block in &database_blocks {
            provider_rw.insert_historical_block(
                block.clone().seal_with_senders().expect("failed to seal block with senders"),
            )?;

            // Insert the receipts into the database using the writer from the provider_rw
            let mut writer =
                provider_rw.static_file_provider().latest_writer(StaticFileSegment::Receipts)?;
            let block_number = block.number as usize;
            for receipt in receipts.get(block_number).unwrap() {
                writer.append_receipt(block.number, receipt)?;
            }
        }
        provider_rw.commit()?;

        let provider = BlockchainProvider2::new(factory)?;

        // Insert the rest of the blocks and receipts into the in-memory state
        let chain = NewCanonicalChain::Commit {
            new: in_memory_blocks
                .iter()
                .map(|block| {
                    let senders = block.senders().expect("failed to recover senders");
                    let block_receipts = receipts.get(block.number as usize).unwrap().clone();
                    let execution_outcome =
                        ExecutionOutcome { receipts: block_receipts.into(), ..Default::default() };

                    ExecutedBlock::new(
                        Arc::new(block.clone()),
                        Arc::new(senders),
                        execution_outcome.into(),
                        Default::default(),
                        Default::default(),
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
        provider.set_canonical_head(canonical_block.header.clone());
        provider.set_safe(safe_block.header.clone());
        provider.set_finalized(finalized_block.header.clone());

        Ok((provider, database_blocks.to_vec(), in_memory_blocks.to_vec(), receipts))
    }

    #[allow(clippy::type_complexity)]
    fn provider_with_random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        requests_count: Option<Range<u8>>,
    ) -> eyre::Result<(
        BlockchainProvider2<Arc<TempDatabase<DatabaseEnv>>>,
        Vec<SealedBlock>,
        Vec<SealedBlock>,
        Vec<Vec<Receipt>>,
    )> {
        provider_with_chain_spec_and_random_blocks(
            rng,
            MAINNET.clone(),
            database_blocks,
            in_memory_blocks,
            requests_count,
        )
    }

    #[test]
    fn test_block_reader_find_block_by_hash() -> eyre::Result<()> {
        // Initialize random number generator and provider factory
        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks and split into database and in-memory blocks
        let blocks = random_block_range(&mut rng, 0..=10, B256::ZERO, 0..1, None);
        let (database_blocks, in_memory_blocks) = blocks.split_at(5);

        // Insert first 5 blocks into the database
        let provider_rw = factory.provider_rw()?;
        for block in database_blocks {
            provider_rw.insert_historical_block(
                block.clone().seal_with_senders().expect("failed to seal block with senders"),
            )?;
        }
        provider_rw.commit()?;

        // Create a new provider
        let provider = BlockchainProvider2::new(factory)?;

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
            new: vec![ExecutedBlock::new(
                Arc::new(first_in_mem_block.clone()),
                Arc::new(in_memory_block_senders),
                Default::default(),
                Default::default(),
                Default::default(),
            )],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        // Now the block should be found in memory
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Any)?,
            Some(first_in_mem_block.clone().into())
        );
        assert_eq!(
            provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Canonical)?,
            Some(first_in_mem_block.clone().into())
        );

        // Find the first block in database by hash
        assert_eq!(
            provider.find_block_by_hash(first_db_block.hash(), BlockSource::Any)?,
            Some(first_db_block.clone().into())
        );
        assert_eq!(
            provider.find_block_by_hash(first_db_block.hash(), BlockSource::Canonical)?,
            Some(first_db_block.clone().into())
        );

        // No pending block in database
        assert_eq!(provider.find_block_by_hash(first_db_block.hash(), BlockSource::Pending)?, None);

        // Insert the last block into the pending state
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlock {
            block: Arc::new(last_in_mem_block.clone()),
            senders: Default::default(),
            execution_output: Default::default(),
            hashed_state: Default::default(),
            trie: Default::default(),
        });

        // Now the last block should be found in memory
        assert_eq!(
            provider.find_block_by_hash(last_in_mem_block.hash(), BlockSource::Pending)?,
            Some(last_in_mem_block.clone().into())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_block() -> eyre::Result<()> {
        // Initialize random number generator and provider factory
        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks and split into database and in-memory blocks
        let blocks = random_block_range(&mut rng, 0..=10, B256::ZERO, 0..1, None);
        let (database_blocks, in_memory_blocks) = blocks.split_at(5);

        // Insert first 5 blocks into the database
        let provider_rw = factory.provider_rw()?;
        for block in database_blocks {
            provider_rw.insert_historical_block(
                block.clone().seal_with_senders().expect("failed to seal block with senders"),
            )?;
        }
        provider_rw.commit()?;

        // Create a new provider
        let provider = BlockchainProvider2::new(factory)?;

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
            new: vec![ExecutedBlock::new(
                Arc::new(first_in_mem_block.clone()),
                Arc::new(in_memory_block_senders),
                Default::default(),
                Default::default(),
                Default::default(),
            )],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        // First in memory block should be found
        assert_eq!(
            provider.block(BlockHashOrNumber::Hash(first_in_mem_block.hash()))?,
            Some(first_in_mem_block.clone().into())
        );
        assert_eq!(
            provider.block(BlockHashOrNumber::Number(first_in_mem_block.number))?,
            Some(first_in_mem_block.clone().into())
        );

        // First database block should be found
        assert_eq!(
            provider.block(BlockHashOrNumber::Hash(first_db_block.hash()))?,
            Some(first_db_block.clone().into())
        );
        assert_eq!(
            provider.block(BlockHashOrNumber::Number(first_db_block.number))?,
            Some(first_db_block.clone().into())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_pending_block() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, _, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        // Generate a random block
        let mut rng = generators::rng();
        let block = random_block(&mut rng, 0, Some(B256::ZERO), None, None, None);

        // Set the block as pending
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlock {
            block: Arc::new(block.clone()),
            senders: Default::default(),
            execution_output: Default::default(),
            hashed_state: Default::default(),
            trie: Default::default(),
        });

        // Assertions related to the pending block
        assert_eq!(provider.pending_block()?, Some(block.clone()));

        assert_eq!(
            provider.pending_block_with_senders()?,
            Some(reth_primitives::SealedBlockWithSenders {
                block: block.clone(),
                senders: block.senders().unwrap()
            })
        );

        assert_eq!(provider.pending_block_and_receipts()?, Some((block, vec![])));

        Ok(())
    }

    #[test]
    fn test_block_reader_ommers() -> eyre::Result<()> {
        // Create a new provider
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let first_in_mem_block = in_memory_blocks.first().unwrap();

        // If the block is after the Merge, we should have an empty ommers list
        assert_eq!(
            provider.ommers(
                (provider.chain_spec().paris_block_and_final_difficulty.unwrap().0 + 2).into()
            )?,
            Some(vec![])
        );

        // First in memory block ommers should be found
        assert_eq!(
            provider.ommers(first_in_mem_block.number.into())?,
            Some(first_in_mem_block.ommers.clone())
        );
        assert_eq!(
            provider.ommers(first_in_mem_block.hash().into())?,
            Some(first_in_mem_block.ommers.clone())
        );

        // A random hash should return None as the block number is not found
        assert_eq!(provider.ommers(B256::random().into())?, None);

        Ok(())
    }

    #[test]
    fn test_block_hash_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

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
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();
        let blocks = [database_blocks, in_memory_blocks].concat();

        assert_eq!(provider.header(&database_block.hash())?, Some(database_block.header().clone()));
        assert_eq!(
            provider.header(&in_memory_block.hash())?,
            Some(in_memory_block.header().clone())
        );

        assert_eq!(
            provider.header_by_number(database_block.number)?,
            Some(database_block.header().clone())
        );
        assert_eq!(
            provider.header_by_number(in_memory_block.number)?,
            Some(in_memory_block.header().clone())
        );

        assert_eq!(
            provider.header_td_by_number(database_block.number)?,
            Some(database_block.difficulty)
        );
        assert_eq!(
            provider.header_td_by_number(in_memory_block.number)?,
            Some(in_memory_block.difficulty)
        );

        assert_eq!(
            provider.headers_range(0..=10)?,
            blocks.iter().map(|b| b.header().clone()).collect::<Vec<_>>()
        );

        assert_eq!(
            provider.sealed_header(database_block.number)?,
            Some(database_block.header.clone())
        );
        assert_eq!(
            provider.sealed_header(in_memory_block.number)?,
            Some(in_memory_block.header.clone())
        );

        assert_eq!(
            provider.sealed_headers_range(0..=10)?,
            blocks.iter().map(|b| b.header.clone()).collect::<Vec<_>>()
        );

        assert_eq!(
            provider.sealed_headers_while(0..=10, |header| header.number <= 8)?,
            blocks
                .iter()
                .take_while(|header| header.number <= 8)
                .map(|b| b.header.clone())
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_block_num_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

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
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.header.hash();
        assert_eq!(
            provider.block_by_id(block_number.into()).unwrap(),
            Some(database_block.clone().unseal())
        );
        assert_eq!(provider.block_by_id(block_hash.into()).unwrap(), Some(database_block.unseal()));

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.header.hash();
        assert_eq!(
            provider.block_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.clone().unseal())
        );
        assert_eq!(
            provider.block_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.unseal())
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_header_by_number_or_tag() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();

        let in_memory_block_count = in_memory_blocks.len();
        let canonical_block = in_memory_blocks.get(in_memory_block_count - 1).unwrap().clone();
        let safe_block = in_memory_blocks.get(in_memory_block_count - 2).unwrap().clone();
        let finalized_block = in_memory_blocks.get(in_memory_block_count - 3).unwrap().clone();

        let block_number = database_block.number;
        assert_eq!(
            provider.header_by_number_or_tag(block_number.into()).unwrap(),
            Some(database_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(block_number.into()).unwrap(),
            Some(database_block.header)
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Latest).unwrap(),
            Some(canonical_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest).unwrap(),
            Some(canonical_block.header)
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Safe).unwrap(),
            Some(safe_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Safe).unwrap(),
            Some(safe_block.header)
        );

        assert_eq!(
            provider.header_by_number_or_tag(BlockNumberOrTag::Finalized).unwrap(),
            Some(finalized_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Finalized).unwrap(),
            Some(finalized_block.header)
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_header_by_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.header.hash();

        assert_eq!(
            provider.header_by_id(block_number.into()).unwrap(),
            Some(database_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_number.into()).unwrap(),
            Some(database_block.header.clone())
        );

        assert_eq!(
            provider.header_by_id(block_hash.into()).unwrap(),
            Some(database_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_hash.into()).unwrap(),
            Some(database_block.header)
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.header.hash();

        assert_eq!(
            provider.header_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_number.into()).unwrap(),
            Some(in_memory_block.header.clone())
        );

        assert_eq!(
            provider.header_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.header.clone().unseal())
        );
        assert_eq!(
            provider.sealed_header_by_id(block_hash.into()).unwrap(),
            Some(in_memory_block.header)
        );

        Ok(())
    }

    #[test]
    fn test_block_reader_id_ext_ommers_by_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.header.hash();

        assert_eq!(
            provider.ommers_by_id(block_number.into()).unwrap().unwrap_or_default(),
            database_block.ommers
        );
        assert_eq!(
            provider.ommers_by_id(block_hash.into()).unwrap().unwrap_or_default(),
            database_block.ommers
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.header.hash();

        assert_eq!(
            provider.ommers_by_id(block_number.into()).unwrap().unwrap_or_default(),
            in_memory_block.ommers
        );
        assert_eq!(
            provider.ommers_by_id(block_hash.into()).unwrap().unwrap_or_default(),
            in_memory_block.ommers
        );

        Ok(())
    }

    #[test]
    fn test_receipt_provider_id_ext_receipts_by_block_id() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.header.hash();

        assert_eq!(
            provider.receipts_by_block_id(block_number.into())?.unwrap_or_default(),
            receipts.get(block_number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_block_id(block_hash.into())?.unwrap_or_default(),
            receipts.get(block_number as usize).unwrap().clone()
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.header.hash();

        assert_eq!(
            provider.receipts_by_block_id(block_number.into())?.unwrap_or_default(),
            receipts.get(block_number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_block_id(block_hash.into())?.unwrap_or_default(),
            receipts.get(block_number as usize).unwrap().clone()
        );

        Ok(())
    }

    #[test]
    fn test_receipt_provider_id_ext_receipts_by_block_number_or_tag() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) =
            provider_with_random_blocks(&mut rng, TEST_BLOCKS_COUNT, TEST_BLOCKS_COUNT, None)?;

        let database_block = database_blocks.first().unwrap().clone();

        let in_memory_block_count = in_memory_blocks.len();
        let canonical_block = in_memory_blocks.get(in_memory_block_count - 1).unwrap().clone();
        let safe_block = in_memory_blocks.get(in_memory_block_count - 2).unwrap().clone();
        let finalized_block = in_memory_blocks.get(in_memory_block_count - 3).unwrap().clone();

        assert_eq!(
            provider.receipts_by_number_or_tag(database_block.number.into())?.unwrap_or_default(),
            receipts.get(database_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap_or_default(),
            receipts.get(canonical_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Safe)?.unwrap_or_default(),
            receipts.get(safe_block.number as usize).unwrap().clone()
        );
        assert_eq!(
            provider.receipts_by_number_or_tag(BlockNumberOrTag::Finalized)?.unwrap_or_default(),
            receipts.get(finalized_block.number as usize).unwrap().clone()
        );

        Ok(())
    }

    #[test]
    fn test_changeset_reader() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let (database_blocks, in_memory_blocks) =
            random_blocks(&mut rng, TEST_BLOCKS_COUNT, 1, None);

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
                .map(|b| b.seal_with_senders().expect("failed to seal block with senders"))
                .collect(),
            ExecutionOutcome {
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

        let provider = BlockchainProvider2::new(factory)?;

        let in_memory_changesets = in_memory_changesets.into_iter().next().unwrap();
        let chain = NewCanonicalChain::Commit {
            new: vec![in_memory_blocks
                .first()
                .map(|block| {
                    let senders = block.senders().expect("failed to recover senders");
                    ExecutedBlock::new(
                        Arc::new(block.clone()),
                        Arc::new(senders),
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
                        Default::default(),
                    )
                })
                .unwrap()],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        assert_eq!(
            provider.account_block_changeset(last_database_block).unwrap(),
            database_changesets
                .into_iter()
                .last()
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
    fn test_requests_provider() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().prague_activated().build());
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_chain_spec_and_random_blocks(
                &mut rng,
                chain_spec.clone(),
                TEST_BLOCKS_COUNT,
                TEST_BLOCKS_COUNT,
                Some(1..2),
            )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let prague_timestamp =
            chain_spec.hardforks.fork(EthereumHardfork::Prague).as_timestamp().unwrap();

        assert_eq!(
            provider.requests_by_block(database_block.number.into(), prague_timestamp,)?,
            database_block.requests.clone()
        );
        assert_eq!(
            provider.requests_by_block(in_memory_block.number.into(), prague_timestamp,)?,
            in_memory_block.requests.clone()
        );

        Ok(())
    }
}
