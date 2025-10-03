use crate::{
    changesets_utils::{
        storage_trie_wiped_changeset_iter, StorageRevertsIter, StorageTrieCurrentValuesIter,
    },
    providers::{
        database::{chain::ChainStorage, metrics},
        static_file::StaticFileWriter,
        NodeTypesForProvider, StaticFileProvider,
    },
    to_range,
    traits::{
        AccountExtReader, BlockSource, ChangeSetReader, ReceiptProvider, StageCheckpointWriter,
    },
    AccountReader, BlockBodyWriter, BlockExecutionWriter, BlockHashReader, BlockNumReader,
    BlockReader, BlockWriter, BundleStateInit, ChainStateBlockReader, ChainStateBlockWriter,
    DBProvider, HashingWriter, HeaderProvider, HeaderSyncGapProvider, HistoricalStateProvider,
    HistoricalStateProviderRef, HistoryWriter, LatestStateProvider, LatestStateProviderRef,
    OriginalValuesKnown, ProviderError, PruneCheckpointReader, PruneCheckpointWriter, RevertsInit,
    StageCheckpointReader, StateProviderBox, StateWriter, StaticFileProviderFactory, StatsReader,
    StorageReader, StorageTrieWriter, TransactionVariant, TransactionsProvider,
    TransactionsProviderExt, TrieReader, TrieWriter,
};
use alloy_consensus::{
    transaction::{SignerRecoverable, TransactionMeta, TxHashRef},
    BlockHeader, TxReceipt,
};
use alloy_eips::{eip2718::Encodable2718, BlockHashOrNumber};
use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map, HashMap, HashSet},
    Address, BlockHash, BlockNumber, TxHash, TxNumber, B256, U256,
};
use itertools::Itertools;
use rayon::slice::ParallelSliceMut;
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates};
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    models::{
        sharded_key, storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress,
        BlockNumberHashedAddress, ShardedKey, StoredBlockBodyIndices,
    },
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList, DatabaseError, PlainAccountState, PlainStorageState,
};
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_node_types::{BlockTy, BodyTy, HeaderTy, NodeTypes, ReceiptTy, TxTy};
use reth_primitives_traits::{
    Account, Block as _, BlockBody as _, Bytecode, GotExpected, RecoveredBlock, SealedHeader,
    StorageEntry,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneModes, PruneSegment, MINIMUM_PRUNING_DISTANCE,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, BlockBodyReader, NodePrimitivesProvider, StateProvider,
    StorageChangeSetReader, TryIntoHistoricalStateProvider,
};
use reth_storage_errors::provider::{ProviderResult, RootMismatch};
use reth_trie::{
    prefix_set::{PrefixSet, PrefixSetMut, TriePrefixSets},
    trie_cursor::{InMemoryTrieCursor, TrieCursor, TrieCursorIter},
    updates::{StorageTrieUpdates, StorageTrieUpdatesSorted, TrieUpdates, TrieUpdatesSorted},
    BranchNodeCompact, HashedPostStateSorted, Nibbles, StateRoot, StoredNibbles,
    StoredNibblesSubKey, TrieChangeSetsEntry,
};
use reth_trie_db::{DatabaseAccountTrieCursor, DatabaseStateRoot, DatabaseStorageTrieCursor};
use revm_database::states::{
    PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    ops::{Deref, DerefMut, Not, Range, RangeBounds, RangeFrom, RangeInclusive},
    sync::{mpsc, Arc},
};
use tracing::{debug, trace};

/// A [`DatabaseProvider`] that holds a read-only database transaction.
pub type DatabaseProviderRO<DB, N> = DatabaseProvider<<DB as Database>::TX, N>;

/// A [`DatabaseProvider`] that holds a read-write database transaction.
///
/// Ideally this would be an alias type. However, there's some weird compiler error (<https://github.com/rust-lang/rust/issues/102211>), that forces us to wrap this in a struct instead.
/// Once that issue is solved, we can probably revert back to being an alias type.
#[derive(Debug)]
pub struct DatabaseProviderRW<DB: Database, N: NodeTypes>(
    pub DatabaseProvider<<DB as Database>::TXMut, N>,
);

impl<DB: Database, N: NodeTypes> Deref for DatabaseProviderRW<DB, N> {
    type Target = DatabaseProvider<<DB as Database>::TXMut, N>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB: Database, N: NodeTypes> DerefMut for DatabaseProviderRW<DB, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<DB: Database, N: NodeTypes> AsRef<DatabaseProvider<<DB as Database>::TXMut, N>>
    for DatabaseProviderRW<DB, N>
{
    fn as_ref(&self) -> &DatabaseProvider<<DB as Database>::TXMut, N> {
        &self.0
    }
}

impl<DB: Database, N: NodeTypes + 'static> DatabaseProviderRW<DB, N> {
    /// Commit database transaction and static file if it exists.
    pub fn commit(self) -> ProviderResult<bool> {
        self.0.commit()
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> <DB as Database>::TXMut {
        self.0.into_tx()
    }
}

impl<DB: Database, N: NodeTypes> From<DatabaseProviderRW<DB, N>>
    for DatabaseProvider<<DB as Database>::TXMut, N>
{
    fn from(provider: DatabaseProviderRW<DB, N>) -> Self {
        provider.0
    }
}

/// A provider struct that fetches data from the database.
/// Wrapper around [`DbTx`] and [`DbTxMut`]. Example: [`HeaderProvider`] [`BlockHashReader`]
#[derive(Debug)]
pub struct DatabaseProvider<TX, N: NodeTypes> {
    /// Database transaction.
    tx: TX,
    /// Chain spec
    chain_spec: Arc<N::ChainSpec>,
    /// Static File provider
    static_file_provider: StaticFileProvider<N::Primitives>,
    /// Pruning configuration
    prune_modes: PruneModes,
    /// Node storage handler.
    storage: Arc<N::Storage>,
}

impl<TX, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Returns reference to prune modes.
    pub const fn prune_modes_ref(&self) -> &PruneModes {
        &self.prune_modes
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// State provider for latest state
    pub fn latest<'a>(&'a self) -> Box<dyn StateProvider + 'a> {
        trace!(target: "providers::db", "Returning latest state provider");
        Box::new(LatestStateProviderRef::new(self))
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash<'a>(
        &'a self,
        block_hash: BlockHash,
    ) -> ProviderResult<Box<dyn StateProvider + 'a>> {
        let mut block_number =
            self.block_number(block_hash)?.ok_or(ProviderError::BlockHashNotFound(block_hash))?;
        if block_number == self.best_block_number().unwrap_or_default() &&
            block_number == self.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProviderRef::new(self)))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        let account_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let storage_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::StorageHistory)?;

        let mut state_provider = HistoricalStateProviderRef::new(self, block_number);

        // If we pruned account or storage history, we can't return state on every historical block.
        // Instead, we should cap it at the latest prune checkpoint for corresponding prune segment.
        if let Some(prune_checkpoint_block_number) =
            account_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_account_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }
        if let Some(prune_checkpoint_block_number) =
            storage_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_storage_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }

        Ok(Box::new(state_provider))
    }

    #[cfg(feature = "test-utils")]
    /// Sets the prune modes for provider.
    pub fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.prune_modes = prune_modes;
    }
}

impl<TX, N: NodeTypes> NodePrimitivesProvider for DatabaseProvider<TX, N> {
    type Primitives = N::Primitives;
}

impl<TX, N: NodeTypes> StaticFileProviderFactory for DatabaseProvider<TX, N> {
    /// Returns a static file provider
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        self.static_file_provider.clone()
    }
}

impl<TX: Debug + Send + Sync, N: NodeTypes<ChainSpec: EthChainSpec + 'static>> ChainSpecProvider
    for DatabaseProvider<TX, N>
{
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<TX: DbTxMut, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Creates a provider with an inner read-write transaction.
    pub const fn new_rw(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
    ) -> Self {
        Self { tx, chain_spec, static_file_provider, prune_modes, storage }
    }
}

