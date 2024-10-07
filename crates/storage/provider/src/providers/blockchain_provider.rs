use crate::{
    providers::StaticFileProvider, AccountReader, BlockHashReader, BlockIdReader, BlockNumReader,
    BlockReader, BlockReaderIdExt, BlockSource, CanonChainTracker, CanonStateNotifications,
    CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderRO, EvmEnvProvider, FinalizedBlockReader, HeaderProvider, ProviderError,
    ProviderFactory, PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt,
    RequestsProvider, StageCheckpointReader, StateProviderBox, StateProviderFactory, StateReader,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumHash, BlockNumberOrTag, HashOrNumber};
use alloy_primitives::{Address, BlockHash, BlockNumber, Sealable, TxHash, TxNumber, B256, U256};
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chain_state::{
    BlockState, CanonicalInMemoryState, ForkChoiceNotifications, ForkChoiceSubscriptions,
    MemoryOverlayStateProvider,
};
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_db::models::BlockNumberAddress;
use reth_db_api::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_evm::ConfigureEvmEnv;
use reth_execution_types::{BundleStateInit, ExecutionOutcome, RevertsInit};
use reth_node_types::NodeTypesWithDB;
use reth_primitives::{
    Account, Block, BlockWithSenders, Header, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, StorageEntry, TransactionMeta, TransactionSigned, TransactionSignedNoHash,
    Withdrawal, Withdrawals,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::StorageChangeSetReader;
use reth_storage_errors::provider::ProviderResult;
use revm::{
    db::states::PlainStorageRevert,
    primitives::{BlockEnv, CfgEnvWithHandlerCfg},
};
use std::{
    collections::{hash_map, HashMap},
    ops::{Add, Bound, RangeBounds, RangeInclusive, Sub},
    sync::Arc,
    time::Instant,
};
use tracing::trace;

use super::ProviderNodeTypes;

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Debug)]
pub struct BlockchainProvider2<N: NodeTypesWithDB> {
    /// Provider type used to access the database.
    database: ProviderFactory<N>,
    /// Tracks the chain info wrt forkchoice updates and in memory canonical
    /// state.
    pub(super) canonical_in_memory_state: CanonicalInMemoryState,
}

impl<N: NodeTypesWithDB> Clone for BlockchainProvider2<N> {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            canonical_in_memory_state: self.canonical_in_memory_state.clone(),
        }
    }
}

