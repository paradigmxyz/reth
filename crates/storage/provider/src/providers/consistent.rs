use super::{DatabaseProviderRO, ProviderFactory, ProviderNodeTypes};
use crate::{
    providers::StaticFileProvider, AccountReader, BlockHashReader, BlockIdReader, BlockNumReader,
    BlockReader, BlockReaderIdExt, BlockSource, ChainSpecProvider, ChangeSetReader, HeaderProvider,
    ProviderError, PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt,
    StageCheckpointReader, StateReader, StaticFileProviderFactory, TransactionVariant,
    TransactionsProvider, WithdrawalsProvider,
};
use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_eips::{
    eip2718::Encodable2718, eip4895::Withdrawals, BlockHashOrNumber, BlockId, BlockNumHash,
    BlockNumberOrTag, HashOrNumber,
};
use alloy_primitives::{
    map::{hash_map, HashMap},
    Address, BlockHash, BlockNumber, TxHash, TxNumber, B256, U256,
};
use reth_chain_state::{BlockState, CanonicalInMemoryState, MemoryOverlayStateProviderRef};
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_db::models::BlockNumberAddress;
use reth_db_api::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_execution_types::{BundleStateInit, ExecutionOutcome, RevertsInit};
use reth_node_types::{BlockTy, HeaderTy, ReceiptTy, TxTy};
use reth_primitives::{Account, RecoveredBlock, SealedBlock, SealedHeader, StorageEntry};
use reth_primitives_traits::BlockBody;
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockBodyIndicesProvider, DatabaseProviderFactory, NodePrimitivesProvider, OmmersProvider,
    StateProvider, StorageChangeSetReader,
};
use reth_storage_errors::provider::ProviderResult;
use revm::db::states::PlainStorageRevert;
use std::{
    ops::{Add, Bound, RangeBounds, RangeInclusive, Sub},
    sync::Arc,
};
use tracing::trace;

/// Type that interacts with a snapshot view of the blockchain (storage and in-memory) at time of
/// instantiation, EXCEPT for pending, safe and finalized block which might change while holding
/// this provider.
///
/// CAUTION: Avoid holding this provider for too long or the inner database transaction will
/// time-out.
#[derive(Debug)]
#[doc(hidden)] // triggers ICE for `cargo docs`
pub struct ConsistentProvider<N: ProviderNodeTypes> {
    /// Storage provider.
    storage_provider: <ProviderFactory<N> as DatabaseProviderFactory>::Provider,
    /// Head block at time of [`Self`] creation
    head_block: Option<Arc<BlockState<N::Primitives>>>,
    /// In-memory canonical state. This is not a snapshot, and can change! Use with caution.
    canonical_in_memory_state: CanonicalInMemoryState<N::Primitives>,
}

impl<N: ProviderNodeTypes> ConsistentProvider<N> {
    /// Create a new provider using [`ProviderFactory`] and [`CanonicalInMemoryState`],
    ///
    /// Underneath it will take a snapshot by fetching [`CanonicalInMemoryState::head_state`] and
    /// [`ProviderFactory::database_provider_ro`] effectively maintaining one single snapshotted
    /// view of memory and database.
    pub fn new(
        storage_provider_factory: ProviderFactory<N>,
        state: CanonicalInMemoryState<N::Primitives>,
    ) -> ProviderResult<Self> {
        // Each one provides a snapshot at the time of instantiation, but its order matters.
        //
        // If we acquire first the database provider, it's possible that before the in-memory chain
        // snapshot is instantiated, it will flush blocks to disk. This would
        // mean that our database provider would not have access to the flushed blocks (since it's
        // working under an older view), while the in-memory state may have deleted them
        // entirely. Resulting in gaps on the range.
        let head_block = state.head_state();
        let storage_provider = storage_provider_factory.database_provider_ro()?;
        Ok(Self { storage_provider, head_block, canonical_in_memory_state: state })
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

    /// Storage provider for latest block
    fn latest_ref<'a>(&'a self) -> ProviderResult<Box<dyn StateProvider + 'a>> {
        trace!(target: "providers::blockchain", "Getting latest block state provider");