impl<TX, N: NodeTypes> AsRef<Self> for DatabaseProvider<TX, N> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<TX: DbTx + DbTxMut + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Writes executed blocks and state to storage.
    pub fn save_blocks(
        &self,
        blocks: Vec<ExecutedBlockWithTrieUpdates<N::Primitives>>,
    ) -> ProviderResult<()> {
        if blocks.is_empty() {
            debug!(target: "providers::db", "Attempted to write empty block range");
            return Ok(())
        }

        // NOTE: checked non-empty above
        let first_block = blocks.first().unwrap().recovered_block();

        let last_block = blocks.last().unwrap().recovered_block();
        let first_number = first_block.number();
        let last_block_number = last_block.number();

        debug!(target: "providers::db", block_count = %blocks.len(), "Writing blocks and execution data to storage");

        // TODO: Do performant / batched writes for each type of object
        // instead of a loop over all blocks,
        // meaning:
        //  * blocks
        //  * state
        //  * hashed state
        //  * trie updates (cannot naively extend, need helper)
        //  * indices (already done basically)
        // Insert the blocks
        for ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock { recovered_block, execution_output, hashed_state },
            mut trie,
        } in blocks
        {
            let block_hash = recovered_block.hash();
            let block_number = recovered_block.number();
            self.insert_block(Arc::unwrap_or_clone(recovered_block))?;

            // Write state and changesets to the database.
            // Must be written after blocks because of the receipt lookup.
            self.write_state(&execution_output, OriginalValuesKnown::No)?;

            // insert hashes and intermediate merkle nodes
            self.write_hashed_state(&Arc::unwrap_or_clone(hashed_state).into_sorted())?;

            let trie_updates =
                trie.take_present().ok_or(ProviderError::MissingTrieUpdates(block_hash))?;

            // sort trie updates and insert changesets
            // TODO(mediocregopher): We should rework `write_trie_updates` to also accept a
            // `TrieUpdatesSorted`, and then the `trie` field of `ExecutedBlockWithTrieUpdates` to
            // carry a TrieUpdatesSorted.
            let trie_updates_sorted = (*trie_updates).clone().into_sorted();
            self.write_trie_changesets(block_number, &trie_updates_sorted, None)?;

            self.write_trie_updates(&trie_updates)?;
        }

        // update history indices
        self.update_history_indices(first_number..=last_block_number)?;

        // Update pipeline progress
        self.update_pipeline_stages(last_block_number, false)?;

        debug!(target: "providers::db", range = ?first_number..=last_block_number, "Appended block data");

        Ok(())
    }

    /// Unwinds trie state for the given range.
    ///
    /// This includes calculating the resulted state root and comparing it with the parent block
    /// state root.
    pub fn unwind_trie_state_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let changed_accounts = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range.clone())?
            .collect::<Result<Vec<_>, _>>()?;

        // Unwind account hashes. Add changed accounts to account prefix set.
        let hashed_addresses = self.unwind_account_hashing(changed_accounts.iter())?;
        let mut account_prefix_set = PrefixSetMut::with_capacity(hashed_addresses.len());
        let mut destroyed_accounts = HashSet::default();
        for (hashed_address, account) in hashed_addresses {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            if account.is_none() {
                destroyed_accounts.insert(hashed_address);
            }
        }

        // Unwind account history indices.
        self.unwind_account_history_indices(changed_accounts.iter())?;
        let storage_range = BlockNumberAddress::range(range.clone());

        let changed_storages = self
            .tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(storage_range)?
            .collect::<Result<Vec<_>, _>>()?;

        // Unwind storage hashes. Add changed account and storage keys to corresponding prefix
        // sets.
        let mut storage_prefix_sets = B256Map::<PrefixSet>::default();
        let storage_entries = self.unwind_storage_hashing(changed_storages.iter().copied())?;
        for (hashed_address, hashed_slots) in storage_entries {
            account_prefix_set.insert(Nibbles::unpack(hashed_address));
            let mut storage_prefix_set = PrefixSetMut::with_capacity(hashed_slots.len());
            for slot in hashed_slots {
                storage_prefix_set.insert(Nibbles::unpack(slot));
            }
            storage_prefix_sets.insert(hashed_address, storage_prefix_set.freeze());
        }

        // Unwind storage history indices.
        self.unwind_storage_history_indices(changed_storages.iter().copied())?;

        // Calculate the reverted merkle root.
        // This is the same as `StateRoot::incremental_root_with_updates`, only the prefix sets
        // are pre-loaded.
        let prefix_sets = TriePrefixSets {
            account_prefix_set: account_prefix_set.freeze(),
            storage_prefix_sets,
            destroyed_accounts,
        };
        let (new_state_root, trie_updates) = StateRoot::from_tx(&self.tx)
            .with_prefix_sets(prefix_sets)
            .root_with_updates()
            .map_err(reth_db_api::DatabaseError::from)?;

        let parent_number = range.start().saturating_sub(1);
        let parent_state_root = self
            .header_by_number(parent_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?
            .state_root();

        // state root should be always correct as we are reverting state.
        // but for sake of double verification we will check it again.
        if new_state_root != parent_state_root {
            let parent_hash = self
                .block_hash(parent_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(parent_number.into()))?;
            return Err(ProviderError::UnwindStateRootMismatch(Box::new(RootMismatch {
                root: GotExpected { got: new_state_root, expected: parent_state_root },
                block_number: parent_number,
                block_hash: parent_hash,
            })))
        }
        self.write_trie_updates(&trie_updates)?;

        Ok(())
    }

    /// Removes receipts from all transactions starting with provided number (inclusive).
    fn remove_receipts_from(
        &self,
        from_tx: TxNumber,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        // iterate over block body and remove receipts
        self.remove::<tables::Receipts<ReceiptTy<N>>>(from_tx..)?;

        if !self.prune_modes.has_receipts_pruning() {
            let static_file_receipt_num =
                self.static_file_provider.get_highest_static_file_tx(StaticFileSegment::Receipts);

            let to_delete = static_file_receipt_num
                .map(|static_num| (static_num + 1).saturating_sub(from_tx))
                .unwrap_or_default();

            self.static_file_provider
                .latest_writer(StaticFileSegment::Receipts)?
                .prune_receipts(to_delete, last_block)?;
        }

        Ok(())
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> TryIntoHistoricalStateProvider for DatabaseProvider<TX, N> {
    fn try_into_history_at_block(
        self,
        mut block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        // if the block number is the same as the currently best block number on disk we can use the
        // latest state provider here
        if block_number == self.best_block_number().unwrap_or_default() {
            return Ok(Box::new(LatestStateProvider::new(self)))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        let account_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let storage_history_prune_checkpoint =
            self.get_prune_checkpoint(PruneSegment::StorageHistory)?;

        let mut state_provider = HistoricalStateProvider::new(self, block_number);

        // If we pruned account or storage history, we can't return state on every historical block.
        // Instead, we should cap it at the latest prune checkpoint for corresponding prune segment.
        if let Some(prune_checkpoint_block_number) =
            account_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_account_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }
        if let Some(prune_checkpoint_block_number) =
            storage_history_prune_checkpoint.and_then(|checkpoint| checkpoint.block_number)
        {
            state_provider = state_provider.with_lowest_available_storage_history_block_number(
                prune_checkpoint_block_number + 1,
            );
        }

        Ok(Box::new(state_provider))
    }
}

/// For a given key, unwind all history shards that contain block numbers at or above the given
/// block number.
///
/// S - Sharded key subtype.
/// T - Table to walk over.
/// C - Cursor implementation.
///
/// This function walks the entries from the given start key and deletes all shards that belong to
/// the key and contain block numbers at or above the given block number. Shards entirely below
/// the block number are preserved.
///
/// The boundary shard (the shard that spans across the block number) is removed from the database.
/// Any indices that are below the block number are filtered out and returned for reinsertion.
/// The boundary shard is returned for reinsertion (if it's not empty).
fn unwind_history_shards<S, T, C>(
    cursor: &mut C,
    start_key: T::Key,
    block_number: BlockNumber,
    mut shard_belongs_to_key: impl FnMut(&T::Key) -> bool,
) -> ProviderResult<Vec<u64>>
where
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<S>>,
    C: DbCursorRO<T> + DbCursorRW<T>,
{
    // Start from the given key and iterate through shards
    let mut item = cursor.seek_exact(start_key)?;
    while let Some((sharded_key, list)) = item {
        // If the shard does not belong to the key, break.
        if !shard_belongs_to_key(&sharded_key) {
            break
        }

        // Always delete the current shard from the database first
        // We'll decide later what (if anything) to reinsert
        cursor.delete_current()?;

        // Get the first (lowest) block number in this shard
        // All block numbers in a shard are sorted in ascending order
        let first = list.iter().next().expect("List can't be empty");

        // Case 1: Entire shard is at or above the unwinding point
        // Keep it deleted (don't return anything for reinsertion)
        if first >= block_number {
            item = cursor.prev()?;
            continue
        }
        // Case 2: This is a boundary shard (spans across the unwinding point)
        // The shard contains some blocks below and some at/above the unwinding point
        else if block_number <= sharded_key.as_ref().highest_block_number {
            // Return only the block numbers that are below the unwinding point
            // These will be reinserted to preserve the historical data
            return Ok(list.iter().take_while(|i| *i < block_number).collect::<Vec<_>>())
        }
        // Case 3: Entire shard is below the unwinding point
        // Return all block numbers for reinsertion (preserve entire shard)
        return Ok(list.iter().collect::<Vec<_>>())
    }

    // No shards found or all processed
    Ok(Vec::new())
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Creates a provider with an inner read-only transaction.
    pub const fn new(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
    ) -> Self {
        Self { tx, chain_spec, static_file_provider, prune_modes, storage }
    }

    /// Consume `DbTx` or `DbTxMut`.
    pub fn into_tx(self) -> TX {
        self.tx
    }

    /// Pass `DbTx` or `DbTxMut` mutable reference.
    pub const fn tx_mut(&mut self) -> &mut TX {
        &mut self.tx
    }

    /// Pass `DbTx` or `DbTxMut` immutable reference.
    pub const fn tx_ref(&self) -> &TX {
        &self.tx
    }

    /// Returns a reference to the chain specification.
    pub fn chain_spec(&self) -> &N::ChainSpec {
        &self.chain_spec
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    fn transactions_by_tx_range_with_cursor<C>(
        &self,
        range: impl RangeBounds<TxNumber>,
        cursor: &mut C,
    ) -> ProviderResult<Vec<TxTy<N>>>
    where
        C: DbCursorRO<tables::Transactions<TxTy<N>>>,
    {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Transactions,
            to_range(range),
            |static_file, range, _| static_file.transactions_by_tx_range(range),
            |range, _| self.cursor_collect(cursor, range),
            |_| true,
        )
    }

    fn recovered_block<H, HF, B, BF>(
        &self,
        id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
        header_by_number: HF,
        construct_block: BF,
    ) -> ProviderResult<Option<B>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: FnOnce(BlockNumber) -> ProviderResult<Option<H>>,
        BF: FnOnce(H, BodyTy<N>, Vec<Address>) -> ProviderResult<Option<B>>,
    {
        let Some(block_number) = self.convert_hash_or_number(id)? else { return Ok(None) };
        let Some(header) = header_by_number(block_number)? else { return Ok(None) };

        // Get the block body
        //
        // If the body indices are not found, this means that the transactions either do not exist
        // in the database yet, or they do exit but are not indexed. If they exist but are not
        // indexed, we don't have enough information to return the block anyways, so we return
        // `None`.
        let Some(body) = self.block_body_indices(block_number)? else { return Ok(None) };

        let tx_range = body.tx_num_range();

        let (transactions, senders) = if tx_range.is_empty() {
            (vec![], vec![])
        } else {
            (self.transactions_by_tx_range(tx_range.clone())?, self.senders_by_tx_range(tx_range)?)
        };

        let body = self
            .storage
            .reader()
            .read_block_bodies(self, vec![(header.as_ref(), transactions)])?
            .pop()
            .ok_or(ProviderError::InvalidStorageOutput)?;

        construct_block(header, body, senders)
    }

    /// Returns a range of blocks from the database.
    ///
    /// Uses the provided `headers_range` to get the headers for the range, and `assemble_block` to
    /// construct blocks from the following inputs:
    ///     – Header
    ///     - Range of transaction numbers
    ///     – Ommers
    ///     – Withdrawals
    ///     – Senders
    fn block_range<F, H, HF, R>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        mut assemble_block: F,
    ) -> ProviderResult<Vec<R>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: FnOnce(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        F: FnMut(H, BodyTy<N>, Range<TxNumber>) -> ProviderResult<R>,
    {
        if range.is_empty() {
            return Ok(Vec::new())
        }

        let len = range.end().saturating_sub(*range.start()) as usize;
        let mut blocks = Vec::with_capacity(len);

        let headers = headers_range(range.clone())?;
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions<TxTy<N>>>()?;

        // If the body indices are not found, this means that the transactions either do
        // not exist in the database yet, or they do exit but are
        // not indexed. If they exist but are not indexed, we don't
        // have enough information to return the block anyways, so
        // we skip the block.
        let present_headers = self
            .block_body_indices_range(range)?
            .into_iter()
            .map(|b| b.tx_num_range())
            .zip(headers)
            .collect::<Vec<_>>();

        let mut inputs = Vec::new();
        for (tx_range, header) in &present_headers {
            let transactions = if tx_range.is_empty() {
                Vec::new()
            } else {
                self.transactions_by_tx_range_with_cursor(tx_range.clone(), &mut tx_cursor)?
            };

            inputs.push((header.as_ref(), transactions));
        }

        let bodies = self.storage.reader().read_block_bodies(self, inputs)?;

        for ((tx_range, header), body) in present_headers.into_iter().zip(bodies) {
            blocks.push(assemble_block(header, body, tx_range)?);
        }

        Ok(blocks)
    }

    /// Returns a range of blocks from the database, along with the senders of each
    /// transaction in the blocks.
    ///
    /// Uses the provided `headers_range` to get the headers for the range, and `assemble_block` to
    /// construct blocks from the following inputs:
    ///     – Header
    ///     - Transactions
    ///     – Ommers
    ///     – Withdrawals
    ///     – Senders
    fn block_with_senders_range<H, HF, B, BF>(
        &self,
        range: RangeInclusive<BlockNumber>,
        headers_range: HF,
        assemble_block: BF,
    ) -> ProviderResult<Vec<B>>
    where
        H: AsRef<HeaderTy<N>>,
        HF: Fn(RangeInclusive<BlockNumber>) -> ProviderResult<Vec<H>>,
        BF: Fn(H, BodyTy<N>, Vec<Address>) -> ProviderResult<B>,
    {
        let mut senders_cursor = self.tx.cursor_read::<tables::TransactionSenders>()?;

        self.block_range(range, headers_range, |header, body, tx_range| {
            let senders = if tx_range.is_empty() {
                Vec::new()
            } else {
                // fetch senders from the senders table
                let known_senders =
                    senders_cursor
                        .walk_range(tx_range.clone())?
                        .collect::<Result<HashMap<_, _>, _>>()?;

                let mut senders = Vec::with_capacity(body.transactions().len());
                for (tx_num, tx) in tx_range.zip(body.transactions()) {
                    match known_senders.get(&tx_num) {
                        None => {
                            // recover the sender from the transaction if not found
                            let sender = tx.recover_signer_unchecked()?;
                            senders.push(sender);
                        }
                        Some(sender) => senders.push(*sender),
                    }
                }

                senders
            };

            assemble_block(header, body, senders)
        })
    }

    /// Populate a [`BundleStateInit`] and [`RevertsInit`] using cursors over the
    /// [`PlainAccountState`] and [`PlainStorageState`] tables, based on the given storage and
    /// account changesets.
    fn populate_bundle_state<A, S>(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        plain_accounts_cursor: &mut A,
        plain_storage_cursor: &mut S,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)>
    where
        A: DbCursorRO<PlainAccountState>,
        S: DbDupCursorRO<PlainStorageState>,
    {
        // iterate previous value and get plain state value to create changeset
        // Double option around Account represent if Account state is know (first option) and
        // account is removed (Second Option)
        let mut state: BundleStateInit = HashMap::default();

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.

        let mut reverts: RevertsInit = HashMap::default();

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let new_info = plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1);
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
                    let present_info = plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1);
                    entry.insert((present_info, present_info, HashMap::default()))
                }
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };

            // match storage.
            match account_state.2.entry(old_storage.key) {
                hash_map::Entry::Vacant(entry) => {
                    let new_storage = plain_storage_cursor
                        .seek_by_key_subkey(address, old_storage.key)?
                        .filter(|storage| storage.key == old_storage.key)
                        .unwrap_or_default();
                    entry.insert((old_storage.value, new_storage.value));
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
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Load shard and remove it. If list is empty, last shard was full or
    /// there are no shards at all.
    fn take_shard<T>(
        &self,
        cursor: &mut <TX as DbTxMut>::CursorMut<T>,
        key: T::Key,
    ) -> ProviderResult<Vec<u64>>
    where
        T: Table<Value = BlockNumberList>,
    {
        if let Some((_, list)) = cursor.seek_exact(key)? {
            // delete old shard so new one can be inserted.
            cursor.delete_current()?;
            let list = list.iter().collect::<Vec<_>>();
            return Ok(list)
        }
        Ok(Vec::new())
    }

    /// Insert history index to the database.
    ///
    /// For each updated partial key, this function removes the last shard from
    /// the database (if any), appends the new indices to it, chunks the resulting integer list and
    /// inserts the new shards back into the database.
    ///
    /// This function is used by history indexing stages.
    fn append_history_index<P, T>(
        &self,
        index_updates: impl IntoIterator<Item = (P, impl IntoIterator<Item = u64>)>,
        mut sharded_key_factory: impl FnMut(P, BlockNumber) -> T::Key,
    ) -> ProviderResult<()>
    where
        P: Copy,
        T: Table<Value = BlockNumberList>,
    {
        let mut cursor = self.tx.cursor_write::<T>()?;
        for (partial_key, indices) in index_updates {
            let mut last_shard =
                self.take_shard::<T>(&mut cursor, sharded_key_factory(partial_key, u64::MAX))?;
            last_shard.extend(indices);
            // Chunk indices and insert them in shards of N size.
            let mut chunks = last_shard.chunks(sharded_key::NUM_OF_INDICES_IN_SHARD).peekable();
            while let Some(list) = chunks.next() {
                let highest_block_number = if chunks.peek().is_some() {
                    *list.last().expect("`chunks` does not return empty list")
                } else {
                    // Insert last list with `u64::MAX`.
                    u64::MAX
                };
                cursor.insert(
                    sharded_key_factory(partial_key, highest_block_number),
                    &BlockNumberList::new_pre_sorted(list.iter().copied()),
                )?;
            }
        }
        Ok(())
    }
}

impl<TX: DbTx, N: NodeTypes> AccountReader for DatabaseProvider<TX, N> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        Ok(self.tx.get_by_encoded_key::<tables::PlainAccountState>(address)?)
    }
}

impl<TX: DbTx, N: NodeTypes> AccountExtReader for DatabaseProvider<TX, N> {
    fn changed_accounts_with_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        self.tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|entry| {
                entry.map(|(_, account_before)| account_before.address).map_err(Into::into)
            })
            .collect()
    }

    fn basic_accounts(
        &self,
        iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account>)>> {
        let mut plain_accounts = self.tx.cursor_read::<tables::PlainAccountState>()?;
        Ok(iter
            .into_iter()
            .map(|address| plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v))))
            .collect::<Result<Vec<_>, _>>()?)
    }

    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<u64>>> {
        let mut changeset_cursor = self.tx.cursor_read::<tables::AccountChangeSets>()?;

        let account_transitions = changeset_cursor.walk_range(range)?.try_fold(
            BTreeMap::new(),
            |mut accounts: BTreeMap<Address, Vec<u64>>, entry| -> ProviderResult<_> {
                let (index, account) = entry?;
                accounts.entry(account.address).or_default().push(index);
                Ok(accounts)
            },
        )?;

        Ok(account_transitions)
    }
}

impl<TX: DbTx, N: NodeTypes> StorageChangeSetReader for DatabaseProvider<TX, N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        let range = block_number..=block_number;
        let storage_range = BlockNumberAddress::range(range);
        self.tx
            .cursor_dup_read::<tables::StorageChangeSets>()?
            .walk_range(storage_range)?
            .map(|result| -> ProviderResult<_> { Ok(result?) })
            .collect()
    }
}