impl<N: ProviderNodeTypes> BlockchainProvider2<N> {
    /// Create a new provider using only the database, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ProviderFactory<N>) -> ProviderResult<Self> {
        let provider = database.provider()?;
        let best: ChainInfo = provider.chain_info()?;
        match provider.header_by_number(best.best_number)? {
            Some(header) => {
                drop(provider);
                Ok(Self::with_latest(database, SealedHeader::new(header, best.best_hash))?)
            }
            None => Err(ProviderError::HeaderNotFound(best.best_number.into())),
        }
    }

    /// Create new provider instance that wraps the database and the blockchain tree, using the
    /// provided latest header to initialize the chain info tracker.
    ///
    /// This returns a `ProviderResult` since it tries the retrieve the last finalized header from
    /// `database`.
    pub fn with_latest(database: ProviderFactory<N>, latest: SealedHeader) -> ProviderResult<Self> {
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

    /// Return the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// If the range is empty, or there are no blocks for the given range, then this returns `None`.
    pub fn get_state(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<ExecutionOutcome>> {
        if range.is_empty() {
            return Ok(None)
        }
        let start_block_number = *range.start();
        let end_block_number = *range.end();

        // We are not removing block meta as it is used to get block changesets.
        let mut block_bodies = Vec::new();
        for block_num in range.clone() {
            let block_body = self
                .block_body_indices(block_num)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block_num))?;
            block_bodies.push((block_num, block_body))
        }

        // get transaction receipts
        let Some(from_transaction_num) = block_bodies.first().map(|body| body.1.first_tx_num())
        else {
            return Ok(None)
        };
        let Some(to_transaction_num) = block_bodies.last().map(|body| body.1.last_tx_num()) else {
            return Ok(None)
        };

        let mut account_changeset = Vec::new();
        for block_num in range.clone() {
            let changeset =
                self.account_block_changeset(block_num)?.into_iter().map(|elem| (block_num, elem));
            account_changeset.extend(changeset);
        }

        let mut storage_changeset = Vec::new();
        for block_num in range {
            let changeset = self.storage_changeset(block_num)?;
            storage_changeset.extend(changeset);
        }

        let (state, reverts) =
            self.populate_bundle_state(account_changeset, storage_changeset, end_block_number)?;

        let mut receipt_iter =
            self.receipts_by_tx_range(from_transaction_num..=to_transaction_num)?.into_iter();

        let mut receipts = Vec::with_capacity(block_bodies.len());
        // loop break if we are at the end of the blocks.
        for (_, block_body) in block_bodies {
            let mut block_receipts = Vec::with_capacity(block_body.tx_count as usize);
            for tx_num in block_body.tx_num_range() {
                let receipt =
                    receipt_iter.next().ok_or(ProviderError::ReceiptNotFound(tx_num.into()))?;
                block_receipts.push(Some(receipt));
            }
            receipts.push(block_receipts);
        }

        Ok(Some(ExecutionOutcome::new_init(
            state,
            reverts,
            // We skip new contracts since we never delete them from the database
            Vec::new(),
            receipts.into(),
            start_block_number,
            Vec::new(),
        )))
    }

    /// Populate a [`BundleStateInit`] and [`RevertsInit`] using cursors over the
    /// [`reth_db::PlainAccountState`] and [`reth_db::PlainStorageState`] tables, based on the given
    /// storage and account changesets.
    fn populate_bundle_state(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        block_range_end: BlockNumber,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)> {
        let mut state: BundleStateInit = HashMap::new();
        let mut reverts: RevertsInit = HashMap::new();
        let state_provider = self.state_by_block_number_or_tag(block_range_end.into())?;

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let new_info = state_provider.basic_account(address)?;
                    entry.insert((old_info, new_info, HashMap::new()));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    // overwrite old account state.
                    entry.get_mut().0 = old_info;
                }
            }
            // insert old info into reverts.
            reverts.entry(block_number).or_default().entry(address).or_default().0 = Some(old_info);
        }

        // add storage changeset changes
        for (block_and_address, old_storage) in storage_changeset.into_iter().rev() {
            let BlockNumberAddress((block_number, address)) = block_and_address;
            // get account state or insert from plain state.
            let account_state = match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let present_info = state_provider.basic_account(address)?;
                    entry.insert((present_info, present_info, HashMap::new()))
                }
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };

            // match storage.
            match account_state.2.entry(old_storage.key) {
                hash_map::Entry::Vacant(entry) => {
                    let new_storage_value =
                        state_provider.storage(address, old_storage.key)?.unwrap_or_default();
                    entry.insert((old_storage.value, new_storage_value));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().0 = old_storage.value;
                }
            };

            reverts
                .entry(block_number)
                .or_default()
                .entry(address)
                .or_default()
                .1
                .push(old_storage);
        }

        Ok((state, reverts))
    }

    /// Fetches a range of data from both in-memory state and persistent storage while a predicate
    /// is met.
    ///
    /// Creates a snapshot of the in-memory chain state and database provider to prevent
    /// inconsistencies. Splits the range into in-memory and storage sections, prioritizing
    /// recent in-memory blocks in case of overlaps.
    ///
    /// * `fetch_db_range` function (`F`) provides access to the database provider, allowing the
    ///   user to retrieve the required items from the database using [`RangeInclusive`].
    /// * `map_block_state_item` function (`G`) provides each block of the range in the in-memory
    ///   state, allowing for selection or filtering for the desired data.
    fn get_in_memory_or_storage_by_block_range_while<T, F, G, P>(
        &self,
        range: impl RangeBounds<BlockNumber>,
        fetch_db_range: F,
        map_block_state_item: G,
        mut predicate: P,
    ) -> ProviderResult<Vec<T>>
    where
        F: FnOnce(
            &DatabaseProviderRO<N::DB, N::ChainSpec>,
            RangeInclusive<BlockNumber>,
            &mut P,
        ) -> ProviderResult<Vec<T>>,
        G: Fn(Arc<BlockState>, &mut P) -> Option<T>,
        P: FnMut(&T) -> bool,
    {
        // Each one provides a snapshot at the time of instantiation, but its order matters.
        //
        // If we acquire first the database provider, it's possible that before the in-memory chain
        // snapshot is instantiated, it will flush blocks to disk. This would
        // mean that our database provider would not have access to the flushed blocks (since it's
        // working under an older view), while the in-memory state may have deleted them
        // entirely. Resulting in gaps on the range.
        let mut in_memory_chain =
            self.canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();
        let db_provider = self.database_provider_ro()?;

        let (start, end) = self.convert_range_bounds(range, || {
            // the first block is the highest one.
            in_memory_chain
                .first()
                .map(|b| b.number())
                .unwrap_or_else(|| db_provider.last_block_number().unwrap_or_default())
        });

        if start > end {
            return Ok(vec![])
        }

        // Split range into storage_range and in-memory range. If the in-memory range is not
        // necessary drop it early.
        //
        // The last block of `in_memory_chain` is the lowest block number.
        let (in_memory, storage_range) = match in_memory_chain.last().as_ref().map(|b| b.number()) {
            Some(lowest_memory_block) if lowest_memory_block <= end => {
                let highest_memory_block =
                    in_memory_chain.first().as_ref().map(|b| b.number()).expect("qed");

                // Database will for a time overlap with in-memory-chain blocks. In
                // case of a re-org, it can mean that the database blocks are of a forked chain, and
                // so, we should prioritize the in-memory overlapped blocks.
                let in_memory_range =
                    lowest_memory_block.max(start)..=end.min(highest_memory_block);

                // If requested range is in the middle of the in-memory range, remove the necessary
                // lowest blocks
                in_memory_chain.truncate(
                    in_memory_chain
                        .len()
                        .saturating_sub(start.saturating_sub(lowest_memory_block) as usize),
                );

                let storage_range =
                    (lowest_memory_block > start).then(|| start..=lowest_memory_block - 1);

                (Some((in_memory_chain, in_memory_range)), storage_range)
            }
            _ => {
                // Drop the in-memory chain so we don't hold blocks in memory.
                drop(in_memory_chain);

                (None, Some(start..=end))
            }
        };

        let mut items = Vec::with_capacity((end - start + 1) as usize);

        if let Some(storage_range) = storage_range {
            let mut db_items = fetch_db_range(&db_provider, storage_range.clone(), &mut predicate)?;
            items.append(&mut db_items);

            // The predicate was not met, if the number of items differs from the expected. So, we
            // return what we have.
            if items.len() as u64 != storage_range.end() - storage_range.start() + 1 {
                return Ok(items)
            }
        }

        if let Some((in_memory_chain, in_memory_range)) = in_memory {
            for (num, block) in in_memory_range.zip(in_memory_chain.into_iter().rev()) {
                debug_assert!(num == block.number());
                if let Some(item) = map_block_state_item(block, &mut predicate) {
                    items.push(item);
                } else {
                    break
                }
            }
        }

        Ok(items)
    }

    /// This uses a given [`BlockState`] to initialize a state provider for that block.
    fn block_state_provider(
        &self,
        state: &BlockState,
    ) -> ProviderResult<MemoryOverlayStateProvider> {
        let anchor_hash = state.anchor().hash;
        let latest_historical = self.database.history_by_block_hash(anchor_hash)?;
        Ok(self.canonical_in_memory_state.state_provider_from_state(state, latest_historical))
    }

    /// Fetches data from either in-memory state or persistent storage for a range of transactions.
    ///
    /// * `fetch_from_db`: has a [`DatabaseProviderRO`] and the storage specific range.
    /// * `fetch_from_block_state`: has a [`RangeInclusive`] of elements that should be fetched from
    ///   [`BlockState`]. [`RangeInclusive`] is necessary to handle partial look-ups of a block.
    fn get_in_memory_or_storage_by_tx_range<S, M, R>(
        &self,
        range: impl RangeBounds<BlockNumber>,
        fetch_from_db: S,
        fetch_from_block_state: M,
    ) -> ProviderResult<Vec<R>>
    where
        S: FnOnce(
            DatabaseProviderRO<N::DB, N::ChainSpec>,
            RangeInclusive<TxNumber>,
        ) -> ProviderResult<Vec<R>>,
        M: Fn(RangeInclusive<usize>, Arc<BlockState>) -> ProviderResult<Vec<R>>,
    {
        let in_mem_chain = self.canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();
        let provider = self.database.provider()?;

        // Get the last block number stored in the storage which does NOT overlap with in-memory
        // chain.
        let last_database_block_number = in_mem_chain
            .last()
            .map(|b| Ok(b.anchor().number))
            .unwrap_or_else(|| provider.last_block_number())?;

        // Get the next tx number for the last block stored in the storage, which marks the start of
        // the in-memory state.
        let last_block_body_index = provider
            .block_body_indices(last_database_block_number)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(last_database_block_number))?;
        let mut in_memory_tx_num = last_block_body_index.next_tx_num();

        let (start, end) = self.convert_range_bounds(range, || {
            in_mem_chain
                .iter()
                .map(|b| b.block_ref().block().body.transactions.len() as u64)
                .sum::<u64>() +
                last_block_body_index.last_tx_num()
        });

        if start > end {
            return Ok(vec![])
        }

        let mut tx_range = start..=end;

        // If the range is entirely before the first in-memory transaction number, fetch from
        // storage
        if *tx_range.end() < in_memory_tx_num {
            return fetch_from_db(provider, tx_range);
        }

        let mut items = Vec::with_capacity((tx_range.end() - tx_range.start() + 1) as usize);

        // If the range spans storage and memory, get elements from storage first.
        if *tx_range.start() < in_memory_tx_num {
            // Determine the range that needs to be fetched from storage.
            let db_range = *tx_range.start()..=in_memory_tx_num.saturating_sub(1);

            // Set the remaining transaction range for in-memory
            tx_range = in_memory_tx_num..=*tx_range.end();

            items.extend(fetch_from_db(provider, db_range)?);
        }

        // Iterate from the lowest block to the highest in-memory chain
        for block_state in in_mem_chain.into_iter().rev() {
            let block_tx_count = block_state.block_ref().block().body.transactions.len();
            let remaining = (tx_range.end() - tx_range.start() + 1) as usize;

            // If the transaction range start is higher than this block last transaction, advance
            if *tx_range.start() > in_memory_tx_num + block_tx_count as u64 - 1 {
                in_memory_tx_num += block_tx_count as u64;
                continue
            }

            // This should only be more than 0 once, in case of a partial range inside a block.
            let skip = (tx_range.start() - in_memory_tx_num) as usize;

            items.extend(fetch_from_block_state(
                skip..=skip + (remaining.min(block_tx_count - skip) - 1),
                block_state,
            )?);

            in_memory_tx_num += block_tx_count as u64;

            // Break if the range has been fully processed
            if in_memory_tx_num > *tx_range.end() {
                break
            }

            // Set updated range
            tx_range = in_memory_tx_num..=*tx_range.end();
        }

        Ok(items)
    }

    /// Fetches data from either in-memory state or persistent storage by transaction
    /// [`HashOrNumber`].
    fn get_in_memory_or_storage_by_tx<S, M, R>(
        &self,
        id: HashOrNumber,
        fetch_from_db: S,
        fetch_from_block_state: M,
    ) -> ProviderResult<Option<R>>
    where
        S: FnOnce(DatabaseProviderRO<N::DB, N::ChainSpec>) -> ProviderResult<Option<R>>,
        M: Fn(usize, TxNumber, Arc<BlockState>) -> ProviderResult<Option<R>>,
    {
        // Order of instantiation matters. More information on:
        // `get_in_memory_or_storage_by_block_range_while`.
        let in_mem_chain = self.canonical_in_memory_state.canonical_chain().collect::<Vec<_>>();
        let provider = self.database.provider()?;

        // Get the last block number stored in the database which does NOT overlap with in-memory
        // chain.
        let last_database_block_number = in_mem_chain
            .last()
            .map(|b| Ok(b.anchor().number))
            .unwrap_or_else(|| provider.last_block_number())?;

        // Get the next tx number for the last block stored in the database and consider it the
        // first tx number of the in-memory state
        let last_block_body_index = provider
            .block_body_indices(last_database_block_number)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(last_database_block_number))?;
        let mut in_memory_tx_num = last_block_body_index.next_tx_num();

        // If the transaction number is less than the first in-memory transaction number, make a
        // database lookup
        if let HashOrNumber::Number(id) = id {
            if id < in_memory_tx_num {
                return fetch_from_db(provider)
            }
        }

        // Iterate from the lowest block to the highest
        for block_state in in_mem_chain.into_iter().rev() {
            let executed_block = block_state.block_ref();
            let block = executed_block.block();

            for tx_index in 0..block.body.transactions.len() {
                match id {
                    HashOrNumber::Hash(tx_hash) => {
                        if tx_hash == block.body.transactions[tx_index].hash() {
                            return fetch_from_block_state(tx_index, in_memory_tx_num, block_state)
                        }
                    }
                    HashOrNumber::Number(id) => {
                        if id == in_memory_tx_num {
                            return fetch_from_block_state(tx_index, in_memory_tx_num, block_state)
                        }
                    }
                }

                in_memory_tx_num += 1;
            }
        }

        // Not found in-memory, so check database.
        if let HashOrNumber::Hash(_) = id {
            return fetch_from_db(provider)
        }

        Ok(None)
    }

    /// Fetches data from either in-memory state or persistent storage by [`BlockHashOrNumber`].
    fn get_in_memory_or_storage_by_block<S, M, R>(
        &self,
        id: BlockHashOrNumber,
        fetch_from_db: S,
        fetch_from_block_state: M,
    ) -> ProviderResult<R>
    where
        S: FnOnce(DatabaseProviderRO<N::DB, N::ChainSpec>) -> ProviderResult<R>,
        M: Fn(Arc<BlockState>) -> ProviderResult<R>,
    {
        let block_state = match id {
            BlockHashOrNumber::Hash(block_hash) => {
                self.canonical_in_memory_state.state_by_hash(block_hash)
            }
            BlockHashOrNumber::Number(block_number) => {
                self.canonical_in_memory_state.state_by_number(block_number)
            }
        };

        if let Some(block_state) = block_state {
            return fetch_from_block_state(block_state)
        }
        fetch_from_db(self.database_provider_ro()?)
    }
}