        // use latest state provider if the head state exists
        if let Some(state) = &self.head_block {
            trace!(target: "providers::blockchain", "Using head state for latest state provider");
            Ok(self.block_state_provider_ref(state)?.boxed())
        } else {
            trace!(target: "providers::blockchain", "Using database state for latest state provider");
            Ok(self.storage_provider.latest())
        }
    }

    fn history_by_block_hash_ref<'a>(
        &'a self,
        block_hash: BlockHash,
    ) -> ProviderResult<Box<dyn StateProvider + 'a>> {
        trace!(target: "providers::blockchain", ?block_hash, "Getting history by block hash");

        self.get_in_memory_or_storage_by_block(
            block_hash.into(),
            |_| self.storage_provider.history_by_block_hash(block_hash),
            |block_state| {
                let state_provider = self.block_state_provider_ref(block_state)?;
                Ok(Box::new(state_provider))
            },
        )
    }

    /// Returns a state provider indexed by the given block number or tag.
    fn state_by_block_number_ref<'a>(
        &'a self,
        number: BlockNumber,
    ) -> ProviderResult<Box<dyn StateProvider + 'a>> {
        let hash =
            self.block_hash(number)?.ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
        self.history_by_block_hash_ref(hash)
    }

    /// Return the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// If the range is empty, or there are no blocks for the given range, then this returns `None`.
    pub fn get_state(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Option<ExecutionOutcome<ReceiptTy<N>>>> {
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
                let receipt = receipt_iter
                    .next()
                    .ok_or_else(|| ProviderError::ReceiptNotFound(tx_num.into()))?;
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
        let mut state: BundleStateInit = HashMap::default();
        let mut reverts: RevertsInit = HashMap::default();
        let state_provider = self.state_by_block_number_ref(block_range_end)?;

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let new_info = state_provider.basic_account(&address)?;
                    entry.insert((old_info, new_info, HashMap::default()));
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
                    let present_info = state_provider.basic_account(&address)?;
                    entry.insert((present_info, present_info, HashMap::default()))
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
            &DatabaseProviderRO<N::DB, N>,
            RangeInclusive<BlockNumber>,
            &mut P,
        ) -> ProviderResult<Vec<T>>,
        G: Fn(&BlockState<N::Primitives>, &mut P) -> Option<T>,
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
            self.head_block.as_ref().map(|b| b.chain().collect::<Vec<_>>()).unwrap_or_default();
        let db_provider = &self.storage_provider;

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
            let mut db_items = fetch_db_range(db_provider, storage_range.clone(), &mut predicate)?;
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
    fn block_state_provider_ref(
        &self,
        state: &BlockState<N::Primitives>,
    ) -> ProviderResult<MemoryOverlayStateProviderRef<'_, N::Primitives>> {
        let anchor_hash = state.anchor().hash;
        let latest_historical = self.history_by_block_hash_ref(anchor_hash)?;
        let in_memory = state.chain().map(|block_state| block_state.block()).collect();
        Ok(MemoryOverlayStateProviderRef::new(latest_historical, in_memory))
    }

    /// Fetches data from either in-memory state or persistent storage for a range of transactions.
    ///
    /// * `fetch_from_db`: has a `DatabaseProviderRO` and the storage specific range.
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
            &DatabaseProviderRO<N::DB, N>,
            RangeInclusive<TxNumber>,
        ) -> ProviderResult<Vec<R>>,
        M: Fn(RangeInclusive<usize>, &BlockState<N::Primitives>) -> ProviderResult<Vec<R>>,
    {
        let in_mem_chain = self.head_block.iter().flat_map(|b| b.chain()).collect::<Vec<_>>();
        let provider = &self.storage_provider;

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
                .map(|b| b.block_ref().recovered_block().body().transactions().len() as u64)
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
        for block_state in in_mem_chain.iter().rev() {
            let block_tx_count =
                block_state.block_ref().recovered_block().body().transactions().len();
            let remaining = (tx_range.end() - tx_range.start() + 1) as usize;

            // If the transaction range start is equal or higher than the next block first
            // transaction, advance
            if *tx_range.start() >= in_memory_tx_num + block_tx_count as u64 {
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
        S: FnOnce(&DatabaseProviderRO<N::DB, N>) -> ProviderResult<Option<R>>,
        M: Fn(usize, TxNumber, &BlockState<N::Primitives>) -> ProviderResult<Option<R>>,
    {
        let in_mem_chain = self.head_block.iter().flat_map(|b| b.chain()).collect::<Vec<_>>();
        let provider = &self.storage_provider;

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
        for block_state in in_mem_chain.iter().rev() {
            let executed_block = block_state.block_ref();
            let block = executed_block.recovered_block();

            for tx_index in 0..block.body().transactions().len() {
                match id {
                    HashOrNumber::Hash(tx_hash) => {
                        if tx_hash == block.body().transactions()[tx_index].trie_hash() {
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
    pub(crate) fn get_in_memory_or_storage_by_block<S, M, R>(
        &self,
        id: BlockHashOrNumber,
        fetch_from_db: S,
        fetch_from_block_state: M,
    ) -> ProviderResult<R>
    where
        S: FnOnce(&DatabaseProviderRO<N::DB, N>) -> ProviderResult<R>,
        M: Fn(&BlockState<N::Primitives>) -> ProviderResult<R>,
    {
        if let Some(Some(block_state)) = self.head_block.as_ref().map(|b| b.block_on_chain(id)) {
            return fetch_from_block_state(block_state)
        }
        fetch_from_db(&self.storage_provider)
    }
}

impl<N: ProviderNodeTypes> ConsistentProvider<N> {
    /// Ensures that the given block number is canonical (synced)
    ///
    /// This is a helper for guarding the `HistoricalStateProvider` against block numbers that are
    /// out of range and would lead to invalid results, mainly during initial sync.
    ///
    /// Verifying the `block_number` would be expensive since we need to lookup sync table
    /// Instead, we ensure that the `block_number` is within the range of the
    /// [`Self::best_block_number`] which is updated when a block is synced.
    #[inline]
    pub(crate) fn ensure_canonical_block(&self, block_number: BlockNumber) -> ProviderResult<()> {
        let latest = self.best_block_number()?;
        if block_number > latest {
            Err(ProviderError::HeaderNotFound(block_number.into()))
        } else {
            Ok(())
        }
    }
}

impl<N: ProviderNodeTypes> NodePrimitivesProvider for ConsistentProvider<N> {
    type Primitives = N::Primitives;
}

impl<N: ProviderNodeTypes> StaticFileProviderFactory for ConsistentProvider<N> {
    fn static_file_provider(&self) -> StaticFileProvider<N::Primitives> {
        self.storage_provider.static_file_provider()
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for ConsistentProvider<N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        self.get_in_memory_or_storage_by_block(
            (*block_hash).into(),
            |db_provider| db_provider.header(block_hash),
            |block_state| Ok(Some(block_state.block_ref().recovered_block().clone_header())),
        )
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.get_in_memory_or_storage_by_block(
            num.into(),
            |db_provider| db_provider.header_by_number(num),
            |block_state| Ok(Some(block_state.block_ref().recovered_block().clone_header())),
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
        let number = if self.head_block.as_ref().map(|b| b.block_on_chain(number.into())).is_some()
        {
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
                self.last_block_number()?
            }
        } else {
            // Otherwise, return what we have on disk for the input block
            number
        };
        self.storage_provider.header_td_by_number(number)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.headers_range(range),
            |block_state, _| Some(block_state.block_ref().recovered_block().header().clone()),
            |_| true,
        )
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.get_in_memory_or_storage_by_block(
            number.into(),
            |db_provider| db_provider.sealed_header(number),
            |block_state| Ok(Some(block_state.block_ref().recovered_block().clone_sealed_header())),
        )
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.sealed_headers_range(range),
            |block_state, _| Some(block_state.block_ref().recovered_block().clone_sealed_header()),
            |_| true,
        )
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, predicate| db_provider.sealed_headers_while(range, predicate),
            |block_state, predicate| {
                let header = block_state.block_ref().recovered_block().sealed_header();
                predicate(header).then(|| header.clone())
            },
            predicate,
        )
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for ConsistentProvider<N> {
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

impl<N: ProviderNodeTypes> BlockNumReader for ConsistentProvider<N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_number = self.best_block_number()?;
        Ok(ChainInfo { best_hash: self.block_hash(best_number)?.unwrap_or_default(), best_number })
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        self.head_block.as_ref().map(|b| Ok(b.number())).unwrap_or_else(|| self.last_block_number())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.storage_provider.last_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.get_in_memory_or_storage_by_block(
            hash.into(),
            |db_provider| db_provider.block_number(hash),
            |block_state| Ok(Some(block_state.number())),
        )
    }
}

impl<N: ProviderNodeTypes> BlockIdReader for ConsistentProvider<N> {
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

impl<N: ProviderNodeTypes> BlockReader for ConsistentProvider<N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        match source {
            BlockSource::Any | BlockSource::Canonical => {
                // Note: it's fine to return the unsealed block because the caller already has
                // the hash
                self.get_in_memory_or_storage_by_block(
                    hash.into(),
                    |db_provider| db_provider.find_block_by_hash(hash, source),
                    |block_state| Ok(Some(block_state.block_ref().recovered_block().clone_block())),
                )
            }
            BlockSource::Pending => {
                Ok(self.canonical_in_memory_state.pending_block().map(|block| block.into_block()))
            }
        }
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.block(id),
            |block_state| Ok(Some(block_state.block_ref().recovered_block().clone_block())),
        )
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock<Self::Block>>> {
        Ok(self.canonical_in_memory_state.pending_block())
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(self.canonical_in_memory_state.pending_recovered_block())
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(SealedBlock<Self::Block>, Vec<Self::Receipt>)>> {
        Ok(self.canonical_in_memory_state.pending_block_and_receipts())
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
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.block_with_senders(id, transaction_kind),
            |block_state| Ok(Some(block_state.block().recovered_block().clone())),
        )
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.sealed_block_with_senders(id, transaction_kind),
            |block_state| Ok(Some(block_state.block().recovered_block().clone())),
        )
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.block_range(range),
            |block_state, _| Some(block_state.block_ref().recovered_block().clone_block()),
            |_| true,
        )
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.block_with_senders_range(range),
            |block_state, _| Some(block_state.block().recovered_block().clone()),
            |_| true,
        )
    }

    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.sealed_block_with_senders_range(range),
            |block_state, _| Some(block_state.block().recovered_block().clone()),
            |_| true,
        )
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for ConsistentProvider<N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.get_in_memory_or_storage_by_tx(
            tx_hash.into(),
            |db_provider| db_provider.transaction_id(tx_hash),
            |_, tx_number, _| Ok(Some(tx_number)),
        )
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_by_id(id),
            |tx_index, _, block_state| {
                Ok(block_state
                    .block_ref()
                    .recovered_block()
                    .body()
                    .transactions()
                    .get(tx_index)
                    .cloned())
            },
        )
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_by_id_unhashed(id),
            |tx_index, _, block_state| {
                Ok(block_state
                    .block_ref()
                    .recovered_block()
                    .body()
                    .transactions()
                    .get(tx_index)
                    .cloned())
            },
        )
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        if let Some(tx) = self.head_block.as_ref().and_then(|b| b.transaction_on_chain(hash)) {
            return Ok(Some(tx))
        }

        self.storage_provider.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        if let Some((tx, meta)) =
            self.head_block.as_ref().and_then(|b| b.transaction_meta_on_chain(tx_hash))
        {
            return Ok(Some((tx, meta)))
        }

        self.storage_provider.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_block(id),
            |_, _, block_state| Ok(Some(block_state.block_ref().recovered_block().number())),
        )
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |provider| provider.transactions_by_block(id),
            |block_state| {
                Ok(Some(block_state.block_ref().recovered_block().body().transactions().to_vec()))
            },
        )
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        self.get_in_memory_or_storage_by_block_range_while(
            range,
            |db_provider, range, _| db_provider.transactions_by_block_range(range),
            |block_state, _| {
                Some(block_state.block_ref().recovered_block().body().transactions().to_vec())
            },
            |_| true,
        )
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.get_in_memory_or_storage_by_tx_range(
            range,
            |db_provider, db_range| db_provider.transactions_by_tx_range(db_range),
            |index_range, block_state| {
                Ok(block_state.block_ref().recovered_block().body().transactions()[index_range]
                    .to_vec())
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
            |index_range, block_state| {
                Ok(block_state.block_ref().recovered_block.senders()[index_range].to_vec())
            },
        )
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.transaction_sender(id),
            |tx_index, _, block_state| {
                Ok(block_state.block_ref().recovered_block.senders().get(tx_index).copied())
            },
        )
    }
}