impl<TX: DbTx, N: NodeTypes> ChangeSetReader for DatabaseProvider<TX, N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        let range = block_number..=block_number;
        self.tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .map(|result| -> ProviderResult<_> {
                let (_, account_before) = result?;
                Ok(account_before)
            })
            .collect()
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> HeaderSyncGapProvider
    for DatabaseProvider<TX, N>
{
    type Header = HeaderTy<N>;

    fn local_tip_header(
        &self,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<SealedHeader<Self::Header>> {
        let static_file_provider = self.static_file_provider();

        // Make sure Headers static file is at the same height. If it's further, this
        // input execution was interrupted previously and we need to unwind the static file.
        let next_static_file_block_num = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .map(|id| id + 1)
            .unwrap_or_default();
        let next_block = highest_uninterrupted_block + 1;

        match next_static_file_block_num.cmp(&next_block) {
            // The node shutdown between an executed static file commit and before the database
            // commit, so we need to unwind the static files.
            Ordering::Greater => {
                let mut static_file_producer =
                    static_file_provider.latest_writer(StaticFileSegment::Headers)?;
                static_file_producer.prune_headers(next_static_file_block_num - next_block)?;
                // Since this is a database <-> static file inconsistency, we commit the change
                // straight away.
                static_file_producer.commit()?
            }
            Ordering::Less => {
                // There's either missing or corrupted files.
                return Err(ProviderError::HeaderNotFound(next_static_file_block_num.into()))
            }
            Ordering::Equal => {}
        }

        let local_head = static_file_provider
            .sealed_header(highest_uninterrupted_block)?
            .ok_or_else(|| ProviderError::HeaderNotFound(highest_uninterrupted_block.into()))?;

        Ok(local_head)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> HeaderProvider for DatabaseProvider<TX, N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        if let Some(num) = self.block_number(*block_hash)? {
            Ok(self.header_by_number(num)?)
        } else {
            Ok(None)
        }
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            num,
            |static_file| static_file.header_by_number(num),
            || Ok(self.tx.get::<tables::Headers<Self::Header>>(num)?),
        )
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        if let Some(num) = self.block_number(*block_hash)? {
            self.header_td_by_number(num)
        } else {
            Ok(None)
        }
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        if self.chain_spec.is_paris_active_at_block(number) &&
            let Some(td) = self.chain_spec.final_paris_total_difficulty()
        {
            // if this block is higher than the final paris(merge) block, return the final paris
            // difficulty
            return Ok(Some(td))
        }

        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.header_td_by_number(number),
            || Ok(self.tx.get::<tables::HeaderTerminalDifficulties>(number)?.map(|td| td.0)),
        )
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, _| static_file.headers_range(range),
            |range, _| self.cursor_read_collect::<tables::Headers<Self::Header>>(range),
            |_| true,
        )
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.sealed_header(number),
            || {
                if let Some(header) = self.header_by_number(number)? {
                    let hash = self
                        .block_hash(number)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
                    Ok(Some(SealedHeader::new(header, hash)))
                } else {
                    Ok(None)
                }
            },
        )
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            to_range(range),
            |static_file, range, predicate| static_file.sealed_headers_while(range, predicate),
            |range, mut predicate| {
                let mut headers = vec![];
                for entry in
                    self.tx.cursor_read::<tables::Headers<Self::Header>>()?.walk_range(range)?
                {
                    let (number, header) = entry?;
                    let hash = self
                        .block_hash(number)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
                    let sealed = SealedHeader::new(header, hash);
                    if !predicate(&sealed) {
                        break
                    }
                    headers.push(sealed);
                }
                Ok(headers)
            },
            predicate,
        )
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> BlockHashReader for DatabaseProvider<TX, N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || Ok(self.tx.get::<tables::CanonicalHeaders>(number)?),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| self.cursor_read_collect::<tables::CanonicalHeaders>(range),
            |_| true,
        )
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> BlockNumReader for DatabaseProvider<TX, N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_number = self.best_block_number()?;
        let best_hash = self.block_hash(best_number)?.unwrap_or_default();
        Ok(ChainInfo { best_hash, best_number })
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        // The best block number is tracked via the finished stage which gets updated in the same tx
        // when new blocks committed
        Ok(self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or_default())
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(self
            .tx
            .cursor_read::<tables::CanonicalHeaders>()?
            .last()?
            .map(|(num, _)| num)
            .max(
                self.static_file_provider.get_highest_static_file_block(StaticFileSegment::Headers),
            )
            .unwrap_or_default())
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(self.tx.get::<tables::HeaderNumbers>(hash)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> BlockReader for DatabaseProvider<TX, N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        if source.is_canonical() {
            self.block(hash.into())
        } else {
            Ok(None)
        }
    }

    /// Returns the block with matching number from database.
    ///
    /// If the header for this block is not found, this returns `None`.
    /// If the header is found, but the transactions either do not exist, or are not indexed, this
    /// will return None.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        if let Some(number) = self.convert_hash_or_number(id)? &&
            let Some(header) = self.header_by_number(number)?
        {
            // If the body indices are not found, this means that the transactions either do not
            // exist in the database yet, or they do exit but are not indexed.
            // If they exist but are not indexed, we don't have enough
            // information to return the block anyways, so we return `None`.
            let Some(transactions) = self.transactions_by_block(number.into())? else {
                return Ok(None)
            };

            let body = self
                .storage
                .reader()
                .read_block_bodies(self, vec![(&header, transactions)])?
                .pop()
                .ok_or(ProviderError::InvalidStorageOutput)?;

            return Ok(Some(Self::Block::new(header, body)))
        }

        Ok(None)
    }
    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        Ok(None)
    }

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// **NOTE: The transactions have invalid hashes, since they would need to be calculated on the
    /// spot, and we want fast querying.**
    ///
    /// If the header for this block is not found, this returns `None`.
    /// If the header is found, but the transactions either do not exist, or are not indexed, this
    /// will return None.
    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.recovered_block(
            id,
            transaction_kind,
            |block_number| self.header_by_number(block_number),
            |header, body, senders| {
                Self::Block::new(header, body)
                    // Note: we're using unchecked here because we know the block contains valid txs
                    // wrt to its height and can ignore the s value check so pre
                    // EIP-2 txs are allowed
                    .try_into_recovered_unchecked(senders)
                    .map(Some)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.recovered_block(
            id,
            transaction_kind,
            |block_number| self.sealed_header(block_number),
            |header, body, senders| {
                Self::Block::new_sealed(header, body)
                    // Note: we're using unchecked here because we know the block contains valid txs
                    // wrt to its height and can ignore the s value check so pre
                    // EIP-2 txs are allowed
                    .try_with_senders_unchecked(senders)
                    .map(Some)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.block_range(
            range,
            |range| self.headers_range(range),
            |header, body, _| Ok(Self::Block::new(header, body)),
        )
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.block_with_senders_range(
            range,
            |range| self.headers_range(range),
            |header, body, senders| {
                Self::Block::new(header, body)
                    .try_into_recovered_unchecked(senders)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.block_with_senders_range(
            range,
            |range| self.sealed_headers_range(range),
            |header, body, senders| {
                Self::Block::new_sealed(header, body)
                    .try_with_senders(senders)
                    .map_err(|_| ProviderError::SenderRecoveryError)
            },
        )
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> TransactionsProviderExt
    for DatabaseProvider<TX, N>
{
    /// Recovers transaction hashes by walking through `Transactions` table and
    /// calculating them in a parallel manner. Returned unsorted.
    fn transaction_hashes_by_range(
        &self,
        tx_range: Range<TxNumber>,
    ) -> ProviderResult<Vec<(TxHash, TxNumber)>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Transactions,
            tx_range,
            |static_file, range, _| static_file.transaction_hashes_by_range(range),
            |tx_range, _| {
                let mut tx_cursor = self.tx.cursor_read::<tables::Transactions<TxTy<N>>>()?;
                let tx_range_size = tx_range.clone().count();
                let tx_walker = tx_cursor.walk_range(tx_range)?;

                let chunk_size = (tx_range_size / rayon::current_num_threads()).max(1);
                let mut channels = Vec::with_capacity(chunk_size);
                let mut transaction_count = 0;

                #[inline]
                fn calculate_hash<T>(
                    entry: Result<(TxNumber, T), DatabaseError>,
                    rlp_buf: &mut Vec<u8>,
                ) -> Result<(B256, TxNumber), Box<ProviderError>>
                where
                    T: Encodable2718,
                {
                    let (tx_id, tx) = entry.map_err(|e| Box::new(e.into()))?;
                    tx.encode_2718(rlp_buf);
                    Ok((keccak256(rlp_buf), tx_id))
                }

                for chunk in &tx_walker.chunks(chunk_size) {
                    let (tx, rx) = mpsc::channel();
                    channels.push(rx);

                    // Note: Unfortunate side-effect of how chunk is designed in itertools (it is
                    // not Send)
                    let chunk: Vec<_> = chunk.collect();
                    transaction_count += chunk.len();

                    // Spawn the task onto the global rayon pool
                    // This task will send the results through the channel after it has calculated
                    // the hash.
                    rayon::spawn(move || {
                        let mut rlp_buf = Vec::with_capacity(128);
                        for entry in chunk {
                            rlp_buf.clear();
                            let _ = tx.send(calculate_hash(entry, &mut rlp_buf));
                        }
                    });
                }
                let mut tx_list = Vec::with_capacity(transaction_count);

                // Iterate over channels and append the tx hashes unsorted
                for channel in channels {
                    while let Ok(tx) = channel.recv() {
                        let (tx_hash, tx_id) = tx.map_err(|boxed| *boxed)?;
                        tx_list.push((tx_hash, tx_id));
                    }
                }

                Ok(tx_list)
            },
            |_| true,
        )
    }
}

// Calculates the hash of the given transaction
impl<TX: DbTx + 'static, N: NodeTypesForProvider> TransactionsProvider for DatabaseProvider<TX, N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Ok(self.tx.get::<tables::TransactionHashNumbers>(tx_hash)?)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id(id),
            || Ok(self.tx.get::<tables::Transactions<Self::Transaction>>(id)?),
        )
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Transactions,
            id,
            |static_file| static_file.transaction_by_id_unhashed(id),
            || Ok(self.tx.get::<tables::Transactions<Self::Transaction>>(id)?),
        )
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        if let Some(id) = self.transaction_id(hash)? {
            Ok(self.transaction_by_id_unhashed(id)?)
        } else {
            Ok(None)
        }
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        let mut transaction_cursor = self.tx.cursor_read::<tables::TransactionBlocks>()?;
        if let Some(transaction_id) = self.transaction_id(tx_hash)? &&
            let Some(transaction) = self.transaction_by_id_unhashed(transaction_id)? &&
            let Some(block_number) =
                transaction_cursor.seek(transaction_id).map(|b| b.map(|(_, bn)| bn))? &&
            let Some(sealed_header) = self.sealed_header(block_number)?
        {
            let (header, block_hash) = sealed_header.split();
            if let Some(block_body) = self.block_body_indices(block_number)? {
                // the index of the tx in the block is the offset:
                // len([start..tx_id])
                // NOTE: `transaction_id` is always `>=` the block's first
                // index
                let index = transaction_id - block_body.first_tx_num();

                let meta = TransactionMeta {
                    tx_hash,
                    index,
                    block_hash,
                    block_number,
                    base_fee: header.base_fee_per_gas(),
                    excess_blob_gas: header.excess_blob_gas(),
                    timestamp: header.timestamp(),
                };

                return Ok(Some((transaction, meta)))
            }
        }

        Ok(None)
    }

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        let mut cursor = self.tx.cursor_read::<tables::TransactionBlocks>()?;
        Ok(cursor.seek(id)?.map(|(_, bn)| bn))
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions<Self::Transaction>>()?;

        if let Some(block_number) = self.convert_hash_or_number(id)? &&
            let Some(body) = self.block_body_indices(block_number)?
        {
            let tx_range = body.tx_num_range();
            return if tx_range.is_empty() {
                Ok(Some(Vec::new()))
            } else {
                Ok(Some(self.transactions_by_tx_range_with_cursor(tx_range, &mut tx_cursor)?))
            }
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        let range = to_range(range);
        let mut tx_cursor = self.tx.cursor_read::<tables::Transactions<Self::Transaction>>()?;

        self.block_body_indices_range(range.start..=range.end.saturating_sub(1))?
            .into_iter()
            .map(|body| {
                let tx_num_range = body.tx_num_range();
                if tx_num_range.is_empty() {
                    Ok(Vec::new())
                } else {
                    Ok(self
                        .transactions_by_tx_range_with_cursor(tx_num_range, &mut tx_cursor)?
                        .into_iter()
                        .collect())
                }
            })
            .collect()
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.transactions_by_tx_range_with_cursor(
            range,
            &mut self.tx.cursor_read::<tables::Transactions<_>>()?,
        )
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        self.cursor_read_collect::<tables::TransactionSenders>(range)
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self.tx.get::<tables::TransactionSenders>(id)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> ReceiptProvider for DatabaseProvider<TX, N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || Ok(self.tx.get::<tables::Receipts<Self::Receipt>>(id)?),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        if let Some(id) = self.transaction_id(hash)? {
            self.receipt(id)
        } else {
            Ok(None)
        }
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        if let Some(number) = self.convert_hash_or_number(block)? &&
            let Some(body) = self.block_body_indices(number)?
        {
            let tx_range = body.tx_num_range();
            return if tx_range.is_empty() {
                Ok(Some(Vec::new()))
            } else {
                self.receipts_by_tx_range(tx_range).map(Some)
            }
        }
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.cursor_read_collect::<tables::Receipts<Self::Receipt>>(range),
            |_| true,
        )
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        if block_range.is_empty() {
            return Ok(Vec::new());
        }

        // collect block body indices for each block in the range
        let mut block_body_indices = Vec::new();
        for block_num in block_range {
            if let Some(indices) = self.block_body_indices(block_num)? {
                block_body_indices.push(indices);
            } else {
                // use default indices for missing blocks (empty block)
                block_body_indices.push(StoredBlockBodyIndices::default());
            }
        }

        if block_body_indices.is_empty() {
            return Ok(Vec::new());
        }

        // find blocks with transactions to determine transaction range
        let non_empty_blocks: Vec<_> =
            block_body_indices.iter().filter(|indices| indices.tx_count > 0).collect();

        if non_empty_blocks.is_empty() {
            // all blocks are empty
            return Ok(vec![Vec::new(); block_body_indices.len()]);
        }

        // calculate the overall transaction range
        let first_tx = non_empty_blocks[0].first_tx_num();
        let last_tx = non_empty_blocks[non_empty_blocks.len() - 1].last_tx_num();

        // fetch all receipts in the transaction range
        let all_receipts = self.receipts_by_tx_range(first_tx..=last_tx)?;
        let mut receipts_iter = all_receipts.into_iter();

        // distribute receipts to their respective blocks
        let mut result = Vec::with_capacity(block_body_indices.len());
        for indices in &block_body_indices {
            if indices.tx_count == 0 {
                result.push(Vec::new());
            } else {
                let block_receipts =
                    receipts_iter.by_ref().take(indices.tx_count as usize).collect();
                result.push(block_receipts);
            }
        }

        Ok(result)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> BlockBodyIndicesProvider
    for DatabaseProvider<TX, N>
{
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(self.tx.get::<tables::BlockBodyIndices>(num)?)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        self.cursor_read_collect::<tables::BlockBodyIndices>(range)
    }
}