impl<N: ProviderNodeTypes> BlockchainProvider2<N> {
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

impl<N: ProviderNodeTypes> DatabaseProviderFactory for BlockchainProvider2<N> {
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

impl<N: ProviderNodeTypes> StaticFileProviderFactory for BlockchainProvider2<N> {
    fn static_file_provider(&self) -> StaticFileProvider {
        self.database.static_file_provider()
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for BlockchainProvider2<N> {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.get_in_memory_or_storage_by_block(
            (*block_hash).into(),
            |db_provider| db_provider.header(block_hash),
            |block_state| Ok(Some(block_state.block_ref().block().header.header().clone())),
        )
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.get_in_memory_or_storage_by_block(
            num.into(),
            |db_provider| db_provider.header_by_number(num),
            |block_state| Ok(Some(block_state.block_ref().block().header.header().clone())),
        )
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        if let Some(num) = self.block_number(*hash)? {
            self.header_td_by_number(num)
        } else {
            Ok(None)
        }
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        let number = if self.canonical_in_memory_state.hash_by_number(number).is_some() {
            // If the block exists in memory, we should return a TD for it.
            //
            // The canonical in memory state should only store post-merge blocks. Post-merge blocks
            // have zero difficulty. This means we can use the total difficulty for the last
            // finalized block number if present (so that we are not affected by reorgs), if not the
            // last number in the database will be used.
            if let Some(last_finalized_num_hash) =
                self.canonical_in_memory_state.get_finalized_num_hash()
            {
                last_finalized_num_hash.number
            } else {
                self.database.last_block_number()?
            }
        } else {
            // Otherwise, return what we have on disk for the input block
            number
        };
        self.database.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.headers_range(range),
            |block_state, _| Some(block_state.block_ref().block().header.header().clone()),
            |_| true,
        )
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.get_in_memory_or_storage_by_block(
            number.into(),
            |db_provider| db_provider.sealed_header(number),
            |block_state| Ok(Some(block_state.block_ref().block().header.clone())),
        )
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.sealed_headers_range(range),
            |block_state, _| Some(block_state.block_ref().block().header.clone()),
            |_| true,
        )
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, predicate| db_provider.sealed_headers_while(range, predicate),
            |block_state, predicate| {
                let header = &block_state.block_ref().block().header;
                predicate(header).then(|| header.clone())
            },
            predicate,
        )
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for BlockchainProvider2<N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.get_in_memory_or_storage_by_block(
            number.into(),
            |db_provider| db_provider.block_hash(number),
            |block_state| Ok(Some(block_state.hash())),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.get_in_memory_or_storage_by_block_range_while(
            start..end,
            |db_provider, inclusive_range, _| {
                db_provider
                    .canonical_hashes_range(*inclusive_range.start(), *inclusive_range.end() + 1)
            },
            |block_state, _| Some(block_state.hash()),
            |_| true,
        )
    }
}

impl<N: ProviderNodeTypes> BlockNumReader for BlockchainProvider2<N> {
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
        self.get_in_memory_or_storage_by_block(
            hash.into(),
            |db_provider| db_provider.block_number(hash),
            |block_state| Ok(Some(block_state.number())),
        )
    }
}

impl<N: ProviderNodeTypes> BlockIdReader for BlockchainProvider2<N> {
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

impl<N: ProviderNodeTypes> BlockReader for BlockchainProvider2<N> {
    fn find_block_by_hash(&self, hash: B256, source: BlockSource) -> ProviderResult<Option<Block>> {
        match source {
            BlockSource::Any | BlockSource::Canonical => {
                // Note: it's fine to return the unsealed block because the caller already has
                // the hash
                self.get_in_memory_or_storage_by_block(
                    hash.into(),
                    |db_provider| db_provider.find_block_by_hash(hash, source),
                    |block_state| Ok(Some(block_state.block_ref().block().clone().unseal())),
                )
            }
            BlockSource::Pending => {
                Ok(self.canonical_in_memory_state.pending_block().map(|block| block.unseal()))
            }
        }
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.block(id),
            |block_state| Ok(Some(block_state.block_ref().block().clone().unseal())),
        )
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
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.ommers(id),
            |block_state| {
                if self
                    .database
                    .chain_spec()
                    .final_paris_total_difficulty(block_state.number())
                    .is_some()
                {
                    return Ok(Some(Vec::new()))
                }

                Ok(Some(block_state.block_ref().block().body.ommers.clone()))
            },
        )
    }

    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.get_in_memory_or_storage_by_block(
            number.into(),
            |db_provider| db_provider.block_body_indices(number),
            |block_state| {
                // Find the last block indices on database
                let last_storage_block_number = block_state.anchor().number;
                let mut stored_indices = self
                    .database
                    .block_body_indices(last_storage_block_number)?
                    .ok_or(ProviderError::BlockBodyIndicesNotFound(last_storage_block_number))?;

                // Prepare our block indices
                stored_indices.first_tx_num = stored_indices.next_tx_num();
                stored_indices.tx_count = 0;

                // Iterate from the lowest block in memory until our target block
                for state in block_state.chain().into_iter().rev() {
                    let block_tx_count = state.block_ref().block.body.transactions.len() as u64;
                    if state.block_ref().block().number == number {
                        stored_indices.tx_count = block_tx_count;
                    } else {
                        stored_indices.first_tx_num += block_tx_count;
                    }
                }

                Ok(Some(stored_indices))
            },
        )
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
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.block_with_senders(id, transaction_kind),
            |block_state| Ok(Some(block_state.block_with_senders())),
        )
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.sealed_block_with_senders(id, transaction_kind),
            |block_state| Ok(Some(block_state.sealed_block_with_senders())),
        )
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.block_range(range),
            |block_state, _| Some(block_state.block_ref().block().clone().unseal()),
            |_| true,
        )
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.block_with_senders_range(range),
            |block_state, _| Some(block_state.block_with_senders()),
            |_| true,
        )
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.sealed_block_with_senders_range(range),
            |block_state, _| Some(block_state.sealed_block_with_senders()),
            |_| true,
        )
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for BlockchainProvider2<N> {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.get_in_memory_or_storage_by_tx(
            tx_hash.into(),
            |db_provider| db_provider.transaction_id(tx_hash),
            |_, tx_number, _| Ok(Some(tx_number)),
        )
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_by_id(id),
            |tx_index, _, block_state| {
                Ok(block_state.block_ref().block().body.transactions.get(tx_index).cloned())
            },
        )
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_by_id_no_hash(id),
            |tx_index, _, block_state| {
                Ok(block_state
                    .block_ref()
                    .block()
                    .body
                    .transactions
                    .get(tx_index)
                    .cloned()
                    .map(Into::into))
            },
        )
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
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_block(id),
            |_, _, block_state| Ok(Some(block_state.block_ref().block().number)),
        )
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |provider| provider.transactions_by_block(id),
            |block_state| Ok(Some(block_state.block_ref().block().body.transactions.clone())),
        )
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.transactions_by_block_range(range),
            |block_state, _| Some(block_state.block_ref().block().body.transactions.clone()),
            |_| true,
        )
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        self.get_in_memory_or_storage_by_tx_range(
            range,
            |db_provider, db_range| db_provider.transactions_by_tx_range(db_range),
            |index_range, block_state| {
                Ok(block_state.block_ref().block().body.transactions[index_range]
                    .iter()
                    .cloned()
                    .map(Into::into)
                    .collect())
            },
        )
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.get_in_memory_or_storage_by_tx_range(
            range,
            |db_provider, db_range| db_provider.senders_by_tx_range(db_range),
            |index_range, block_state| Ok(block_state.block_ref().senders[index_range].to_vec()),
        )
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_sender(id),
            |tx_index, _, block_state| Ok(block_state.block_ref().senders.get(tx_index).copied()),
        )
    }
}

impl<N: ProviderNodeTypes> ReceiptProvider for BlockchainProvider2<N> {
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Receipt>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.receipt(id),
            |tx_index, _, block_state| {
                Ok(block_state.executed_block_receipts().get(tx_index).cloned())
            },
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Receipt>> {
        for block_state in self.canonical_in_memory_state.canonical_chain() {
            let executed_block = block_state.block_ref();
            let block = executed_block.block();
            let receipts = block_state.executed_block_receipts();

            // assuming 1:1 correspondence between transactions and receipts
            debug_assert_eq!(
                block.body.transactions.len(),
                receipts.len(),
                "Mismatch between transaction and receipt count"
            );

            if let Some(tx_index) = block.body.transactions.iter().position(|tx| tx.hash() == hash)
            {
                // safe to use tx_index for receipts due to 1:1 correspondence
                return Ok(receipts.get(tx_index).cloned());
            }
        }

        self.database.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        self.get_in_memory_or_storage_by_block(
            block,
            |db_provider| db_provider.receipts_by_block(block),
            |block_state| Ok(Some(block_state.executed_block_receipts())),
        )
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        self.get_in_memory_or_storage_by_tx_range(
            range,
            |db_provider, db_range| db_provider.receipts_by_tx_range(db_range),
            |index_range, block_state| {
                Ok(block_state.executed_block_receipts().drain(index_range).collect())
            },
        )
    }
}