impl<N: ProviderNodeTypes> ReceiptProvider for ConsistentProvider<N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.get_in_memory_or_storage_by_tx(
            id.into(),
            |provider| provider.receipt(id),
            |tx_index, _, block_state| {
                Ok(block_state.executed_block_receipts().get(tx_index).cloned())
            },
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        for block_state in self.head_block.iter().flat_map(|b| b.chain()) {
            let executed_block = block_state.block_ref();
            let block = executed_block.recovered_block();
            let receipts = block_state.executed_block_receipts();

            // assuming 1:1 correspondence between transactions and receipts
            debug_assert_eq!(
                block.body().transactions().len(),
                receipts.len(),
                "Mismatch between transaction and receipt count"
            );

            if let Some(tx_index) =
                block.body().transactions().iter().position(|tx| tx.trie_hash() == hash)
            {
                // safe to use tx_index for receipts due to 1:1 correspondence
                return Ok(receipts.get(tx_index).cloned());
            }
        }

        self.storage_provider.receipt_by_hash(hash)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.get_in_memory_or_storage_by_block(
            block,
            |db_provider| db_provider.receipts_by_block(block),
            |block_state| Ok(Some(block_state.executed_block_receipts())),
        )
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.get_in_memory_or_storage_by_tx_range(
            range,
            |db_provider, db_range| db_provider.receipts_by_tx_range(db_range),
            |index_range, block_state| {
                Ok(block_state.executed_block_receipts().drain(index_range).collect())
            },
        )
    }
}

