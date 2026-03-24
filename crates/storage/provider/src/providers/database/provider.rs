use crate::{
    changesets_utils::StorageRevertsIter,
    providers::{
        database::{chain::ChainStorage, metrics},
        rocksdb::{PendingRocksDBBatches, RocksDBProvider, RocksDBWriteCtx},
        static_file::{StaticFileWriteCtx, StaticFileWriter},
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
    RocksDBProviderFactory, StageCheckpointReader, StateProviderBox, StateWriter,
    StaticFileProviderFactory, StatsReader, StorageReader, StorageTrieWriter, TransactionVariant,
    TransactionsProvider, TransactionsProviderExt, TrieWriter,
};
use alloy_consensus::{
    transaction::{SignerRecoverable, TransactionMeta, TxHashRef},
    BlockHeader, TxReceipt,
};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{
    keccak256,
    map::{hash_map, AddressSet, B256Map, HashMap},
    Address, BlockHash, BlockNumber, TxHash, TxNumber, B256,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rayon::slice::ParallelSliceMut;
use reth_chain_state::{ComputedTrieData, ExecutedBlock};
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    models::{
        sharded_key, storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress,
        BlockNumberAddressRange, ShardedKey, StorageBeforeTx, StorageSettings,
        StoredBlockBodyIndices,
    },
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList, PlainAccountState, PlainStorageState,
};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, Chain, ExecutionOutcome};
use reth_node_types::{BlockTy, BodyTy, HeaderTy, NodeTypes, ReceiptTy, TxTy};
use reth_primitives_traits::{
    Account, Block as _, BlockBody as _, Bytecode, FastInstant as Instant, RecoveredBlock,
    SealedHeader, StorageEntry,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneModes, PruneSegment, MINIMUM_UNWIND_SAFE_DISTANCE,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, BlockBodyReader, MetadataProvider, MetadataWriter,
    NodePrimitivesProvider, StateProvider, StateWriteConfig, StorageChangeSetReader, StoragePath,
    StorageSettingsCache, TryIntoHistoricalStateProvider, WriteStateInput,
};
use reth_storage_errors::provider::{ProviderResult, StaticFileWriterError};
use reth_trie::{
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
    HashedPostStateSorted,
};
use reth_trie_db::{ChangesetCache, DatabaseStorageTrieCursor, TrieTableAdapter};
use revm_database::states::{
    PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    ops::{Deref, DerefMut, Range, RangeBounds, RangeInclusive},
    path::PathBuf,
    sync::Arc,
};
use tracing::{debug, instrument, trace};

/// Determines the commit order for database operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CommitOrder {
    /// Normal commit order: static files first, then `RocksDB`, then MDBX.
    #[default]
    Normal,
    /// Unwind commit order: MDBX first, then `RocksDB`, then static files.
    /// Used for unwind operations to allow recovery by truncating static files on restart.
    Unwind,
}

impl CommitOrder {
    /// Returns true if this is unwind commit order.
    pub const fn is_unwind(&self) -> bool {
        matches!(self, Self::Unwind)
    }
}

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
    pub fn commit(self) -> ProviderResult<()> {
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

/// Mode for [`DatabaseProvider::save_blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveBlocksMode {
    /// Full mode: write block structure + receipts + state + trie.
    /// Used by engine/production code.
    Full,
    /// Blocks only: write block structure (headers, txs, senders, indices).
    /// Receipts/state/trie are skipped - they may come later via separate calls.
    /// Used by `insert_block`.
    BlocksOnly,
}