impl<N: ProviderNodeTypes> ReceiptProviderIdExt for BlockchainProvider2<N> {
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

impl<N: ProviderNodeTypes> WithdrawalsProvider for BlockchainProvider2<N> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        if !self.database.chain_spec().is_shanghai_active_at_timestamp(timestamp) {
            return Ok(None)
        }

        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.withdrawals_by_block(id, timestamp),
            |block_state| Ok(block_state.block_ref().block().body.withdrawals.clone()),
        )
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        let best_block_num = self.best_block_number()?;

        self.get_in_memory_or_storage_by_block(
            best_block_num.into(),
            |db_provider| db_provider.latest_withdrawal(),
            |block_state| {
                Ok(block_state
                    .block_ref()
                    .block()
                    .body
                    .withdrawals
                    .clone()
                    .and_then(|mut w| w.pop()))
            },
        )
    }
}

impl<N: ProviderNodeTypes> RequestsProvider for BlockchainProvider2<N> {
    fn requests_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<reth_primitives::Requests>> {
        if !self.database.chain_spec().is_prague_active_at_timestamp(timestamp) {
            return Ok(None)
        }

        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.requests_by_block(id, timestamp),
            |block_state| Ok(block_state.block_ref().block().body.requests.clone()),
        )
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for BlockchainProvider2<N> {
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

impl<N: ProviderNodeTypes> EvmEnvProvider for BlockchainProvider2<N> {
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_and_block_env(cfg, block_env, header, total_difficulty);
        Ok(())
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
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        let total_difficulty = self
            .header_td_by_number(header.number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(header.number.into()))?;
        evm_config.fill_cfg_env(cfg, header, total_difficulty);
        Ok(())
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for BlockchainProvider2<N> {
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

impl<N: NodeTypesWithDB> ChainSpecProvider for BlockchainProvider2<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<N::ChainSpec> {
        self.database.chain_spec()
    }
}

impl<N: ProviderNodeTypes> StateProviderFactory for BlockchainProvider2<N> {
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
        self.ensure_canonical_block(block_number)?;
        let hash = self
            .block_hash(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        self.history_by_block_hash(hash)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");

        self.get_in_memory_or_storage_by_block(
            block_hash.into(),
            |_| {
                // TODO(joshie): port history_by_block_hash to DatabaseProvider and use db_provider
                self.database.history_by_block_hash(block_hash)
            },
            |block_state| {
                let state_provider = self.block_state_provider(&block_state)?;
                Ok(Box::new(state_provider))
            },
        )
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

impl<N: NodeTypesWithDB> CanonChainTracker for BlockchainProvider2<N>
where
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

impl<N: NodeTypesWithDB> BlockReaderIdExt for BlockchainProvider2<N>
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
            BlockNumberOrTag::Earliest => self.header_by_number(0)?.map_or_else(
                || Ok(None),
                |h| {
                    let sealed = h.seal_slow();
                    let (header, seal) = sealed.into_parts();
                    Ok(Some(SealedHeader::new(header, seal)))
                },
            ),
            BlockNumberOrTag::Pending => Ok(self.canonical_in_memory_state.pending_sealed_header()),
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

impl<N: NodeTypesWithDB> CanonStateSubscriptions for BlockchainProvider2<N> {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        self.canonical_in_memory_state.subscribe_canon_state()
    }
}

impl<N: NodeTypesWithDB> ForkChoiceSubscriptions for BlockchainProvider2<N> {
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications {
        let receiver = self.canonical_in_memory_state.subscribe_safe_block();
        ForkChoiceNotifications(receiver)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications {
        let receiver = self.canonical_in_memory_state.subscribe_finalized_block();
        ForkChoiceNotifications(receiver)
    }
}

impl<N: ProviderNodeTypes> StorageChangeSetReader for BlockchainProvider2<N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if let Some(state) = self.canonical_in_memory_state.state_by_number(block_number) {
            let changesets = state
                .block()
                .execution_output
                .bundle
                .reverts
                .clone()
                .into_plain_state_reverts()
                .storage
                .into_iter()
                .flatten()
                .flat_map(|revert: PlainStorageRevert| {
                    revert.storage_revert.into_iter().map(move |(key, value)| {
                        (
                            BlockNumberAddress((block_number, revert.address)),
                            StorageEntry { key: key.into(), value: value.to_previous_value() },
                        )
                    })
                })
                .collect();
            Ok(changesets)
        } else {
            // Perform checks on whether or not changesets exist for the block.
            let provider = self.database.provider()?;

            // No prune checkpoint means history should exist and we should `unwrap_or(true)`
            let storage_history_exists = provider
                .get_prune_checkpoint(PruneSegment::StorageHistory)?
                .and_then(|checkpoint| {
                    // return true if the block number is ahead of the prune checkpoint.
                    //
                    // The checkpoint stores the highest pruned block number, so we should make
                    // sure the block_number is strictly greater.
                    checkpoint.block_number.map(|checkpoint| block_number > checkpoint)
                })
                .unwrap_or(true);

            if !storage_history_exists {
                return Err(ProviderError::StateAtBlockPruned(block_number))
            }

            provider.storage_changeset(block_number)
        }
    }
}

impl<N: ProviderNodeTypes> ChangeSetReader for BlockchainProvider2<N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        if let Some(state) = self.canonical_in_memory_state.state_by_number(block_number) {
            let changesets = state
                .block_ref()
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
            // Perform checks on whether or not changesets exist for the block.
            let provider = self.database.provider()?;
            // No prune checkpoint means history should exist and we should `unwrap_or(true)`
            let account_history_exists = provider
                .get_prune_checkpoint(PruneSegment::AccountHistory)?
                .and_then(|checkpoint| {
                    // return true if the block number is ahead of the prune checkpoint.
                    //
                    // The checkpoint stores the highest pruned block number, so we should make
                    // sure the block_number is strictly greater.
                    checkpoint.block_number.map(|checkpoint| block_number > checkpoint)
                })
                .unwrap_or(true);

            if !account_history_exists {
                return Err(ProviderError::StateAtBlockPruned(block_number))
            }

            provider.account_block_changeset(block_number)
        }
    }
}

impl<N: ProviderNodeTypes> AccountReader for BlockchainProvider2<N> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        // use latest state provider
        let state_provider = self.latest()?;
        state_provider.basic_account(address)
    }
}

