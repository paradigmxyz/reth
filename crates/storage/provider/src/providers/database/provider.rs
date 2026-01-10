use crate::{
    changesets_utils::{
        storage_trie_wiped_changeset_iter, StorageRevertsIter, StorageTrieCurrentValuesIter,
    },
    providers::{
        database::{chain::ChainStorage, metrics},
        rocksdb::RocksDBProvider,
        static_file::StaticFileWriter,
        NodeTypesForProvider, StaticFileProvider,
    },
    to_range,
    traits::{
        AccountExtReader, BlockSource, ChangeSetReader, ReceiptProvider, StageCheckpointWriter,
    },
    AccountReader, BlockBodyWriter, BlockExecutionWriter, BlockHashReader, BlockNumReader,
    BlockReader, BlockWriter, BundleStateInit, ChainStateBlockReader, ChainStateBlockWriter,
    DBProvider, EitherReader, EitherWriter, EitherWriterDestination, HashingWriter, HeaderProvider,
    HeaderSyncGapProvider, HistoricalStateProvider, HistoricalStateProviderRef, HistoryWriter,
    LatestStateProvider, LatestStateProviderRef, OriginalValuesKnown, ProviderError,
    PruneCheckpointReader, PruneCheckpointWriter, RawRocksDBBatch, RevertsInit, RocksBatchArg,
    RocksDBProviderFactory, RocksTxRefArg, StageCheckpointReader, StateProviderBox, StateWriter,
    StaticFileProviderFactory, StatsReader, StorageReader, StorageTrieWriter, TransactionVariant,
    TransactionsProvider, TransactionsProviderExt, TrieReader, TrieWriter,
};
use alloy_consensus::{
    transaction::{SignerRecoverable, TransactionMeta, TxHashRef},
    BlockHeader, TxReceipt,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map, HashMap, HashSet},
    Address, BlockHash, BlockNumber, TxHash, TxNumber, B256,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rayon::slice::ParallelSliceMut;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    models::{
        sharded_key, storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress,
        BlockNumberHashedAddress, ShardedKey, StorageBeforeTx, StorageSettings,
        StoredBlockBodyIndices,
    },
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList, PlainAccountState, PlainStorageState,
};
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_node_types::{BlockTy, BodyTy, HeaderTy, NodeTypes, ReceiptTy, TxTy};
use reth_primitives_traits::{
    Account, Block as _, BlockBody as _, Bytecode, RecoveredBlock, SealedHeader, StorageEntry,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneModes, PruneSegment, MINIMUM_PRUNING_DISTANCE,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, BlockBodyReader, MetadataProvider, MetadataWriter,
    NodePrimitivesProvider, StateProvider, StorageChangeSetReader, StorageSettingsCache,
    TryIntoHistoricalStateProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    trie_cursor::{
        InMemoryTrieCursor, InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory,
        TrieCursorIter,
    },
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
    HashedPostStateSorted, StoredNibbles, StoredNibblesSubKey, TrieChangeSetsEntry,
};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
use revm_database::states::{
    PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    ops::{Deref, DerefMut, Range, RangeBounds, RangeFrom, RangeInclusive},
    sync::Arc,
    time::{Duration, Instant},
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

    /// Override the minimum pruning distance for testing purposes.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn with_minimum_pruning_distance(mut self, distance: u64) -> Self {
        self.0.minimum_pruning_distance = distance;
        self
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
    /// Storage configuration settings for this node
    storage_settings: Arc<RwLock<StorageSettings>>,
    /// `RocksDB` provider
    rocksdb_provider: RocksDBProvider,
    /// Pending `RocksDB` batches to be committed at provider commit time.
    #[cfg(all(unix, feature = "rocksdb"))]
    pending_rocksdb_batches: parking_lot::Mutex<Vec<rocksdb::WriteBatchWithTransaction<true>>>,
    /// Minimum distance from tip required for pruning
    minimum_pruning_distance: u64,
    /// Database provider metrics
    metrics: metrics::DatabaseProviderMetrics,
}

impl<TX: Debug, N: NodeTypes> Debug for DatabaseProvider<TX, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("DatabaseProvider");
        s.field("tx", &self.tx)
            .field("chain_spec", &self.chain_spec)
            .field("static_file_provider", &self.static_file_provider)
            .field("prune_modes", &self.prune_modes)
            .field("storage", &self.storage)
            .field("storage_settings", &self.storage_settings)
            .field("rocksdb_provider", &self.rocksdb_provider);
        #[cfg(all(unix, feature = "rocksdb"))]
        s.field("pending_rocksdb_batches", &"<pending batches>");
        s.field("minimum_pruning_distance", &self.minimum_pruning_distance).finish()
    }
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

    fn get_static_file_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<crate::providers::StaticFileProviderRWRefMut<'_, Self::Primitives>> {
        self.static_file_provider.get_writer(block, segment)
    }
}