impl<N: ProviderNodeTypes> ReceiptProviderIdExt for ConsistentProvider<N> {
    fn receipts_by_block_id(&self, block: BlockId) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        match block {
            BlockId::Hash(rpc_block_hash) => {
                let mut receipts = self.receipts_by_block(rpc_block_hash.block_hash.into())?;
                if receipts.is_none() && !rpc_block_hash.require_canonical.unwrap_or(false) {
                    if let Some(state) = self
                        .head_block
                        .as_ref()
                        .and_then(|b| b.block_on_chain(rpc_block_hash.block_hash.into()))
                    {
                        receipts = Some(state.executed_block_receipts());
                    }
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

impl<N: ProviderNodeTypes> WithdrawalsProvider for ConsistentProvider<N> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        if !self.chain_spec().is_shanghai_active_at_timestamp(timestamp) {
            return Ok(None)
        }

        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.withdrawals_by_block(id, timestamp),
            |block_state| {
                Ok(block_state.block_ref().recovered_block().body().withdrawals().cloned())
            },
        )
    }
}

impl<N: ProviderNodeTypes> OmmersProvider for ConsistentProvider<N> {
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<HeaderTy<N>>>> {
        self.get_in_memory_or_storage_by_block(
            id,
            |db_provider| db_provider.ommers(id),
            |block_state| {
                if self.chain_spec().final_paris_total_difficulty(block_state.number()).is_some() {
                    return Ok(Some(Vec::new()))
                }

                Ok(block_state.block_ref().recovered_block().body().ommers().map(|o| o.to_vec()))
            },
        )
    }
}

impl<N: ProviderNodeTypes> BlockBodyIndicesProvider for ConsistentProvider<N> {
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
                    .storage_provider
                    .block_body_indices(last_storage_block_number)?
                    .ok_or(ProviderError::BlockBodyIndicesNotFound(last_storage_block_number))?;