impl<TX: DbTx, N: NodeTypes> StageCheckpointReader for DatabaseProvider<TX, N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(if let Some(encoded) = id.get_pre_encoded() {
            self.tx.get_by_encoded_key::<tables::StageCheckpoints>(encoded)?
        } else {
            self.tx.get::<tables::StageCheckpoints>(id.to_string())?
        })
    }

    /// Get stage checkpoint progress.
    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(self.tx.get::<tables::StageCheckpointProgresses>(id.to_string())?)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.tx
            .cursor_read::<tables::StageCheckpoints>()?
            .walk(None)?
            .collect::<Result<Vec<(String, StageCheckpoint)>, _>>()
            .map_err(ProviderError::Database)
    }
}

impl<TX: DbTxMut, N: NodeTypes> StageCheckpointWriter for DatabaseProvider<TX, N> {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(
        &self,
        id: StageId,
        checkpoint: StageCheckpoint,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StageCheckpoints>(id.to_string(), checkpoint)?)
    }

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(
        &self,
        id: StageId,
        checkpoint: Vec<u8>,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::StageCheckpointProgresses>(id.to_string(), checkpoint)?)
    }

    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> ProviderResult<()> {
        // iterate over all existing stages in the table and update its progress.
        let mut cursor = self.tx.cursor_write::<tables::StageCheckpoints>()?;
        for stage_id in StageId::ALL {
            let (_, checkpoint) = cursor.seek_exact(stage_id.to_string())?.unwrap_or_default();
            cursor.upsert(
                stage_id.to_string(),
                &StageCheckpoint {
                    block_number,
                    ..if drop_stage_checkpoint { Default::default() } else { checkpoint }
                },
            )?;
        }

        Ok(())
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> StorageReader for DatabaseProvider<TX, N> {
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> ProviderResult<Vec<(Address, Vec<StorageEntry>)>> {
        let mut plain_storage = self.tx.cursor_dup_read::<tables::PlainStorageState>()?;

        addresses_with_keys
            .into_iter()
            .map(|(address, storage)| {
                storage
                    .into_iter()
                    .map(|key| -> ProviderResult<_> {
                        Ok(plain_storage
                            .seek_by_key_subkey(address, key)?
                            .filter(|v| v.key == key)
                            .unwrap_or_else(|| StorageEntry { key, value: Default::default() }))
                    })
                    .collect::<ProviderResult<Vec<_>>>()
                    .map(|storage| (address, storage))
            })
            .collect::<ProviderResult<Vec<(_, _)>>>()
    }

    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>> {
        self.tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(BlockNumberAddress::range(range))?
            // fold all storages and save its old state so we can remove it from HashedStorage
            // it is needed as it is dup table.
            .try_fold(BTreeMap::new(), |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                let (BlockNumberAddress((_, address)), storage_entry) = entry?;
                accounts.entry(address).or_default().insert(storage_entry.key);
                Ok(accounts)
            })
    }

    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>> {
        let mut changeset_cursor = self.tx.cursor_read::<tables::StorageChangeSets>()?;

        let storage_changeset_lists =
            changeset_cursor.walk_range(BlockNumberAddress::range(range))?.try_fold(
                BTreeMap::new(),
                |mut storages: BTreeMap<(Address, B256), Vec<u64>>, entry| -> ProviderResult<_> {
                    let (index, storage) = entry?;
                    storages
                        .entry((index.address(), storage.key))
                        .or_default()
                        .push(index.block_number());
                    Ok(storages)
                },
            )?;

        Ok(storage_changeset_lists)
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider> StateWriter
    for DatabaseProvider<TX, N>
{
    type Receipt = ReceiptTy<N>;

    fn write_state(
        &self,
        execution_outcome: &ExecutionOutcome<Self::Receipt>,
        is_value_known: OriginalValuesKnown,
    ) -> ProviderResult<()> {
        let first_block = execution_outcome.first_block();
        let block_count = execution_outcome.len() as u64;
        let last_block = execution_outcome.last_block();
        let block_range = first_block..=last_block;

        let tip = self.last_block_number()?.max(last_block);

        let (plain_state, reverts) =
            execution_outcome.bundle.to_plain_state_and_reverts(is_value_known);

        self.write_state_reverts(reverts, first_block)?;
        self.write_state_changes(plain_state)?;

        // Fetch the first transaction number for each block in the range
        let block_indices: Vec<_> = self
            .block_body_indices_range(block_range)?
            .into_iter()
            .map(|b| b.first_tx_num)
            .collect();

        // Ensure all expected blocks are present.
        if block_indices.len() < block_count as usize {
            let missing_blocks = block_count - block_indices.len() as u64;
            return Err(ProviderError::BlockBodyIndicesNotFound(
                last_block.saturating_sub(missing_blocks - 1),
            ));
        }

        let has_receipts_pruning = self.prune_modes.has_receipts_pruning();

        // Prepare receipts cursor if we are going to write receipts to the database
        //
        // We are writing to database if requested or if there's any kind of receipt pruning
        // configured
        let mut receipts_cursor = self.tx.cursor_write::<tables::Receipts<Self::Receipt>>()?;

        // Prepare receipts static writer if we are going to write receipts to static files
        //
        // We are writing to static files if requested and if there's no receipt pruning configured
        let mut receipts_static_writer = has_receipts_pruning
            .not()
            .then(|| self.static_file_provider.get_writer(first_block, StaticFileSegment::Receipts))
            .transpose()?;

        let has_contract_log_filter = !self.prune_modes.receipts_log_filter.is_empty();
        let contract_log_pruner = self.prune_modes.receipts_log_filter.group_by_block(tip, None)?;

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PruneSegment::ContractLogs`].
        let prunable_receipts =
            PruneMode::Distance(MINIMUM_PRUNING_DISTANCE).should_prune(first_block, tip);

        // Prepare set of addresses which logs should not be pruned.
        let mut allowed_addresses: HashSet<Address, _> = HashSet::new();
        for (_, addresses) in contract_log_pruner.range(..first_block) {
            allowed_addresses.extend(addresses.iter().copied());
        }

        for (idx, (receipts, first_tx_index)) in
            execution_outcome.receipts.iter().zip(block_indices).enumerate()
        {
            let block_number = first_block + idx as u64;

            // Increment block number for receipts static file writer
            if let Some(writer) = receipts_static_writer.as_mut() {
                writer.increment_block(block_number)?;
            }

            // Skip writing receipts if pruning configuration requires us to.
            if prunable_receipts &&
                self.prune_modes
                    .receipts
                    .is_some_and(|mode| mode.should_prune(block_number, tip))
            {
                continue
            }

            // If there are new addresses to retain after this block number, track them
            if let Some(new_addresses) = contract_log_pruner.get(&block_number) {
                allowed_addresses.extend(new_addresses.iter().copied());
            }

            for (idx, receipt) in receipts.iter().enumerate() {
                let receipt_idx = first_tx_index + idx as u64;
                // Skip writing receipt if log filter is active and it does not have any logs to
                // retain
                if prunable_receipts &&
                    has_contract_log_filter &&
                    !receipt.logs().iter().any(|log| allowed_addresses.contains(&log.address))
                {
                    continue
                }

                if let Some(writer) = &mut receipts_static_writer {
                    writer.append_receipt(receipt_idx, receipt)?;
                }

                receipts_cursor.append(receipt_idx, receipt)?;
            }
        }

        Ok(())
    }

    fn write_state_reverts(
        &self,
        reverts: PlainStateReverts,
        first_block: BlockNumber,
    ) -> ProviderResult<()> {
        // Write storage changes
        tracing::trace!("Writing storage changes");
        let mut storages_cursor = self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        let mut storage_changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::StorageChangeSets>()?;
        for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;

            tracing::trace!(block_number, "Writing block change");
            // sort changes by address.
            storage_changes.par_sort_unstable_by_key(|a| a.address);
            for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                let storage_id = BlockNumberAddress((block_number, address));

                let mut storage = storage_revert
                    .into_iter()
                    .map(|(k, v)| (B256::new(k.to_be_bytes()), v))
                    .collect::<Vec<_>>();
                // sort storage slots by key.
                storage.par_sort_unstable_by_key(|a| a.0);

                // If we are writing the primary storage wipe transition, the pre-existing plain
                // storage state has to be taken from the database and written to storage history.
                // See [StorageWipe::Primary] for more details.
                //
                // TODO(mediocregopher): This could be rewritten in a way which doesn't require
                // collecting wiped entries into a Vec like this, see
                // `write_storage_trie_changesets`.
                let mut wiped_storage = Vec::new();
                if wiped {
                    tracing::trace!(?address, "Wiping storage");
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        wiped_storage.push((entry.key, entry.value));
                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            wiped_storage.push((entry.key, entry.value))
                        }
                    }
                }

                tracing::trace!(?address, ?storage, "Writing storage reverts");
                for (key, value) in StorageRevertsIter::new(storage, wiped_storage) {
                    storage_changeset_cursor.append_dup(storage_id, StorageEntry { key, value })?;
                }
            }
        }

        // Write account changes
        tracing::trace!("Writing account changes");
        let mut account_changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::AccountChangeSets>()?;

        for (block_index, mut account_block_reverts) in reverts.accounts.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;
            // Sort accounts by address.
            account_block_reverts.par_sort_by_key(|a| a.0);

            for (address, info) in account_block_reverts {
                account_changeset_cursor.append_dup(
                    block_number,
                    AccountBeforeTx { address, info: info.map(Into::into) },
                )?;
            }
        }

        Ok(())
    }

    fn write_state_changes(&self, mut changes: StateChangeset) -> ProviderResult<()> {
        // sort all entries so they can be written to database in more performant way.
        // and take smaller memory footprint.
        changes.accounts.par_sort_by_key(|a| a.0);
        changes.storage.par_sort_by_key(|a| a.address);
        changes.contracts.par_sort_by_key(|a| a.0);

        // Write new account state
        tracing::trace!(len = changes.accounts.len(), "Writing new account state");
        let mut accounts_cursor = self.tx_ref().cursor_write::<tables::PlainAccountState>()?;
        // write account to database.
        for (address, account) in changes.accounts {
            if let Some(account) = account {
                tracing::trace!(?address, "Updating plain state account");
                accounts_cursor.upsert(address, &account.into())?;
            } else if accounts_cursor.seek_exact(address)?.is_some() {
                tracing::trace!(?address, "Deleting plain state account");
                accounts_cursor.delete_current()?;
            }
        }

        // Write bytecode
        tracing::trace!(len = changes.contracts.len(), "Writing bytecodes");
        let mut bytecodes_cursor = self.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in changes.contracts {
            bytecodes_cursor.upsert(hash, &Bytecode(bytecode))?;
        }

        // Write new storage state and wipe storage if needed.
        tracing::trace!(len = changes.storage.len(), "Writing new storage state");
        let mut storages_cursor = self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for PlainStorageChangeset { address, wipe_storage, storage } in changes.storage {
            // Wiping of storage.
            if wipe_storage && storages_cursor.seek_exact(address)?.is_some() {
                storages_cursor.delete_current_duplicates()?;
            }
            // cast storages to B256.
            let mut storage = storage
                .into_iter()
                .map(|(k, value)| StorageEntry { key: k.into(), value })
                .collect::<Vec<_>>();
            // sort storage slots by key.
            storage.par_sort_unstable_by_key(|a| a.key);

            for entry in storage {
                tracing::trace!(?address, ?entry.key, "Updating plain state storage");
                if let Some(db_entry) = storages_cursor.seek_by_key_subkey(address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    storages_cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    storages_cursor.upsert(address, &entry)?;
                }
            }
        }

        Ok(())
    }

    fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()> {
        // Write hashed account updates.
        let mut hashed_accounts_cursor = self.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in hashed_state.accounts().accounts_sorted() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, &account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = hashed_state.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            self.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in sorted_storages {
            if storage.is_wiped() && hashed_storage_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_storage_cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage.storage_slots_sorted() {
                let entry = StorageEntry { key: hashed_slot, value };
                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(*hashed_address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    hashed_storage_cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    hashed_storage_cursor.upsert(*hashed_address, &entry)?;
                }
            }
        }

        Ok(())
    }

    /// Remove the last N blocks of state.
    ///
    /// The latest state will be unwound
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn remove_state_above(&self, block: BlockNumber) -> ProviderResult<()> {
        let range = block + 1..=self.last_block_number()?;

        if range.is_empty() {
            return Ok(());
        }

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.block_body_indices_range(range.clone())?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").first_tx_num();

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset = self.take::<tables::StorageChangeSets>(storage_range)?;
        let account_changeset = self.take::<tables::AccountChangeSets>(range)?;

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

        let (state, _) = self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            &mut plain_accounts_cursor,
            &mut plain_storage_cursor,
        )?;

        // iterate over local plain state remove all account and all storages.
        for (address, (old_account, new_account, storage)) in &state {
            // revert account if needed.
            if old_account != new_account {
                let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                if let Some(account) = old_account {
                    plain_accounts_cursor.upsert(*address, account)?;
                } else if existing_entry.is_some() {
                    plain_accounts_cursor.delete_current()?;
                }
            }

            // revert storages
            for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                let storage_entry = StorageEntry { key: *storage_key, value: *old_storage_value };
                // delete previous value
                // TODO: This does not use dupsort features
                if plain_storage_cursor
                    .seek_by_key_subkey(*address, *storage_key)?
                    .filter(|s| s.key == *storage_key)
                    .is_some()
                {
                    plain_storage_cursor.delete_current()?
                }

                // insert value if needed
                if !old_storage_value.is_zero() {
                    plain_storage_cursor.upsert(*address, &storage_entry)?;
                }
            }
        }

        self.remove_receipts_from(from_transaction_num, block)?;

        Ok(())
    }

    /// Take the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// The latest state will be unwound and returned back with all the blocks
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn take_state_above(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<ExecutionOutcome<Self::Receipt>> {
        let range = block + 1..=self.last_block_number()?;

        if range.is_empty() {
            return Ok(ExecutionOutcome::default())
        }
        let start_block_number = *range.start();

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.block_body_indices_range(range.clone())?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").first_tx_num();
        let to_transaction_num =
            block_bodies.last().expect("already checked if there are blocks").last_tx_num();

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset = self.take::<tables::StorageChangeSets>(storage_range)?;
        let account_changeset = self.take::<tables::AccountChangeSets>(range)?;

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

        // populate bundle state and reverts from changesets / state cursors, to iterate over,
        // remove, and return later
        let (state, reverts) = self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            &mut plain_accounts_cursor,
            &mut plain_storage_cursor,
        )?;

        // iterate over local plain state remove all account and all storages.
        for (address, (old_account, new_account, storage)) in &state {
            // revert account if needed.
            if old_account != new_account {
                let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                if let Some(account) = old_account {
                    plain_accounts_cursor.upsert(*address, account)?;
                } else if existing_entry.is_some() {
                    plain_accounts_cursor.delete_current()?;
                }
            }

            // revert storages
            for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                let storage_entry = StorageEntry { key: *storage_key, value: *old_storage_value };
                // delete previous value
                // TODO: This does not use dupsort features
                if plain_storage_cursor
                    .seek_by_key_subkey(*address, *storage_key)?
                    .filter(|s| s.key == *storage_key)
                    .is_some()
                {
                    plain_storage_cursor.delete_current()?
                }

                // insert value if needed
                if !old_storage_value.is_zero() {
                    plain_storage_cursor.upsert(*address, &storage_entry)?;
                }
            }
        }

        // Collect receipts into tuples (tx_num, receipt) to correctly handle pruned receipts
        let mut receipts_iter = self
            .static_file_provider
            .get_range_with_static_file_or_database(
                StaticFileSegment::Receipts,
                from_transaction_num..to_transaction_num + 1,
                |static_file, range, _| {
                    static_file
                        .receipts_by_tx_range(range.clone())
                        .map(|r| range.into_iter().zip(r).collect())
                },
                |range, _| {
                    self.tx
                        .cursor_read::<tables::Receipts<Self::Receipt>>()?
                        .walk_range(range)?
                        .map(|r| r.map_err(Into::into))
                        .collect()
                },
                |_| true,
            )?
            .into_iter()
            .peekable();

        let mut receipts = Vec::with_capacity(block_bodies.len());
        // loop break if we are at the end of the blocks.
        for block_body in block_bodies {
            let mut block_receipts = Vec::with_capacity(block_body.tx_count as usize);
            for num in block_body.tx_num_range() {
                if receipts_iter.peek().is_some_and(|(n, _)| *n == num) {
                    block_receipts.push(receipts_iter.next().unwrap().1);
                }
            }
            receipts.push(block_receipts);
        }

        self.remove_receipts_from(from_transaction_num, block)?;

        Ok(ExecutionOutcome::new_init(
            state,
            reverts,
            Vec::new(),
            receipts,
            start_block_number,
            Vec::new(),
        ))
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> TrieWriter for DatabaseProvider<TX, N> {
    /// Writes trie updates. Returns the number of entries modified.
    fn write_trie_updates(&self, trie_updates: &TrieUpdates) -> ProviderResult<usize> {
        if trie_updates.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        // Merge updated and removed nodes. Updated nodes must take precedence.
        let mut account_updates = trie_updates
            .removed_nodes_ref()
            .iter()
            .filter_map(|n| {
                (!trie_updates.account_nodes_ref().contains_key(n)).then_some((n, None))
            })
            .collect::<Vec<_>>();
        account_updates.extend(
            trie_updates.account_nodes_ref().iter().map(|(nibbles, node)| (nibbles, Some(node))),
        );
        // Sort trie node updates.
        account_updates.sort_unstable_by(|a, b| a.0.cmp(b.0));

        let tx = self.tx_ref();
        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;
        for (key, updated_node) in account_updates {
            let nibbles = StoredNibbles(*key);
            match updated_node {
                Some(node) => {
                    if !nibbles.0.is_empty() {
                        num_entries += 1;
                        account_trie_cursor.upsert(nibbles, node)?;
                    }
                }
                None => {
                    num_entries += 1;
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
            }
        }

        num_entries += self.write_storage_trie_updates(trie_updates.storage_tries_ref().iter())?;

        Ok(num_entries)
    }

    /// Records the current values of all trie nodes which will be updated using the [`TrieUpdates`]
    /// into the trie changesets tables.
    ///
    /// The intended usage of this method is to call it _prior_ to calling `write_trie_updates` with
    /// the same [`TrieUpdates`].
    ///
    /// Returns the number of keys written.
    fn write_trie_changesets(
        &self,
        block_number: BlockNumber,
        trie_updates: &TrieUpdatesSorted,
        updates_overlay: Option<&TrieUpdatesSorted>,
    ) -> ProviderResult<usize> {
        let mut num_entries = 0;

        let mut changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::AccountsTrieChangeSets>()?;
        let curr_values_cursor = self.tx_ref().cursor_read::<tables::AccountsTrie>()?;

        // Wrap the cursor in DatabaseAccountTrieCursor
        let mut db_account_cursor = DatabaseAccountTrieCursor::new(curr_values_cursor);

        // Static empty array for when updates_overlay is None
        static EMPTY_ACCOUNT_UPDATES: Vec<(Nibbles, Option<BranchNodeCompact>)> = Vec::new();

        // Get the overlay updates for account trie, or use an empty array
        let account_overlay_updates = updates_overlay
            .map(|overlay| overlay.account_nodes_ref())
            .unwrap_or(&EMPTY_ACCOUNT_UPDATES);

        // Wrap the cursor in InMemoryTrieCursor with the overlay
        let mut in_memory_account_cursor =
            InMemoryTrieCursor::new(Some(&mut db_account_cursor), account_overlay_updates);

        for (path, _) in trie_updates.account_nodes_ref() {
            num_entries += 1;
            let node = in_memory_account_cursor.seek_exact(*path)?.map(|(_, node)| node);
            changeset_cursor.append_dup(
                block_number,
                TrieChangeSetsEntry { nibbles: StoredNibblesSubKey(*path), node },
            )?;
        }

        let mut storage_updates = trie_updates.storage_tries.iter().collect::<Vec<_>>();
        storage_updates.sort_unstable_by(|a, b| a.0.cmp(b.0));

        num_entries += self.write_storage_trie_changesets(
            block_number,
            storage_updates.into_iter(),
            updates_overlay,
        )?;

        Ok(num_entries)
    }

    fn clear_trie_changesets(&self) -> ProviderResult<()> {
        let tx = self.tx_ref();
        tx.clear::<tables::AccountsTrieChangeSets>()?;
        tx.clear::<tables::StoragesTrieChangeSets>()?;
        Ok(())
    }

    fn clear_trie_changesets_from(&self, from: BlockNumber) -> ProviderResult<()> {
        let tx = self.tx_ref();
        {
            let range = from..;
            let mut cursor = tx.cursor_dup_write::<tables::AccountsTrieChangeSets>()?;
            let mut walker = cursor.walk_range(range)?;

            while walker.next().transpose()?.is_some() {
                walker.delete_current()?;
            }
        }

        {
            let range: RangeFrom<BlockNumberHashedAddress> = (from, B256::ZERO).into()..;
            let mut cursor = tx.cursor_dup_write::<tables::StoragesTrieChangeSets>()?;
            let mut walker = cursor.walk_range(range)?;

            while walker.next().transpose()?.is_some() {
                walker.delete_current()?;
            }
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> TrieReader for DatabaseProvider<TX, N> {
    fn revert_trie(&self, from: BlockNumber) -> ProviderResult<TrieUpdatesSorted> {
        let tx = self.tx_ref();

        // Read account trie changes directly into a Vec - data is already sorted by nibbles
        // within each block, and we want the oldest (first) version of each node
        let mut account_nodes = Vec::new();
        let mut seen_account_keys = HashSet::new();
        let mut accounts_cursor = tx.cursor_dup_read::<tables::AccountsTrieChangeSets>()?;

        for entry in accounts_cursor.walk_range(from..)? {
            let (_, TrieChangeSetsEntry { nibbles, node }) = entry?;
            // Only keep the first (oldest) version of each node
            if seen_account_keys.insert(nibbles.0) {
                account_nodes.push((nibbles.0, node));
            }
        }

        // Read storage trie changes - data is sorted by (block, hashed_address, nibbles)
        // Keep track of seen (address, nibbles) pairs to only keep the oldest version
        let mut storage_tries = B256Map::<Vec<_>>::default();
        let mut seen_storage_keys = HashSet::new();
        let mut storages_cursor = tx.cursor_dup_read::<tables::StoragesTrieChangeSets>()?;
        let storage_range = BlockNumberHashedAddress((from, B256::ZERO))..;

        for entry in storages_cursor.walk_range(storage_range)? {
            let (
                BlockNumberHashedAddress((_, hashed_address)),
                TrieChangeSetsEntry { nibbles, node },
            ) = entry?;

            // Only keep the first (oldest) version of each node for this address
            if seen_storage_keys.insert((hashed_address, nibbles.0)) {
                storage_tries.entry(hashed_address).or_default().push((nibbles.0, node));
            }
        }

        // Convert to StorageTrieUpdatesSorted
        let storage_tries = storage_tries
            .into_iter()
            .map(|(address, nodes)| {
                (address, StorageTrieUpdatesSorted { storage_nodes: nodes, is_deleted: false })
            })
            .collect();

        Ok(TrieUpdatesSorted { account_nodes, storage_tries })
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> StorageTrieWriter for DatabaseProvider<TX, N> {
    /// Writes storage trie updates from the given storage trie map.
    ///
    /// First sorts the storage trie updates by the hashed address key, writing in sorted order.
    ///
    /// Returns the number of entries modified.
    fn write_storage_trie_updates<'a>(
        &self,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdates)>,
    ) -> ProviderResult<usize> {
        let mut num_entries = 0;
        let mut storage_tries = storage_tries.collect::<Vec<_>>();
        storage_tries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let mut db_storage_trie_cursor =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }

        Ok(num_entries)
    }

    /// Records the current values of all trie nodes which will be updated using the
    /// [`StorageTrieUpdates`] into the storage trie changesets table.
    ///
    /// The intended usage of this method is to call it _prior_ to calling
    /// `write_storage_trie_updates` with the same set of [`StorageTrieUpdates`].
    ///
    /// Returns the number of keys written.
    fn write_storage_trie_changesets<'a>(
        &self,
        block_number: BlockNumber,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdatesSorted)>,
        updates_overlay: Option<&TrieUpdatesSorted>,
    ) -> ProviderResult<usize> {
        let mut num_written = 0;

        let mut changeset_cursor =
            self.tx_ref().cursor_dup_write::<tables::StoragesTrieChangeSets>()?;

        // We hold two cursors to the same table because we use them simultaneously when an
        // account's storage is wiped. We keep them outside the for-loop so they can be re-used
        // between accounts.
        let changed_curr_values_cursor = self.tx_ref().cursor_dup_read::<tables::StoragesTrie>()?;
        let wiped_nodes_cursor = self.tx_ref().cursor_dup_read::<tables::StoragesTrie>()?;

        // DatabaseStorageTrieCursor requires ownership of the cursor. The easiest way to deal with
        // this is to create this outer variable with an initial dummy account, and overwrite it on
        // every loop for every real account.
        let mut changed_curr_values_cursor = DatabaseStorageTrieCursor::new(
            changed_curr_values_cursor,
            B256::default(), // Will be set per iteration
        );
        let mut wiped_nodes_cursor = DatabaseStorageTrieCursor::new(
            wiped_nodes_cursor,
            B256::default(), // Will be set per iteration
        );

        // Static empty array for when updates_overlay is None
        static EMPTY_UPDATES: Vec<(Nibbles, Option<BranchNodeCompact>)> = Vec::new();

        for (hashed_address, storage_trie_updates) in storage_tries {
            let changeset_key = BlockNumberHashedAddress((block_number, *hashed_address));

            // Update the hashed address for the cursors
            changed_curr_values_cursor =
                DatabaseStorageTrieCursor::new(changed_curr_values_cursor.cursor, *hashed_address);

            // Get the overlay updates for this storage trie, or use an empty array
            let overlay_updates = updates_overlay
                .and_then(|overlay| overlay.storage_tries.get(hashed_address))
                .map(|updates| updates.storage_nodes_ref())
                .unwrap_or(&EMPTY_UPDATES);

            // Wrap the cursor in InMemoryTrieCursor with the overlay
            let mut in_memory_changed_cursor =
                InMemoryTrieCursor::new(Some(&mut changed_curr_values_cursor), overlay_updates);

            // Create an iterator which produces the current values of all updated paths, or None if
            // they are currently unset.
            let curr_values_of_changed = StorageTrieCurrentValuesIter::new(
                storage_trie_updates.storage_nodes.iter().map(|e| e.0),
                &mut in_memory_changed_cursor,
            )?;

            if storage_trie_updates.is_deleted() {
                // Create an iterator that starts from the beginning of the storage trie for this
                // account
                wiped_nodes_cursor =
                    DatabaseStorageTrieCursor::new(wiped_nodes_cursor.cursor, *hashed_address);

                // Wrap the wiped nodes cursor in InMemoryTrieCursor with the overlay
                let mut in_memory_wiped_cursor =
                    InMemoryTrieCursor::new(Some(&mut wiped_nodes_cursor), overlay_updates);

                let all_nodes = TrieCursorIter::new(&mut in_memory_wiped_cursor);

                for wiped in storage_trie_wiped_changeset_iter(curr_values_of_changed, all_nodes)? {
                    let (path, node) = wiped?;
                    num_written += 1;
                    changeset_cursor.append_dup(
                        changeset_key,
                        TrieChangeSetsEntry { nibbles: StoredNibblesSubKey(path), node },
                    )?;
                }
            } else {
                for curr_value in curr_values_of_changed {
                    let (path, node) = curr_value?;
                    num_written += 1;
                    changeset_cursor.append_dup(
                        changeset_key,
                        TrieChangeSetsEntry { nibbles: StoredNibblesSubKey(path), node },
                    )?;
                }
            }
        }

        Ok(num_written)
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> HashingWriter for DatabaseProvider<TX, N> {
    fn unwind_account_hashing<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        // Aggregate all block changesets and make a list of accounts that have been changed.
        // Note that collecting and then reversing the order is necessary to ensure that the
        // changes are applied in the correct order.
        let hashed_accounts = changesets
            .into_iter()
            .map(|(_, e)| (keccak256(e.address), e.info))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<BTreeMap<_, _>>();

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        Ok(hashed_accounts)
    }

    fn unwind_account_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        let changesets = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;
        self.unwind_account_hashing(changesets.iter())
    }

    fn insert_account_for_hashing(
        &self,
        changesets: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        let hashed_accounts =
            changesets.into_iter().map(|(ad, ac)| (keccak256(ad), ac)).collect::<BTreeMap<_, _>>();
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }
        Ok(hashed_accounts)
    }

    fn unwind_storage_hashing(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>> {
        // Aggregate all block changesets and make list of accounts that have been changed.
        let mut hashed_storages = changesets
            .into_iter()
            .map(|(BlockNumberAddress((_, address)), storage_entry)| {
                (keccak256(address), keccak256(storage_entry.key), storage_entry.value)
            })
            .collect::<Vec<_>>();
        hashed_storages.sort_by_key(|(ha, hk, _)| (*ha, *hk));

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_storage_keys: HashMap<B256, BTreeSet<B256>> =
            HashMap::with_capacity_and_hasher(hashed_storages.len(), Default::default());
        let mut hashed_storage = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, key, value) in hashed_storages.into_iter().rev() {
            hashed_storage_keys.entry(hashed_address).or_default().insert(key);

            if hashed_storage
                .seek_by_key_subkey(hashed_address, key)?
                .filter(|entry| entry.key == key)
                .is_some()
            {
                hashed_storage.delete_current()?;
            }

            if !value.is_zero() {
                hashed_storage.upsert(hashed_address, &StorageEntry { key, value })?;
            }
        }
        Ok(hashed_storage_keys)
    }

    fn unwind_storage_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumberAddress>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>> {
        let changesets = self
            .tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;
        self.unwind_storage_hashing(changesets.into_iter())
    }

    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<HashMap<B256, BTreeSet<B256>>> {
        // hash values
        let hashed_storages =
            storages.into_iter().fold(BTreeMap::new(), |mut map, (address, storage)| {
                let storage = storage.into_iter().fold(BTreeMap::new(), |mut map, entry| {
                    map.insert(keccak256(entry.key), entry.value);
                    map
                });
                map.insert(keccak256(address), storage);
                map
            });

        let hashed_storage_keys = hashed_storages
            .iter()
            .map(|(hashed_address, entries)| (*hashed_address, entries.keys().copied().collect()))
            .collect();

        let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        // Hash the address and key and apply them to HashedStorage (if Storage is None
        // just remove it);
        hashed_storages.into_iter().try_for_each(|(hashed_address, storage)| {
            storage.into_iter().try_for_each(|(key, value)| -> ProviderResult<()> {
                if hashed_storage_cursor
                    .seek_by_key_subkey(hashed_address, key)?
                    .filter(|entry| entry.key == key)
                    .is_some()
                {
                    hashed_storage_cursor.delete_current()?;
                }

                if !value.is_zero() {
                    hashed_storage_cursor.upsert(hashed_address, &StorageEntry { key, value })?;
                }
                Ok(())
            })
        })?;

        Ok(hashed_storage_keys)
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> HistoryWriter for DatabaseProvider<TX, N> {
    fn unwind_account_history_indices<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx)>,
    ) -> ProviderResult<usize> {
        let mut last_indices = changesets
            .into_iter()
            .map(|(index, account)| (account.address, *index))
            .collect::<Vec<_>>();
        last_indices.sort_by_key(|(a, _)| *a);

        // Unwind the account history index.
        let mut cursor = self.tx.cursor_write::<tables::AccountsHistory>()?;
        for &(address, rem_index) in &last_indices {
            let partial_shard = unwind_history_shards::<_, tables::AccountsHistory, _>(
                &mut cursor,
                ShardedKey::last(address),
                rem_index,
                |sharded_key| sharded_key.key == address,
            )?;

            // Check the last returned partial shard.
            // If it's not empty, the shard needs to be reinserted.
            if !partial_shard.is_empty() {
                cursor.insert(
                    ShardedKey::last(address),
                    &BlockNumberList::new_pre_sorted(partial_shard),
                )?;
            }
        }

        let changesets = last_indices.len();
        Ok(changesets)
    }

    fn unwind_account_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize> {
        let changesets = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;
        self.unwind_account_history_indices(changesets.iter())
    }

    fn insert_account_history_index(
        &self,
        account_transitions: impl IntoIterator<Item = (Address, impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::AccountsHistory>(
            account_transitions,
            ShardedKey::new,
        )
    }

    fn unwind_storage_history_indices(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<usize> {
        let mut storage_changesets = changesets
            .into_iter()
            .map(|(BlockNumberAddress((bn, address)), storage)| (address, storage.key, bn))
            .collect::<Vec<_>>();
        storage_changesets.sort_by_key(|(address, key, _)| (*address, *key));

        let mut cursor = self.tx.cursor_write::<tables::StoragesHistory>()?;
        for &(address, storage_key, rem_index) in &storage_changesets {
            let partial_shard = unwind_history_shards::<_, tables::StoragesHistory, _>(
                &mut cursor,
                StorageShardedKey::last(address, storage_key),
                rem_index,
                |storage_sharded_key| {
                    storage_sharded_key.address == address &&
                        storage_sharded_key.sharded_key.key == storage_key
                },
            )?;

            // Check the last returned partial shard.
            // If it's not empty, the shard needs to be reinserted.
            if !partial_shard.is_empty() {
                cursor.insert(
                    StorageShardedKey::last(address, storage_key),
                    &BlockNumberList::new_pre_sorted(partial_shard),
                )?;
            }
        }

        let changesets = storage_changesets.len();
        Ok(changesets)
    }

    fn unwind_storage_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumberAddress>,
    ) -> ProviderResult<usize> {
        let changesets = self
            .tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;
        self.unwind_storage_history_indices(changesets.into_iter())
    }

    fn insert_storage_history_index(
        &self,
        storage_transitions: impl IntoIterator<Item = ((Address, B256), impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::StoragesHistory>(
            storage_transitions,
            |(address, storage_key), highest_block_number| {
                StorageShardedKey::new(address, storage_key, highest_block_number)
            },
        )
    }

    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()> {
        // account history stage
        {
            let indices = self.changed_accounts_and_blocks_with_range(range.clone())?;
            self.insert_account_history_index(indices)?;
        }

        // storage history stage
        {
            let indices = self.changed_storages_and_blocks_with_range(range)?;
            self.insert_storage_history_index(indices)?;
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider + 'static> BlockExecutionWriter
    for DatabaseProvider<TX, N>
{
    fn take_block_and_execution_above(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<Chain<Self::Primitives>> {
        let range = block + 1..=self.last_block_number()?;

        self.unwind_trie_state_range(range.clone())?;

        // get execution res
        let execution_state = self.take_state_above(block)?;

        let blocks = self.recovered_block_range(range)?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove_blocks_above(block)?;

        // Update pipeline progress
        self.update_pipeline_stages(block, true)?;

        Ok(Chain::new(blocks, execution_state, None))
    }

    fn remove_block_and_execution_above(&self, block: BlockNumber) -> ProviderResult<()> {
        let range = block + 1..=self.last_block_number()?;

        self.unwind_trie_state_range(range)?;

        // remove execution res
        self.remove_state_above(block)?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove_blocks_above(block)?;

        // Update pipeline progress
        self.update_pipeline_stages(block, true)?;

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider + 'static> BlockWriter
    for DatabaseProvider<TX, N>
{
    type Block = BlockTy<N>;
    type Receipt = ReceiptTy<N>;

    /// Inserts the block into the database, always modifying the following tables:
    /// * [`CanonicalHeaders`](tables::CanonicalHeaders)
    /// * [`Headers`](tables::Headers)
    /// * [`HeaderNumbers`](tables::HeaderNumbers)
    /// * [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties)
    /// * [`BlockBodyIndices`](tables::BlockBodyIndices)
    ///
    /// If there are transactions in the block, the following tables will be modified:
    /// * [`Transactions`](tables::Transactions)
    /// * [`TransactionBlocks`](tables::TransactionBlocks)
    ///
    /// If ommers are not empty, this will modify [`BlockOmmers`](tables::BlockOmmers).
    /// If withdrawals are not empty, this will modify
    /// [`BlockWithdrawals`](tables::BlockWithdrawals).
    ///
    /// If the provider has __not__ configured full sender pruning, this will modify
    /// [`TransactionSenders`](tables::TransactionSenders).
    ///
    /// If the provider has __not__ configured full transaction lookup pruning, this will modify
    /// [`TransactionHashNumbers`](tables::TransactionHashNumbers).
    fn insert_block(
        &self,
        block: RecoveredBlock<Self::Block>,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        let block_number = block.number();

        let mut durations_recorder = metrics::DurationsRecorder::default();

        // total difficulty
        let ttd = if block_number == 0 {
            block.header().difficulty()
        } else {
            let parent_block_number = block_number - 1;
            let parent_ttd = self.header_td_by_number(parent_block_number)?.unwrap_or_default();
            durations_recorder.record_relative(metrics::Action::GetParentTD);
            parent_ttd + block.header().difficulty()
        };

        self.static_file_provider
            .get_writer(block_number, StaticFileSegment::Headers)?
            .append_header(block.header(), ttd, &block.hash())?;

        self.tx.put::<tables::HeaderNumbers>(block.hash(), block_number)?;
        durations_recorder.record_relative(metrics::Action::InsertHeaderNumbers);

        let mut next_tx_num = self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .last()?
            .map(|(n, _)| n + 1)
            .unwrap_or_default();
        durations_recorder.record_relative(metrics::Action::GetNextTxNum);
        let first_tx_num = next_tx_num;

        let tx_count = block.body().transaction_count() as u64;

        // Ensures we have all the senders for the block's transactions.
        for (transaction, sender) in block.body().transactions_iter().zip(block.senders_iter()) {
            let hash = transaction.tx_hash();

            if self.prune_modes.sender_recovery.as_ref().is_none_or(|m| !m.is_full()) {
                self.tx.put::<tables::TransactionSenders>(next_tx_num, *sender)?;
            }

            if self.prune_modes.transaction_lookup.is_none_or(|m| !m.is_full()) {
                self.tx.put::<tables::TransactionHashNumbers>(*hash, next_tx_num)?;
            }
            next_tx_num += 1;
        }

        self.append_block_bodies(vec![(block_number, Some(block.into_body()))])?;

        debug!(
            target: "providers::db",
            ?block_number,
            actions = ?durations_recorder.actions,
            "Inserted block"
        );

        Ok(StoredBlockBodyIndices { first_tx_num, tx_count })
    }

    fn append_block_bodies(
        &self,
        bodies: Vec<(BlockNumber, Option<BodyTy<N>>)>,
    ) -> ProviderResult<()> {
        let Some(from_block) = bodies.first().map(|(block, _)| *block) else { return Ok(()) };

        // Initialize writer if we will be writing transactions to staticfiles
        let mut tx_writer =
            self.static_file_provider.get_writer(from_block, StaticFileSegment::Transactions)?;

        let mut block_indices_cursor = self.tx.cursor_write::<tables::BlockBodyIndices>()?;
        let mut tx_block_cursor = self.tx.cursor_write::<tables::TransactionBlocks>()?;

        // Get id for the next tx_num or zero if there are no transactions.
        let mut next_tx_num = tx_block_cursor.last()?.map(|(id, _)| id + 1).unwrap_or_default();

        for (block_number, body) in &bodies {
            // Increment block on static file header.
            tx_writer.increment_block(*block_number)?;

            let tx_count = body.as_ref().map(|b| b.transactions().len() as u64).unwrap_or_default();
            let block_indices = StoredBlockBodyIndices { first_tx_num: next_tx_num, tx_count };

            let mut durations_recorder = metrics::DurationsRecorder::default();

            // insert block meta
            block_indices_cursor.append(*block_number, &block_indices)?;

            durations_recorder.record_relative(metrics::Action::InsertBlockBodyIndices);

            let Some(body) = body else { continue };

            // write transaction block index
            if !body.transactions().is_empty() {
                tx_block_cursor.append(block_indices.last_tx_num(), block_number)?;
                durations_recorder.record_relative(metrics::Action::InsertTransactionBlocks);
            }

            // write transactions
            for transaction in body.transactions() {
                tx_writer.append_transaction(next_tx_num, transaction)?;

                // Increment transaction id for each transaction.
                next_tx_num += 1;
            }
        }

        self.storage.writer().write_block_bodies(self, bodies)?;

        Ok(())
    }

    fn remove_blocks_above(&self, block: BlockNumber) -> ProviderResult<()> {
        // Clean up HeaderNumbers for blocks being removed, we must clear all indexes from MDBX.
        for hash in self.canonical_hashes_range(block + 1, self.last_block_number()? + 1)? {
            self.tx.delete::<tables::HeaderNumbers>(hash, None)?;
        }

        // Get highest static file block for the total block range
        let highest_static_file_block = self
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .expect("todo: error handling, headers should exist");

        // IMPORTANT: we use `highest_static_file_block.saturating_sub(block_number)` to make sure
        // we remove only what is ABOVE the block.
        //
        // i.e., if the highest static file block is 8, we want to remove above block 5 only, we
        // will have three blocks to remove, which will be block 8, 7, and 6.
        debug!(target: "providers::db", ?block, "Removing static file blocks above block_number");
        self.static_file_provider()
            .get_writer(block, StaticFileSegment::Headers)?
            .prune_headers(highest_static_file_block.saturating_sub(block))?;

        // First transaction to be removed
        let unwind_tx_from = self
            .block_body_indices(block)?
            .map(|b| b.next_tx_num())
            .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

        // Last transaction to be removed
        let unwind_tx_to = self
            .tx
            .cursor_read::<tables::BlockBodyIndices>()?
            .last()?
            // shouldn't happen because this was OK above
            .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?
            .1
            .last_tx_num();

        if unwind_tx_from <= unwind_tx_to {
            for (hash, _) in self.transaction_hashes_by_range(unwind_tx_from..(unwind_tx_to + 1))? {
                self.tx.delete::<tables::TransactionHashNumbers>(hash, None)?;
            }
        }

        self.remove::<tables::TransactionSenders>(unwind_tx_from..)?;

        self.remove_bodies_above(block)?;

        Ok(())
    }

    fn remove_bodies_above(&self, block: BlockNumber) -> ProviderResult<()> {
        self.storage.writer().remove_block_bodies_above(self, block)?;

        // First transaction to be removed
        let unwind_tx_from = self
            .block_body_indices(block)?
            .map(|b| b.next_tx_num())
            .ok_or(ProviderError::BlockBodyIndicesNotFound(block))?;

        self.remove::<tables::BlockBodyIndices>(block + 1..)?;
        self.remove::<tables::TransactionBlocks>(unwind_tx_from..)?;

        let static_file_tx_num =
            self.static_file_provider.get_highest_static_file_tx(StaticFileSegment::Transactions);

        let to_delete = static_file_tx_num
            .map(|static_tx| (static_tx + 1).saturating_sub(unwind_tx_from))
            .unwrap_or_default();

        self.static_file_provider
            .latest_writer(StaticFileSegment::Transactions)?
            .prune_transactions(to_delete, block)?;

        Ok(())
    }

    /// TODO(joshie): this fn should be moved to `UnifiedStorageWriter` eventually
    fn append_blocks_with_state(
        &self,
        blocks: Vec<RecoveredBlock<Self::Block>>,
        execution_outcome: &ExecutionOutcome<Self::Receipt>,
        hashed_state: HashedPostStateSorted,
    ) -> ProviderResult<()> {
        if blocks.is_empty() {
            debug!(target: "providers::db", "Attempted to append empty block range");
            return Ok(())
        }

        // Blocks are not empty, so no need to handle the case of `blocks.first()` being
        // `None`.
        let first_number = blocks[0].number();

        // Blocks are not empty, so no need to handle the case of `blocks.last()` being
        // `None`.
        let last_block_number = blocks[blocks.len() - 1].number();

        let mut durations_recorder = metrics::DurationsRecorder::default();

        // Insert the blocks
        for block in blocks {
            self.insert_block(block)?;
            durations_recorder.record_relative(metrics::Action::InsertBlock);
        }

        self.write_state(execution_outcome, OriginalValuesKnown::No)?;
        durations_recorder.record_relative(metrics::Action::InsertState);

        // insert hashes and intermediate merkle nodes
        self.write_hashed_state(&hashed_state)?;
        durations_recorder.record_relative(metrics::Action::InsertHashes);

        self.update_history_indices(first_number..=last_block_number)?;
        durations_recorder.record_relative(metrics::Action::InsertHistoryIndices);

        // Update pipeline progress
        self.update_pipeline_stages(last_block_number, false)?;
        durations_recorder.record_relative(metrics::Action::UpdatePipelineStages);

        debug!(target: "providers::db", range = ?first_number..=last_block_number, actions = ?durations_recorder.actions, "Appended blocks");

        Ok(())
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> PruneCheckpointReader for DatabaseProvider<TX, N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        Ok(self.tx.get::<tables::PruneCheckpoints>(segment)?)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        Ok(self
            .tx
            .cursor_read::<tables::PruneCheckpoints>()?
            .walk(None)?
            .collect::<Result<_, _>>()?)
    }
}

impl<TX: DbTxMut, N: NodeTypes> PruneCheckpointWriter for DatabaseProvider<TX, N> {
    fn save_prune_checkpoint(
        &self,
        segment: PruneSegment,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        Ok(self.tx.put::<tables::PruneCheckpoints>(segment, checkpoint)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> StatsReader for DatabaseProvider<TX, N> {
    fn count_entries<T: Table>(&self) -> ProviderResult<usize> {
        let db_entries = self.tx.entries::<T>()?;
        let static_file_entries = match self.static_file_provider.count_entries::<T>() {
            Ok(entries) => entries,
            Err(ProviderError::UnsupportedProvider) => 0,
            Err(err) => return Err(err),
        };

        Ok(db_entries + static_file_entries)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> ChainStateBlockReader for DatabaseProvider<TX, N> {
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>> {
        let mut finalized_blocks = self
            .tx
            .cursor_read::<tables::ChainState>()?
            .walk(Some(tables::ChainStateKey::LastFinalizedBlock))?
            .take(1)
            .collect::<Result<BTreeMap<tables::ChainStateKey, BlockNumber>, _>>()?;

        let last_finalized_block_number = finalized_blocks.pop_first().map(|pair| pair.1);
        Ok(last_finalized_block_number)
    }

    fn last_safe_block_number(&self) -> ProviderResult<Option<BlockNumber>> {
        let mut finalized_blocks = self
            .tx
            .cursor_read::<tables::ChainState>()?
            .walk(Some(tables::ChainStateKey::LastSafeBlockBlock))?
            .take(1)
            .collect::<Result<BTreeMap<tables::ChainStateKey, BlockNumber>, _>>()?;

        let last_finalized_block_number = finalized_blocks.pop_first().map(|pair| pair.1);
        Ok(last_finalized_block_number)
    }
}

impl<TX: DbTxMut, N: NodeTypes> ChainStateBlockWriter for DatabaseProvider<TX, N> {
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        Ok(self
            .tx
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, block_number)?)
    }

    fn save_safe_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        Ok(self
            .tx
            .put::<tables::ChainState>(tables::ChainStateKey::LastSafeBlockBlock, block_number)?)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes + 'static> DBProvider for DatabaseProvider<TX, N> {
    type Tx = TX;

    fn tx_ref(&self) -> &Self::Tx {
        &self.tx
    }

    fn tx_mut(&mut self) -> &mut Self::Tx {
        &mut self.tx
    }

    fn into_tx(self) -> Self::Tx {
        self.tx
    }

    fn prune_modes_ref(&self) -> &PruneModes {
        self.prune_modes_ref()
    }

    /// Commit database transaction and static files.
    fn commit(self) -> ProviderResult<bool> {
        // For unwinding it makes more sense to commit the database first, since if
        // it is interrupted before the static files commit, we can just
        // truncate the static files according to the
        // checkpoints on the next start-up.
        if self.static_file_provider.has_unwind_queued() {
            self.tx.commit()?;
            self.static_file_provider.commit()?;
        } else {
            self.static_file_provider.commit()?;
            self.tx.commit()?;
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{blocks::BlockchainTestData, create_test_provider_factory},
        BlockWriter,
    };
    use reth_testing_utils::generators::{self, random_block, BlockParams};

    #[test]
    fn test_receipts_by_block_range_empty_range() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();

        // empty range should return empty vec
        let start = 10u64;
        let end = 9u64;
        let result = provider.receipts_by_block_range(start..=end).unwrap();
        assert_eq!(result, Vec::<Vec<reth_ethereum_primitives::Receipt>>::new());
    }

    #[test]
    fn test_receipts_by_block_range_nonexistent_blocks() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();

        // non-existent blocks should return empty vecs for each block
        let result = provider.receipts_by_block_range(10..=12).unwrap();
        assert_eq!(result, vec![vec![], vec![], vec![]]);
    }

    #[test]
    fn test_receipts_by_block_range_single_block() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        provider_rw.insert_block(data.blocks[0].0.clone()).unwrap();
        provider_rw.write_state(&data.blocks[0].1, crate::OriginalValuesKnown::No).unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let result = provider.receipts_by_block_range(1..=1).unwrap();

        // should have one vec with one receipt
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result[0][0], data.blocks[0].1.receipts()[0][0]);
    }

    #[test]
    fn test_receipts_by_block_range_multiple_blocks() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(data.blocks[i].0.clone()).unwrap();
            provider_rw.write_state(&data.blocks[i].1, crate::OriginalValuesKnown::No).unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let result = provider.receipts_by_block_range(1..=3).unwrap();

        // should have 3 vecs, each with one receipt
        assert_eq!(result.len(), 3);
        for (i, block_receipts) in result.iter().enumerate() {
            assert_eq!(block_receipts.len(), 1);
            assert_eq!(block_receipts[0], data.blocks[i].1.receipts()[0][0]);
        }
    }

    #[test]
    fn test_receipts_by_block_range_blocks_with_varying_tx_counts() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();

        // insert blocks 1-3 with receipts
        for i in 0..3 {
            provider_rw.insert_block(data.blocks[i].0.clone()).unwrap();
            provider_rw.write_state(&data.blocks[i].1, crate::OriginalValuesKnown::No).unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let result = provider.receipts_by_block_range(1..=3).unwrap();

        // verify each block has one receipt
        assert_eq!(result.len(), 3);
        for block_receipts in &result {
            assert_eq!(block_receipts.len(), 1);
        }
    }

    #[test]
    fn test_receipts_by_block_range_partial_range() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(data.blocks[i].0.clone()).unwrap();
            provider_rw.write_state(&data.blocks[i].1, crate::OriginalValuesKnown::No).unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();

        // request range that includes both existing and non-existing blocks
        let result = provider.receipts_by_block_range(2..=5).unwrap();
        assert_eq!(result.len(), 4);

        // blocks 2-3 should have receipts, blocks 4-5 should be empty
        assert_eq!(result[0].len(), 1); // block 2
        assert_eq!(result[1].len(), 1); // block 3
        assert_eq!(result[2].len(), 0); // block 4 (doesn't exist)
        assert_eq!(result[3].len(), 0); // block 5 (doesn't exist)

        assert_eq!(result[0][0], data.blocks[1].1.receipts()[0][0]);
        assert_eq!(result[1][0], data.blocks[2].1.receipts()[0][0]);
    }

    #[test]
    fn test_receipts_by_block_range_all_empty_blocks() {
        let factory = create_test_provider_factory();
        let mut rng = generators::rng();

        // create blocks with no transactions
        let mut blocks = Vec::new();
        for i in 0..3 {
            let block =
                random_block(&mut rng, i, BlockParams { tx_count: Some(0), ..Default::default() });
            blocks.push(block);
        }

        let provider_rw = factory.provider_rw().unwrap();
        for block in blocks {
            provider_rw.insert_block(block.try_recover().unwrap()).unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let result = provider.receipts_by_block_range(1..=3).unwrap();

        assert_eq!(result.len(), 3);
        for block_receipts in result {
            assert_eq!(block_receipts.len(), 0);
        }
    }

    #[test]
    fn test_receipts_by_block_range_consistency_with_individual_calls() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(data.blocks[i].0.clone()).unwrap();
            provider_rw.write_state(&data.blocks[i].1, crate::OriginalValuesKnown::No).unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();

        // get receipts using block range method
        let range_result = provider.receipts_by_block_range(1..=3).unwrap();

        // get receipts using individual block calls
        let mut individual_results = Vec::new();
        for block_num in 1..=3 {
            let receipts =
                provider.receipts_by_block(block_num.into()).unwrap().unwrap_or_default();
            individual_results.push(receipts);
        }

        assert_eq!(range_result, individual_results);
    }

    #[test]
    fn test_write_trie_changesets() {
        use reth_db_api::models::BlockNumberHashedAddress;
        use reth_trie::{BranchNodeCompact, StorageTrieEntry};

        let factory = create_test_provider_factory();
        let provider_rw = factory.provider_rw().unwrap();

        let block_number = 1u64;

        // Create some test nibbles and nodes
        let account_nibbles1 = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
        let account_nibbles2 = Nibbles::from_nibbles([0x5, 0x6, 0x7, 0x8]);

        let node1 = BranchNodeCompact::new(
            0b1111_1111_1111_1111, // state_mask
            0b0000_0000_0000_0000, // tree_mask
            0b0000_0000_0000_0000, // hash_mask
            vec![],                // hashes
            None,                  // root hash
        );

        // Pre-populate AccountsTrie with a node that will be updated (for account_nibbles1)
        {
            let mut cursor = provider_rw.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();
            cursor.insert(StoredNibbles(account_nibbles1), &node1).unwrap();
        }

        // Create account trie updates: one Some (update) and one None (removal)
        let account_nodes = vec![
            (account_nibbles1, Some(node1.clone())), // This will update existing node
            (account_nibbles2, None),                // This will be a removal (no existing node)
        ];

        // Create storage trie updates
        let storage_address1 = B256::from([1u8; 32]); // Normal storage trie
        let storage_address2 = B256::from([2u8; 32]); // Wiped storage trie

        let storage_nibbles1 = Nibbles::from_nibbles([0xa, 0xb]);
        let storage_nibbles2 = Nibbles::from_nibbles([0xc, 0xd]);
        let storage_nibbles3 = Nibbles::from_nibbles([0xe, 0xf]);

        let storage_node1 = BranchNodeCompact::new(
            0b1111_0000_0000_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        let storage_node2 = BranchNodeCompact::new(
            0b0000_1111_0000_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Create an old version of storage_node1 to prepopulate
        let storage_node1_old = BranchNodeCompact::new(
            0b1010_0000_0000_0000, // Different mask to show it's an old value
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Pre-populate StoragesTrie for normal storage (storage_address1)
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            // Add node that will be updated (storage_nibbles1) with old value
            let entry = StorageTrieEntry {
                nibbles: StoredNibblesSubKey(storage_nibbles1),
                node: storage_node1_old.clone(),
            };
            cursor.upsert(storage_address1, &entry).unwrap();
        }

        // Pre-populate StoragesTrie for wiped storage (storage_address2)
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            // Add node that will be updated (storage_nibbles1)
            let entry1 = StorageTrieEntry {
                nibbles: StoredNibblesSubKey(storage_nibbles1),
                node: storage_node1.clone(),
            };
            cursor.upsert(storage_address2, &entry1).unwrap();
            // Add node that won't be updated but exists (storage_nibbles3)
            let entry3 = StorageTrieEntry {
                nibbles: StoredNibblesSubKey(storage_nibbles3),
                node: storage_node2.clone(),
            };
            cursor.upsert(storage_address2, &entry3).unwrap();
        }

        // Normal storage trie: one Some (update) and one None (new)
        let storage_trie1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1.clone())), // This will update existing node
                (storage_nibbles2, None),                        // This is a new node
            ],
        };

        // Wiped storage trie
        let storage_trie2 = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1.clone())), // Updated node already in db
                (storage_nibbles2, Some(storage_node2.clone())), /* Updated node not in db
                                                                  * storage_nibbles3 is in db
                                                                  * but not updated */
            ],
        };

        let mut storage_tries = B256Map::default();
        storage_tries.insert(storage_address1, storage_trie1);
        storage_tries.insert(storage_address2, storage_trie2);

        let trie_updates = TrieUpdatesSorted { account_nodes, storage_tries };

        // Write the changesets
        let num_written =
            provider_rw.write_trie_changesets(block_number, &trie_updates, None).unwrap();

        // Verify number of entries written
        // Account changesets: 2 (one update, one removal)
        // Storage changesets:
        //   - Normal storage: 2 (one update, one removal)
        //   - Wiped storage: 3 (two updated, one existing not updated)
        // Total: 2 + 2 + 3 = 7
        assert_eq!(num_written, 7);

        // Verify account changesets were written correctly
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_read::<tables::AccountsTrieChangeSets>().unwrap();

            // Get all entries for this block to see what was written
            let all_entries = cursor
                .walk_dup(Some(block_number), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            // Assert the full value of all_entries in a single assert_eq
            assert_eq!(
                all_entries,
                vec![
                    (
                        block_number,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(account_nibbles1),
                            node: Some(node1),
                        }
                    ),
                    (
                        block_number,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(account_nibbles2),
                            node: None,
                        }
                    ),
                ]
            );
        }

        // Verify storage changesets were written correctly
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_read::<tables::StoragesTrieChangeSets>().unwrap();

            // Check normal storage trie changesets
            let key1 = BlockNumberHashedAddress((block_number, storage_address1));
            let entries1 =
                cursor.walk_dup(Some(key1), None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();

            assert_eq!(
                entries1,
                vec![
                    (
                        key1,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles1),
                            node: Some(storage_node1_old), // Old value that was prepopulated
                        }
                    ),
                    (
                        key1,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles2),
                            node: None, // New node, no previous value
                        }
                    ),
                ]
            );

            // Check wiped storage trie changesets
            let key2 = BlockNumberHashedAddress((block_number, storage_address2));
            let entries2 =
                cursor.walk_dup(Some(key2), None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();

            assert_eq!(
                entries2,
                vec![
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles1),
                            node: Some(storage_node1), // Was in db, so has old value
                        }
                    ),
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles2),
                            node: None, // Was not in db
                        }
                    ),
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles3),
                            node: Some(storage_node2), // Existing node in wiped storage
                        }
                    ),
                ]
            );
        }

        provider_rw.commit().unwrap();
    }

    #[test]
    fn test_write_trie_changesets_with_overlay() {
        use reth_db_api::models::BlockNumberHashedAddress;
        use reth_trie::BranchNodeCompact;

        let factory = create_test_provider_factory();
        let provider_rw = factory.provider_rw().unwrap();

        let block_number = 1u64;

        // Create some test nibbles and nodes
        let account_nibbles1 = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
        let account_nibbles2 = Nibbles::from_nibbles([0x5, 0x6, 0x7, 0x8]);

        let node1 = BranchNodeCompact::new(
            0b1111_1111_1111_1111, // state_mask
            0b0000_0000_0000_0000, // tree_mask
            0b0000_0000_0000_0000, // hash_mask
            vec![],                // hashes
            None,                  // root hash
        );

        // NOTE: Unlike the previous test, we're NOT pre-populating the database
        // All node values will come from the overlay

        // Create the overlay with existing values that would normally be in the DB
        let node1_old = BranchNodeCompact::new(
            0b1010_1010_1010_1010, // Different mask to show it's the overlay "existing" value
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Create overlay account nodes
        let overlay_account_nodes = vec![
            (account_nibbles1, Some(node1_old.clone())), // This simulates existing node in overlay
        ];

        // Create account trie updates: one Some (update) and one None (removal)
        let account_nodes = vec![
            (account_nibbles1, Some(node1)), // This will update overlay node
            (account_nibbles2, None),        // This will be a removal (no existing node)
        ];

        // Create storage trie updates
        let storage_address1 = B256::from([1u8; 32]); // Normal storage trie
        let storage_address2 = B256::from([2u8; 32]); // Wiped storage trie

        let storage_nibbles1 = Nibbles::from_nibbles([0xa, 0xb]);
        let storage_nibbles2 = Nibbles::from_nibbles([0xc, 0xd]);
        let storage_nibbles3 = Nibbles::from_nibbles([0xe, 0xf]);

        let storage_node1 = BranchNodeCompact::new(
            0b1111_0000_0000_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        let storage_node2 = BranchNodeCompact::new(
            0b0000_1111_0000_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Create old versions for overlay
        let storage_node1_old = BranchNodeCompact::new(
            0b1010_0000_0000_0000, // Different mask to show it's an old value
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Create overlay storage nodes
        let mut overlay_storage_tries = B256Map::default();

        // Overlay for normal storage (storage_address1)
        let overlay_storage_trie1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1_old.clone())), /* Simulates existing in
                                                                      * overlay */
            ],
        };

        // Overlay for wiped storage (storage_address2)
        let overlay_storage_trie2 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1.clone())), // Existing in overlay
                (storage_nibbles3, Some(storage_node2.clone())), // Also existing in overlay
            ],
        };

        overlay_storage_tries.insert(storage_address1, overlay_storage_trie1);
        overlay_storage_tries.insert(storage_address2, overlay_storage_trie2);

        let overlay = TrieUpdatesSorted {
            account_nodes: overlay_account_nodes,
            storage_tries: overlay_storage_tries,
        };

        // Normal storage trie: one Some (update) and one None (new)
        let storage_trie1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1.clone())), // This will update overlay node
                (storage_nibbles2, None),                        // This is a new node
            ],
        };

        // Wiped storage trie
        let storage_trie2 = StorageTrieUpdatesSorted {
            is_deleted: true,
            storage_nodes: vec![
                (storage_nibbles1, Some(storage_node1.clone())), // Updated node from overlay
                (storage_nibbles2, Some(storage_node2.clone())), /* Updated node not in overlay
                                                                  * storage_nibbles3 is in
                                                                  * overlay
                                                                  * but not updated */
            ],
        };

        let mut storage_tries = B256Map::default();
        storage_tries.insert(storage_address1, storage_trie1);
        storage_tries.insert(storage_address2, storage_trie2);

        let trie_updates = TrieUpdatesSorted { account_nodes, storage_tries };

        // Write the changesets WITH OVERLAY
        let num_written =
            provider_rw.write_trie_changesets(block_number, &trie_updates, Some(&overlay)).unwrap();

        // Verify number of entries written
        // Account changesets: 2 (one update from overlay, one removal)
        // Storage changesets:
        //   - Normal storage: 2 (one update from overlay, one new)
        //   - Wiped storage: 3 (two updated, one existing from overlay not updated)
        // Total: 2 + 2 + 3 = 7
        assert_eq!(num_written, 7);

        // Verify account changesets were written correctly
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_read::<tables::AccountsTrieChangeSets>().unwrap();

            // Get all entries for this block to see what was written
            let all_entries = cursor
                .walk_dup(Some(block_number), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            // Assert the full value of all_entries in a single assert_eq
            assert_eq!(
                all_entries,
                vec![
                    (
                        block_number,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(account_nibbles1),
                            node: Some(node1_old), // Value from overlay, not DB
                        }
                    ),
                    (
                        block_number,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(account_nibbles2),
                            node: None,
                        }
                    ),
                ]
            );
        }

        // Verify storage changesets were written correctly
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_read::<tables::StoragesTrieChangeSets>().unwrap();

            // Check normal storage trie changesets
            let key1 = BlockNumberHashedAddress((block_number, storage_address1));
            let entries1 =
                cursor.walk_dup(Some(key1), None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();

            assert_eq!(
                entries1,
                vec![
                    (
                        key1,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles1),
                            node: Some(storage_node1_old), // Old value from overlay
                        }
                    ),
                    (
                        key1,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles2),
                            node: None, // New node, no previous value
                        }
                    ),
                ]
            );

            // Check wiped storage trie changesets
            let key2 = BlockNumberHashedAddress((block_number, storage_address2));
            let entries2 =
                cursor.walk_dup(Some(key2), None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();

            assert_eq!(
                entries2,
                vec![
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles1),
                            node: Some(storage_node1), // Value from overlay
                        }
                    ),
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles2),
                            node: None, // Was not in overlay
                        }
                    ),
                    (
                        key2,
                        TrieChangeSetsEntry {
                            nibbles: StoredNibblesSubKey(storage_nibbles3),
                            node: Some(storage_node2), /* Existing node from overlay in wiped
                                                        * storage */
                        }
                    ),
                ]
            );
        }

        provider_rw.commit().unwrap();
    }

    #[test]
    fn test_clear_trie_changesets_from() {
        use alloy_primitives::hex_literal::hex;
        use reth_db_api::models::BlockNumberHashedAddress;
        use reth_trie::{BranchNodeCompact, StoredNibblesSubKey, TrieChangeSetsEntry};

        let factory = create_test_provider_factory();

        // Create some test data for different block numbers
        let block1 = 100u64;
        let block2 = 101u64;
        let block3 = 102u64;
        let block4 = 103u64;
        let block5 = 104u64;

        // Create test addresses for storage changesets
        let storage_address1 =
            B256::from(hex!("1111111111111111111111111111111111111111111111111111111111111111"));
        let storage_address2 =
            B256::from(hex!("2222222222222222222222222222222222222222222222222222222222222222"));

        // Create test nibbles
        let nibbles1 = StoredNibblesSubKey(Nibbles::from_nibbles([0x1, 0x2, 0x3]));
        let nibbles2 = StoredNibblesSubKey(Nibbles::from_nibbles([0x4, 0x5, 0x6]));
        let nibbles3 = StoredNibblesSubKey(Nibbles::from_nibbles([0x7, 0x8, 0x9]));

        // Create test nodes
        let node1 = BranchNodeCompact::new(
            0b1111_1111_1111_1111,
            0b1111_1111_1111_1111,
            0b0000_0000_0000_0001,
            vec![B256::from(hex!(
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
            ))],
            None,
        );
        let node2 = BranchNodeCompact::new(
            0b1111_1111_1111_1110,
            0b1111_1111_1111_1110,
            0b0000_0000_0000_0010,
            vec![B256::from(hex!(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
            ))],
            Some(B256::from(hex!(
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
            ))),
        );

        // Populate AccountsTrieChangeSets with data across multiple blocks
        {
            let provider_rw = factory.provider_rw().unwrap();
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::AccountsTrieChangeSets>().unwrap();

            // Block 100: 2 entries (will be kept - before start block)
            cursor
                .upsert(
                    block1,
                    &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: Some(node1.clone()) },
                )
                .unwrap();
            cursor
                .upsert(block1, &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: None })
                .unwrap();

            // Block 101: 3 entries with duplicates (will be deleted - from this block onwards)
            cursor
                .upsert(
                    block2,
                    &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: Some(node2.clone()) },
                )
                .unwrap();
            cursor
                .upsert(
                    block2,
                    &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: Some(node1.clone()) },
                )
                .unwrap(); // duplicate key
            cursor
                .upsert(block2, &TrieChangeSetsEntry { nibbles: nibbles3.clone(), node: None })
                .unwrap();

            // Block 102: 2 entries (will be deleted - after start block)
            cursor
                .upsert(
                    block3,
                    &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: Some(node1.clone()) },
                )
                .unwrap();
            cursor
                .upsert(
                    block3,
                    &TrieChangeSetsEntry { nibbles: nibbles3.clone(), node: Some(node2.clone()) },
                )
                .unwrap();

            // Block 103: 1 entry (will be deleted - after start block)
            cursor
                .upsert(block4, &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: None })
                .unwrap();

            // Block 104: 2 entries (will be deleted - after start block)
            cursor
                .upsert(
                    block5,
                    &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: Some(node2.clone()) },
                )
                .unwrap();
            cursor
                .upsert(block5, &TrieChangeSetsEntry { nibbles: nibbles3.clone(), node: None })
                .unwrap();

            provider_rw.commit().unwrap();
        }

        // Populate StoragesTrieChangeSets with data across multiple blocks
        {
            let provider_rw = factory.provider_rw().unwrap();
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrieChangeSets>().unwrap();

            // Block 100, address1: 2 entries (will be kept - before start block)
            let key1_block1 = BlockNumberHashedAddress((block1, storage_address1));
            cursor
                .upsert(
                    key1_block1,
                    &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: Some(node1.clone()) },
                )
                .unwrap();
            cursor
                .upsert(key1_block1, &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: None })
                .unwrap();

            // Block 101, address1: 3 entries with duplicates (will be deleted - from this block
            // onwards)
            let key1_block2 = BlockNumberHashedAddress((block2, storage_address1));
            cursor
                .upsert(
                    key1_block2,
                    &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: Some(node2.clone()) },
                )
                .unwrap();
            cursor
                .upsert(key1_block2, &TrieChangeSetsEntry { nibbles: nibbles1.clone(), node: None })
                .unwrap(); // duplicate key
            cursor
                .upsert(
                    key1_block2,
                    &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: Some(node1.clone()) },
                )
                .unwrap();

            // Block 102, address2: 2 entries (will be deleted - after start block)
            let key2_block3 = BlockNumberHashedAddress((block3, storage_address2));
            cursor
                .upsert(
                    key2_block3,
                    &TrieChangeSetsEntry { nibbles: nibbles2.clone(), node: Some(node2.clone()) },
                )
                .unwrap();
            cursor
                .upsert(key2_block3, &TrieChangeSetsEntry { nibbles: nibbles3.clone(), node: None })
                .unwrap();

            // Block 103, address1: 2 entries with duplicate (will be deleted - after start block)
            let key1_block4 = BlockNumberHashedAddress((block4, storage_address1));
            cursor
                .upsert(
                    key1_block4,
                    &TrieChangeSetsEntry { nibbles: nibbles3.clone(), node: Some(node1) },
                )
                .unwrap();
            cursor
                .upsert(
                    key1_block4,
                    &TrieChangeSetsEntry { nibbles: nibbles3, node: Some(node2.clone()) },
                )
                .unwrap(); // duplicate key

            // Block 104, address2: 2 entries (will be deleted - after start block)
            let key2_block5 = BlockNumberHashedAddress((block5, storage_address2));
            cursor
                .upsert(key2_block5, &TrieChangeSetsEntry { nibbles: nibbles1, node: None })
                .unwrap();
            cursor
                .upsert(key2_block5, &TrieChangeSetsEntry { nibbles: nibbles2, node: Some(node2) })
                .unwrap();

            provider_rw.commit().unwrap();
        }

        // Clear all changesets from block 101 onwards
        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw.clear_trie_changesets_from(block2).unwrap();
            provider_rw.commit().unwrap();
        }

        // Verify AccountsTrieChangeSets after clearing
        {
            let provider = factory.provider().unwrap();
            let mut cursor =
                provider.tx_ref().cursor_dup_read::<tables::AccountsTrieChangeSets>().unwrap();

            // Block 100 should still exist (before range)
            let block1_entries = cursor
                .walk_dup(Some(block1), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(block1_entries.len(), 2, "Block 100 entries should be preserved");
            assert_eq!(block1_entries[0].0, block1);
            assert_eq!(block1_entries[1].0, block1);

            // Blocks 101-104 should be deleted
            let block2_entries = cursor
                .walk_dup(Some(block2), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block2_entries.is_empty(), "Block 101 entries should be deleted");

            let block3_entries = cursor
                .walk_dup(Some(block3), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block3_entries.is_empty(), "Block 102 entries should be deleted");

            let block4_entries = cursor
                .walk_dup(Some(block4), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block4_entries.is_empty(), "Block 103 entries should be deleted");

            // Block 104 should also be deleted
            let block5_entries = cursor
                .walk_dup(Some(block5), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block5_entries.is_empty(), "Block 104 entries should be deleted");
        }

        // Verify StoragesTrieChangeSets after clearing
        {
            let provider = factory.provider().unwrap();
            let mut cursor =
                provider.tx_ref().cursor_dup_read::<tables::StoragesTrieChangeSets>().unwrap();

            // Block 100 entries should still exist (before range)
            let key1_block1 = BlockNumberHashedAddress((block1, storage_address1));
            let block1_entries = cursor
                .walk_dup(Some(key1_block1), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(block1_entries.len(), 2, "Block 100 storage entries should be preserved");

            // Blocks 101-104 entries should be deleted
            let key1_block2 = BlockNumberHashedAddress((block2, storage_address1));
            let block2_entries = cursor
                .walk_dup(Some(key1_block2), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block2_entries.is_empty(), "Block 101 storage entries should be deleted");

            let key2_block3 = BlockNumberHashedAddress((block3, storage_address2));
            let block3_entries = cursor
                .walk_dup(Some(key2_block3), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block3_entries.is_empty(), "Block 102 storage entries should be deleted");

            let key1_block4 = BlockNumberHashedAddress((block4, storage_address1));
            let block4_entries = cursor
                .walk_dup(Some(key1_block4), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block4_entries.is_empty(), "Block 103 storage entries should be deleted");

            // Block 104 entries should also be deleted
            let key2_block5 = BlockNumberHashedAddress((block5, storage_address2));
            let block5_entries = cursor
                .walk_dup(Some(key2_block5), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(block5_entries.is_empty(), "Block 104 storage entries should be deleted");
        }
    }
}