impl<TX, N: NodeTypes> RocksDBProviderFactory for DatabaseProvider<TX, N> {
    /// Returns the `RocksDB` provider.
    fn rocksdb_provider(&self) -> RocksDBProvider {
        self.rocksdb_provider.clone()
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    fn set_pending_rocksdb_batch(&self, batch: rocksdb::WriteBatchWithTransaction<true>) {
        self.pending_rocksdb_batches.lock().push(batch);
    }
}

impl<TX: Debug + Send, N: NodeTypes<ChainSpec: EthChainSpec + 'static>> ChainSpecProvider
    for DatabaseProvider<TX, N>
{
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<TX: DbTxMut, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Creates a provider with an inner read-write transaction.
    pub fn new_rw(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
    ) -> Self {
        Self {
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            #[cfg(all(unix, feature = "rocksdb"))]
            pending_rocksdb_batches: parking_lot::Mutex::new(Vec::new()),
            minimum_pruning_distance: MINIMUM_PRUNING_DISTANCE,
            metrics: metrics::DatabaseProviderMetrics::default(),
        }
    }
}

impl<TX, N: NodeTypes> AsRef<Self> for DatabaseProvider<TX, N> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<TX: DbTx + DbTxMut + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Executes a closure with a `RocksDB` batch, automatically registering it for commit.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` batch handling.
    pub fn with_rocksdb_batch<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksBatchArg<'_>) -> ProviderResult<(R, Option<RawRocksDBBatch>)>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb = self.rocksdb_provider();
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_batch = rocksdb.batch();
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let rocksdb_batch = ();

        let (result, raw_batch) = f(rocksdb_batch)?;

        #[cfg(all(unix, feature = "rocksdb"))]
        if let Some(batch) = raw_batch {
            self.set_pending_rocksdb_batch(batch);
        }
        let _ = raw_batch; // silence unused warning when rocksdb feature is disabled

        Ok(result)
    }

    /// Writes executed blocks and state to storage.
    pub fn save_blocks(&self, blocks: Vec<ExecutedBlock<N::Primitives>>) -> ProviderResult<()> {
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

        // Accumulate durations for each step
        let mut total_insert_block = Duration::ZERO;
        let mut total_write_state = Duration::ZERO;
        let mut total_write_hashed_state = Duration::ZERO;
        let mut total_write_trie_changesets = Duration::ZERO;
        let mut total_write_trie_updates = Duration::ZERO;

        // TODO: Do performant / batched writes for each type of object
        // instead of a loop over all blocks,
        // meaning:
        //  * blocks
        //  * state
        //  * hashed state
        //  * trie updates (cannot naively extend, need helper)
        //  * indices (already done basically)
        // Insert the blocks
        for block in blocks {
            let trie_data = block.trie_data();
            let ExecutedBlock { recovered_block, execution_output, .. } = block;
            let block_number = recovered_block.number();

            let start = Instant::now();
            self.insert_block(&recovered_block)?;
            total_insert_block += start.elapsed();

            // Write state and changesets to the database.
            // Must be written after blocks because of the receipt lookup.
            let start = Instant::now();
            self.write_state(&execution_output, OriginalValuesKnown::No)?;
            total_write_state += start.elapsed();

            // insert hashes and intermediate merkle nodes
            let start = Instant::now();
            self.write_hashed_state(&trie_data.hashed_state)?;
            total_write_hashed_state += start.elapsed();

            let start = Instant::now();
            self.write_trie_changesets(block_number, &trie_data.trie_updates, None)?;
            total_write_trie_changesets += start.elapsed();

            let start = Instant::now();
            self.write_trie_updates_sorted(&trie_data.trie_updates)?;
            total_write_trie_updates += start.elapsed();
        }

        // update history indices
        let start = Instant::now();
        self.update_history_indices(first_number..=last_block_number)?;
        let duration_update_history_indices = start.elapsed();

        // Update pipeline progress
        let start = Instant::now();
        self.update_pipeline_stages(last_block_number, false)?;
        let duration_update_pipeline_stages = start.elapsed();

        // Record all metrics at the end
        self.metrics.record_duration(metrics::Action::SaveBlocksInsertBlock, total_insert_block);
        self.metrics.record_duration(metrics::Action::SaveBlocksWriteState, total_write_state);
        self.metrics
            .record_duration(metrics::Action::SaveBlocksWriteHashedState, total_write_hashed_state);
        self.metrics.record_duration(
            metrics::Action::SaveBlocksWriteTrieChangesets,
            total_write_trie_changesets,
        );
        self.metrics
            .record_duration(metrics::Action::SaveBlocksWriteTrieUpdates, total_write_trie_updates);
        self.metrics.record_duration(
            metrics::Action::SaveBlocksUpdateHistoryIndices,
            duration_update_history_indices,
        );
        self.metrics.record_duration(
            metrics::Action::SaveBlocksUpdatePipelineStages,
            duration_update_pipeline_stages,
        );

        debug!(target: "providers::db", range = ?first_number..=last_block_number, "Appended block data");

        Ok(())
    }

    /// Unwinds trie state starting at and including the given block.
    ///
    /// This includes calculating the resulted state root and comparing it with the parent block
    /// state root.
    pub fn unwind_trie_state_from(&self, from: BlockNumber) -> ProviderResult<()> {
        let changed_accounts = self
            .tx
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(from..)?
            .collect::<Result<Vec<_>, _>>()?;

        // Unwind account hashes.
        self.unwind_account_hashing(changed_accounts.iter())?;

        // Unwind account history indices.
        self.unwind_account_history_indices(changed_accounts.iter())?;

        let storage_start = BlockNumberAddress((from, Address::ZERO));
        let changed_storages = self
            .tx
            .cursor_read::<tables::StorageChangeSets>()?
            .walk_range(storage_start..)?
            .collect::<Result<Vec<_>, _>>()?;

        // Unwind storage hashes.
        self.unwind_storage_hashing(changed_storages.iter().copied())?;

        // Unwind storage history indices.
        self.unwind_storage_history_indices(changed_storages.iter().copied())?;

        // Unwind accounts/storages trie tables using the revert.
        let trie_revert = self.trie_reverts(from)?;
        self.write_trie_updates_sorted(&trie_revert)?;

        // Clear trie changesets which have been unwound.
        self.clear_trie_changesets_from(from)?;

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

        if EitherWriter::receipts_destination(self).is_static_file() {
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
    pub fn new(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
    ) -> Self {
        Self {
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            #[cfg(all(unix, feature = "rocksdb"))]
            pending_rocksdb_batches: parking_lot::Mutex::new(Vec::new()),
            minimum_pruning_distance: MINIMUM_PRUNING_DISTANCE,
            metrics: metrics::DatabaseProviderMetrics::default(),
        }
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

    /// Executes a closure with a `RocksDB` transaction for reading.
    ///
    /// This helper encapsulates all the cfg-gated `RocksDB` transaction handling for reads.
    fn with_rocksdb_tx<F, R>(&self, f: F) -> ProviderResult<R>
    where
        F: FnOnce(RocksTxRefArg<'_>) -> ProviderResult<R>,
    {
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb = self.rocksdb_provider();
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_tx = rocksdb.tx();
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_tx_ref = &rocksdb_tx;
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let rocksdb_tx_ref = ();

        f(rocksdb_tx_ref)
    }
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
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

        let mut inputs = Vec::with_capacity(present_headers.len());
        for (tx_range, header) in &present_headers {
            let transactions = if tx_range.is_empty() {
                Vec::new()
            } else {
                self.transactions_by_tx_range(tx_range.clone())?
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
        self.block_range(range, headers_range, |header, body, tx_range| {
            let senders = if tx_range.is_empty() {
                Vec::new()
            } else {
                let known_senders: HashMap<TxNumber, Address> =
                    EitherReader::new_senders(self)?.senders_by_tx_range(tx_range.clone())?;

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
    /// Insert history index to the database.
    ///
    /// For each updated partial key, this function retrieves the last shard from the database
    /// (if any), appends the new indices to it, chunks the resulting list if needed, and upserts
    /// the shards back into the database.
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
        // This function cannot be used with DUPSORT tables because `upsert` on DUPSORT tables
        // will append duplicate entries instead of updating existing ones, causing data corruption.
        assert!(!T::DUPSORT, "append_history_index cannot be used with DUPSORT tables");

        let mut cursor = self.tx.cursor_write::<T>()?;

        for (partial_key, indices) in index_updates {
            let last_key = sharded_key_factory(partial_key, u64::MAX);
            let mut last_shard = cursor
                .seek_exact(last_key.clone())?
                .map(|(_, list)| list)
                .unwrap_or_else(BlockNumberList::empty);

            last_shard.append(indices).map_err(ProviderError::other)?;

            // fast path: all indices fit in one shard
            if last_shard.len() <= sharded_key::NUM_OF_INDICES_IN_SHARD as u64 {
                cursor.upsert(last_key, &last_shard)?;
                continue;
            }

            // slow path: rechunk into multiple shards
            let chunks = last_shard.iter().chunks(sharded_key::NUM_OF_INDICES_IN_SHARD);
            let mut chunks_peekable = chunks.into_iter().peekable();

            while let Some(chunk) = chunks_peekable.next() {
                let shard = BlockNumberList::new_pre_sorted(chunk);
                let highest_block_number = if chunks_peekable.peek().is_some() {
                    shard.iter().next_back().expect("`chunks` does not return empty list")
                } else {
                    // Insert last list with `u64::MAX`.
                    u64::MAX
                };

                cursor.upsert(sharded_key_factory(partial_key, highest_block_number), &shard)?;
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

impl<TX: DbTx + 'static, N: NodeTypes> AccountExtReader for DatabaseProvider<TX, N> {
    fn changed_accounts_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        let mut reader = EitherReader::new_account_changesets(self)?;

        reader.changed_accounts_with_range(range)
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
        let highest_static_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);

        if let Some(highest) = highest_static_block &&
            self.cached_storage_settings().account_changesets_in_static_files
        {
            let start = *range.start();
            let static_end = (*range.end()).min(highest + 1);

            let mut changed_accounts_and_blocks: BTreeMap<_, Vec<u64>> = BTreeMap::default();
            if start <= static_end {
                for block in start..=static_end {
                    let block_changesets = self.account_block_changeset(block)?;
                    for changeset in block_changesets {
                        changed_accounts_and_blocks
                            .entry(changeset.address)
                            .or_default()
                            .push(block);
                    }
                }
            }

            Ok(changed_accounts_and_blocks)
        } else {
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
}

impl<TX: DbTx, N: NodeTypes> StorageChangeSetReader for DatabaseProvider<TX, N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.static_file_provider.storage_changeset(block_number)
        } else {
            let range = block_number..=block_number;
            let storage_range = BlockNumberAddress::range(range);
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .walk_range(storage_range)?
                .map(|result| -> ProviderResult<_> { Ok(result?) })
                .collect()
        }
    }

    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>> {
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.static_file_provider.get_storage_before_block(block_number, address, storage_key)
        } else {
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .seek_by_key_subkey(BlockNumberAddress((block_number, address)), storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(Ok)
                .transpose()
        }
    }

    fn storage_changesets_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.static_file_provider.storage_changesets_range(range)
        } else {
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .walk_range(BlockNumberAddress::range(range))?
                .map(|result| -> ProviderResult<_> { Ok(result?) })
                .collect()
        }
    }

    fn storage_changeset_count(&self) -> ProviderResult<usize> {
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.static_file_provider.storage_changeset_count()
        } else {
            Ok(self.tx.entries::<tables::StorageChangeSets>()?)
        }
    }
}

impl<TX: DbTx, N: NodeTypes> ChangeSetReader for DatabaseProvider<TX, N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        if self.cached_storage_settings().account_changesets_in_static_files {
            let static_changesets =
                self.static_file_provider.account_block_changeset(block_number)?;
            Ok(static_changesets)
        } else {
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

    fn get_account_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>> {
        if self.cached_storage_settings().account_changesets_in_static_files {
            Ok(self.static_file_provider.get_account_before_block(block_number, address)?)
        } else {
            self.tx
                .cursor_dup_read::<tables::AccountChangeSets>()?
                .seek_by_key_subkey(block_number, address)?
                .filter(|acc| acc.address == address)
                .map(Ok)
                .transpose()
        }
    }

    fn account_changesets_range(
        &self,
        range: impl core::ops::RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx)>> {
        let range = to_range(range);
        let mut changesets = Vec::new();
        if self.cached_storage_settings().account_changesets_in_static_files &&
            let Some(highest) = self
                .static_file_provider
                .get_highest_static_file_block(StaticFileSegment::AccountChangeSets)
        {
            let static_end = range.end.min(highest + 1);
            if range.start < static_end {
                for block in range.start..static_end {
                    let block_changesets = self.account_block_changeset(block)?;
                    for changeset in block_changesets {
                        changesets.push((block, changeset));
                    }
                }
            }
        } else {
            // Fetch from database for blocks not in static files
            let mut cursor = self.tx.cursor_read::<tables::AccountChangeSets>()?;
            for entry in cursor.walk_range(range)? {
                let (block_num, account_before) = entry?;
                changesets.push((block_num, account_before));
            }
        }

        Ok(changesets)
    }

    fn account_changeset_count(&self) -> ProviderResult<usize> {
        // check if account changesets are in static files, otherwise just count the changeset
        // entries in the DB
        if self.cached_storage_settings().account_changesets_in_static_files {
            self.static_file_provider.account_changeset_count()
        } else {
            Ok(self.tx.entries::<tables::AccountChangeSets>()?)
        }
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

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        if let Some(num) = self.block_number(block_hash)? {
            Ok(self.header_by_number(num)?)
        } else {
            Ok(None)
        }
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.static_file_provider.header_by_number(num)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.static_file_provider.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_header(number)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.static_file_provider.sealed_headers_while(range, predicate)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> BlockHashReader for DatabaseProvider<TX, N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.canonical_hashes_range(start, end)
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
        self.static_file_provider.last_block_number()
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

    fn block_by_transaction_id(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        Ok(self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .seek(id)
            .map(|b| b.map(|(_, bn)| bn))?)
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
        self.static_file_provider.transaction_hashes_by_range(tx_range)
    }
}

// Calculates the hash of the given transaction
impl<TX: DbTx + 'static, N: NodeTypesForProvider> TransactionsProvider for DatabaseProvider<TX, N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.with_rocksdb_tx(|tx_ref| {
            let mut reader = EitherReader::new_transaction_hash_numbers(self, tx_ref)?;
            reader.get_transaction_hash_number(tx_hash)
        })
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id(id)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.static_file_provider.transaction_by_id_unhashed(id)
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
        if let Some(transaction_id) = self.transaction_id(tx_hash)? &&
            let Some(transaction) = self.transaction_by_id_unhashed(transaction_id)? &&
            let Some(block_number) = self.block_by_transaction_id(transaction_id)? &&
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

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        if let Some(block_number) = self.convert_hash_or_number(id)? &&
            let Some(body) = self.block_body_indices(block_number)?
        {
            let tx_range = body.tx_num_range();
            return if tx_range.is_empty() {
                Ok(Some(Vec::new()))
            } else {
                self.transactions_by_tx_range(tx_range).map(Some)
            }
        }
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        let range = to_range(range);

        self.block_body_indices_range(range.start..=range.end.saturating_sub(1))?
            .into_iter()
            .map(|body| {
                let tx_num_range = body.tx_num_range();
                if tx_num_range.is_empty() {
                    Ok(Vec::new())
                } else {
                    self.transactions_by_tx_range(tx_num_range)
                }
            })
            .collect()
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.static_file_provider.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.senders_by_tx_range(range)
        } else {
            self.cursor_read_collect::<tables::TransactionSenders>(range)
        }
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.static_file_provider.transaction_sender(id)
        } else {
            Ok(self.tx.get::<tables::TransactionSenders>(id)?)
        }
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
        let range_len = block_range.end().saturating_sub(*block_range.start()) as usize + 1;
        let mut block_body_indices = Vec::with_capacity(range_len);
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
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.storage_changesets_range(range)?.into_iter().try_fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                    let (BlockNumberAddress((_, address)), storage_entry) = entry;
                    accounts.entry(address).or_default().insert(storage_entry.key);
                    Ok(accounts)
                },
            )
        } else {
            self.tx
                .cursor_read::<tables::StorageChangeSets>()?
                .walk_range(BlockNumberAddress::range(range))?
                // fold all storages and save its old state so we can remove it from HashedStorage
                // it is needed as it is dup table.
                .try_fold(
                    BTreeMap::new(),
                    |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                        let (BlockNumberAddress((_, address)), storage_entry) = entry?;
                        accounts.entry(address).or_default().insert(storage_entry.key);
                        Ok(accounts)
                    },
                )
        }
    }

    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>> {
        if self.cached_storage_settings().storage_changesets_in_static_files {
            self.storage_changesets_range(range)?.into_iter().try_fold(
                BTreeMap::new(),
                |mut storages: BTreeMap<(Address, B256), Vec<u64>>, (index, storage)| {
                    storages
                        .entry((index.address(), storage.key))
                        .or_default()
                        .push(index.block_number());
                    Ok(storages)
                },
            )
        } else {
            let mut changeset_cursor = self.tx.cursor_read::<tables::StorageChangeSets>()?;

            let storage_changeset_lists =
                changeset_cursor.walk_range(BlockNumberAddress::range(range))?.try_fold(
                    BTreeMap::new(),
                    |mut storages: BTreeMap<(Address, B256), Vec<u64>>,
                     entry|
                     -> ProviderResult<_> {
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

        let mut receipts_writer = EitherWriter::new_receipts(self, first_block)?;

        let has_contract_log_filter = !self.prune_modes.receipts_log_filter.is_empty();
        let contract_log_pruner = self.prune_modes.receipts_log_filter.group_by_block(tip, None)?;

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PruneSegment::ContractLogs`].
        //
        // Receipts can only be skipped if we're dealing with legacy nodes that write them to
        // Database, OR if receipts_in_static_files is enabled but no receipts exist in static
        // files yet. Once receipts exist in static files, we must continue writing to maintain
        // continuity and have no gaps.
        let prunable_receipts = (EitherWriter::receipts_destination(self).is_database() ||
            self.static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .is_none()) &&
            PruneMode::Distance(self.minimum_pruning_distance).should_prune(first_block, tip);

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
            receipts_writer.increment_block(block_number)?;

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

                receipts_writer.append_receipt(receipt_idx, receipt)?;
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
        for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;

            tracing::trace!(block_number, "Writing block change");
            // sort changes by address.
            storage_changes.par_sort_unstable_by_key(|a| a.address);
            let mut changeset = Vec::new();
            for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
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
                    changeset.push(StorageBeforeTx { address, key, value });
                }
            }

            let mut storage_changesets_writer =
                EitherWriter::new_storage_changesets(self, block_number)?;
            storage_changesets_writer.append_storage_changeset(block_number, changeset)?;
        }

        // Write account changes to static files
        tracing::debug!(target: "sync::stages::merkle_changesets", ?first_block, "Writing account changes");
        for (block_index, account_block_reverts) in reverts.accounts.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;
            let changeset = account_block_reverts
                .into_iter()
                .map(|(address, info)| AccountBeforeTx { address, info: info.map(Into::into) })
                .collect::<Vec<_>>();
            let mut account_changesets_writer =
                EitherWriter::new_account_changesets(self, block_number)?;

            account_changesets_writer.append_account_changeset(block_number, changeset)?;
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
        for (hashed_address, account) in hashed_state.accounts() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
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

            for (hashed_slot, value) in storage.storage_slots_ref() {
                let entry = StorageEntry { key: *hashed_slot, value: *value };

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
        let storage_changeset = if let Some(_highest_block) = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::StorageChangeSets) &&
            self.cached_storage_settings().storage_changesets_in_static_files
        {
            let changesets = self.storage_changesets_range(range.clone())?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            self.take::<tables::StorageChangeSets>(storage_range)?
        };
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
        let storage_changeset = if let Some(highest_block) = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::StorageChangeSets) &&
            self.cached_storage_settings().storage_changesets_in_static_files
        {
            let changesets = self.storage_changesets_range(block + 1..=highest_block)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            self.take::<tables::StorageChangeSets>(storage_range)?
        };

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.
        let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

        // if there are static files for this segment, prune them.
        let highest_changeset_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
        let account_changeset = if let Some(highest_block) = highest_changeset_block &&
            self.cached_storage_settings().account_changesets_in_static_files
        {
            // TODO: add a `take` method that removes and returns the items instead of doing this
            let changesets = self.account_changesets_range(block + 1..highest_block + 1)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
            changeset_writer.prune_account_changesets(block)?;

            changesets
        } else {
            // Have to remove from static files if they exist, otherwise remove using `take` for the
            // changeset tables
            self.take::<tables::AccountChangeSets>(range)?
        };

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
    /// Writes trie updates to the database with already sorted updates.
    ///
    /// Returns the number of entries modified.
    fn write_trie_updates_sorted(&self, trie_updates: &TrieUpdatesSorted) -> ProviderResult<usize> {
        if trie_updates.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        let tx = self.tx_ref();
        let mut account_trie_cursor = tx.cursor_write::<tables::AccountsTrie>()?;

        // Process sorted account nodes
        for (key, updated_node) in trie_updates.account_nodes_ref() {
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

        num_entries +=
            self.write_storage_trie_updates_sorted(trie_updates.storage_tries_ref().iter())?;

        Ok(num_entries)
    }

    /// Records the current values of all trie nodes which will be updated using the `TrieUpdates`
    /// into the trie changesets tables.
    ///
    /// The intended usage of this method is to call it _prior_ to calling `write_trie_updates` with
    /// the same `TrieUpdates`.
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

        // Create empty TrieUpdatesSorted for when updates_overlay is None
        let empty_updates = TrieUpdatesSorted::default();
        let overlay = updates_overlay.unwrap_or(&empty_updates);

        // Wrap the cursor in InMemoryTrieCursor with the overlay
        let mut in_memory_account_cursor =
            InMemoryTrieCursor::new_account(&mut db_account_cursor, overlay);

        for (path, _) in trie_updates.account_nodes_ref() {
            num_entries += 1;
            let node = in_memory_account_cursor.seek_exact(*path)?.map(|(_, node)| node);
            changeset_cursor.append_dup(
                block_number,
                TrieChangeSetsEntry { nibbles: StoredNibblesSubKey(*path), node },
            )?;
        }

        let mut storage_updates = trie_updates.storage_tries_ref().iter().collect::<Vec<_>>();
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

impl<TX: DbTx + 'static, N: NodeTypes> TrieReader for DatabaseProvider<TX, N> {
    fn trie_reverts(&self, from: BlockNumber) -> ProviderResult<TrieUpdatesSorted> {
        let tx = self.tx_ref();

        // Read account trie changes directly into a Vec - data is already sorted by nibbles
        // within each block, and we want the oldest (first) version of each node sorted by path.
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

        account_nodes.sort_by_key(|(path, _)| *path);

        // Read storage trie changes - data is sorted by (block, hashed_address, nibbles)
        // Keep track of seen (address, nibbles) pairs to only keep the oldest version per address,
        // sorted by path.
        let mut storage_tries = B256Map::<Vec<_>>::default();
        let mut seen_storage_keys = HashSet::new();
        let mut storages_cursor = tx.cursor_dup_read::<tables::StoragesTrieChangeSets>()?;

        // Create storage range starting from `from` block
        let storage_range_start = BlockNumberHashedAddress((from, B256::ZERO));

        for entry in storages_cursor.walk_range(storage_range_start..)? {
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
            .map(|(address, mut nodes)| {
                nodes.sort_by_key(|(path, _)| *path);
                (address, StorageTrieUpdatesSorted { storage_nodes: nodes, is_deleted: false })
            })
            .collect();

        Ok(TrieUpdatesSorted::new(account_nodes, storage_tries))
    }

    fn get_block_trie_updates(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<TrieUpdatesSorted> {
        let tx = self.tx_ref();

        // Step 1: Get the trie reverts for the state after the target block
        let reverts = self.trie_reverts(block_number + 1)?;

        // Step 2: Create an InMemoryTrieCursorFactory with the reverts
        // This gives us the trie state as it was after the target block was processed
        let db_cursor_factory = DatabaseTrieCursorFactory::new(tx);
        let cursor_factory = InMemoryTrieCursorFactory::new(db_cursor_factory, &reverts);

        // Step 3: Collect all account trie nodes that changed in the target block
        let mut account_nodes = Vec::new();

        // Walk through all account trie changes for this block
        let mut accounts_trie_cursor = tx.cursor_dup_read::<tables::AccountsTrieChangeSets>()?;
        let mut account_cursor = cursor_factory.account_trie_cursor()?;

        for entry in accounts_trie_cursor.walk_dup(Some(block_number), None)? {
            let (_, TrieChangeSetsEntry { nibbles, .. }) = entry?;
            // Look up the current value of this trie node using the overlay cursor
            let node_value = account_cursor.seek_exact(nibbles.0)?.map(|(_, node)| node);
            account_nodes.push((nibbles.0, node_value));
        }

        // Step 4: Collect all storage trie nodes that changed in the target block
        let mut storage_tries = B256Map::default();
        let mut storages_trie_cursor = tx.cursor_dup_read::<tables::StoragesTrieChangeSets>()?;
        let storage_range_start = BlockNumberHashedAddress((block_number, B256::ZERO));
        let storage_range_end = BlockNumberHashedAddress((block_number + 1, B256::ZERO));

        let mut current_hashed_address = None;
        let mut storage_cursor = None;

        for entry in storages_trie_cursor.walk_range(storage_range_start..storage_range_end)? {
            let (
                BlockNumberHashedAddress((_, hashed_address)),
                TrieChangeSetsEntry { nibbles, .. },
            ) = entry?;

            // Check if we need to create a new storage cursor for a different account
            if current_hashed_address != Some(hashed_address) {
                storage_cursor = Some(cursor_factory.storage_trie_cursor(hashed_address)?);
                current_hashed_address = Some(hashed_address);
            }

            // Look up the current value of this storage trie node
            let cursor =
                storage_cursor.as_mut().expect("storage_cursor was just initialized above");
            let node_value = cursor.seek_exact(nibbles.0)?.map(|(_, node)| node);
            storage_tries
                .entry(hashed_address)
                .or_insert_with(|| StorageTrieUpdatesSorted {
                    storage_nodes: Vec::new(),
                    is_deleted: false,
                })
                .storage_nodes
                .push((nibbles.0, node_value));
        }

        Ok(TrieUpdatesSorted::new(account_nodes, storage_tries))
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> StorageTrieWriter for DatabaseProvider<TX, N> {
    /// Writes storage trie updates from the given storage trie map with already sorted updates.
    ///
    /// Expects the storage trie updates to already be sorted by the hashed address key.
    ///
    /// Returns the number of entries modified.
    fn write_storage_trie_updates_sorted<'a>(
        &self,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdatesSorted)>,
    ) -> ProviderResult<usize> {
        let mut num_entries = 0;
        let mut storage_tries = storage_tries.collect::<Vec<_>>();
        storage_tries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut cursor = self.tx_ref().cursor_dup_write::<tables::StoragesTrie>()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let mut db_storage_trie_cursor =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates_sorted(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }

        Ok(num_entries)
    }

    /// Records the current values of all trie nodes which will be updated using the
    /// `StorageTrieUpdates` into the storage trie changesets table.
    ///
    /// The intended usage of this method is to call it _prior_ to calling
    /// `write_storage_trie_updates` with the same set of `StorageTrieUpdates`.
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

        // Create empty TrieUpdatesSorted for when updates_overlay is None
        let empty_updates = TrieUpdatesSorted::default();

        for (hashed_address, storage_trie_updates) in storage_tries {
            let changeset_key = BlockNumberHashedAddress((block_number, *hashed_address));

            // Update the hashed address for the cursors
            changed_curr_values_cursor =
                DatabaseStorageTrieCursor::new(changed_curr_values_cursor.cursor, *hashed_address);

            // Get the overlay updates, or use empty updates
            let overlay = updates_overlay.unwrap_or(&empty_updates);

            // Wrap the cursor in InMemoryTrieCursor with the overlay
            let mut in_memory_changed_cursor = InMemoryTrieCursor::new_storage(
                &mut changed_curr_values_cursor,
                overlay,
                *hashed_address,
            );

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
                let mut in_memory_wiped_cursor = InMemoryTrieCursor::new_storage(
                    &mut wiped_nodes_cursor,
                    overlay,
                    *hashed_address,
                );

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

        self.unwind_trie_state_from(block + 1)?;

        // get execution res
        let execution_state = self.take_state_above(block)?;

        let blocks = self.recovered_block_range(range)?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove_blocks_above(block)?;

        // Update pipeline progress
        self.update_pipeline_stages(block, true)?;

        Ok(Chain::new(blocks, execution_state, BTreeMap::new(), BTreeMap::new()))
    }

    fn remove_block_and_execution_above(&self, block: BlockNumber) -> ProviderResult<()> {
        self.unwind_trie_state_from(block + 1)?;

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

    /// Inserts the block into the database, always modifying the following static file segments and
    /// tables:
    /// * [`StaticFileSegment::Headers`]
    /// * [`tables::HeaderNumbers`]
    /// * [`tables::BlockBodyIndices`]
    ///
    /// If there are transactions in the block, the following static file segments and tables will
    /// be modified:
    /// * [`StaticFileSegment::Transactions`]
    /// * [`tables::TransactionBlocks`]
    ///
    /// If ommers are not empty, this will modify [`BlockOmmers`](tables::BlockOmmers).
    /// If withdrawals are not empty, this will modify
    /// [`BlockWithdrawals`](tables::BlockWithdrawals).
    ///
    /// If the provider has __not__ configured full sender pruning, this will modify either:
    /// * [`StaticFileSegment::TransactionSenders`] if senders are written to static files
    /// * [`tables::TransactionSenders`] if senders are written to the database
    ///
    /// If the provider has __not__ configured full transaction lookup pruning, this will modify
    /// [`TransactionHashNumbers`](tables::TransactionHashNumbers).
    fn insert_block(
        &self,
        block: &RecoveredBlock<Self::Block>,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        let block_number = block.number();
        let tx_count = block.body().transaction_count() as u64;

        let mut durations_recorder = metrics::DurationsRecorder::new(&self.metrics);

        self.static_file_provider
            .get_writer(block_number, StaticFileSegment::Headers)?
            .append_header(block.header(), &block.hash())?;

        self.tx.put::<tables::HeaderNumbers>(block.hash(), block_number)?;
        durations_recorder.record_relative(metrics::Action::InsertHeaderNumbers);

        let first_tx_num = self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .last()?
            .map(|(n, _)| n + 1)
            .unwrap_or_default();
        durations_recorder.record_relative(metrics::Action::GetNextTxNum);

        let tx_nums_iter = std::iter::successors(Some(first_tx_num), |n| Some(n + 1));

        if self.prune_modes.sender_recovery.as_ref().is_none_or(|m| !m.is_full()) {
            let mut senders_writer = EitherWriter::new_senders(self, block.number())?;
            senders_writer.increment_block(block.number())?;
            senders_writer
                .append_senders(tx_nums_iter.clone().zip(block.senders_iter().copied()))?;
            durations_recorder.record_relative(metrics::Action::InsertTransactionSenders);
        }

        if self.prune_modes.transaction_lookup.is_none_or(|m| !m.is_full()) {
            self.with_rocksdb_batch(|batch| {
                let mut writer = EitherWriter::new_transaction_hash_numbers(self, batch)?;
                for (tx_num, transaction) in tx_nums_iter.zip(block.body().transactions_iter()) {
                    let hash = transaction.tx_hash();
                    writer.put_transaction_hash_number(*hash, tx_num, false)?;
                }
                Ok(((), writer.into_raw_rocksdb_batch()))
            })?;
            durations_recorder.record_relative(metrics::Action::InsertTransactionHashNumbers);
        }

        self.append_block_bodies(vec![(block_number, Some(block.body()))])?;

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
        bodies: Vec<(BlockNumber, Option<&BodyTy<N>>)>,
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

            let mut durations_recorder = metrics::DurationsRecorder::new(&self.metrics);

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
        let last_block_number = self.last_block_number()?;
        // Clean up HeaderNumbers for blocks being removed, we must clear all indexes from MDBX.
        for hash in self.canonical_hashes_range(block + 1, last_block_number + 1)? {
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
            let hashes = self.transaction_hashes_by_range(unwind_tx_from..(unwind_tx_to + 1))?;
            self.with_rocksdb_batch(|batch| {
                let mut writer = EitherWriter::new_transaction_hash_numbers(self, batch)?;
                for (hash, _) in hashes {
                    writer.delete_transaction_hash_number(hash)?;
                }
                Ok(((), writer.into_raw_rocksdb_batch()))
            })?;
        }

        EitherWriter::new_senders(self, last_block_number)?.prune_senders(unwind_tx_from, block)?;

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

        let mut durations_recorder = metrics::DurationsRecorder::new(&self.metrics);

        // Extract account and storage transitions from the bundle reverts BEFORE writing state.
        // This is necessary because with edge storage, changesets are written to static files
        // whose index isn't updated until commit, making them invisible to subsequent reads
        // within the same transaction.
        let (account_transitions, storage_transitions) = {
            let mut account_transitions: BTreeMap<Address, Vec<u64>> = BTreeMap::new();
            let mut storage_transitions: BTreeMap<(Address, B256), Vec<u64>> = BTreeMap::new();
            for (block_idx, block_reverts) in execution_outcome.bundle.reverts.iter().enumerate() {
                let block_number = first_number + block_idx as u64;
                for (address, account_revert) in block_reverts {
                    account_transitions.entry(*address).or_default().push(block_number);
                    for storage_key in account_revert.storage.keys() {
                        let key = B256::new(storage_key.to_be_bytes());
                        storage_transitions.entry((*address, key)).or_default().push(block_number);
                    }
                }
            }
            (account_transitions, storage_transitions)
        };

        // Insert the blocks
        for block in blocks {
            self.insert_block(&block)?;
            durations_recorder.record_relative(metrics::Action::InsertBlock);
        }

        self.write_state(execution_outcome, OriginalValuesKnown::No)?;
        durations_recorder.record_relative(metrics::Action::InsertState);

        // insert hashes and intermediate merkle nodes
        self.write_hashed_state(&hashed_state)?;
        durations_recorder.record_relative(metrics::Action::InsertHashes);

        // Use pre-computed transitions for history indices since static file
        // writes aren't visible until commit.
        self.insert_account_history_index(account_transitions)?;
        self.insert_storage_history_index(storage_transitions)?;
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
        Ok(PruneSegment::variants()
            .filter_map(|segment| {
                self.tx
                    .get::<tables::PruneCheckpoints>(segment)
                    .transpose()
                    .map(|chk| chk.map(|chk| (segment, chk)))
            })
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
            .walk(Some(tables::ChainStateKey::LastSafeBlock))?
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
        Ok(self.tx.put::<tables::ChainState>(tables::ChainStateKey::LastSafeBlock, block_number)?)
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

    /// Commit database transaction, static files, and pending `RocksDB` batches.
    fn commit(self) -> ProviderResult<bool> {
        // For unwinding it makes more sense to commit the database first, since if
        // it is interrupted before the static files commit, we can just
        // truncate the static files according to the
        // checkpoints on the next start-up.
        if self.static_file_provider.has_unwind_queued() {
            self.tx.commit()?;

            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
                for batch in batches {
                    self.rocksdb_provider.commit_batch(batch)?;
                }
            }

            self.static_file_provider.commit()?;
        } else {
            self.static_file_provider.commit()?;

            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
                for batch in batches {
                    self.rocksdb_provider.commit_batch(batch)?;
                }
            }

            self.tx.commit()?;
        }

        Ok(true)
    }
}

impl<TX: DbTx, N: NodeTypes> MetadataProvider for DatabaseProvider<TX, N> {
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>> {
        self.tx.get::<tables::Metadata>(key.to_string()).map_err(Into::into)
    }
}

impl<TX: DbTxMut, N: NodeTypes> MetadataWriter for DatabaseProvider<TX, N> {
    fn write_metadata(&self, key: &str, value: Vec<u8>) -> ProviderResult<()> {
        self.tx.put::<tables::Metadata>(key.to_string(), value).map_err(Into::into)
    }
}

impl<TX: Send, N: NodeTypes> StorageSettingsCache for DatabaseProvider<TX, N> {
    fn cached_storage_settings(&self) -> StorageSettings {
        *self.storage_settings.read()
    }

    fn set_storage_settings_cache(&self, settings: StorageSettings) {
        *self.storage_settings.write() = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{blocks::BlockchainTestData, create_test_provider_factory},
        BlockWriter,
    };
    use reth_ethereum_primitives::Receipt;
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use reth_trie::Nibbles;

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
        provider_rw.insert_block(&data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        provider_rw.insert_block(&data.blocks[0].0).unwrap();
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
        provider_rw.insert_block(&data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
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
        provider_rw.insert_block(&data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();

        // insert blocks 1-3 with receipts
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
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
        provider_rw.insert_block(&data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
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
            provider_rw.insert_block(&block.try_recover().unwrap()).unwrap();
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
        provider_rw.insert_block(&data.genesis.clone().try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
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

        let trie_updates = TrieUpdatesSorted::new(account_nodes, storage_tries);

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

        let overlay = TrieUpdatesSorted::new(overlay_account_nodes, overlay_storage_tries);

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

        let trie_updates = TrieUpdatesSorted::new(account_nodes, storage_tries);

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

    #[test]
    fn test_write_trie_updates_sorted() {
        use reth_trie::{
            updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
            BranchNodeCompact, StorageTrieEntry,
        };

        let factory = create_test_provider_factory();
        let provider_rw = factory.provider_rw().unwrap();

        // Pre-populate account trie with data that will be deleted
        {
            let tx = provider_rw.tx_ref();
            let mut cursor = tx.cursor_write::<tables::AccountsTrie>().unwrap();

            // Add account node that will be deleted
            let to_delete = StoredNibbles(Nibbles::from_nibbles([0x3, 0x4]));
            cursor
                .upsert(
                    to_delete,
                    &BranchNodeCompact::new(
                        0b1010_1010_1010_1010, // state_mask
                        0b0000_0000_0000_0000, // tree_mask
                        0b0000_0000_0000_0000, // hash_mask
                        vec![],
                        None,
                    ),
                )
                .unwrap();

            // Add account node that will be updated
            let to_update = StoredNibbles(Nibbles::from_nibbles([0x1, 0x2]));
            cursor
                .upsert(
                    to_update,
                    &BranchNodeCompact::new(
                        0b0101_0101_0101_0101, // old state_mask (will be updated)
                        0b0000_0000_0000_0000, // tree_mask
                        0b0000_0000_0000_0000, // hash_mask
                        vec![],
                        None,
                    ),
                )
                .unwrap();
        }

        // Pre-populate storage tries with data
        let storage_address1 = B256::from([1u8; 32]);
        let storage_address2 = B256::from([2u8; 32]);
        {
            let tx = provider_rw.tx_ref();
            let mut storage_cursor = tx.cursor_dup_write::<tables::StoragesTrie>().unwrap();

            // Add storage nodes for address1 (one will be deleted)
            storage_cursor
                .upsert(
                    storage_address1,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(Nibbles::from_nibbles([0x2, 0x0])),
                        node: BranchNodeCompact::new(
                            0b0011_0011_0011_0011, // will be deleted
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        ),
                    },
                )
                .unwrap();

            // Add storage nodes for address2 (will be wiped)
            storage_cursor
                .upsert(
                    storage_address2,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(Nibbles::from_nibbles([0xa, 0xb])),
                        node: BranchNodeCompact::new(
                            0b1100_1100_1100_1100, // will be wiped
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        ),
                    },
                )
                .unwrap();
            storage_cursor
                .upsert(
                    storage_address2,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(Nibbles::from_nibbles([0xc, 0xd])),
                        node: BranchNodeCompact::new(
                            0b0011_1100_0011_1100, // will be wiped
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        ),
                    },
                )
                .unwrap();
        }

        // Create sorted account trie updates
        let account_nodes = vec![
            (
                Nibbles::from_nibbles([0x1, 0x2]),
                Some(BranchNodeCompact::new(
                    0b1111_1111_1111_1111, // state_mask (updated)
                    0b0000_0000_0000_0000, // tree_mask
                    0b0000_0000_0000_0000, // hash_mask (no hashes)
                    vec![],
                    None,
                )),
            ),
            (Nibbles::from_nibbles([0x3, 0x4]), None), // Deletion
            (
                Nibbles::from_nibbles([0x5, 0x6]),
                Some(BranchNodeCompact::new(
                    0b1111_1111_1111_1111, // state_mask
                    0b0000_0000_0000_0000, // tree_mask
                    0b0000_0000_0000_0000, // hash_mask (no hashes)
                    vec![],
                    None,
                )),
            ),
        ];

        // Create sorted storage trie updates
        let storage_trie1 = StorageTrieUpdatesSorted {
            is_deleted: false,
            storage_nodes: vec![
                (
                    Nibbles::from_nibbles([0x1, 0x0]),
                    Some(BranchNodeCompact::new(
                        0b1111_0000_0000_0000, // state_mask
                        0b0000_0000_0000_0000, // tree_mask
                        0b0000_0000_0000_0000, // hash_mask (no hashes)
                        vec![],
                        None,
                    )),
                ),
                (Nibbles::from_nibbles([0x2, 0x0]), None), // Deletion of existing node
            ],
        };

        let storage_trie2 = StorageTrieUpdatesSorted {
            is_deleted: true, // Wipe all storage for this address
            storage_nodes: vec![],
        };

        let mut storage_tries = B256Map::default();
        storage_tries.insert(storage_address1, storage_trie1);
        storage_tries.insert(storage_address2, storage_trie2);

        let trie_updates = TrieUpdatesSorted::new(account_nodes, storage_tries);

        // Write the sorted trie updates
        let num_entries = provider_rw.write_trie_updates_sorted(&trie_updates).unwrap();

        // We should have 2 account insertions + 1 account deletion + 1 storage insertion + 1
        // storage deletion = 5
        assert_eq!(num_entries, 5);

        // Verify account trie updates were written correctly
        let tx = provider_rw.tx_ref();
        let mut cursor = tx.cursor_read::<tables::AccountsTrie>().unwrap();

        // Check first account node was updated
        let nibbles1 = StoredNibbles(Nibbles::from_nibbles([0x1, 0x2]));
        let entry1 = cursor.seek_exact(nibbles1).unwrap();
        assert!(entry1.is_some(), "Updated account node should exist");
        let expected_mask = reth_trie::TrieMask::new(0b1111_1111_1111_1111);
        assert_eq!(
            entry1.unwrap().1.state_mask,
            expected_mask,
            "Account node should have updated state_mask"
        );

        // Check deleted account node no longer exists
        let nibbles2 = StoredNibbles(Nibbles::from_nibbles([0x3, 0x4]));
        let entry2 = cursor.seek_exact(nibbles2).unwrap();
        assert!(entry2.is_none(), "Deleted account node should not exist");

        // Check new account node exists
        let nibbles3 = StoredNibbles(Nibbles::from_nibbles([0x5, 0x6]));
        let entry3 = cursor.seek_exact(nibbles3).unwrap();
        assert!(entry3.is_some(), "New account node should exist");

        // Verify storage trie updates were written correctly
        let mut storage_cursor = tx.cursor_dup_read::<tables::StoragesTrie>().unwrap();

        // Check storage for address1
        let storage_entries1: Vec<_> = storage_cursor
            .walk_dup(Some(storage_address1), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            storage_entries1.len(),
            1,
            "Storage address1 should have 1 entry after deletion"
        );
        assert_eq!(
            storage_entries1[0].1.nibbles.0,
            Nibbles::from_nibbles([0x1, 0x0]),
            "Remaining entry should be [0x1, 0x0]"
        );

        // Check storage for address2 was wiped
        let storage_entries2: Vec<_> = storage_cursor
            .walk_dup(Some(storage_address2), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(storage_entries2.len(), 0, "Storage address2 should be empty after wipe");

        provider_rw.commit().unwrap();
    }

    #[test]
    fn test_get_block_trie_updates() {
        use reth_db_api::models::BlockNumberHashedAddress;
        use reth_trie::{BranchNodeCompact, StorageTrieEntry};

        let factory = create_test_provider_factory();
        let provider_rw = factory.provider_rw().unwrap();

        let target_block = 2u64;
        let next_block = 3u64;

        // Create test nibbles and nodes for accounts
        let account_nibbles1 = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
        let account_nibbles2 = Nibbles::from_nibbles([0x5, 0x6, 0x7, 0x8]);
        let account_nibbles3 = Nibbles::from_nibbles([0x9, 0xa, 0xb, 0xc]);

        let node1 = BranchNodeCompact::new(
            0b1111_1111_0000_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        let node2 = BranchNodeCompact::new(
            0b0000_0000_1111_1111,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        let node3 = BranchNodeCompact::new(
            0b1010_1010_1010_1010,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Pre-populate AccountsTrie with nodes that will be the final state
        {
            let mut cursor = provider_rw.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();
            cursor.insert(StoredNibbles(account_nibbles1), &node1).unwrap();
            cursor.insert(StoredNibbles(account_nibbles2), &node2).unwrap();
            // account_nibbles3 will be deleted (not in final state)
        }

        // Insert trie changesets for target_block
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::AccountsTrieChangeSets>().unwrap();
            // nibbles1 was updated in target_block (old value stored)
            cursor
                .append_dup(
                    target_block,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(account_nibbles1),
                        node: Some(BranchNodeCompact::new(
                            0b1111_0000_0000_0000, // old value
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        )),
                    },
                )
                .unwrap();
            // nibbles2 was created in target_block (no old value)
            cursor
                .append_dup(
                    target_block,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(account_nibbles2),
                        node: None,
                    },
                )
                .unwrap();
        }

        // Insert trie changesets for next_block (to test overlay)
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::AccountsTrieChangeSets>().unwrap();
            // nibbles3 was deleted in next_block (old value stored)
            cursor
                .append_dup(
                    next_block,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(account_nibbles3),
                        node: Some(node3),
                    },
                )
                .unwrap();
        }

        // Storage trie updates
        let storage_address1 = B256::from([1u8; 32]);
        let storage_nibbles1 = Nibbles::from_nibbles([0xa, 0xb]);
        let storage_nibbles2 = Nibbles::from_nibbles([0xc, 0xd]);

        let storage_node1 = BranchNodeCompact::new(
            0b1111_1111_1111_0000,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        let storage_node2 = BranchNodeCompact::new(
            0b0101_0101_0101_0101,
            0b0000_0000_0000_0000,
            0b0000_0000_0000_0000,
            vec![],
            None,
        );

        // Pre-populate StoragesTrie with final state
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
            cursor
                .upsert(
                    storage_address1,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(storage_nibbles1),
                        node: storage_node1.clone(),
                    },
                )
                .unwrap();
            // storage_nibbles2 was deleted in next_block, so it's not in final state
        }

        // Insert storage trie changesets for target_block
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrieChangeSets>().unwrap();
            let key = BlockNumberHashedAddress((target_block, storage_address1));

            // storage_nibbles1 was updated
            cursor
                .append_dup(
                    key,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(storage_nibbles1),
                        node: Some(BranchNodeCompact::new(
                            0b0000_0000_1111_1111, // old value
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        )),
                    },
                )
                .unwrap();

            // storage_nibbles2 was created
            cursor
                .append_dup(
                    key,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(storage_nibbles2),
                        node: None,
                    },
                )
                .unwrap();
        }

        // Insert storage trie changesets for next_block (to test overlay)
        {
            let mut cursor =
                provider_rw.tx_ref().cursor_dup_write::<tables::StoragesTrieChangeSets>().unwrap();
            let key = BlockNumberHashedAddress((next_block, storage_address1));

            // storage_nibbles2 was deleted in next_block
            cursor
                .append_dup(
                    key,
                    TrieChangeSetsEntry {
                        nibbles: StoredNibblesSubKey(storage_nibbles2),
                        node: Some(BranchNodeCompact::new(
                            0b0101_0101_0101_0101, // value that was deleted
                            0b0000_0000_0000_0000,
                            0b0000_0000_0000_0000,
                            vec![],
                            None,
                        )),
                    },
                )
                .unwrap();
        }

        provider_rw.commit().unwrap();

        // Now test get_block_trie_updates
        let provider = factory.provider().unwrap();
        let result = provider.get_block_trie_updates(target_block).unwrap();

        // Verify account trie updates
        assert_eq!(result.account_nodes_ref().len(), 2, "Should have 2 account trie updates");

        // Check nibbles1 - should have the current value (node1)
        let nibbles1_update = result
            .account_nodes_ref()
            .iter()
            .find(|(n, _)| n == &account_nibbles1)
            .expect("Should find nibbles1");
        assert!(nibbles1_update.1.is_some(), "nibbles1 should have a value");
        assert_eq!(
            nibbles1_update.1.as_ref().unwrap().state_mask,
            node1.state_mask,
            "nibbles1 should have current value"
        );

        // Check nibbles2 - should have the current value (node2)
        let nibbles2_update = result
            .account_nodes_ref()
            .iter()
            .find(|(n, _)| n == &account_nibbles2)
            .expect("Should find nibbles2");
        assert!(nibbles2_update.1.is_some(), "nibbles2 should have a value");
        assert_eq!(
            nibbles2_update.1.as_ref().unwrap().state_mask,
            node2.state_mask,
            "nibbles2 should have current value"
        );

        // nibbles3 should NOT be in the result (it was changed in next_block, not target_block)
        assert!(
            !result.account_nodes_ref().iter().any(|(n, _)| n == &account_nibbles3),
            "nibbles3 should not be in target_block updates"
        );

        // Verify storage trie updates
        assert_eq!(result.storage_tries_ref().len(), 1, "Should have 1 storage trie");
        let storage_updates = result
            .storage_tries_ref()
            .get(&storage_address1)
            .expect("Should have storage updates for address1");

        assert_eq!(storage_updates.storage_nodes.len(), 2, "Should have 2 storage node updates");

        // Check storage_nibbles1 - should have current value
        let storage1_update = storage_updates
            .storage_nodes
            .iter()
            .find(|(n, _)| n == &storage_nibbles1)
            .expect("Should find storage_nibbles1");
        assert!(storage1_update.1.is_some(), "storage_nibbles1 should have a value");
        assert_eq!(
            storage1_update.1.as_ref().unwrap().state_mask,
            storage_node1.state_mask,
            "storage_nibbles1 should have current value"
        );

        // Check storage_nibbles2 - was created in target_block, will be deleted in next_block
        // So it should have a value (the value that will be deleted)
        let storage2_update = storage_updates
            .storage_nodes
            .iter()
            .find(|(n, _)| n == &storage_nibbles2)
            .expect("Should find storage_nibbles2");
        assert!(
            storage2_update.1.is_some(),
            "storage_nibbles2 should have a value (the node that will be deleted in next block)"
        );
        assert_eq!(
            storage2_update.1.as_ref().unwrap().state_mask,
            storage_node2.state_mask,
            "storage_nibbles2 should have the value that was created and will be deleted"
        );
    }

    #[test]
    fn test_prunable_receipts_logic() {
        let insert_blocks =
            |provider_rw: &DatabaseProviderRW<_, _>, tip_block: u64, tx_count: u8| {
                let mut rng = generators::rng();
                for block_num in 0..=tip_block {
                    let block = random_block(
                        &mut rng,
                        block_num,
                        BlockParams { tx_count: Some(tx_count), ..Default::default() },
                    );
                    provider_rw.insert_block(&block.try_recover().unwrap()).unwrap();
                }
            };

        let write_receipts = |provider_rw: DatabaseProviderRW<_, _>, block: u64| {
            let outcome = ExecutionOutcome {
                first_block: block,
                receipts: vec![vec![Receipt {
                    tx_type: Default::default(),
                    success: true,
                    cumulative_gas_used: block, // identifier to assert against
                    logs: vec![],
                }]],
                ..Default::default()
            };
            provider_rw.write_state(&outcome, crate::OriginalValuesKnown::No).unwrap();
            provider_rw.commit().unwrap();
        };

        // Legacy mode (receipts in DB) - should be prunable
        {
            let factory = create_test_provider_factory();
            let storage_settings = StorageSettings::legacy();
            factory.set_storage_settings_cache(storage_settings);
            let factory = factory.with_prune_modes(PruneModes {
                receipts: Some(PruneMode::Before(100)),
                ..Default::default()
            });

            let tip_block = 200u64;
            let first_block = 1u64;

            // create chain
            let provider_rw = factory.provider_rw().unwrap();
            insert_blocks(&provider_rw, tip_block, 1);
            provider_rw.commit().unwrap();

            write_receipts(
                factory.provider_rw().unwrap().with_minimum_pruning_distance(100),
                first_block,
            );
            write_receipts(
                factory.provider_rw().unwrap().with_minimum_pruning_distance(100),
                tip_block - 1,
            );

            let provider = factory.provider().unwrap();

            for (block, num_receipts) in [(0, 0), (tip_block - 1, 1)] {
                assert!(provider
                    .receipts_by_block(block.into())
                    .unwrap()
                    .is_some_and(|r| r.len() == num_receipts));
            }
        }

        // Static files mode
        {
            let factory = create_test_provider_factory();
            let storage_settings = StorageSettings::legacy().with_receipts_in_static_files(true);
            factory.set_storage_settings_cache(storage_settings);
            let factory = factory.with_prune_modes(PruneModes {
                receipts: Some(PruneMode::Before(2)),
                ..Default::default()
            });

            let tip_block = 200u64;

            // create chain
            let provider_rw = factory.provider_rw().unwrap();
            insert_blocks(&provider_rw, tip_block, 1);
            provider_rw.commit().unwrap();

            // Attempt to write receipts for block 0 and 1 (should be skipped)
            write_receipts(factory.provider_rw().unwrap().with_minimum_pruning_distance(100), 0);
            write_receipts(factory.provider_rw().unwrap().with_minimum_pruning_distance(100), 1);

            assert!(factory
                .static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .is_none(),);
            assert!(factory
                .static_file_provider()
                .get_highest_static_file_block(StaticFileSegment::Receipts)
                .is_some_and(|b| b == 1),);

            // Since we have prune mode Before(2), the next receipt (block 2) should be written to
            // static files.
            write_receipts(factory.provider_rw().unwrap().with_minimum_pruning_distance(100), 2);
            assert!(factory
                .static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .is_some_and(|num| num == 2),);

            // After having a receipt already in static files, attempt to skip the next receipt by
            // changing the prune mode. It should NOT skip it and should still write the receipt,
            // since static files do not support gaps.
            let factory = factory.with_prune_modes(PruneModes {
                receipts: Some(PruneMode::Before(100)),
                ..Default::default()
            });
            let provider_rw = factory.provider_rw().unwrap().with_minimum_pruning_distance(1);
            assert!(PruneMode::Distance(1).should_prune(3, tip_block));
            write_receipts(provider_rw, 3);

            // Ensure we can only fetch the 2 last receipts.
            //
            // Test setup only has 1 tx per block and each receipt has its cumulative_gas_used set
            // to the block number it belongs to easily identify and assert.
            let provider = factory.provider().unwrap();
            assert!(EitherWriter::receipts_destination(&provider).is_static_file());
            for (num, num_receipts) in [(0, 0), (1, 0), (2, 1), (3, 1)] {
                assert!(provider
                    .receipts_by_block(num.into())
                    .unwrap()
                    .is_some_and(|r| r.len() == num_receipts));

                let receipt = provider.receipt(num).unwrap();
                if num_receipts > 0 {
                    assert!(receipt.is_some_and(|r| r.cumulative_gas_used == num));
                } else {
                    assert!(receipt.is_none());
                }
            }
        }
    }
}