impl SaveBlocksMode {
    /// Returns `true` if this is [`SaveBlocksMode::Full`].
    pub const fn with_state(self) -> bool {
        matches!(self, Self::Full)
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
    /// Changeset cache for trie unwinding
    changeset_cache: ChangesetCache,
    /// Task runtime for spawning parallel I/O work.
    runtime: reth_tasks::Runtime,
    /// Path to the database directory.
    db_path: PathBuf,
    /// Pending `RocksDB` batches to be committed at provider commit time.
    #[cfg_attr(not(all(unix, feature = "rocksdb")), allow(dead_code))]
    pending_rocksdb_batches: PendingRocksDBBatches,
    /// Commit order for database operations.
    commit_order: CommitOrder,
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
            .field("rocksdb_provider", &self.rocksdb_provider)
            .field("changeset_cache", &self.changeset_cache)
            .field("runtime", &self.runtime)
            .field("pending_rocksdb_batches", &"<pending batches>")
            .field("commit_order", &self.commit_order)
            .field("minimum_pruning_distance", &self.minimum_pruning_distance)
            .finish()
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

    #[cfg(all(unix, feature = "rocksdb"))]
    fn commit_pending_rocksdb_batches(&self) -> ProviderResult<()> {
        let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
        for batch in batches {
            self.rocksdb_provider.commit_batch(batch)?;
        }
        Ok(())
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
    #[allow(clippy::too_many_arguments)]
    fn new_rw_inner(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
        changeset_cache: ChangesetCache,
        runtime: reth_tasks::Runtime,
        db_path: PathBuf,
        commit_order: CommitOrder,
    ) -> Self {
        Self {
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            changeset_cache,
            runtime,
            db_path,
            pending_rocksdb_batches: Default::default(),
            commit_order,
            minimum_pruning_distance: MINIMUM_UNWIND_SAFE_DISTANCE,
            metrics: metrics::DatabaseProviderMetrics::default(),
        }
    }

    /// Creates a provider with an inner read-write transaction using normal commit order.
    #[allow(clippy::too_many_arguments)]
    pub fn new_rw(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
        changeset_cache: ChangesetCache,
        runtime: reth_tasks::Runtime,
        db_path: PathBuf,
    ) -> Self {
        Self::new_rw_inner(
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            changeset_cache,
            runtime,
            db_path,
            CommitOrder::Normal,
        )
    }

    /// Creates a provider with an inner read-write transaction using unwind commit order.
    #[allow(clippy::too_many_arguments)]
    pub fn new_unwind_rw(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
        changeset_cache: ChangesetCache,
        runtime: reth_tasks::Runtime,
        db_path: PathBuf,
    ) -> Self {
        Self::new_rw_inner(
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            changeset_cache,
            runtime,
            db_path,
            CommitOrder::Unwind,
        )
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

    /// Creates the context for static file writes.
    fn static_file_write_ctx(
        &self,
        save_mode: SaveBlocksMode,
        first_block: BlockNumber,
        last_block: BlockNumber,
    ) -> ProviderResult<StaticFileWriteCtx> {
        let tip = self.last_block_number()?.max(last_block);
        Ok(StaticFileWriteCtx {
            write_senders: EitherWriterDestination::senders(self).is_static_file() &&
                self.prune_modes.sender_recovery.is_none_or(|m| !m.is_full()),
            write_receipts: save_mode.with_state() &&
                EitherWriter::receipts_destination(self).is_static_file(),
            write_account_changesets: save_mode.with_state() &&
                EitherWriterDestination::account_changesets(self).is_static_file(),
            write_storage_changesets: save_mode.with_state() &&
                EitherWriterDestination::storage_changesets(self).is_static_file(),
            tip,
            receipts_prune_mode: self.prune_modes.receipts,
            // Receipts are prunable if no receipts exist in SF yet and within pruning distance
            receipts_prunable: self
                .static_file_provider
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .is_none() &&
                PruneMode::Distance(self.minimum_pruning_distance)
                    .should_prune(first_block, tip),
        })
    }

    /// Creates the context for `RocksDB` writes.
    #[cfg_attr(not(all(unix, feature = "rocksdb")), allow(dead_code))]
    fn rocksdb_write_ctx(&self, first_block: BlockNumber) -> RocksDBWriteCtx {
        RocksDBWriteCtx {
            first_block_number: first_block,
            prune_tx_lookup: self.prune_modes.transaction_lookup,
            storage_settings: self.cached_storage_settings(),
            pending_batches: self.pending_rocksdb_batches.clone(),
        }
    }

    /// Writes executed blocks and state to storage.
    ///
    /// This method parallelizes static file (SF) writes with MDBX writes.
    /// The SF thread writes headers, transactions, senders (if SF), and receipts (if SF, Full mode
    /// only). The main thread writes MDBX data (indices, state, trie - Full mode only).
    ///
    /// Use [`SaveBlocksMode::Full`] for production (includes receipts, state, trie).
    /// Use [`SaveBlocksMode::BlocksOnly`] for block structure only (used by `insert_block`).
    #[instrument(level = "debug", target = "providers::db", skip_all, fields(block_count = blocks.len()))]
    pub fn save_blocks(
        &self,
        blocks: Vec<ExecutedBlock<N::Primitives>>,
        save_mode: SaveBlocksMode,
    ) -> ProviderResult<()> {
        if blocks.is_empty() {
            debug!(target: "providers::db", "Attempted to write empty block range");
            return Ok(())
        }

        let total_start = Instant::now();
        let block_count = blocks.len() as u64;
        let first_number = blocks.first().unwrap().recovered_block().number();
        let last_block_number = blocks.last().unwrap().recovered_block().number();

        debug!(target: "providers::db", block_count, "Writing blocks and execution data to storage");

        // Compute tx_nums upfront (both threads need these)
        let first_tx_num = self
            .tx
            .cursor_read::<tables::TransactionBlocks>()?
            .last()?
            .map(|(n, _)| n + 1)
            .unwrap_or_default();

        let tx_nums: Vec<TxNumber> = {
            let mut nums = Vec::with_capacity(blocks.len());
            let mut current = first_tx_num;
            for block in &blocks {
                nums.push(current);
                current += block.recovered_block().body().transaction_count() as u64;
            }
            nums
        };

        let mut timings = metrics::SaveBlocksTimings { block_count, ..Default::default() };

        // avoid capturing &self.tx in scope below.
        let sf_provider = &self.static_file_provider;
        let sf_ctx = self.static_file_write_ctx(save_mode, first_number, last_block_number)?;
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_provider = self.rocksdb_provider.clone();
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_ctx = self.rocksdb_write_ctx(first_number);
        #[cfg(all(unix, feature = "rocksdb"))]
        let rocksdb_enabled = rocksdb_ctx.storage_settings.storage_v2;

        let mut sf_result = None;
        #[cfg(all(unix, feature = "rocksdb"))]
        let mut rocksdb_result = None;

        // Write to all backends in parallel.
        let runtime = &self.runtime;
        // Propagate tracing context into rayon-spawned threads so that static file
        // and RocksDB write spans appear as children of save_blocks in traces.
        let span = tracing::Span::current();
        runtime.storage_pool().in_place_scope(|s| {
            // SF writes
            s.spawn(|_| {
                let _guard = span.enter();
                let start = Instant::now();
                sf_result = Some(
                    sf_provider
                        .write_blocks_data(&blocks, &tx_nums, sf_ctx, runtime)
                        .map(|()| start.elapsed()),
                );
            });

            // RocksDB writes
            #[cfg(all(unix, feature = "rocksdb"))]
            if rocksdb_enabled {
                s.spawn(|_| {
                    let _guard = span.enter();
                    let start = Instant::now();
                    rocksdb_result = Some(
                        rocksdb_provider
                            .write_blocks_data(&blocks, &tx_nums, rocksdb_ctx, runtime)
                            .map(|()| start.elapsed()),
                    );
                });
            }

            // MDBX writes
            let mdbx_start = Instant::now();

            // Collect all transaction hashes across all blocks, sort them, and write in batch
            if !self.cached_storage_settings().storage_v2 &&
                self.prune_modes.transaction_lookup.is_none_or(|m| !m.is_full())
            {
                let start = Instant::now();
                let total_tx_count: usize =
                    blocks.iter().map(|b| b.recovered_block().body().transaction_count()).sum();
                let mut all_tx_hashes = Vec::with_capacity(total_tx_count);
                for (i, block) in blocks.iter().enumerate() {
                    let recovered_block = block.recovered_block();
                    for (tx_num, transaction) in
                        (tx_nums[i]..).zip(recovered_block.body().transactions_iter())
                    {
                        all_tx_hashes.push((*transaction.tx_hash(), tx_num));
                    }
                }

                // Sort by hash for optimal MDBX insertion performance
                all_tx_hashes.sort_unstable_by_key(|(hash, _)| *hash);

                // Write all transaction hash numbers in a single batch
                self.with_rocksdb_batch(|batch| {
                    let mut tx_hash_writer =
                        EitherWriter::new_transaction_hash_numbers(self, batch)?;
                    tx_hash_writer.put_transaction_hash_numbers_batch(all_tx_hashes, false)?;
                    let raw_batch = tx_hash_writer.into_raw_rocksdb_batch();
                    Ok(((), raw_batch))
                })?;
                self.metrics.record_duration(
                    metrics::Action::InsertTransactionHashNumbers,
                    start.elapsed(),
                );
            }

            for (i, block) in blocks.iter().enumerate() {
                let recovered_block = block.recovered_block();

                let start = Instant::now();
                self.insert_block_mdbx_only(recovered_block, tx_nums[i])?;
                timings.insert_block += start.elapsed();

                if save_mode.with_state() {
                    let execution_output = block.execution_outcome();

                    // Write state and changesets to the database.
                    // Must be written after blocks because of the receipt lookup.
                    // Skip receipts/account changesets if they're being written to static files.
                    let start = Instant::now();
                    self.write_state(
                        WriteStateInput::Single {
                            outcome: execution_output,
                            block: recovered_block.number(),
                        },
                        OriginalValuesKnown::No,
                        StateWriteConfig {
                            write_receipts: !sf_ctx.write_receipts,
                            write_account_changesets: !sf_ctx.write_account_changesets,
                            write_storage_changesets: !sf_ctx.write_storage_changesets,
                        },
                    )?;
                    timings.write_state += start.elapsed();
                }
            }

            // Write all hashed state and trie updates in single batches.
            // This reduces cursor open/close overhead from N calls to 1.
            if save_mode.with_state() {
                // Blocks are oldest-to-newest, merge_batch expects newest-to-oldest.
                let start = Instant::now();
                let merged_hashed_state = HashedPostStateSorted::merge_batch(
                    blocks.iter().rev().map(|b| b.trie_data().hashed_state),
                );
                if !merged_hashed_state.is_empty() {
                    self.write_hashed_state(&merged_hashed_state)?;
                }
                timings.write_hashed_state += start.elapsed();

                let start = Instant::now();
                let merged_trie =
                    TrieUpdatesSorted::merge_batch(blocks.iter().rev().map(|b| b.trie_updates()));
                if !merged_trie.is_empty() {
                    self.write_trie_updates_sorted(&merged_trie)?;
                }
                timings.write_trie_updates += start.elapsed();
            }

            // Full mode: update history indices
            if save_mode.with_state() {
                let start = Instant::now();
                self.update_history_indices(first_number..=last_block_number)?;
                timings.update_history_indices = start.elapsed();
            }

            // Update pipeline progress
            let start = Instant::now();
            self.update_pipeline_stages(last_block_number, false)?;
            timings.update_pipeline_stages = start.elapsed();

            timings.mdbx = mdbx_start.elapsed();

            Ok::<_, ProviderError>(())
        })?;

        // Collect results from spawned tasks
        timings.sf = sf_result.ok_or(StaticFileWriterError::ThreadPanic("static file"))??;

        #[cfg(all(unix, feature = "rocksdb"))]
        if rocksdb_enabled {
            timings.rocksdb = rocksdb_result.ok_or_else(|| {
                ProviderError::Database(reth_db_api::DatabaseError::Other(
                    "RocksDB thread panicked".into(),
                ))
            })??;
        }

        timings.total = total_start.elapsed();

        self.metrics.record_save_blocks(&timings);
        debug!(target: "providers::db", range = ?first_number..=last_block_number, "Appended block data");

        Ok(())
    }

    /// Writes MDBX-only data for a block (indices, lookups, and senders if configured for MDBX).
    ///
    /// SF data (headers, transactions, senders if SF, receipts if SF) must be written separately.
    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn insert_block_mdbx_only(
        &self,
        block: &RecoveredBlock<BlockTy<N>>,
        first_tx_num: TxNumber,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        if self.prune_modes.sender_recovery.is_none_or(|m| !m.is_full()) &&
            EitherWriterDestination::senders(self).is_database()
        {
            let start = Instant::now();
            let tx_nums_iter = std::iter::successors(Some(first_tx_num), |n| Some(n + 1));
            let mut cursor = self.tx.cursor_write::<tables::TransactionSenders>()?;
            for (tx_num, sender) in tx_nums_iter.zip(block.senders_iter().copied()) {
                cursor.append(tx_num, &sender)?;
            }
            self.metrics
                .record_duration(metrics::Action::InsertTransactionSenders, start.elapsed());
        }

        let block_number = block.number();
        let tx_count = block.body().transaction_count() as u64;

        let start = Instant::now();
        self.tx.put::<tables::HeaderNumbers>(block.hash(), block_number)?;
        self.metrics.record_duration(metrics::Action::InsertHeaderNumbers, start.elapsed());

        self.write_block_body_indices(block_number, block.body(), first_tx_num, tx_count)?;

        Ok(StoredBlockBodyIndices { first_tx_num, tx_count })
    }

    /// Writes MDBX block body indices (`BlockBodyIndices`, `TransactionBlocks`,
    /// `Ommers`/`Withdrawals`).
    fn write_block_body_indices(
        &self,
        block_number: BlockNumber,
        body: &BodyTy<N>,
        first_tx_num: TxNumber,
        tx_count: u64,
    ) -> ProviderResult<()> {
        // MDBX: BlockBodyIndices
        let start = Instant::now();
        self.tx
            .cursor_write::<tables::BlockBodyIndices>()?
            .append(block_number, &StoredBlockBodyIndices { first_tx_num, tx_count })?;
        self.metrics.record_duration(metrics::Action::InsertBlockBodyIndices, start.elapsed());

        // MDBX: TransactionBlocks (last tx -> block mapping)
        if tx_count > 0 {
            let start = Instant::now();
            self.tx
                .cursor_write::<tables::TransactionBlocks>()?
                .append(first_tx_num + tx_count - 1, &block_number)?;
            self.metrics.record_duration(metrics::Action::InsertTransactionBlocks, start.elapsed());
        }

        // MDBX: Ommers/Withdrawals
        self.storage.writer().write_block_bodies(self, vec![(block_number, Some(body))])?;

        Ok(())
    }

    /// Unwinds trie state starting at and including the given block.
    ///
    /// This includes calculating the resulted state root and comparing it with the parent block
    /// state root.
    pub fn unwind_trie_state_from(&self, from: BlockNumber) -> ProviderResult<()> {
        let changed_accounts = self.account_changesets_range(from..)?;

        // Unwind account hashes.
        self.unwind_account_hashing(changed_accounts.iter())?;

        // Unwind account history indices.
        self.unwind_account_history_indices(changed_accounts.iter())?;

        let changed_storages = self.storage_changesets_range(from..)?;

        // Unwind storage hashes.
        self.unwind_storage_hashing(changed_storages.iter().copied())?;

        // Unwind storage history indices.
        self.unwind_storage_history_indices(changed_storages.iter().copied())?;

        // Unwind accounts/storages trie tables using the revert.
        // Get the database tip block number
        let db_tip_block = self
            .get_stage_checkpoint(reth_stages_types::StageId::Finish)?
            .as_ref()
            .map(|chk| chk.block_number)
            .ok_or_else(|| ProviderError::InsufficientChangesets {
                requested: from,
                available: 0..=0,
            })?;

        let trie_revert = self.changeset_cache.get_or_compute_range(self, from..=db_tip_block)?;
        self.write_trie_updates_sorted(&trie_revert)?;

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

    /// Writes bytecodes to MDBX.
    fn write_bytecodes(
        &self,
        bytecodes: impl IntoIterator<Item = (B256, Bytecode)>,
    ) -> ProviderResult<()> {
        let mut bytecodes_cursor = self.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in bytecodes {
            bytecodes_cursor.upsert(hash, &bytecode)?;
        }
        Ok(())
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> TryIntoHistoricalStateProvider for DatabaseProvider<TX, N> {
    fn try_into_history_at_block(
        self,
        mut block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        let best_block = self.best_block_number().unwrap_or_default();

        // Reject requests for blocks beyond the best block
        if block_number > best_block {
            return Err(ProviderError::BlockNotExecuted {
                requested: block_number,
                executed: best_block,
            });
        }

        // If requesting state at the best block, use the latest state provider
        if block_number == best_block {
            return Ok(Box::new(LatestStateProvider::new(self)));
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tx: TX,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        prune_modes: PruneModes,
        storage: Arc<N::Storage>,
        storage_settings: Arc<RwLock<StorageSettings>>,
        rocksdb_provider: RocksDBProvider,
        changeset_cache: ChangesetCache,
        runtime: reth_tasks::Runtime,
        db_path: PathBuf,
    ) -> Self {
        Self {
            tx,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            changeset_cache,
            runtime,
            db_path,
            pending_rocksdb_batches: Default::default(),
            commit_order: CommitOrder::Normal,
            minimum_pruning_distance: MINIMUM_UNWIND_SAFE_DISTANCE,
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

        let transactions = if tx_range.is_empty() {
            vec![]
        } else {
            self.transactions_by_tx_range(tx_range.clone())?
        };

        let body = self
            .storage
            .reader()
            .read_block_bodies(self, vec![(header.as_ref(), transactions)])?
            .pop()
            .ok_or(ProviderError::InvalidStorageOutput)?;

        let senders = if tx_range.is_empty() {
            vec![]
        } else {
            let known_senders: HashMap<TxNumber, Address> =
                EitherReader::new_senders(self)?.senders_by_tx_range(tx_range.clone())?;

            let mut senders = Vec::with_capacity(body.transactions().len());
            for (tx_num, tx) in tx_range.zip(body.transactions()) {
                match known_senders.get(&tx_num) {
                    None => {
                        let sender = tx.recover_signer_unchecked()?;
                        senders.push(sender);
                    }
                    Some(sender) => senders.push(*sender),
                }
            }
            senders
        };

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

    /// Like [`populate_bundle_state`](Self::populate_bundle_state), but reads current values from
    /// `HashedAccounts`/`HashedStorages`. Addresses and storage keys are hashed via `keccak256`
    /// for DB lookups. The output `BundleStateInit`/`RevertsInit` structures remain keyed by
    /// plain address and plain storage key.
    fn populate_bundle_state_hashed(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        hashed_accounts_cursor: &mut impl DbCursorRO<tables::HashedAccounts>,
        hashed_storage_cursor: &mut impl DbDupCursorRO<tables::HashedStorages>,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)> {
        let mut state: BundleStateInit = HashMap::default();
        let mut reverts: RevertsInit = HashMap::default();

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let hashed_address = keccak256(address);
                    let new_info =
                        hashed_accounts_cursor.seek_exact(hashed_address)?.map(|kv| kv.1);
                    entry.insert((old_info, new_info, HashMap::default()));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().0 = old_info;
                }
            }
            reverts.entry(block_number).or_default().entry(address).or_default().0 = Some(old_info);
        }

        // add storage changeset changes
        for (block_and_address, old_storage) in storage_changeset.into_iter().rev() {
            let BlockNumberAddress((block_number, address)) = block_and_address;
            let account_state = match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let hashed_address = keccak256(address);
                    let present_info =
                        hashed_accounts_cursor.seek_exact(hashed_address)?.map(|kv| kv.1);
                    entry.insert((present_info, present_info, HashMap::default()))
                }
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };

            // Storage keys in changesets are plain; hash them for HashedStorages lookup.
            let hashed_storage_key = keccak256(old_storage.key);
            match account_state.2.entry(old_storage.key) {
                hash_map::Entry::Vacant(entry) => {
                    let hashed_address = keccak256(address);
                    let new_storage = hashed_storage_cursor
                        .seek_by_key_subkey(hashed_address, hashed_storage_key)?
                        .filter(|storage| storage.key == hashed_storage_key)
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
        if self.cached_storage_settings().use_hashed_state() {
            let hashed_address = keccak256(address);
            Ok(self.tx.get_by_encoded_key::<tables::HashedAccounts>(&hashed_address)?)
        } else {
            Ok(self.tx.get_by_encoded_key::<tables::PlainAccountState>(address)?)
        }
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
        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts = self.tx.cursor_read::<tables::HashedAccounts>()?;
            Ok(iter
                .into_iter()
                .map(|address| {
                    let hashed_address = keccak256(address);
                    hashed_accounts.seek_exact(hashed_address).map(|a| (address, a.map(|(_, v)| v)))
                })
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            let mut plain_accounts = self.tx.cursor_read::<tables::PlainAccountState>()?;
            Ok(iter
                .into_iter()
                .map(|address| {
                    plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v)))
                })
                .collect::<Result<Vec<_>, _>>()?)
        }
    }

    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<u64>>> {
        let highest_static_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);

        if let Some(highest) = highest_static_block &&
            self.cached_storage_settings().storage_v2
        {
            let start = *range.start();
            let static_end = (*range.end()).min(highest);

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
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.storage_changeset(block_number)
        } else {
            let range = block_number..=block_number;
            let storage_range = BlockNumberAddress::range(range);
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .walk_range(storage_range)?
                .map(|r| {
                    let (bna, entry) = r?;
                    Ok((bna, entry))
                })
                .collect()
        }
    }

    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.get_storage_before_block(block_number, address, storage_key)
        } else {
            Ok(self
                .tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .seek_by_key_subkey(BlockNumberAddress((block_number, address)), storage_key)?
                .filter(|entry| entry.key == storage_key))
        }
    }

    fn storage_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.storage_changesets_range(range)
        } else {
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .walk_range(BlockNumberAddressRange::from(range))?
                .map(|r| {
                    let (bna, entry) = r?;
                    Ok((bna, entry))
                })
                .collect()
        }
    }

    fn storage_changeset_count(&self) -> ProviderResult<usize> {
        if self.cached_storage_settings().storage_v2 {
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
        if self.cached_storage_settings().storage_v2 {
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
        if self.cached_storage_settings().storage_v2 {
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
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.account_changesets_range(range)
        } else {
            self.tx
                .cursor_read::<tables::AccountChangeSets>()?
                .walk_range(to_range(range))?
                .map(|r| r.map_err(Into::into))
                .collect()
        }
    }

    fn account_changeset_count(&self) -> ProviderResult<usize> {
        // check if account changesets are in static files, otherwise just count the changeset
        // entries in the DB
        if self.cached_storage_settings().storage_v2 {
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
    ///
    /// Returns an error if the requested block is below the earliest available history.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        if let Some(number) = self.convert_hash_or_number(id)? {
            let earliest_available = self.static_file_provider.earliest_history_height();
            if number < earliest_available {
                return Err(ProviderError::BlockExpired { requested: number, earliest_available })
            }

            let Some(header) = self.header_by_number(number)? else { return Ok(None) };

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

    #[instrument(level = "debug", target = "providers::db", skip_all)]
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
        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_storage = self.tx.cursor_dup_read::<tables::HashedStorages>()?;

            addresses_with_keys
                .into_iter()
                .map(|(address, storage)| {
                    let hashed_address = keccak256(address);
                    storage
                        .into_iter()
                        .map(|key| -> ProviderResult<_> {
                            let hashed_key = keccak256(key);
                            let value = hashed_storage
                                .seek_by_key_subkey(hashed_address, hashed_key)?
                                .filter(|v| v.key == hashed_key)
                                .map(|v| v.value)
                                .unwrap_or_default();
                            Ok(StorageEntry { key, value })
                        })
                        .collect::<ProviderResult<Vec<_>>>()
                        .map(|storage| (address, storage))
                })
                .collect::<ProviderResult<Vec<(_, _)>>>()
        } else {
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
    }

    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>> {
        if self.cached_storage_settings().storage_v2 {
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
        if self.cached_storage_settings().storage_v2 {
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

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn write_state<'a>(
        &self,
        execution_outcome: impl Into<WriteStateInput<'a, Self::Receipt>>,
        is_value_known: OriginalValuesKnown,
        config: StateWriteConfig,
    ) -> ProviderResult<()> {
        let execution_outcome = execution_outcome.into();

        if self.cached_storage_settings().use_hashed_state() &&
            !config.write_receipts &&
            !config.write_account_changesets &&
            !config.write_storage_changesets
        {
            // In storage v2 with all outputs directed to static files, plain state and changesets
            // are written elsewhere. Only bytecodes need MDBX writes, so skip the expensive
            // to_plain_state_and_reverts conversion that iterates all accounts and storage.
            self.write_bytecodes(
                execution_outcome.state().contracts.iter().map(|(h, b)| (*h, Bytecode(b.clone()))),
            )?;
            return Ok(());
        }

        let first_block = execution_outcome.first_block();
        let (plain_state, reverts) =
            execution_outcome.state().to_plain_state_and_reverts(is_value_known);

        self.write_state_reverts(reverts, first_block, config)?;
        self.write_state_changes(plain_state)?;

        if !config.write_receipts {
            return Ok(());
        }

        let block_count = execution_outcome.len() as u64;
        let last_block = execution_outcome.last_block();
        let block_range = first_block..=last_block;

        let tip = self.last_block_number()?.max(last_block);

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
        let mut allowed_addresses: AddressSet = AddressSet::default();
        for (_, addresses) in contract_log_pruner.range(..first_block) {
            allowed_addresses.extend(addresses.iter().copied());
        }

        for (idx, (receipts, first_tx_index)) in
            execution_outcome.receipts().zip(block_indices).enumerate()
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
        config: StateWriteConfig,
    ) -> ProviderResult<()> {
        // Write storage changes
        if config.write_storage_changesets {
            tracing::trace!("Writing storage changes");
            let mut storages_cursor =
                self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
            for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
                let block_number = first_block + block_index as BlockNumber;

                tracing::trace!(block_number, "Writing block change");
                // sort changes by address.
                storage_changes.par_sort_unstable_by_key(|a| a.address);
                let total_changes =
                    storage_changes.iter().map(|change| change.storage_revert.len()).sum();
                let mut changeset = Vec::with_capacity(total_changes);
                for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                    let mut storage = storage_revert
                        .into_iter()
                        .map(|(k, v)| (B256::from(k.to_be_bytes()), v))
                        .collect::<Vec<_>>();
                    // sort storage slots by key.
                    storage.par_sort_unstable_by_key(|a| a.0);

                    // If we are writing the primary storage wipe transition, the pre-existing
                    // storage state has to be taken from the database and written to storage
                    // history. See [StorageWipe::Primary] for more details.
                    //
                    // TODO(mediocregopher): This could be rewritten in a way which doesn't
                    // require collecting wiped entries into a Vec like this, see
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
        }

        if !config.write_account_changesets {
            return Ok(());
        }

        // Write account changes
        tracing::trace!(?first_block, "Writing account changes");
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

        if !self.cached_storage_settings().use_hashed_state() {
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

            // Write new storage state and wipe storage if needed.
            tracing::trace!(len = changes.storage.len(), "Writing new storage state");
            let mut storages_cursor =
                self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
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
                    if let Some(db_entry) =
                        storages_cursor.seek_by_key_subkey(address, entry.key)? &&
                        db_entry.key == entry.key
                    {
                        storages_cursor.delete_current()?;
                    }

                    if !entry.value.is_zero() {
                        storages_cursor.upsert(address, &entry)?;
                    }
                }
            }
        }

        // Write bytecode
        tracing::trace!(len = changes.contracts.len(), "Writing bytecodes");
        self.write_bytecodes(
            changes.contracts.into_iter().map(|(hash, bytecode)| (hash, Bytecode(bytecode))),
        )?;

        Ok(())
    }

    #[instrument(level = "debug", target = "providers::db", skip_all)]
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
        let storage_changeset = if self.cached_storage_settings().storage_v2 {
            let changesets = self.storage_changesets_range(range.clone())?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            self.take::<tables::StorageChangeSets>(storage_range)?.into_iter().collect()
        };
        let account_changeset = if self.cached_storage_settings().storage_v2 {
            let changesets = self.account_changesets_range(range)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
            changeset_writer.prune_account_changesets(block)?;
            changesets
        } else {
            self.take::<tables::AccountChangeSets>(range)?
        };

        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
            let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;

            let (state, _) = self.populate_bundle_state_hashed(
                account_changeset,
                storage_changeset,
                &mut hashed_accounts_cursor,
                &mut hashed_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let hashed_address = keccak256(address);
                    let existing_entry = hashed_accounts_cursor.seek_exact(hashed_address)?;
                    if let Some(account) = old_account {
                        hashed_accounts_cursor.upsert(hashed_address, account)?;
                    } else if existing_entry.is_some() {
                        hashed_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let hashed_address = keccak256(address);
                    let hashed_storage_key = keccak256(storage_key);
                    let storage_entry =
                        StorageEntry { key: hashed_storage_key, value: *old_storage_value };
                    if hashed_storage_cursor
                        .seek_by_key_subkey(hashed_address, hashed_storage_key)?
                        .is_some_and(|s| s.key == hashed_storage_key)
                    {
                        hashed_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        hashed_storage_cursor.upsert(hashed_address, &storage_entry)?;
                    }
                }
            }
        } else {
            // This is not working for blocks that are not at tip. as plain state is not the last
            // state of end range. We should rename the functions or add support to access
            // History state. Accessing history state can be tricky but we are not gaining
            // anything.
            let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
            let mut plain_storage_cursor =
                self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

            let (state, _) = self.populate_bundle_state(
                account_changeset,
                storage_changeset,
                &mut plain_accounts_cursor,
                &mut plain_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                    if let Some(account) = old_account {
                        plain_accounts_cursor.upsert(*address, account)?;
                    } else if existing_entry.is_some() {
                        plain_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let storage_entry =
                        StorageEntry { key: *storage_key, value: *old_storage_value };
                    if plain_storage_cursor
                        .seek_by_key_subkey(*address, *storage_key)?
                        .is_some_and(|s| s.key == *storage_key)
                    {
                        plain_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        plain_storage_cursor.upsert(*address, &storage_entry)?;
                    }
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
            self.cached_storage_settings().storage_v2
        {
            let changesets = self.storage_changesets_range(block + 1..=highest_block)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            self.take::<tables::StorageChangeSets>(storage_range)?.into_iter().collect()
        };

        // if there are static files for this segment, prune them.
        let highest_changeset_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
        let account_changeset = if let Some(highest_block) = highest_changeset_block &&
            self.cached_storage_settings().storage_v2
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

        let (state, reverts) = if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
            let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;

            let (state, reverts) = self.populate_bundle_state_hashed(
                account_changeset,
                storage_changeset,
                &mut hashed_accounts_cursor,
                &mut hashed_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let hashed_address = keccak256(address);
                    let existing_entry = hashed_accounts_cursor.seek_exact(hashed_address)?;
                    if let Some(account) = old_account {
                        hashed_accounts_cursor.upsert(hashed_address, account)?;
                    } else if existing_entry.is_some() {
                        hashed_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let hashed_address = keccak256(address);
                    let hashed_storage_key = keccak256(storage_key);
                    let storage_entry =
                        StorageEntry { key: hashed_storage_key, value: *old_storage_value };
                    if hashed_storage_cursor
                        .seek_by_key_subkey(hashed_address, hashed_storage_key)?
                        .is_some_and(|s| s.key == hashed_storage_key)
                    {
                        hashed_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        hashed_storage_cursor.upsert(hashed_address, &storage_entry)?;
                    }
                }
            }

            (state, reverts)
        } else {
            // This is not working for blocks that are not at tip. as plain state is not the last
            // state of end range. We should rename the functions or add support to access
            // History state. Accessing history state can be tricky but we are not gaining
            // anything.
            let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
            let mut plain_storage_cursor =
                self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

            let (state, reverts) = self.populate_bundle_state(
                account_changeset,
                storage_changeset,
                &mut plain_accounts_cursor,
                &mut plain_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                    if let Some(account) = old_account {
                        plain_accounts_cursor.upsert(*address, account)?;
                    } else if existing_entry.is_some() {
                        plain_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let storage_entry =
                        StorageEntry { key: *storage_key, value: *old_storage_value };
                    if plain_storage_cursor
                        .seek_by_key_subkey(*address, *storage_key)?
                        .is_some_and(|s| s.key == *storage_key)
                    {
                        plain_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        plain_storage_cursor.upsert(*address, &storage_entry)?;
                    }
                }
            }

            (state, reverts)
        };

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

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    fn write_account_trie_updates<A: TrieTableAdapter>(
        tx: &TX,
        trie_updates: &TrieUpdatesSorted,
        num_entries: &mut usize,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut,
    {
        let mut account_trie_cursor = tx.cursor_write::<A::AccountTrieTable>()?;
        // Process sorted account nodes
        for (key, updated_node) in trie_updates.account_nodes_ref() {
            let nibbles = A::AccountKey::from(*key);
            match updated_node {
                Some(node) => {
                    if !key.is_empty() {
                        *num_entries += 1;
                        account_trie_cursor.upsert(nibbles, node)?;
                    }
                }
                None => {
                    *num_entries += 1;
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
            }
        }
        Ok(())
    }

    fn write_storage_tries<A: TrieTableAdapter>(
        tx: &TX,
        storage_tries: Vec<(&B256, &StorageTrieUpdatesSorted)>,
        num_entries: &mut usize,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut,
    {
        let mut cursor = tx.cursor_dup_write::<A::StorageTrieTable>()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let mut db_storage_trie_cursor: DatabaseStorageTrieCursor<_, A> =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            *num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates_sorted(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }
        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> TrieWriter for DatabaseProvider<TX, N> {
    /// Writes trie updates to the database with already sorted updates.
    ///
    /// Returns the number of entries modified.
    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn write_trie_updates_sorted(&self, trie_updates: &TrieUpdatesSorted) -> ProviderResult<usize> {
        if trie_updates.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        reth_trie_db::with_adapter!(self, |A| {
            Self::write_account_trie_updates::<A>(self.tx_ref(), trie_updates, &mut num_entries)?;
        });

        num_entries +=
            self.write_storage_trie_updates_sorted(trie_updates.storage_tries_ref().iter())?;

        Ok(num_entries)
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
        reth_trie_db::with_adapter!(self, |A| {
            Self::write_storage_tries::<A>(self.tx_ref(), storage_tries, &mut num_entries)?;
        });
        Ok(num_entries)
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
        let changesets = self.account_changesets_range(range)?;
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
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        // Aggregate all block changesets and make list of accounts that have been changed.
        let mut hashed_storages = changesets
            .into_iter()
            .map(|(BlockNumberAddress((_, address)), storage_entry)| {
                let hashed_key = keccak256(storage_entry.key);
                (keccak256(address), hashed_key, storage_entry.value)
            })
            .collect::<Vec<_>>();
        hashed_storages.sort_by_key(|(ha, hk, _)| (*ha, *hk));

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_storage_keys: B256Map<BTreeSet<B256>> =
            B256Map::with_capacity_and_hasher(hashed_storages.len(), Default::default());
        let mut hashed_storage = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, key, value) in hashed_storages.into_iter().rev() {
            hashed_storage_keys.entry(hashed_address).or_default().insert(key);

            if hashed_storage
                .seek_by_key_subkey(hashed_address, key)?
                .is_some_and(|entry| entry.key == key)
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
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        let changesets = self.storage_changesets_range(range)?;
        self.unwind_storage_hashing(changesets.into_iter())
    }

    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
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
                    .is_some_and(|entry| entry.key == key)
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
        last_indices.sort_unstable_by_key(|(a, _)| *a);

        if self.cached_storage_settings().storage_v2 {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let batch = self.rocksdb_provider.unwind_account_history_indices(&last_indices)?;
                self.pending_rocksdb_batches.lock().push(batch);
            }
        } else {
            // Unwind the account history index in MDBX.
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
        }

        let changesets = last_indices.len();
        Ok(changesets)
    }

    fn unwind_account_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize> {
        let changesets = self.account_changesets_range(range)?;
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

        if self.cached_storage_settings().storage_v2 {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let batch =
                    self.rocksdb_provider.unwind_storage_history_indices(&storage_changesets)?;
                self.pending_rocksdb_batches.lock().push(batch);
            }
        } else {
            // Unwind the storage history index in MDBX.
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
        }

        let changesets = storage_changesets.len();
        Ok(changesets)
    }

    fn unwind_storage_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize> {
        let changesets = self.storage_changesets_range(range)?;
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

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()> {
        let storage_settings = self.cached_storage_settings();
        if !storage_settings.storage_v2 {
            let indices = self.changed_accounts_and_blocks_with_range(range.clone())?;
            self.insert_account_history_index(indices)?;
        }

        if !storage_settings.storage_v2 {
            let indices = self.changed_storages_and_blocks_with_range(range)?;
            self.insert_storage_history_index(indices)?;
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider> BlockExecutionWriter
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

        Ok(Chain::new(blocks, execution_state, BTreeMap::new()))
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

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider> BlockWriter
    for DatabaseProvider<TX, N>
{
    type Block = BlockTy<N>;
    type Receipt = ReceiptTy<N>;

    /// Inserts the block into the database, writing to both static files and MDBX.
    ///
    /// This is a convenience method primarily used in tests. For production use,
    /// prefer [`Self::save_blocks`] which handles execution output and trie data.
    fn insert_block(
        &self,
        block: &RecoveredBlock<Self::Block>,
    ) -> ProviderResult<StoredBlockBodyIndices> {
        let block_number = block.number();

        // Wrap block in ExecutedBlock with empty execution output (no receipts/state/trie)
        let executed_block = ExecutedBlock::new(
            Arc::new(block.clone()),
            Arc::new(BlockExecutionOutput {
                result: BlockExecutionResult {
                    receipts: Default::default(),
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
                state: Default::default(),
            }),
            ComputedTrieData::default(),
        );

        // Delegate to save_blocks with BlocksOnly mode (skips receipts/state/trie)
        self.save_blocks(vec![executed_block], SaveBlocksMode::BlocksOnly)?;

        // Return the body indices
        self.block_body_indices(block_number)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(block_number))
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

        // Skip sender pruning when sender_recovery is fully pruned, since no sender data
        // exists in static files or the database.
        if self.prune_modes.sender_recovery.is_none_or(|m| !m.is_full()) {
            EitherWriter::new_senders(self, last_block_number)?
                .prune_senders(unwind_tx_from, block)?;
        }

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

    /// Appends blocks with their execution state to the database.
    ///
    /// **Note:** This function is only used in tests.
    ///
    /// History indices are written to the appropriate backend based on storage settings:
    /// MDBX when `*_history_in_rocksdb` is false, `RocksDB` when true.
    ///
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
                        let key = B256::from(storage_key.to_be_bytes());
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

        self.write_state(execution_outcome, OriginalValuesKnown::No, StateWriteConfig::default())?;
        durations_recorder.record_relative(metrics::Action::InsertState);

        // insert hashes and intermediate merkle nodes
        self.write_hashed_state(&hashed_state)?;
        durations_recorder.record_relative(metrics::Action::InsertHashes);

        // Use pre-computed transitions for history indices since static file
        // writes aren't visible until commit.
        // Note: For MDBX we use insert_*_history_index. For RocksDB we use
        // append_*_history_shard which handles read-merge-write internally.
        let storage_settings = self.cached_storage_settings();
        if storage_settings.storage_v2 {
            #[cfg(all(unix, feature = "rocksdb"))]
            self.with_rocksdb_batch(|mut batch| {
                for (address, blocks) in account_transitions {
                    batch.append_account_history_shard(address, blocks)?;
                }
                Ok(((), Some(batch.into_inner())))
            })?;
        } else {
            self.insert_account_history_index(account_transitions)?;
        }
        if storage_settings.storage_v2 {
            #[cfg(all(unix, feature = "rocksdb"))]
            self.with_rocksdb_batch(|mut batch| {
                for ((address, key), blocks) in storage_transitions {
                    batch.append_storage_history_shard(address, key, blocks)?;
                }
                Ok(((), Some(batch.into_inner())))
            })?;
        } else {
            self.insert_storage_history_index(storage_transitions)?;
        }
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
    #[instrument(
        name = "DatabaseProvider::commit",
        level = "debug",
        target = "providers::db",
        skip_all
    )]
    fn commit(self) -> ProviderResult<()> {
        // For unwinding it makes more sense to commit the database first, since if
        // it is interrupted before the static files commit, we can just
        // truncate the static files according to the
        // checkpoints on the next start-up.
        if self.static_file_provider.has_unwind_queued() || self.commit_order.is_unwind() {
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
            // Normal path: finalize() will call sync_all() if not already synced
            let mut timings = metrics::CommitTimings::default();

            let start = Instant::now();
            self.static_file_provider.finalize()?;
            timings.sf = start.elapsed();

            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let start = Instant::now();
                let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
                for batch in batches {
                    self.rocksdb_provider.commit_batch(batch)?;
                }
                timings.rocksdb = start.elapsed();
            }

            let start = Instant::now();
            self.tx.commit()?;
            timings.mdbx = start.elapsed();

            self.metrics.record_commit(&timings);
        }

        Ok(())
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

impl<TX: Send, N: NodeTypes> StoragePath for DatabaseProvider<TX, N> {
    fn storage_path(&self) -> PathBuf {
        self.db_path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{blocks::BlockchainTestData, create_test_provider_factory},
        BlockWriter,
    };
    use alloy_consensus::Header;
    use alloy_primitives::{
        map::{AddressMap, B256Map},
        U256,
    };
    use reth_chain_state::ExecutedBlock;
    use reth_ethereum_primitives::Receipt;
    use reth_execution_types::{AccountRevertInit, BlockExecutionOutput, BlockExecutionResult};
    use reth_primitives_traits::SealedBlock;
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use reth_trie::{
        HashedPostState, KeccakKeyHasher, Nibbles, StoredNibbles, StoredNibblesSubKey,
    };
    use revm_database::BundleState;
    use revm_state::AccountInfo;

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
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        provider_rw.insert_block(&data.blocks[0].0).unwrap();
        provider_rw
            .write_state(
                &data.blocks[0].1,
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
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
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
            provider_rw
                .write_state(
                    &data.blocks[i].1,
                    crate::OriginalValuesKnown::No,
                    StateWriteConfig::default(),
                )
                .unwrap();
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
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();

        // insert blocks 1-3 with receipts
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
            provider_rw
                .write_state(
                    &data.blocks[i].1,
                    crate::OriginalValuesKnown::No,
                    StateWriteConfig::default(),
                )
                .unwrap();
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
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
            provider_rw
                .write_state(
                    &data.blocks[i].1,
                    crate::OriginalValuesKnown::No,
                    StateWriteConfig::default(),
                )
                .unwrap();
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
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        for i in 0..3 {
            provider_rw.insert_block(&data.blocks[i].0).unwrap();
            provider_rw
                .write_state(
                    &data.blocks[i].1,
                    crate::OriginalValuesKnown::No,
                    StateWriteConfig::default(),
                )
                .unwrap();
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
            provider_rw
                .write_state(&outcome, crate::OriginalValuesKnown::No, StateWriteConfig::default())
                .unwrap();
            provider_rw.commit().unwrap();
        };

        // Legacy mode (receipts in DB) - should be prunable
        {
            let factory = create_test_provider_factory();
            let storage_settings = StorageSettings::v1();
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
            let storage_settings = StorageSettings::v2();
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

    #[test]
    fn test_try_into_history_rejects_unexecuted_blocks() {
        use reth_storage_api::TryIntoHistoricalStateProvider;

        let factory = create_test_provider_factory();

        // Insert genesis block to have some data
        let data = BlockchainTestData::default();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw
            .write_state(
                &ExecutionOutcome { first_block: 0, receipts: vec![vec![]], ..Default::default() },
                crate::OriginalValuesKnown::No,
                StateWriteConfig::default(),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        // Get a fresh provider - Execution checkpoint is 0, no receipts written beyond genesis
        let provider = factory.provider().unwrap();

        // Requesting historical state for block 0 (executed) should succeed
        let result = provider.try_into_history_at_block(0);
        assert!(result.is_ok(), "Block 0 should be available");

        // Get another provider and request state for block 100 (not executed)
        let provider = factory.provider().unwrap();
        let result = provider.try_into_history_at_block(100);

        // Should fail with BlockNotExecuted error
        match result {
            Err(ProviderError::BlockNotExecuted { requested: 100, .. }) => {}
            Err(e) => panic!("Expected BlockNotExecuted error, got: {e:?}"),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_unwind_storage_hashing_with_hashed_state() {
        let factory = create_test_provider_factory();
        let storage_settings = StorageSettings::v2();
        factory.set_storage_settings_cache(storage_settings);

        let address = Address::random();
        let hashed_address = keccak256(address);

        let plain_slot = B256::random();
        let hashed_slot = keccak256(plain_slot);

        let current_value = U256::from(100);
        let old_value = U256::from(42);

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .tx
            .cursor_dup_write::<tables::HashedStorages>()
            .unwrap()
            .upsert(hashed_address, &StorageEntry { key: hashed_slot, value: current_value })
            .unwrap();

        let changesets = vec![(
            BlockNumberAddress((1, address)),
            StorageEntry { key: plain_slot, value: old_value },
        )];

        let result = provider_rw.unwind_storage_hashing(changesets.into_iter()).unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.contains_key(&hashed_address));
        assert!(result[&hashed_address].contains(&hashed_slot));

        let mut cursor = provider_rw.tx.cursor_dup_read::<tables::HashedStorages>().unwrap();
        let entry = cursor
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap()
            .expect("entry should exist");
        assert_eq!(entry.key, hashed_slot);
        assert_eq!(entry.value, old_value);
    }

    #[test]
    fn test_write_and_remove_state_roundtrip_legacy() {
        let factory = create_test_provider_factory();
        let storage_settings = StorageSettings::v1();
        assert!(!storage_settings.use_hashed_state());
        factory.set_storage_settings_cache(storage_settings);

        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let slot = U256::from(5);
        let slot_key = B256::from(slot);
        let hashed_slot = keccak256(slot_key);

        let mut rng = generators::rng();
        let block0 =
            random_block(&mut rng, 0, BlockParams { tx_count: Some(0), ..Default::default() });
        let block1 =
            random_block(&mut rng, 1, BlockParams { tx_count: Some(0), ..Default::default() });

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw.insert_block(&block0.try_recover().unwrap()).unwrap();
            provider_rw.insert_block(&block1.try_recover().unwrap()).unwrap();
            provider_rw
                .tx
                .cursor_write::<tables::PlainAccountState>()
                .unwrap()
                .upsert(address, &Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None })
                .unwrap();
            provider_rw.commit().unwrap();
        }

        let provider_rw = factory.provider_rw().unwrap();

        let mut state_init: BundleStateInit = AddressMap::default();
        let mut storage_map: B256Map<(U256, U256)> = B256Map::default();
        storage_map.insert(slot_key, (U256::ZERO, U256::from(10)));
        state_init.insert(
            address,
            (
                Some(Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None }),
                Some(Account { nonce: 1, balance: U256::ZERO, bytecode_hash: None }),
                storage_map,
            ),
        );

        let mut reverts_init: RevertsInit = HashMap::default();
        let mut block_reverts: AddressMap<AccountRevertInit> = AddressMap::default();
        block_reverts.insert(
            address,
            (
                Some(Some(Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None })),
                vec![StorageEntry { key: slot_key, value: U256::ZERO }],
            ),
        );
        reverts_init.insert(1, block_reverts);

        let execution_outcome =
            ExecutionOutcome::new_init(state_init, reverts_init, [], vec![vec![]], 1, vec![]);

        provider_rw
            .write_state(
                &execution_outcome,
                OriginalValuesKnown::Yes,
                StateWriteConfig {
                    write_receipts: false,
                    write_account_changesets: true,
                    write_storage_changesets: true,
                },
            )
            .unwrap();

        let hashed_state =
            execution_outcome.hash_state_slow::<reth_trie::KeccakKeyHasher>().into_sorted();
        provider_rw.write_hashed_state(&hashed_state).unwrap();

        let account = provider_rw
            .tx
            .cursor_read::<tables::PlainAccountState>()
            .unwrap()
            .seek_exact(address)
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(account.nonce, 1);

        let storage_entry = provider_rw
            .tx
            .cursor_dup_read::<tables::PlainStorageState>()
            .unwrap()
            .seek_by_key_subkey(address, slot_key)
            .unwrap()
            .unwrap();
        assert_eq!(storage_entry.key, slot_key);
        assert_eq!(storage_entry.value, U256::from(10));

        let hashed_entry = provider_rw
            .tx
            .cursor_dup_read::<tables::HashedStorages>()
            .unwrap()
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap()
            .unwrap();
        assert_eq!(hashed_entry.key, hashed_slot);
        assert_eq!(hashed_entry.value, U256::from(10));

        let account_cs_entries = provider_rw
            .tx
            .cursor_dup_read::<tables::AccountChangeSets>()
            .unwrap()
            .walk(Some(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!account_cs_entries.is_empty());

        let storage_cs_entries = provider_rw
            .tx
            .cursor_read::<tables::StorageChangeSets>()
            .unwrap()
            .walk(Some(BlockNumberAddress((1, address))))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!storage_cs_entries.is_empty());
        assert_eq!(storage_cs_entries[0].1.key, slot_key);

        provider_rw.remove_state_above(0).unwrap();

        let restored_account = provider_rw
            .tx
            .cursor_read::<tables::PlainAccountState>()
            .unwrap()
            .seek_exact(address)
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(restored_account.nonce, 0);

        let storage_gone = provider_rw
            .tx
            .cursor_dup_read::<tables::PlainStorageState>()
            .unwrap()
            .seek_by_key_subkey(address, slot_key)
            .unwrap();
        assert!(storage_gone.is_none() || storage_gone.unwrap().key != slot_key);

        let account_cs_after = provider_rw
            .tx
            .cursor_dup_read::<tables::AccountChangeSets>()
            .unwrap()
            .walk(Some(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(account_cs_after.is_empty());

        let storage_cs_after = provider_rw
            .tx
            .cursor_read::<tables::StorageChangeSets>()
            .unwrap()
            .walk(Some(BlockNumberAddress((1, address))))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(storage_cs_after.is_empty());
    }

    #[test]
    fn test_unwind_storage_hashing_legacy() {
        let factory = create_test_provider_factory();
        let storage_settings = StorageSettings::v1();
        assert!(!storage_settings.use_hashed_state());
        factory.set_storage_settings_cache(storage_settings);

        let address = Address::random();
        let hashed_address = keccak256(address);

        let plain_slot = B256::random();
        let hashed_slot = keccak256(plain_slot);

        let current_value = U256::from(100);
        let old_value = U256::from(42);

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .tx
            .cursor_dup_write::<tables::HashedStorages>()
            .unwrap()
            .upsert(hashed_address, &StorageEntry { key: hashed_slot, value: current_value })
            .unwrap();

        let changesets = vec![(
            BlockNumberAddress((1, address)),
            StorageEntry { key: plain_slot, value: old_value },
        )];

        let result = provider_rw.unwind_storage_hashing(changesets.into_iter()).unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.contains_key(&hashed_address));
        assert!(result[&hashed_address].contains(&hashed_slot));

        let mut cursor = provider_rw.tx.cursor_dup_read::<tables::HashedStorages>().unwrap();
        let entry = cursor
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap()
            .expect("entry should exist");
        assert_eq!(entry.key, hashed_slot);
        assert_eq!(entry.value, old_value);
    }

    #[test]
    fn test_write_state_and_historical_read_hashed() {
        use reth_storage_api::StateProvider;
        use reth_trie::{HashedPostState, KeccakKeyHasher};
        use revm_database::BundleState;
        use revm_state::AccountInfo;

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = Address::with_last_byte(1);
        let slot = U256::from(5);
        let slot_key = B256::from(slot);
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot_key);

        {
            let sf = factory.static_file_provider();
            let mut hw = sf.latest_writer(StaticFileSegment::Headers).unwrap();
            let h0 = alloy_consensus::Header { number: 0, ..Default::default() };
            hw.append_header(&h0, &B256::ZERO).unwrap();
            let h1 = alloy_consensus::Header { number: 1, ..Default::default() };
            hw.append_header(&h1, &B256::ZERO).unwrap();
            hw.commit().unwrap();

            let mut aw = sf.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            aw.append_account_changeset(vec![], 0).unwrap();
            aw.commit().unwrap();

            let mut sw = sf.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            sw.append_storage_changeset(vec![], 0).unwrap();
            sw.commit().unwrap();
        }

        let provider_rw = factory.provider_rw().unwrap();

        let bundle = BundleState::builder(1..=1)
            .state_present_account_info(
                address,
                AccountInfo { nonce: 1, balance: U256::from(10), ..Default::default() },
            )
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(10)))]))
            .revert_account_info(1, address, Some(None))
            .revert_storage(1, address, vec![(slot, U256::ZERO)])
            .build();

        let execution_outcome = ExecutionOutcome::new(bundle.clone(), vec![vec![]], 1, Vec::new());

        provider_rw
            .tx
            .put::<tables::BlockBodyIndices>(
                1,
                StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
            )
            .unwrap();

        provider_rw
            .write_state(
                &execution_outcome,
                OriginalValuesKnown::Yes,
                StateWriteConfig {
                    write_receipts: false,
                    write_account_changesets: true,
                    write_storage_changesets: true,
                },
            )
            .unwrap();

        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state()).into_sorted();
        provider_rw.write_hashed_state(&hashed_state).unwrap();

        let plain_storage_entries = provider_rw
            .tx
            .cursor_dup_read::<tables::PlainStorageState>()
            .unwrap()
            .walk(None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(plain_storage_entries.is_empty());

        let hashed_entry = provider_rw
            .tx
            .cursor_dup_read::<tables::HashedStorages>()
            .unwrap()
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap()
            .unwrap();
        assert_eq!(hashed_entry.key, hashed_slot);
        assert_eq!(hashed_entry.value, U256::from(10));

        provider_rw.static_file_provider().commit().unwrap();

        let sf = factory.static_file_provider();
        let storage_cs = sf.storage_changeset(1).unwrap();
        assert!(!storage_cs.is_empty());
        assert_eq!(storage_cs[0].1.key, slot_key);

        let account_cs = sf.account_block_changeset(1).unwrap();
        assert!(!account_cs.is_empty());
        assert_eq!(account_cs[0].address, address);

        let historical_value =
            HistoricalStateProviderRef::new(&*provider_rw, 0).storage(address, slot_key).unwrap();
        assert_eq!(historical_value, None);
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StorageMode {
        V1,
        V2,
    }

    fn run_save_blocks_and_verify(mode: StorageMode) {
        use alloy_primitives::map::HashMap;

        let factory = create_test_provider_factory();

        match mode {
            StorageMode::V1 => factory.set_storage_settings_cache(StorageSettings::v1()),
            StorageMode::V2 => factory.set_storage_settings_cache(StorageSettings::v2()),
        }

        let num_blocks = 3u64;
        let accounts_per_block = 5usize;
        let slots_per_account = 3usize;

        let genesis = SealedBlock::<reth_ethereum_primitives::Block>::from_sealed_parts(
            SealedHeader::new(
                Header { number: 0, difficulty: U256::from(1), ..Default::default() },
                B256::ZERO,
            ),
            Default::default(),
        );

        let genesis_executed = ExecutedBlock::new(
            Arc::new(genesis.try_recover().unwrap()),
            Arc::new(BlockExecutionOutput {
                result: BlockExecutionResult {
                    receipts: vec![],
                    requests: Default::default(),
                    gas_used: 0,
                    blob_gas_used: 0,
                },
                state: Default::default(),
            }),
            ComputedTrieData::default(),
        );
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.save_blocks(vec![genesis_executed], SaveBlocksMode::Full).unwrap();
        provider_rw.commit().unwrap();

        let mut blocks: Vec<ExecutedBlock> = Vec::new();
        let mut parent_hash = B256::ZERO;

        for block_num in 1..=num_blocks {
            let mut builder = BundleState::builder(block_num..=block_num);

            for acct_idx in 0..accounts_per_block {
                let address = Address::with_last_byte((block_num * 10 + acct_idx as u64) as u8);
                let info = AccountInfo {
                    nonce: block_num,
                    balance: U256::from(block_num * 100 + acct_idx as u64),
                    ..Default::default()
                };

                let storage: HashMap<U256, (U256, U256)> = (1..=slots_per_account as u64)
                    .map(|s| {
                        (
                            U256::from(s + acct_idx as u64 * 100),
                            (U256::ZERO, U256::from(block_num * 1000 + s)),
                        )
                    })
                    .collect();

                let revert_storage: Vec<(U256, U256)> = (1..=slots_per_account as u64)
                    .map(|s| (U256::from(s + acct_idx as u64 * 100), U256::ZERO))
                    .collect();

                builder = builder
                    .state_present_account_info(address, info)
                    .revert_account_info(block_num, address, Some(None))
                    .state_storage(address, storage)
                    .revert_storage(block_num, address, revert_storage);
            }

            let bundle = builder.build();

            let hashed_state =
                HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state()).into_sorted();

            let header = Header {
                number: block_num,
                parent_hash,
                difficulty: U256::from(1),
                ..Default::default()
            };
            let block = SealedBlock::<reth_ethereum_primitives::Block>::seal_parts(
                header,
                Default::default(),
            );
            parent_hash = block.hash();

            let executed = ExecutedBlock::new(
                Arc::new(block.try_recover().unwrap()),
                Arc::new(BlockExecutionOutput {
                    result: BlockExecutionResult {
                        receipts: vec![],
                        requests: Default::default(),
                        gas_used: 0,
                        blob_gas_used: 0,
                    },
                    state: bundle,
                }),
                ComputedTrieData { hashed_state: Arc::new(hashed_state), ..Default::default() },
            );
            blocks.push(executed);
        }

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.save_blocks(blocks, SaveBlocksMode::Full).unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();

        for block_num in 1..=num_blocks {
            for acct_idx in 0..accounts_per_block {
                let address = Address::with_last_byte((block_num * 10 + acct_idx as u64) as u8);
                let hashed_address = keccak256(address);

                let ha_entry = provider
                    .tx_ref()
                    .cursor_read::<tables::HashedAccounts>()
                    .unwrap()
                    .seek_exact(hashed_address)
                    .unwrap();
                assert!(
                    ha_entry.is_some(),
                    "HashedAccounts missing for block {block_num} acct {acct_idx}"
                );

                for s in 1..=slots_per_account as u64 {
                    let slot = U256::from(s + acct_idx as u64 * 100);
                    let slot_key = B256::from(slot);
                    let hashed_slot = keccak256(slot_key);

                    let hs_entry = provider
                        .tx_ref()
                        .cursor_dup_read::<tables::HashedStorages>()
                        .unwrap()
                        .seek_by_key_subkey(hashed_address, hashed_slot)
                        .unwrap();
                    assert!(
                        hs_entry.is_some(),
                        "HashedStorages missing for block {block_num} acct {acct_idx} slot {s}"
                    );
                    let entry = hs_entry.unwrap();
                    assert_eq!(entry.key, hashed_slot);
                    assert_eq!(entry.value, U256::from(block_num * 1000 + s));
                }
            }
        }

        for block_num in 1..=num_blocks {
            let header = provider.header_by_number(block_num).unwrap();
            assert!(header.is_some(), "Header missing for block {block_num}");

            let indices = provider.block_body_indices(block_num).unwrap();
            assert!(indices.is_some(), "BlockBodyIndices missing for block {block_num}");
        }

        let plain_accounts = provider.tx_ref().entries::<tables::PlainAccountState>().unwrap();
        let plain_storage = provider.tx_ref().entries::<tables::PlainStorageState>().unwrap();

        if mode == StorageMode::V2 {
            assert_eq!(plain_accounts, 0, "v2: PlainAccountState should be empty");
            assert_eq!(plain_storage, 0, "v2: PlainStorageState should be empty");

            let mdbx_account_cs = provider.tx_ref().entries::<tables::AccountChangeSets>().unwrap();
            assert_eq!(mdbx_account_cs, 0, "v2: AccountChangeSets in MDBX should be empty");

            let mdbx_storage_cs = provider.tx_ref().entries::<tables::StorageChangeSets>().unwrap();
            assert_eq!(mdbx_storage_cs, 0, "v2: StorageChangeSets in MDBX should be empty");

            provider.static_file_provider().commit().unwrap();
            let sf = factory.static_file_provider();

            for block_num in 1..=num_blocks {
                let account_cs = sf.account_block_changeset(block_num).unwrap();
                assert!(
                    !account_cs.is_empty(),
                    "v2: static file AccountChangeSets should exist for block {block_num}"
                );

                let storage_cs = sf.storage_changeset(block_num).unwrap();
                assert!(
                    !storage_cs.is_empty(),
                    "v2: static file StorageChangeSets should exist for block {block_num}"
                );

                for (_, entry) in &storage_cs {
                    assert!(
                        entry.key != keccak256(entry.key),
                        "v2: static file storage changeset should have plain slot keys"
                    );
                }
            }

            #[cfg(all(unix, feature = "rocksdb"))]
            {
                let rocksdb = factory.rocksdb_provider();
                for block_num in 1..=num_blocks {
                    for acct_idx in 0..accounts_per_block {
                        let address =
                            Address::with_last_byte((block_num * 10 + acct_idx as u64) as u8);
                        let shards = rocksdb.account_history_shards(address).unwrap();
                        assert!(
                            !shards.is_empty(),
                            "v2: RocksDB AccountsHistory missing for block {block_num} acct {acct_idx}"
                        );

                        for s in 1..=slots_per_account as u64 {
                            let slot = U256::from(s + acct_idx as u64 * 100);
                            let slot_key = B256::from(slot);
                            let shards = rocksdb.storage_history_shards(address, slot_key).unwrap();
                            assert!(
                                !shards.is_empty(),
                                "v2: RocksDB StoragesHistory missing for block {block_num} acct {acct_idx} slot {s}"
                            );
                        }
                    }
                }
            }
        } else {
            assert!(plain_accounts > 0, "v1: PlainAccountState should not be empty");
            assert!(plain_storage > 0, "v1: PlainStorageState should not be empty");

            let mdbx_account_cs = provider.tx_ref().entries::<tables::AccountChangeSets>().unwrap();
            assert!(mdbx_account_cs > 0, "v1: AccountChangeSets in MDBX should not be empty");

            let mdbx_storage_cs = provider.tx_ref().entries::<tables::StorageChangeSets>().unwrap();
            assert!(mdbx_storage_cs > 0, "v1: StorageChangeSets in MDBX should not be empty");

            for block_num in 1..=num_blocks {
                let storage_entries: Vec<_> = provider
                    .tx_ref()
                    .cursor_dup_read::<tables::StorageChangeSets>()
                    .unwrap()
                    .walk_range(BlockNumberAddress::range(block_num..=block_num))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert!(
                    !storage_entries.is_empty(),
                    "v1: MDBX StorageChangeSets should have entries for block {block_num}"
                );

                for (_, entry) in &storage_entries {
                    let slot_key = B256::from(entry.key);
                    assert!(
                        slot_key != keccak256(slot_key),
                        "v1: storage changeset keys should be plain (not hashed)"
                    );
                }
            }

            let mdbx_account_history =
                provider.tx_ref().entries::<tables::AccountsHistory>().unwrap();
            assert!(mdbx_account_history > 0, "v1: AccountsHistory in MDBX should not be empty");

            let mdbx_storage_history =
                provider.tx_ref().entries::<tables::StoragesHistory>().unwrap();
            assert!(mdbx_storage_history > 0, "v1: StoragesHistory in MDBX should not be empty");
        }
    }

    #[test]
    fn test_save_blocks_v1_table_assertions() {
        run_save_blocks_and_verify(StorageMode::V1);
    }

    #[test]
    fn test_save_blocks_v2_table_assertions() {
        run_save_blocks_and_verify(StorageMode::V2);
    }

    #[test]
    fn test_write_and_remove_state_roundtrip_v2() {
        let factory = create_test_provider_factory();
        let storage_settings = StorageSettings::v2();
        assert!(storage_settings.use_hashed_state());
        factory.set_storage_settings_cache(storage_settings);

        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let slot = U256::from(5);
        let slot_key = B256::from(slot);
        let hashed_slot = keccak256(slot_key);

        {
            let sf = factory.static_file_provider();
            let mut hw = sf.latest_writer(StaticFileSegment::Headers).unwrap();
            let h0 = alloy_consensus::Header { number: 0, ..Default::default() };
            hw.append_header(&h0, &B256::ZERO).unwrap();
            let h1 = alloy_consensus::Header { number: 1, ..Default::default() };
            hw.append_header(&h1, &B256::ZERO).unwrap();
            hw.commit().unwrap();

            let mut aw = sf.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            aw.append_account_changeset(vec![], 0).unwrap();
            aw.commit().unwrap();

            let mut sw = sf.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            sw.append_storage_changeset(vec![], 0).unwrap();
            sw.commit().unwrap();
        }

        {
            let provider_rw = factory.provider_rw().unwrap();
            provider_rw
                .tx
                .put::<tables::BlockBodyIndices>(
                    0,
                    StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
                )
                .unwrap();
            provider_rw
                .tx
                .put::<tables::BlockBodyIndices>(
                    1,
                    StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
                )
                .unwrap();
            provider_rw
                .tx
                .cursor_write::<tables::HashedAccounts>()
                .unwrap()
                .upsert(
                    hashed_address,
                    &Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None },
                )
                .unwrap();
            provider_rw.commit().unwrap();
        }

        let provider_rw = factory.provider_rw().unwrap();

        let bundle = BundleState::builder(1..=1)
            .state_present_account_info(
                address,
                AccountInfo { nonce: 1, balance: U256::from(10), ..Default::default() },
            )
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(10)))]))
            .revert_account_info(1, address, Some(None))
            .revert_storage(1, address, vec![(slot, U256::ZERO)])
            .build();

        let execution_outcome = ExecutionOutcome::new(bundle.clone(), vec![vec![]], 1, Vec::new());

        provider_rw
            .write_state(
                &execution_outcome,
                OriginalValuesKnown::Yes,
                StateWriteConfig {
                    write_receipts: false,
                    write_account_changesets: true,
                    write_storage_changesets: true,
                },
            )
            .unwrap();

        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state()).into_sorted();
        provider_rw.write_hashed_state(&hashed_state).unwrap();

        let hashed_account = provider_rw
            .tx
            .cursor_read::<tables::HashedAccounts>()
            .unwrap()
            .seek_exact(hashed_address)
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(hashed_account.nonce, 1);

        let hashed_entry = provider_rw
            .tx
            .cursor_dup_read::<tables::HashedStorages>()
            .unwrap()
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap()
            .unwrap();
        assert_eq!(hashed_entry.key, hashed_slot);
        assert_eq!(hashed_entry.value, U256::from(10));

        let plain_accounts = provider_rw.tx.entries::<tables::PlainAccountState>().unwrap();
        assert_eq!(plain_accounts, 0, "v2: PlainAccountState should be empty");

        let plain_storage = provider_rw.tx.entries::<tables::PlainStorageState>().unwrap();
        assert_eq!(plain_storage, 0, "v2: PlainStorageState should be empty");

        provider_rw.static_file_provider().commit().unwrap();

        let sf = factory.static_file_provider();
        let storage_cs = sf.storage_changeset(1).unwrap();
        assert!(!storage_cs.is_empty(), "v2: storage changesets should be in static files");
        assert_eq!(storage_cs[0].1.key, slot_key, "v2: changeset key should be plain");

        provider_rw.remove_state_above(0).unwrap();

        let restored_account = provider_rw
            .tx
            .cursor_read::<tables::HashedAccounts>()
            .unwrap()
            .seek_exact(hashed_address)
            .unwrap();
        assert!(
            restored_account.is_none(),
            "v2: account should be removed (didn't exist before block 1)"
        );

        let storage_gone = provider_rw
            .tx
            .cursor_dup_read::<tables::HashedStorages>()
            .unwrap()
            .seek_by_key_subkey(hashed_address, hashed_slot)
            .unwrap();
        assert!(
            storage_gone.is_none() || storage_gone.unwrap().key != hashed_slot,
            "v2: storage should be reverted (removed or different key)"
        );

        let mdbx_storage_cs = provider_rw.tx.entries::<tables::StorageChangeSets>().unwrap();
        assert_eq!(mdbx_storage_cs, 0, "v2: MDBX StorageChangeSets should remain empty");

        let mdbx_account_cs = provider_rw.tx.entries::<tables::AccountChangeSets>().unwrap();
        assert_eq!(mdbx_account_cs, 0, "v2: MDBX AccountChangeSets should remain empty");
    }

    #[test]
    #[cfg(all(unix, feature = "rocksdb"))]
    fn test_unwind_storage_history_indices_v2() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = Address::with_last_byte(1);
        let slot_key = B256::from(U256::from(42));

        {
            let rocksdb = factory.rocksdb_provider();
            let mut batch = rocksdb.batch();
            batch.append_storage_history_shard(address, slot_key, vec![3u64, 7, 10]).unwrap();
            batch.commit().unwrap();

            let shards = rocksdb.storage_history_shards(address, slot_key).unwrap();
            assert!(!shards.is_empty(), "history should be written to rocksdb");
        }

        let provider_rw = factory.provider_rw().unwrap();

        let changesets = vec![
            (
                BlockNumberAddress((7, address)),
                StorageEntry { key: slot_key, value: U256::from(5) },
            ),
            (
                BlockNumberAddress((10, address)),
                StorageEntry { key: slot_key, value: U256::from(8) },
            ),
        ];

        let count = provider_rw.unwind_storage_history_indices(changesets.into_iter()).unwrap();
        assert_eq!(count, 2);

        provider_rw.commit().unwrap();

        let rocksdb = factory.rocksdb_provider();
        let shards = rocksdb.storage_history_shards(address, slot_key).unwrap();

        assert!(
            !shards.is_empty(),
            "history shards should still exist with block 3 after partial unwind"
        );

        let all_blocks: Vec<u64> = shards.iter().flat_map(|(_, list)| list.iter()).collect();
        assert!(all_blocks.contains(&3), "block 3 should remain");
        assert!(!all_blocks.contains(&7), "block 7 should be unwound");
        assert!(!all_blocks.contains(&10), "block 10 should be unwound");
    }
}