                // Prepare our block indices
                stored_indices.first_tx_num = stored_indices.next_tx_num();
                stored_indices.tx_count = 0;

                // Iterate from the lowest block in memory until our target block
                for state in block_state.chain().collect::<Vec<_>>().into_iter().rev() {
                    let block_tx_count =
                        state.block_ref().recovered_block().body().transactions().len() as u64;
                    if state.block_ref().recovered_block().number() == number {
                        stored_indices.tx_count = block_tx_count;
                    } else {
                        stored_indices.first_tx_num += block_tx_count;
                    }
                }

                Ok(Some(stored_indices))
            },
        )
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for ConsistentProvider<N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.storage_provider.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.storage_provider.get_stage_checkpoint_progress(id)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.storage_provider.get_all_checkpoints()
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for ConsistentProvider<N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.storage_provider.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.storage_provider.get_prune_checkpoints()
    }
}

impl<N: ProviderNodeTypes> ChainSpecProvider for ConsistentProvider<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<N::ChainSpec> {
        ChainSpecProvider::chain_spec(&self.storage_provider)
    }
}

impl<N: ProviderNodeTypes> BlockReaderIdExt for ConsistentProvider<N> {
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Block>> {
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

    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<HeaderTy<N>>> {
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
    ) -> ProviderResult<Option<SealedHeader<HeaderTy<N>>>> {
        match id {
            BlockNumberOrTag::Latest => {
                Ok(Some(self.canonical_in_memory_state.get_canonical_head()))
            }
            BlockNumberOrTag::Finalized => {
                Ok(self.canonical_in_memory_state.get_finalized_header())
            }
            BlockNumberOrTag::Safe => Ok(self.canonical_in_memory_state.get_safe_header()),
            BlockNumberOrTag::Earliest => self
                .header_by_number(0)?
                .map_or_else(|| Ok(None), |h| Ok(Some(SealedHeader::seal_slow(h)))),
            BlockNumberOrTag::Pending => Ok(self.canonical_in_memory_state.pending_sealed_header()),
            BlockNumberOrTag::Number(num) => self
                .header_by_number(num)?
                .map_or_else(|| Ok(None), |h| Ok(Some(SealedHeader::seal_slow(h)))),
        }
    }

    fn sealed_header_by_id(
        &self,
        id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<HeaderTy<N>>>> {
        Ok(match id {
            BlockId::Number(num) => self.sealed_header_by_number_or_tag(num)?,
            BlockId::Hash(hash) => self.header(&hash.block_hash)?.map(SealedHeader::seal_slow),
        })
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<HeaderTy<N>>> {
        Ok(match id {
            BlockId::Number(num) => self.header_by_number_or_tag(num)?,
            BlockId::Hash(hash) => self.header(&hash.block_hash)?,
        })
    }

    fn ommers_by_id(&self, id: BlockId) -> ProviderResult<Option<Vec<HeaderTy<N>>>> {
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

impl<N: ProviderNodeTypes> StorageChangeSetReader for ConsistentProvider<N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if let Some(state) =
            self.head_block.as_ref().and_then(|b| b.block_on_chain(block_number.into()))
        {
            let changesets = state
                .block()
                .execution_output
                .bundle
                .reverts
                .clone()
                .to_plain_state_reverts()
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

            // No prune checkpoint means history should exist and we should `unwrap_or(true)`
            let storage_history_exists = self
                .storage_provider
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

            self.storage_provider.storage_changeset(block_number)
        }
    }
}

impl<N: ProviderNodeTypes> ChangeSetReader for ConsistentProvider<N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        if let Some(state) =
            self.head_block.as_ref().and_then(|b| b.block_on_chain(block_number.into()))
        {
            let changesets = state
                .block_ref()
                .execution_output
                .bundle
                .reverts
                .clone()
                .to_plain_state_reverts()
                .accounts
                .into_iter()
                .flatten()
                .map(|(address, info)| AccountBeforeTx { address, info: info.map(Into::into) })
                .collect();
            Ok(changesets)
        } else {
            // Perform checks on whether or not changesets exist for the block.

            // No prune checkpoint means history should exist and we should `unwrap_or(true)`
            let account_history_exists = self
                .storage_provider
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

            self.storage_provider.account_block_changeset(block_number)
        }
    }
}

impl<N: ProviderNodeTypes> AccountReader for ConsistentProvider<N> {
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        // use latest state provider
        let state_provider = self.latest_ref()?;
        state_provider.basic_account(address)
    }
}