impl<N: ProviderNodeTypes> StateReader for BlockchainProvider2<N> {
    /// Re-constructs the [`ExecutionOutcome`] from in-memory and database state, if necessary.
    ///
    /// If data for the block does not exist, this will return [`None`].
    ///
    /// NOTE: This cannot be called safely in a loop outside of the blockchain tree thread. This is
    /// because the [`CanonicalInMemoryState`] could change during a reorg, causing results to be
    /// inconsistent. Currently this can safely be called within the blockchain tree thread,
    /// because the tree thread is responsible for modifying the [`CanonicalInMemoryState`] in the
    /// first place.
    fn get_state(&self, block: BlockNumber) -> ProviderResult<Option<ExecutionOutcome>> {
        if let Some(state) = self.canonical_in_memory_state.state_by_number(block) {
            let state = state.block_ref().execution_outcome().clone();
            Ok(Some(state))
        } else {
            self.get_state(block..=block)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ops::{Range, RangeBounds},
        sync::Arc,
        time::Instant,
    };

    use crate::{
        providers::BlockchainProvider2,
        test_utils::{
            create_test_provider_factory, create_test_provider_factory_with_chain_spec,
            MockNodeTypesWithDB,
        },
        writer::UnifiedStorageWriter,
        BlockWriter, CanonChainTracker, ProviderFactory, StaticFileProviderFactory,
        StaticFileWriter,
    };
    use alloy_eips::{BlockHashOrNumber, BlockNumHash, BlockNumberOrTag};
    use alloy_primitives::{BlockNumber, B256};
    use itertools::Itertools;
    use rand::Rng;
    use reth_chain_state::{
        test_utils::TestBlockBuilder, CanonStateNotification, CanonStateSubscriptions,
        CanonicalInMemoryState, ExecutedBlock, NewCanonicalChain,
    };
    use reth_chainspec::{
        ChainSpec, ChainSpecBuilder, ChainSpecProvider, EthereumHardfork, MAINNET,
    };
    use reth_db::{
        models::{AccountBeforeTx, StoredBlockBodyIndices},
        tables,
    };
    use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
    use reth_errors::ProviderError;
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_primitives::{
        Receipt, SealedBlock, StaticFileSegment, TransactionMeta, TransactionSignedNoHash,
        Withdrawals,
    };
    use reth_storage_api::{
        BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt, BlockSource,
        ChangeSetReader, DatabaseProviderFactory, HeaderProvider, ReceiptProvider,
        ReceiptProviderIdExt, RequestsProvider, StateProviderFactory, TransactionVariant,
        TransactionsProvider, WithdrawalsProvider,
    };
    use reth_testing_utils::generators::{
        self, random_block, random_block_range, random_changeset_range, random_eoa_accounts,
        random_receipt, BlockParams, BlockRangeParams,
    };
    use revm::db::BundleState;
    use std::ops::Bound;

    const TEST_BLOCKS_COUNT: usize = 5;

    const TEST_TRANSACTIONS_COUNT: u8 = 4;

    fn random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        requests_count: Option<Range<u8>>,
        withdrawals_count: Option<Range<u8>>,
        tx_count: impl RangeBounds<u8>,
    ) -> (Vec<SealedBlock>, Vec<SealedBlock>) {
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

    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn provider_with_chain_spec_and_random_blocks(
        rng: &mut impl Rng,
        chain_spec: Arc<ChainSpec>,
        database_blocks: usize,
        in_memory_blocks: usize,
        block_range_params: BlockRangeParams,
    ) -> eyre::Result<(
        BlockchainProvider2<MockNodeTypesWithDB>,
        Vec<SealedBlock>,
        Vec<SealedBlock>,
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
            .map(|block| block.body.transactions.iter())
            .map(|tx| tx.map(|tx| random_receipt(rng, tx, Some(2))).collect())
            .collect();

        let factory = create_test_provider_factory_with_chain_spec(chain_spec);
        let provider_rw = factory.database_provider_rw()?;
        let static_file_provider = factory.static_file_provider();

        // Write transactions to static files with the right `tx_num``
        let mut bodies_cursor = provider_rw.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;
        let mut tx_num = bodies_cursor
            .seek_exact(database_blocks.first().as_ref().unwrap().number.saturating_sub(1))?
            .map(|(_, indices)| indices.next_tx_num())
            .unwrap_or_default();

        // Insert blocks into the database
        for block in &database_blocks {
            // TODO: this should be moved inside `insert_historical_block`: <https://github.com/paradigmxyz/reth/issues/11524>
            let mut transactions_writer =
                static_file_provider.latest_writer(StaticFileSegment::Transactions)?;
            transactions_writer.increment_block(block.number)?;
            for tx in block.body.transactions() {
                let tx: TransactionSignedNoHash = tx.clone().into();
                transactions_writer.append_transaction(tx_num, &tx)?;
                tx_num += 1;
            }

            provider_rw.insert_historical_block(
                block.clone().seal_with_senders().expect("failed to seal block with senders"),
            )?;
        }

        // Insert receipts into the static files
        UnifiedStorageWriter::new(
            &provider_rw,
            Some(factory.static_file_provider().latest_writer(StaticFileSegment::Receipts)?),
        )
        .append_receipts_from_blocks(
            // The initial block number is required
            database_blocks.first().map(|b| b.number).unwrap_or_default(),
            receipts[..database_blocks.len()]
                .iter()
                .map(|vec| vec.clone().into_iter().map(Some).collect::<Vec<_>>()),
        )?;

        // Commit to both storages: database and static files
        UnifiedStorageWriter::commit(provider_rw, factory.static_file_provider())?;

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

        Ok((provider, database_blocks.clone(), in_memory_blocks.clone(), receipts))
    }

    #[allow(clippy::type_complexity)]
    fn provider_with_random_blocks(
        rng: &mut impl Rng,
        database_blocks: usize,
        in_memory_blocks: usize,
        block_range_params: BlockRangeParams,
    ) -> eyre::Result<(
        BlockchainProvider2<MockNodeTypesWithDB>,
        Vec<SealedBlock>,
        Vec<SealedBlock>,
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
        provider: Arc<BlockchainProvider2<MockNodeTypesWithDB>>,
        block_number: BlockNumber,
    ) {
        let hook_provider = provider.clone();
        provider.database.db_ref().set_post_transaction_hook(Box::new(move || {
            if let Some(state) = hook_provider.canonical_in_memory_state.head_state() {
                if state.anchor().number + 1 == block_number {
                    let mut lowest_memory_block =
                        state.parent_state_chain().last().expect("qed").block();
                    let num_hash = lowest_memory_block.block().num_hash();

                    let mut execution_output = (*lowest_memory_block.execution_output).clone();
                    execution_output.first_block = lowest_memory_block.block().number;
                    lowest_memory_block.execution_output = Arc::new(execution_output);

                    // Push to disk
                    let provider_rw = hook_provider.database_provider_rw().unwrap();
                    UnifiedStorageWriter::from(&provider_rw, &hook_provider.static_file_provider())
                        .save_blocks(&[lowest_memory_block])
                        .unwrap();
                    UnifiedStorageWriter::commit(provider_rw, hook_provider.static_file_provider())
                        .unwrap();

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
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

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
            Some(first_in_mem_block.body.ommers.clone())
        );
        assert_eq!(
            provider.ommers(first_in_mem_block.hash().into())?,
            Some(first_in_mem_block.body.ommers.clone())
        );

        // A random hash should return None as the block number is not found
        assert_eq!(provider.ommers(B256::random().into())?, None);

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
            new: vec![ExecutedBlock::new(
                Arc::new(first_in_mem_block.clone()),
                Arc::new(in_memory_block_senders),
                Default::default(),
                Default::default(),
                Default::default(),
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
        let mut rng = rand::thread_rng();
        let random_block_number: u64 = rng.gen();
        assert_eq!(provider.block_body_indices(random_block_number)?, None);

        Ok(())
    }

    #[test]
    fn test_block_with_senders_by_hash_in_memory() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first in-memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let block_hash = first_in_mem_block.hash();

        // Get the block with senders by hash and check if it matches the first in-memory block
        let block_with_senders = provider
            .block_with_senders(BlockHashOrNumber::Hash(block_hash), TransactionVariant::WithHash)?
            .unwrap();
        assert_eq!(block_with_senders.block.seal(block_hash), first_in_mem_block.clone());
        assert_eq!(block_with_senders.senders, first_in_mem_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_block_with_senders_by_number_in_memory() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first in-memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let block_number = first_in_mem_block.number;

        // Get the block with senders by number and check if it matches the first in-memory block
        let block_with_senders = provider
            .block_with_senders(
                BlockHashOrNumber::Number(block_number),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(
            block_with_senders.block.seal(first_in_mem_block.hash()),
            first_in_mem_block.clone()
        );
        assert_eq!(block_with_senders.senders, first_in_mem_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_block_with_senders_by_hash_in_database() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first database block
        let first_db_block = database_blocks.first().unwrap();
        let block_hash = first_db_block.hash();

        // Get the block with senders by hash and check if it matches the first database block
        let block_with_senders = provider
            .block_with_senders(BlockHashOrNumber::Hash(block_hash), TransactionVariant::WithHash)?
            .unwrap();
        assert_eq!(block_with_senders.block.seal(block_hash), first_db_block.clone());
        assert_eq!(block_with_senders.senders, first_db_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_block_with_senders_by_number_in_database() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first database block
        let first_db_block = database_blocks.first().unwrap();
        let block_number = first_db_block.number;

        // Get the block with senders by number and check if it matches the first database block
        let block_with_senders = provider
            .block_with_senders(
                BlockHashOrNumber::Number(block_number),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(block_with_senders.block.seal(first_db_block.hash()), first_db_block.clone());
        assert_eq!(block_with_senders.senders, first_db_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_block_with_senders_non_existent_block() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Generate a random hash (non-existent block)
        let non_existent_hash = B256::random();
        let result = provider.block_with_senders(
            BlockHashOrNumber::Hash(non_existent_hash),
            TransactionVariant::WithHash,
        )?;
        // The block should not be found
        assert!(result.is_none());

        // Generate a random number (non-existent block)
        let non_existent_number = 9999;
        let result = provider.block_with_senders(
            BlockHashOrNumber::Number(non_existent_number),
            TransactionVariant::WithHash,
        )?;
        // The block should not be found
        assert!(result.is_none());

        Ok(())
    }

    #[test]
    fn test_sealed_block_with_senders_by_hash_in_memory() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first in-memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let block_hash = first_in_mem_block.hash();

        // Get the sealed block with senders by hash and check if it matches the first in-memory
        // block
        let sealed_block_with_senders = provider
            .sealed_block_with_senders(
                BlockHashOrNumber::Hash(block_hash),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(sealed_block_with_senders.block, first_in_mem_block.clone());
        assert_eq!(sealed_block_with_senders.senders, first_in_mem_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_sealed_block_with_senders_by_number_in_memory() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first in-memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let block_number = first_in_mem_block.number;

        // Get the sealed block with senders by number and check if it matches the first in-memory
        let sealed_block_with_senders = provider
            .sealed_block_with_senders(
                BlockHashOrNumber::Number(block_number),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(sealed_block_with_senders.block, first_in_mem_block.clone());
        assert_eq!(sealed_block_with_senders.senders, first_in_mem_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_sealed_block_with_senders_by_hash_in_database() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first database block
        let first_db_block = database_blocks.first().unwrap();
        let block_hash = first_db_block.hash();

        // Get the sealed block with senders by hash and check if it matches the first database
        // block
        let sealed_block_with_senders = provider
            .sealed_block_with_senders(
                BlockHashOrNumber::Hash(block_hash),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(sealed_block_with_senders.block, first_db_block.clone());
        assert_eq!(sealed_block_with_senders.senders, first_db_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_sealed_block_with_senders_by_number_in_database() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Get the first database block
        let first_db_block = database_blocks.first().unwrap();
        let block_number = first_db_block.number;

        // Get the sealed block with senders by number and check if it matches the first database
        // block
        let sealed_block_with_senders = provider
            .sealed_block_with_senders(
                BlockHashOrNumber::Number(block_number),
                TransactionVariant::WithHash,
            )?
            .unwrap();
        assert_eq!(sealed_block_with_senders.block, first_db_block.clone());
        assert_eq!(sealed_block_with_senders.senders, first_db_block.senders().unwrap());

        Ok(())
    }

    #[test]
    fn test_sealed_block_with_senders_non_existent_block() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        // Generate a random hash (non-existent block)
        let non_existent_hash = B256::random();
        let result = provider.sealed_block_with_senders(
            BlockHashOrNumber::Hash(non_existent_hash),
            TransactionVariant::WithHash,
        )?;
        // The block should not be found
        assert!(result.is_none());

        // Generate a random number (non-existent block)
        let non_existent_number = 9999;
        let result = provider.sealed_block_with_senders(
            BlockHashOrNumber::Number(non_existent_number),
            TransactionVariant::WithHash,
        )?;
        // The block should not be found
        assert!(result.is_none());

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
        provider.set_finalized(finalized_block.header.clone());

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
            provider.sealed_header(database_block.number)?,
            Some(database_block.header.clone())
        );
        assert_eq!(
            provider.sealed_header(in_memory_block.number)?,
            Some(in_memory_block.header.clone())
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

    #[tokio::test]
    async fn test_canon_state_subscriptions() -> eyre::Result<()> {
        let factory = create_test_provider_factory();

        // Generate a random block to initialise the blockchain provider.
        let mut test_block_builder = TestBlockBuilder::default();
        let block_1 = test_block_builder.generate_random_block(0, B256::ZERO);
        let block_hash_1 = block_1.hash();

        // Insert and commit the block.
        let provider_rw = factory.provider_rw()?;
        provider_rw.insert_historical_block(block_1)?;
        provider_rw.commit()?;

        let provider = BlockchainProvider2::new(factory)?;

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
    fn test_withdrawals_provider() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());
        let (provider, database_blocks, in_memory_blocks, _) =
            provider_with_chain_spec_and_random_blocks(
                &mut rng,
                chain_spec.clone(),
                TEST_BLOCKS_COUNT,
                TEST_BLOCKS_COUNT,
                BlockRangeParams { withdrawals_count: Some(1..3), ..Default::default() },
            )?;
        let blocks = [database_blocks, in_memory_blocks].concat();

        let shainghai_timestamp =
            chain_spec.hardforks.fork(EthereumHardfork::Shanghai).as_timestamp().unwrap();

        assert_eq!(
            provider
                .withdrawals_by_block(
                    reth_primitives::BlockHashOrNumber::Number(15),
                    shainghai_timestamp
                )
                .expect("could not call withdrawals by block"),
            Some(Withdrawals::new(vec![])),
            "Expected withdrawals_by_block to return empty list if block does not exist"
        );

        for block in blocks.clone() {
            assert_eq!(
                provider
                    .withdrawals_by_block(
                        reth_primitives::BlockHashOrNumber::Number(block.number),
                        shainghai_timestamp
                    )?
                    .unwrap(),
                block.body.withdrawals.unwrap(),
                "Expected withdrawals_by_block to return correct withdrawals"
            );
        }

        let canonical_block_num = provider.best_block_number().unwrap();
        let canonical_block = blocks.get(canonical_block_num as usize).unwrap();

        assert_eq!(
            Some(provider.latest_withdrawal()?.unwrap()),
            canonical_block.body.withdrawals.clone().unwrap().pop(),
            "Expected latest withdrawal to be equal to last withdrawal entry in canonical block"
        );

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
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

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
        let (provider, database_blocks, in_memory_blocks, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let block_number = database_block.number;
        let block_hash = database_block.header.hash();

        assert_eq!(
            provider.ommers_by_id(block_number.into()).unwrap().unwrap_or_default(),
            database_block.body.ommers
        );
        assert_eq!(
            provider.ommers_by_id(block_hash.into()).unwrap().unwrap_or_default(),
            database_block.body.ommers
        );

        let block_number = in_memory_block.number;
        let block_hash = in_memory_block.header.hash();

        assert_eq!(
            provider.ommers_by_id(block_number.into()).unwrap().unwrap_or_default(),
            in_memory_block.body.ommers
        );
        assert_eq!(
            provider.ommers_by_id(block_hash.into()).unwrap().unwrap_or_default(),
            in_memory_block.body.ommers
        );

        Ok(())
    }

    #[test]
    fn test_receipt_provider() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams { tx_count: 1..3, ..Default::default() },
        )?;

        let blocks = [database_blocks, in_memory_blocks].concat();

        for block in blocks {
            let block_number = block.number as usize;
            for (txn_number, _) in block.body.transactions.iter().enumerate() {
                let txn_hash = block.body.transactions.get(txn_number).unwrap().hash();
                let txn_id = provider.transaction_id(txn_hash)?.unwrap();
                assert_eq!(
                    provider.receipt(txn_id)?.unwrap(),
                    receipts.get(block_number).unwrap().clone().get(txn_number).unwrap().clone()
                );
                assert_eq!(
                    provider.receipt_by_hash(txn_hash)?.unwrap(),
                    receipts.get(block_number).unwrap().clone().get(txn_number).unwrap().clone()
                );
            }
        }

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
        let block_hash = database_block.header.hash();

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
        let block_hash = in_memory_block.header.hash();

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
                BlockRangeParams { requests_count: Some(1..2), ..Default::default() },
            )?;

        let database_block = database_blocks.first().unwrap().clone();
        let in_memory_block = in_memory_blocks.last().unwrap().clone();

        let prague_timestamp =
            chain_spec.hardforks.fork(EthereumHardfork::Prague).as_timestamp().unwrap();

        assert_eq!(
            provider.requests_by_block(database_block.number.into(), prague_timestamp,)?,
            database_block.body.requests.clone()
        );
        assert_eq!(
            provider.requests_by_block(in_memory_block.number.into(), prague_timestamp,)?,
            in_memory_block.body.requests.clone()
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
        only_database_provider.canonical_in_memory_state.set_pending_block(ExecutedBlock {
            block: Arc::new(pending_block.clone()),
            senders: Default::default(),
            execution_output: Default::default(),
            hashed_state: Default::default(),
            trie: Default::default(),
        });

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
        in_memory_provider.canonical_in_memory_state.set_safe(safe_block.header.clone());
        assert_eq!(
            safe_block.hash(),
            in_memory_provider
                .state_by_block_number_or_tag(BlockNumberOrTag::Safe)?
                .block_hash(safe_block.number)?
                .unwrap()
        );
        // test state by block tag for finalized block
        let finalized_block = in_memory_blocks[in_memory_blocks.len() - 3].clone();
        in_memory_provider.canonical_in_memory_state.set_finalized(finalized_block.header.clone());
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
    fn test_canon_state_tracker() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, _, _, _) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams::default(),
        )?;

        let before = Instant::now();
        provider.on_forkchoice_update_received(&Default::default());
        let last_update_ts = provider.last_received_update_timestamp().unwrap();
        let after = Instant::now();

        // Ensure the timestamp is updated and between the before and after timestamps
        assert!(before < last_update_ts && last_update_ts < after);

        let before = Instant::now();
        provider.on_transition_configuration_exchanged();
        let last_update_ts = provider.last_exchanged_transition_configuration_timestamp().unwrap();
        let after = Instant::now();

        // Ensure the timestamp is updated and between the before and after timestamps
        assert!(before < last_update_ts && last_update_ts < after);

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
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlock {
            block: Arc::new(pending_block.clone()),
            senders: Default::default(),
            execution_output: Default::default(),
            hashed_state: Default::default(),
            trie: Default::default(),
        });

        // Set the safe block in memory
        let safe_block = in_memory_blocks[in_memory_blocks.len() - 2].clone();
        provider.canonical_in_memory_state.set_safe(safe_block.header.clone());

        // Set the finalized block in memory
        let finalized_block = in_memory_blocks[in_memory_blocks.len() - 3].clone();
        provider.canonical_in_memory_state.set_finalized(finalized_block.header.clone());

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

    #[test]
    fn test_transaction_id() -> eyre::Result<()> {
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

        // Database
        // Choose a random transaction from the database blocks
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Ensure the transaction ID can be found in the database
        let result = provider.transaction_id(tx_hash)?;
        assert_eq!(result, Some(0));

        // In memory
        // Choose a random transaction from the in-memory blocks
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Ensure the transaction ID can be found in the in-memory state
        let result = provider.transaction_id(tx_hash)?;
        assert!(result.is_some(), "Transaction ID should be found in the in-memory state");

        // Check that the transaction ID is greater than the last database transaction ID
        let last_db_tx_id = provider.database.last_block_number()?;
        let last_db_tx_id =
            provider.database.block_body_indices(last_db_tx_id)?.unwrap().last_tx_num();

        assert!(
            result.unwrap() > last_db_tx_id,
            "In-memory transaction ID should be greater than the last database transaction ID"
        );
        assert_eq!(result, Some(last_db_tx_id + 1));

        // Generate a random hash not present in any transaction
        let random_tx_hash = B256::random();

        // Ensure the transaction ID is not found
        let result = provider.transaction_id(random_tx_hash)?;
        assert!(result.is_none(), "Transaction ID should not be found");

        Ok(())
    }

    #[test]
    fn test_transaction_by_id() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction ID from in-memory blocks
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Ensure the transaction can be retrieved by its ID
        let result = provider.transaction_by_id(tx_id)?;
        assert_eq!(
            result.unwrap(),
            *tx,
            "The retrieved transaction should match the expected transaction"
        );

        // Database
        // Choose a random transaction ID from the database blocks
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Ensure the transaction can be retrieved by its ID
        let result = provider.transaction_by_id(tx_id)?;
        assert!(result.is_some(), "Transaction should be found in the database");
        assert_eq!(
            result.unwrap(),
            *tx,
            "The retrieved transaction should match the expected transaction"
        );

        // Generate a random transaction ID not present in any block
        let random_tx_id = 999999;

        // Ensure the transaction is not found
        let result = provider.transaction_by_id(random_tx_id)?;
        assert!(result.is_none(), "Transaction should not be found");

        Ok(())
    }

    #[test]
    fn test_transaction_by_id_no_hash() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction ID from in-memory blocks
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Ensure the transaction can be retrieved by its ID without hash
        let result = provider.transaction_by_id_no_hash(tx_id)?;
        let expected_tx: TransactionSignedNoHash = tx.clone().into();
        assert_eq!(
            result.unwrap(),
            expected_tx,
            "The retrieved transaction without hash should match the expected transaction"
        );

        // Database
        // Choose a random transaction ID from the database blocks
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Ensure the transaction can be retrieved by its ID without hash
        let result = provider.transaction_by_id_no_hash(tx_id)?;
        let expected_tx: TransactionSignedNoHash = tx.clone().into();
        assert_eq!(
            result.unwrap(),
            expected_tx,
            "The retrieved transaction without hash should match the expected transaction"
        );

        // Generate a random transaction ID not present in any block
        let random_tx_id = 7656898;

        // Ensure the transaction is not found without hash
        let result = provider.transaction_by_id_no_hash(random_tx_id)?;
        assert!(result.is_none(), "Transaction should not be found without hash");

        Ok(())
    }

    #[test]
    fn test_transaction_by_hash() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction hash from the in-memory blocks
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Ensure the transaction can be retrieved by its hash from the in-memory state
        let result = provider.transaction_by_hash(tx_hash)?;
        assert_eq!(
            result.unwrap(),
            *tx,
            "The retrieved transaction should match the expected transaction"
        );

        // Database
        // Choose a random transaction hash from the database blocks
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Ensure the transaction can be retrieved by its hash from the database
        let result = provider.transaction_by_hash(tx_hash)?;
        assert_eq!(
            result.unwrap(),
            *tx,
            "The retrieved transaction should match the expected transaction"
        );

        // Generate a random hash not present in any transaction
        let random_tx_hash = B256::random();

        // Ensure the transaction is not found by the random hash
        let result = provider.transaction_by_hash(random_tx_hash)?;
        assert!(result.is_none(), "Transaction should not be found");

        Ok(())
    }

    #[test]
    fn test_transaction_by_hash_with_meta() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction from the in-memory block
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Create the expected metadata for this transaction
        let meta = TransactionMeta {
            tx_hash,
            index: 0,
            block_hash: in_memory_blocks[0].header.hash(),
            block_number: in_memory_blocks[0].header.number,
            base_fee: in_memory_blocks[0].header.base_fee_per_gas,
            excess_blob_gas: None,
            timestamp: in_memory_blocks[0].header.timestamp,
        };

        // Ensure the transaction and its metadata can be retrieved from the in-memory state
        let result = provider.transaction_by_hash_with_meta(tx_hash)?;
        let (retrieved_tx, retrieved_meta) = result.unwrap();
        assert_eq!(
            retrieved_tx, *tx,
            "The retrieved transaction should match the expected transaction"
        );
        assert_eq!(
            retrieved_meta, meta,
            "The retrieved metadata should match the expected metadata"
        );

        // Database
        // Choose a random transaction from the database blocks
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Create the expected metadata for this transaction
        let meta = TransactionMeta {
            tx_hash,
            index: 0,
            block_hash: database_blocks[0].header.hash(),
            block_number: database_blocks[0].header.number,
            base_fee: database_blocks[0].header.base_fee_per_gas,
            excess_blob_gas: None,
            timestamp: database_blocks[0].header.timestamp,
        };

        // Ensure the transaction and its metadata can be retrieved from the database
        let result = provider.transaction_by_hash_with_meta(tx_hash)?;
        let (retrieved_tx, retrieved_meta) = result.unwrap();
        assert_eq!(
            retrieved_tx, *tx,
            "The retrieved transaction should match the expected transaction"
        );
        assert_eq!(
            retrieved_meta, meta,
            "The retrieved metadata should match the expected metadata"
        );

        // Generate a random hash not present in any transaction
        let random_tx_hash = B256::random();

        // Ensure the transaction with metadata is not found by the random hash
        let result = provider.transaction_by_hash_with_meta(random_tx_hash)?;
        assert!(result.is_none(), "Transaction with metadata should not be found");

        Ok(())
    }

    #[test]
    fn test_transaction_block() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction ID from in-memory blocks
        let tx = &in_memory_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Retrieve the block number for this transaction
        let result = provider.transaction_block(tx_id)?;
        let block_number = result.unwrap();

        // Ensure the block number matches the expected block number
        assert_eq!(
            block_number, in_memory_blocks[0].header.number,
            "The block number should match the in-memory block number"
        );

        // Database
        // Choose a random transaction from the database block
        let tx = &database_blocks[0].body.transactions[0];
        let tx_hash = tx.hash();

        // Fetch the transaction ID
        let tx_id = provider.transaction_id(tx_hash)?.unwrap();

        // Retrieve the block number for this transaction
        let result = provider.transaction_block(tx_id)?;
        assert_eq!(Some(0), result, "The block number should match the database block number");

        // Ensure that invalid transaction ID returns None
        let result = provider.transaction_block(67675657)?;

        assert!(result.is_none(), "Block number should not be found for an invalid transaction ID");

        Ok(())
    }

    #[test]
    fn transactions_found_by_block_hash() -> eyre::Result<()> {
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

        // In memory
        // Choose a random block hash from in-memory blocks
        let block_hash = in_memory_blocks[0].header.hash();

        // Retrieve the transactions for this block using the block hash
        let result = provider.transactions_by_block(BlockHashOrNumber::Hash(block_hash))?;
        let transactions = result.unwrap();

        // Ensure the transactions match the expected transactions in the block
        assert_eq!(
            transactions, in_memory_blocks[0].body.transactions,
            "The transactions should match the in-memory block transactions"
        );

        // Database
        // Choose a random block hash from the database blocks
        let block_hash = database_blocks[0].header.hash();

        // Retrieve the transactions for this block using the block hash
        let result = provider.transactions_by_block(BlockHashOrNumber::Hash(block_hash))?;
        let transactions = result.unwrap();

        // Ensure the transactions match the expected transactions in the block
        assert_eq!(
            transactions, database_blocks[0].body.transactions,
            "The transactions should match the database block transactions"
        );

        // Generate a random block hash that does not exist
        let random_block_hash = B256::random();

        // Try to retrieve transactions for a non-existent block hash
        let result = provider.transactions_by_block(BlockHashOrNumber::Hash(random_block_hash))?;

        // Ensure no transactions are found
        assert!(result.is_none(), "No transactions should be found for a non-existent block hash");

        Ok(())
    }

    #[test]
    fn transactions_found_by_block_number() -> eyre::Result<()> {
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

        // In memory
        // Choose a random block number from in-memory blocks
        let block_number = in_memory_blocks[0].header.number;

        // Retrieve the transactions for this block using the block number
        let result = provider.transactions_by_block(BlockHashOrNumber::Number(block_number))?;
        let transactions = result.unwrap();

        // Ensure the transactions match the expected transactions in the block
        assert_eq!(
            transactions, in_memory_blocks[0].body.transactions,
            "The transactions should match the in-memory block transactions"
        );

        // Database
        // Choose a random block number from the database blocks
        let block_number = database_blocks[0].header.number;

        // Retrieve the transactions for this block using the block number
        let result = provider.transactions_by_block(BlockHashOrNumber::Number(block_number))?;
        let transactions = result.unwrap();

        // Ensure the transactions match the expected transactions in the block
        assert_eq!(
            transactions, database_blocks[0].body.transactions,
            "The transactions should match the database block transactions"
        );

        // Generate a block number that is out of range (non-existent)
        let non_existent_block_number = u64::MAX;

        // Try to retrieve transactions for a non-existent block number
        let result =
            provider.transactions_by_block(BlockHashOrNumber::Number(non_existent_block_number))?;

        // Ensure no transactions are found
        assert!(
            result.is_none(),
            "No transactions should be found for a non-existent block number"
        );

        Ok(())
    }

    #[test]
    fn transactions_found_entirely_in_memory() -> eyre::Result<()> {
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

        // In memory
        // Define a block range entirely within in-memory blocks
        let start_block = in_memory_blocks[0].header.number;
        let end_block = in_memory_blocks[1].header.number;

        // Retrieve the transactions for this block range
        let result = provider.transactions_by_block_range(start_block..=end_block)?;

        // Ensure the transactions match the expected transactions in the in-memory blocks
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], in_memory_blocks[0].body.transactions);
        assert_eq!(result[1], in_memory_blocks[1].body.transactions);

        // Database
        // Define a block range entirely within database blocks
        let start_block = database_blocks[0].header.number;
        let end_block = database_blocks[1].header.number;

        // Retrieve the transactions for this block range
        let result = provider.transactions_by_block_range(start_block..=end_block)?;

        // Ensure the transactions match the expected transactions in the database blocks
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], database_blocks[0].body.transactions);
        assert_eq!(result[1], database_blocks[1].body.transactions);

        Ok(())
    }

    macro_rules! test_by_tx_range {
        ($provider:expr, $database_blocks:expr, $in_memory_blocks:expr, [$(($method:ident, $data_extractor:expr)),* $(,)?]) => {{
            let db_tx_count =
                $database_blocks.iter().map(|b| b.body.transactions.len()).sum::<usize>() as u64;
            let in_mem_tx_count =
                $in_memory_blocks.iter().map(|b| b.body.transactions.len()).sum::<usize>() as u64;

            let db_range = 0..=(db_tx_count - 1);
            let in_mem_range = db_tx_count..=(in_mem_tx_count + db_range.end());

            $(
                // Retrieve the expected database data
                let database_data =
                    $database_blocks.iter().flat_map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!($provider.$method(db_range.clone())?, database_data);

                // Retrieve the expected in-memory data
                let in_memory_data =
                    $in_memory_blocks.iter().flat_map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!($provider.$method(in_mem_range.clone())?, in_memory_data);

                // Test partial in-memory range
                assert_eq!(
                    &$provider.$method(in_mem_range.start() + 1..=in_mem_range.end() - 1)?,
                    &in_memory_data[1..in_memory_data.len() - 1]
                );

                // Test range in in-memory to unbounded end
                assert_eq!($provider.$method(in_mem_range.start() + 1..)?, &in_memory_data[1..]);

                // Test last element in-memory
                assert_eq!($provider.$method(in_mem_range.end()..)?, &in_memory_data[in_memory_data.len() -1 ..]);

                // Test range that spans database and in-memory
                assert_eq!(
                    $provider.$method(in_mem_range.start() - 2..=in_mem_range.end() - 1)?,
                    database_data[database_data.len() - 2..]
                        .iter()
                        .chain(&in_memory_data[..in_memory_data.len() - 1])
                        .cloned()
                        .collect::<Vec<_>>()
                );

                // Test range that spans database and in-memory with unbounded end
                assert_eq!(
                    $provider.$method(in_mem_range.start() - 2..)?,
                    database_data[database_data.len() - 2..]
                        .iter()
                        .chain(&in_memory_data[..])
                        .cloned()
                        .collect::<Vec<_>>()
                );

                // Test invalid range
                let start_tx_num = u64::MAX;
                let end_tx_num = u64::MAX;
                let result = $provider.$method(start_tx_num..end_tx_num)?;
                assert!(result.is_empty(), "No data should be found for an invalid transaction range");

                // Test empty range
                let result = $provider.$method(in_mem_range.end()+10..in_mem_range.end()+20)?;
                assert!(result.is_empty(), "No data should be found for an empty transaction range");
            )*
        }};
    }

    #[test]
    fn test_methods_by_tx_range() -> eyre::Result<()> {
        let mut rng = generators::rng();
        let (provider, database_blocks, in_memory_blocks, receipts) = provider_with_random_blocks(
            &mut rng,
            TEST_BLOCKS_COUNT,
            TEST_BLOCKS_COUNT,
            BlockRangeParams {
                tx_count: TEST_TRANSACTIONS_COUNT..TEST_TRANSACTIONS_COUNT,
                ..Default::default()
            },
        )?;

        test_by_tx_range!(
            provider,
            database_blocks,
            in_memory_blocks,
            [
                (senders_by_tx_range, |block: &SealedBlock| block.senders().unwrap()),
                (transactions_by_tx_range, |block: &SealedBlock| block
                    .body
                    .transactions
                    .iter()
                    .map(|tx| Into::<TransactionSignedNoHash>::into(tx.clone()))
                    .collect::<Vec<_>>()),
                (receipts_by_tx_range, |block: &SealedBlock| receipts[block.number as usize]
                    .clone())
            ]
        );

        Ok(())
    }

    macro_rules! test_by_block_range {
        ($provider:expr, $database_blocks:expr, $in_memory_blocks:expr, [$(($method:ident, $data_extractor:expr)),* $(,)?]) => {{
            let db_block_count = $database_blocks.len() as u64;
            let in_mem_block_count = $in_memory_blocks.len() as u64;

            let db_range = 0..=db_block_count - 1;
            let in_mem_range = db_block_count..=(in_mem_block_count + db_range.end());

            $(
                // Retrieve the expected database data
                let database_data =
                    $database_blocks.iter().map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!($provider.$method(db_range.clone())?, database_data);

                // Retrieve the expected in-memory data
                let in_memory_data =
                    $in_memory_blocks.iter().map(|b| $data_extractor(b)).collect::<Vec<_>>();
                assert_eq!($provider.$method(in_mem_range.clone())?, in_memory_data);

                // Test partial in-memory range
                assert_eq!(
                    &$provider.$method(in_mem_range.start() + 1..=in_mem_range.end() - 1)?,
                    &in_memory_data[1..in_memory_data.len() - 1]
                );

                // Test range that spans database and in-memory
                assert_eq!(
                    $provider.$method(in_mem_range.start() - 2..=in_mem_range.end() - 1)?,
                    database_data[database_data.len() - 2..]
                        .iter()
                        .chain(&in_memory_data[..in_memory_data.len() - 1])
                        .cloned()
                        .collect::<Vec<_>>()
                );

                // Test invalid range
                let start_block_num = u64::MAX;
                let end_block_num = u64::MAX;
                let result = $provider.$method(start_block_num..=end_block_num-1)?;
                assert!(result.is_empty(), "No data should be found for an invalid block range");

                // Test valid range with empty results
                let result = $provider.$method(in_mem_range.end() + 10..=in_mem_range.end() + 20)?;
                assert!(result.is_empty(), "No data should be found for an empty block range");
            )*
        }};
    }

    #[test]
    fn test_methods_by_block_range() -> eyre::Result<()> {
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

        // todo(joshie) add canonical_hashes_range below after changing its interface into range
        // instead start end
        test_by_block_range!(
            provider,
            database_blocks,
            in_memory_blocks,
            [
                (headers_range, |block: &SealedBlock| block.header().clone()),
                (sealed_headers_range, |block: &SealedBlock| block.header.clone()),
                (block_range, |block: &SealedBlock| block.clone().unseal()),
                (block_with_senders_range, |block: &SealedBlock| block
                    .clone()
                    .unseal()
                    .with_senders_unchecked(vec![])),
                (sealed_block_with_senders_range, |block: &SealedBlock| block
                    .clone()
                    .with_senders_unchecked(vec![])),
                (transactions_by_block_range, |block: &SealedBlock| block
                    .body
                    .transactions
                    .clone()),
            ]
        );

        Ok(())
    }

    #[test]
    fn transaction_sender_found_in_memory() -> eyre::Result<()> {
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

        // In memory
        // Choose a random transaction from the in-memory block
        let tx = &in_memory_blocks[0].body.transactions[0];

        // Retrieve the transaction ID
        let tx_id = provider.transaction_id(tx.hash())?.unwrap();

        // Retrieve the sender address for this transaction
        let result = provider.transaction_sender(tx_id)?;

        // Ensure the sender address matches the expected sender address
        let expected_sender = tx.recover_signer().unwrap();
        assert_eq!(
            result,
            Some(expected_sender),
            "The sender address should match the expected sender address"
        );

        // Database
        // Choose a random transaction from the database block
        let tx = &database_blocks[0].body.transactions[0];

        // Retrieve the transaction ID
        let tx_id = provider.transaction_id(tx.hash())?.unwrap();

        // Retrieve the sender address for this transaction
        let result = provider.transaction_sender(tx_id)?;

        // Ensure the sender address matches the expected sender address
        let expected_sender = tx.recover_signer().unwrap();
        assert_eq!(
            result,
            Some(expected_sender),
            "The sender address should match the expected sender address"
        );

        // Generate a random transaction ID that does not exist
        let invalid_tx_id = u64::MAX;

        // Attempt to retrieve the sender address for this invalid transaction ID
        let result = provider.transaction_sender(invalid_tx_id)?;

        // Ensure no sender address is found
        assert!(
            result.is_none(),
            "No sender address should be found for an invalid transaction ID"
        );

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

        let provider = Arc::new(provider);

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
                    return Ok::<_, ProviderError>(Some(tx))
                }
                panic!("should not be in database");
                // _factory.transaction_by_hash(hash)
            };

        // OLD BEHAVIOUR
        {
            // This will persist block 1 AFTER a database is created. Moving it from memory to
            // storage.
            persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[0].number);
            let to_be_persisted_tx = in_memory_blocks[0].body.transactions[0].clone();

            // Even though the block exists, given the order of provider queries done in the method
            // above, we do not see it.
            assert_eq!(
                old_transaction_hash_fn(
                    to_be_persisted_tx.hash(),
                    provider.canonical_in_memory_state(),
                    provider.database.clone()
                ),
                Ok(None)
            );
        }

        // CORRECT BEHAVIOUR
        {
            // This will persist block 1 AFTER a database is created. Moving it from memory to
            // storage.
            persist_block_after_db_tx_creation(provider.clone(), in_memory_blocks[1].number);
            let to_be_persisted_tx = in_memory_blocks[1].body.transactions[0].clone();

            assert_eq!(
                correct_transaction_hash_fn(
                    to_be_persisted_tx.hash(),
                    provider.canonical_in_memory_state(),
                    provider.database.clone()
                ),
                Ok(Some(to_be_persisted_tx))
            );
        }

        Ok(())
    }
}