impl<N: ProviderNodeTypes> StateReader for ConsistentProvider<N> {
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
        if let Some(state) = self.head_block.as_ref().and_then(|b| b.block_on_chain(block.into())) {
            let state = state.block_ref().execution_outcome().clone();
            Ok(Some(state))
        } else {
            Self::get_state(self, block..=block)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        providers::blockchain_provider::BlockchainProvider,
        test_utils::create_test_provider_factory, BlockWriter,
    };
    use alloy_eips::BlockHashOrNumber;
    use alloy_primitives::B256;
    use itertools::Itertools;
    use rand::Rng;
    use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
    use reth_db::models::AccountBeforeTx;
    use reth_execution_types::ExecutionOutcome;
    use reth_primitives::{RecoveredBlock, SealedBlock};
    use reth_storage_api::{BlockReader, BlockSource, ChangeSetReader};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_eoa_accounts, BlockRangeParams,
    };
    use revm::db::BundleState;
    use std::{
        ops::{Bound, Range, RangeBounds},
        sync::Arc,
    };

    const TEST_BLOCKS_COUNT: usize = 5;

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
        let consistent_provider = provider.consistent_provider()?;

        // Useful blocks
        let first_db_block = database_blocks.first().unwrap();
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        let last_in_mem_block = in_memory_blocks.last().unwrap();

        // No block in memory before setting in memory state
        assert_eq!(
            consistent_provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Any)?,
            None
        );
        assert_eq!(
            consistent_provider
                .find_block_by_hash(first_in_mem_block.hash(), BlockSource::Canonical)?,
            None
        );
        // No pending block in memory
        assert_eq!(
            consistent_provider
                .find_block_by_hash(first_in_mem_block.hash(), BlockSource::Pending)?,
            None
        );

        // Insert first block into the in-memory state
        let in_memory_block_senders =
            first_in_mem_block.senders().expect("failed to recover senders");
        let chain = NewCanonicalChain::Commit {
            new: vec![ExecutedBlock::new(
                Arc::new(RecoveredBlock::new_sealed(
                    first_in_mem_block.clone(),
                    in_memory_block_senders,
                )),
                Default::default(),
                Default::default(),
                Default::default(),
            )],
        };
        consistent_provider.canonical_in_memory_state.update_chain(chain);
        let consistent_provider = provider.consistent_provider()?;

        // Now the block should be found in memory
        assert_eq!(
            consistent_provider.find_block_by_hash(first_in_mem_block.hash(), BlockSource::Any)?,
            Some(first_in_mem_block.clone().into_block())
        );
        assert_eq!(
            consistent_provider
                .find_block_by_hash(first_in_mem_block.hash(), BlockSource::Canonical)?,
            Some(first_in_mem_block.clone().into_block())
        );

        // Find the first block in database by hash
        assert_eq!(
            consistent_provider.find_block_by_hash(first_db_block.hash(), BlockSource::Any)?,
            Some(first_db_block.clone().into_block())
        );
        assert_eq!(
            consistent_provider
                .find_block_by_hash(first_db_block.hash(), BlockSource::Canonical)?,
            Some(first_db_block.clone().into_block())
        );

        // No pending block in database
        assert_eq!(
            consistent_provider.find_block_by_hash(first_db_block.hash(), BlockSource::Pending)?,
            None
        );

        // Insert the last block into the pending state
        provider.canonical_in_memory_state.set_pending_block(ExecutedBlock {
            recovered_block: Arc::new(RecoveredBlock::new_sealed(
                last_in_mem_block.clone(),
                Default::default(),
            )),
            execution_output: Default::default(),
            hashed_state: Default::default(),
            trie: Default::default(),
        });

        // Now the last block should be found in memory
        assert_eq!(
            consistent_provider
                .find_block_by_hash(last_in_mem_block.hash(), BlockSource::Pending)?,
            Some(last_in_mem_block.clone_block())
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
        let consistent_provider = provider.consistent_provider()?;

        // First in memory block
        let first_in_mem_block = in_memory_blocks.first().unwrap();
        // First database block
        let first_db_block = database_blocks.first().unwrap();

        // First in memory block should not be found yet as not integrated to the in-memory state
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Hash(first_in_mem_block.hash()))?,
            None
        );
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Number(first_in_mem_block.number))?,
            None
        );

        // Insert first block into the in-memory state
        let in_memory_block_senders =
            first_in_mem_block.senders().expect("failed to recover senders");
        let chain = NewCanonicalChain::Commit {
            new: vec![ExecutedBlock::new(
                Arc::new(RecoveredBlock::new_sealed(
                    first_in_mem_block.clone(),
                    in_memory_block_senders,
                )),
                Default::default(),
                Default::default(),
                Default::default(),
            )],
        };
        consistent_provider.canonical_in_memory_state.update_chain(chain);

        let consistent_provider = provider.consistent_provider()?;

        // First in memory block should be found
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Hash(first_in_mem_block.hash()))?,
            Some(first_in_mem_block.clone().into_block())
        );
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Number(first_in_mem_block.number))?,
            Some(first_in_mem_block.clone().into_block())
        );

        // First database block should be found
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Hash(first_db_block.hash()))?,
            Some(first_db_block.clone().into_block())
        );
        assert_eq!(
            consistent_provider.block(BlockHashOrNumber::Number(first_db_block.number))?,
            Some(first_db_block.clone().into_block())
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
                    ExecutedBlock::new(
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
                        Default::default(),
                    )
                })
                .unwrap()],
        };
        provider.canonical_in_memory_state.update_chain(chain);

        let consistent_provider = provider.consistent_provider()?;

        assert_eq!(
            consistent_provider.account_block_changeset(last_database_block).unwrap(),
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
            consistent_provider.account_block_changeset(first_in_memory_block).unwrap(),
            in_memory_changesets
                .into_iter()
                .sorted_by_key(|(address, _, _)| *address)
                .map(|(address, account, _)| AccountBeforeTx { address, info: Some(account) })
                .collect::<Vec<_>>()
        );

        Ok(())
    }
}
