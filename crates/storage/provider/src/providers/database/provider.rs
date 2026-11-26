use super::{ProviderFactory, ProviderNodeTypes, SaveBlocksInput};
use crate::{
    providers::{
        database::{chain::ChainStorage, metrics, DatabaseProviderMetrics},
        rocksdb::{
            OwnedRocksReadSnapshot, PendingRocksDBBatches, RocksDBProvider, RocksDBWriteCtx,
        },
        static_file::{StaticFileWriteCtx, StaticFileWriter},
        NodeTypesForProvider, StaticFileProvider,
    },
    BlockBodyWriter, BlockExecutionWriter, BlockHashReader, BlockNumReader, BlockReader,
    BlockWriter, ChangeSetReader, DBProvider, DbTxProvider, EitherWriter, EitherWriterDestination,
    HashingWriter, HeaderProvider, HeaderSyncGapProvider, HistoryWriter, LatestStateProviderRef,
    OriginalValuesKnown, PersistenceFrontiers, ProviderError, RawRocksDBBatch, RocksBatchArg,
    RocksDBProviderFactory, StageCheckpointReader, StageCheckpointWriter, StateWriter,
    StaticFileProviderFactory, StatsReader, TransactionsProviderExt, TrieWriter,
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_primitives::{Address, BlockNumber, TxNumber, B256};
use parking_lot::RwLock;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    database::{Database, ReaderTxnTracker},
    models::{StorageSettings, StoredBlockBodyIndices},
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_execution_types::{BlockExecutionOutput, BlockExecutionResult, Chain, ExecutionOutcome};
use reth_node_types::{BlockTy, BodyTy, HeaderTy, NodeTypes, ReceiptTy};
use reth_primitives_traits::{
    BlockBody as _, FastInstant as Instant, RecoveredBlock, SealedHeader,
};
use reth_prune_types::{PruneMode, PruneModes};
use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, NodePrimitivesProvider, StateProvider, StateWriteConfig,
    StorageChangeSetReader, StoragePath, StorageSettingsCache, WriteStateInput,
};
use reth_storage_errors::provider::{ProviderResult, StaticFileWriterError};
use reth_storage_overlay::OverlayContext;
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, SortedTrieData};
use smallvec::SmallVec;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt::Debug,
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use tracing::{debug, instrument, trace};

mod blocks;
mod checkpoints;
mod history;
mod state;

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

impl<DB: Database, N: NodeTypes> DatabaseProviderRW<DB, N> {
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

/// Mode for [`DatabaseProvider::save_blocks_inner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveBlocksMode {
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
    const fn with_state(self) -> bool {
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
    /// `RocksDB` snapshot shared by all history lookups made through this provider.
    ///
    /// It lives as long as the provider, next to the MDBX read transaction, and pins the
    /// `RocksDB` versions its cached iterators were created on for that long; see
    /// [`Self::history_rocksdb_snapshot`].
    rocksdb_history_snapshot: OnceLock<Option<OwnedRocksReadSnapshot>>,
    /// Shared context for resolving trie reverts on write providers. Not needed by read providers.
    overlay_context: Option<OverlayContext<N::Primitives>>,
    /// Task runtime for spawning parallel I/O work.
    runtime: reth_tasks::Runtime,
    /// Path to the database directory.
    db_path: PathBuf,
    /// Pending `RocksDB` batches to be committed at provider commit time.
    pending_rocksdb_batches: PendingRocksDBBatches,
    /// Commit order for database operations.
    commit_order: CommitOrder,
    /// Minimum distance from tip required for pruning
    minimum_pruning_distance: u64,
    /// Database provider metrics
    metrics: Arc<DatabaseProviderMetrics>,
    /// Database handle used to inspect active MDBX readers during unwind commits.
    reader_txn_tracker: Option<Arc<dyn ReaderTxnTracker>>,
}

impl<TX, N: ProviderNodeTypes> DatabaseProvider<TX, N> {
    /// Attaches a transaction to the factory's storage. Only writers need the overlay context.
    pub(super) fn new(
        tx: TX,
        factory: &ProviderFactory<N>,
        commit_order: Option<CommitOrder>,
    ) -> Self {
        Self {
            tx,
            chain_spec: factory.chain_spec.clone(),
            static_file_provider: factory.static_file_provider.clone(),
            prune_modes: factory.prune_modes.clone(),
            storage: factory.storage.clone(),
            storage_settings: factory.storage_settings.clone(),
            rocksdb_provider: factory.rocksdb_provider.clone(),
            overlay_context: commit_order.map(|_| factory.overlay_context().clone()),
            runtime: factory.runtime.clone(),
            db_path: factory.db.path(),
            rocksdb_history_snapshot: OnceLock::new(),
            pending_rocksdb_batches: Default::default(),
            commit_order: commit_order.unwrap_or_default(),
            minimum_pruning_distance: factory.minimum_pruning_distance,
            metrics: factory.database_provider_metrics.clone(),
            reader_txn_tracker: None,
        }
    }
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
            .field("overlay_context", &self.overlay_context)
            .field("runtime", &self.runtime)
            .field("pending_rocksdb_batches", &"<pending batches>")
            .field("commit_order", &self.commit_order)
            .field("minimum_pruning_distance", &self.minimum_pruning_distance)
            .field("reader_txn_tracker", &"<reader txn tracker>")
            .finish()
    }
}

impl<TX, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Returns reference to prune modes.
    pub const fn prune_modes_ref(&self) -> &PruneModes {
        &self.prune_modes
    }

    /// Sets the minimum pruning distance.
    pub const fn with_minimum_pruning_distance(mut self, distance: u64) -> Self {
        self.minimum_pruning_distance = distance;
        self
    }

    /// Attaches reader tracking so unwind commits can wait on active readers.
    pub(crate) fn with_reader_txn_tracker<T>(mut self, reader_txn_tracker: T) -> Self
    where
        T: ReaderTxnTracker + 'static,
    {
        self.reader_txn_tracker = Some(Arc::new(reader_txn_tracker));
        self
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Commits unwind writes in MDBX -> `RocksDB` -> static-file order.
    ///
    /// This keeps MDBX as the first durable step so an interrupted unwind can be recovered by
    /// truncating static files from checkpoints on the next startup. The MDBX commit is synced in
    /// every sync mode, so power loss can't roll it back above the truncated static files.
    ///
    /// This waits after the MDBX commit so readers holding older MDBX-visible views cannot overlap
    /// later cross-store unwind steps. If the wait fails because MDBX stopped answering queries,
    /// the later steps are skipped and the same startup recovery applies.
    ///
    /// Historical `storage_v2` reads ignore `RocksDB` history entries above their MDBX-visible tip,
    /// so no additional post-`RocksDB` wait is needed before static-file commit.
    fn commit_unwind(self) -> ProviderResult<()> {
        let storage_v2 = self.cached_storage_settings().storage_v2;
        let reader_txn_tracker = self.reader_txn_tracker.clone();
        self.tx.commit_durable()?;

        if let Some(reader_txn_tracker) = reader_txn_tracker.as_ref() {
            reader_txn_tracker.wait_for_pre_commit_readers()?;
        }

        if storage_v2 {
            let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
            for batch in batches {
                self.rocksdb_provider.commit_batch(batch)?;
            }
        }

        self.static_file_provider.commit()?;
        Ok(())
    }

    /// State provider for latest state
    pub fn latest<'a>(&'a self) -> Box<dyn StateProvider + 'a> {
        trace!(target: "providers::db", "Returning latest state provider");
        Box::new(LatestStateProviderRef::new(self))
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

    fn set_pending_rocksdb_batch(&self, batch: rocksdb::WriteBatchWithTransaction<true>) {
        self.pending_rocksdb_batches.lock().push(batch);
    }

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
        let rocksdb = self.rocksdb_provider();
        let rocksdb_batch = rocksdb.batch();

        let (result, raw_batch) = f(rocksdb_batch)?;

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
    fn rocksdb_write_ctx(&self, first_block: BlockNumber) -> RocksDBWriteCtx {
        RocksDBWriteCtx {
            first_block_number: first_block,
            prune_tx_lookup: self.prune_modes.transaction_lookup,
            storage_settings: self.cached_storage_settings(),
            pending_batches: self.pending_rocksdb_batches.clone(),
        }
    }

    /// Advances the independent persistence frontiers described by [`SaveBlocksInput`].
    ///
    /// Ordinary block data and hashed-state/trie updates advance independently according to the
    /// ranges derived by the input.
    #[instrument(level = "debug", target = "providers::db", skip_all, fields(block_count = input.persist_rest_blocks().len()))]
    pub fn save_blocks(&self, input: &SaveBlocksInput<N::Primitives>) -> ProviderResult<()> {
        let (db_tip, partial_state_trie) = self
            .get_stage_checkpoint(StageId::Finish)?
            .map(|checkpoint| {
                let partial_state_trie = checkpoint
                    .finish_stage_checkpoint()
                    .and_then(|finish| finish.partial_state_trie())
                    .unwrap_or(checkpoint.block_number);
                (checkpoint.block_number, partial_state_trie)
            })
            .unwrap_or_default();

        if db_tip != input.prev_db_tip() || partial_state_trie != input.prev_partial_state_trie() {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "persistence frontiers do not match Finish checkpoint: expected database/state-trie tips #{}/{}, got #{}/{}",
                input.prev_db_tip(),
                input.prev_partial_state_trie(),
                db_tip,
                partial_state_trie,
            ))))
        }

        self.save_blocks_inner(
            input.persist_rest_blocks(),
            input.state_trie_blocks(),
            input.state_trie_masking_blocks(),
            (input.new_partial_state_trie() < input.new_db_tip())
                .then_some(input.new_partial_state_trie()),
            SaveBlocksMode::Full,
        )
    }

    fn save_blocks_inner(
        &self,
        blocks: &[ExecutedBlock<N::Primitives>],
        state_trie_blocks: &[ExecutedBlock<N::Primitives>],
        state_trie_masking_blocks: &[ExecutedBlock<N::Primitives>],
        partial_state_trie: Option<BlockNumber>,
        save_mode: SaveBlocksMode,
    ) -> ProviderResult<()> {
        let total_start = Instant::now();
        let block_count = blocks.len() as u64;
        // With no new block data, the masking suffix still ends at the database tip. If the
        // masking suffix is empty, the state/trie range ends there instead.
        let last_block_number = blocks
            .last()
            .or_else(|| state_trie_masking_blocks.last())
            .or_else(|| state_trie_blocks.last())
            .expect("at least one persistence range must be non-empty")
            .recovered_block()
            .number();
        let first_number = blocks.first().map(|block| block.recovered_block().number());

        debug!(target: "providers::db", block_count, "Writing blocks and execution data to storage");

        // Compute tx_nums upfront (both threads need these)
        let mut tx_nums: SmallVec<[TxNumber; 4]> = SmallVec::with_capacity(blocks.len());
        if !blocks.is_empty() {
            let first_tx_num = self
                .tx
                .cursor_read::<tables::TransactionBlocks>()?
                .last()?
                .map(|(n, _)| n + 1)
                .unwrap_or_default();
            let mut current = first_tx_num;
            for block in blocks {
                tx_nums.push(current);
                current += block.recovered_block().body().transaction_count() as u64;
            }
        }

        let mut timings =
            metrics::SaveBlocksTimings { batch_size: block_count, ..Default::default() };

        // avoid capturing &self.tx in scope below.
        let sf_provider = &self.static_file_provider;
        let rocksdb_provider = &self.rocksdb_provider;
        let sf_ctx = first_number
            .map(|first_number| {
                self.static_file_write_ctx(save_mode, first_number, last_block_number)
            })
            .transpose()?;
        let rocksdb_ctx = first_number.map(|first_number| self.rocksdb_write_ctx(first_number));
        let rocksdb_enabled =
            rocksdb_ctx.as_ref().is_some_and(|ctx| ctx.storage_settings.storage_v2);

        let mut sf_result = None;
        let mut rocksdb_result = None;

        // Write to all backends in parallel.
        let runtime = &self.runtime;
        // Propagate tracing context into rayon-spawned threads so that static file
        // and RocksDB write spans appear as children of save_blocks in traces.
        let span = tracing::Span::current();
        runtime.storage_pool().in_place_scope(|s| {
            // SF writes
            if sf_ctx.is_some() {
                s.spawn(|_| {
                    let _guard = span.enter();
                    let start = Instant::now();
                    let sf_ctx =
                        sf_ctx.expect("static file context exists when blocks are persisted");
                    sf_result = Some(
                        sf_provider
                            .write_blocks_data(blocks, &tx_nums, sf_ctx, runtime)
                            .map(|()| start.elapsed()),
                    );
                });
            }

            // RocksDB writes
            if rocksdb_enabled {
                s.spawn(|_| {
                    let _guard = span.enter();
                    let start = Instant::now();
                    let rocksdb_ctx =
                        rocksdb_ctx.clone().expect("RocksDB context exists when enabled");
                    rocksdb_result = Some(
                        rocksdb_provider
                            .write_blocks_data(blocks, &tx_nums, rocksdb_ctx, runtime)
                            .map(|()| start.elapsed()),
                    );
                });
            }

            // MDBX writes
            let mdbx_start = Instant::now();

            // Collect all transaction hashes across all blocks, sort them, and write in batch
            if !blocks.is_empty() &&
                !self.cached_storage_settings().storage_v2 &&
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
                    let sf_ctx =
                        sf_ctx.expect("static file context exists when blocks are persisted");

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
            if save_mode.with_state() && !state_trie_blocks.is_empty() {
                let start = Instant::now();
                let batch = ExecutedBlock::hashed_state_refs(state_trie_blocks);
                let mask = ExecutedBlock::hashed_state_refs(state_trie_masking_blocks);
                let merged_hashed_state =
                    HashedPostStateSorted::disjointed_merge_batch(&batch, &mask);
                if !merged_hashed_state.is_empty() {
                    self.write_hashed_state(&merged_hashed_state)?;
                }
                timings.write_hashed_state += start.elapsed();

                let start = Instant::now();
                let batch = ExecutedBlock::trie_updates_refs(state_trie_blocks);
                let mask = ExecutedBlock::trie_updates_refs(state_trie_masking_blocks);
                let merged_trie =
                    Arc::new(TrieUpdatesSorted::disjointed_merge_batch(&batch, &mask));
                if !merged_trie.is_empty() {
                    self.write_trie_updates_sorted(&merged_trie)?;
                }
                timings.write_trie_updates += start.elapsed();
            }

            // Full mode: update history indices
            if save_mode.with_state() &&
                let Some(first_number) = first_number
            {
                let start = Instant::now();
                self.update_history_indices(first_number..=last_block_number)?;
                timings.update_history_indices = start.elapsed();
            }

            // Update pipeline progress
            let start = Instant::now();
            if !blocks.is_empty() {
                self.update_pipeline_stages(last_block_number, false)?;
            }
            if save_mode.with_state() {
                let checkpoint = match partial_state_trie {
                    Some(partial_state_trie) => StageCheckpoint::new(last_block_number)
                        .with_finish_stage_checkpoint(FinishCheckpoint {
                            partial_state_trie: Some(partial_state_trie),
                        }),
                    None => StageCheckpoint::new(last_block_number),
                };
                self.save_stage_checkpoint(StageId::Finish, checkpoint)?;
            }
            timings.update_pipeline_stages = start.elapsed();

            timings.mdbx = mdbx_start.elapsed();

            Ok::<_, ProviderError>(())
        })?;

        // Collect results from spawned tasks
        if !blocks.is_empty() {
            timings.sf = sf_result.ok_or(StaticFileWriterError::ThreadPanic("static file"))??;
        }

        if rocksdb_enabled {
            timings.rocksdb = rocksdb_result.ok_or_else(|| {
                ProviderError::Database(reth_db_api::DatabaseError::Other(
                    "RocksDB thread panicked".into(),
                ))
            })??;
        }

        timings.total = total_start.elapsed();

        self.metrics.record_save_blocks(&timings);
        if let Some(first_number) = first_number {
            debug!(target: "providers::db", range = ?first_number..=last_block_number, "Appended block data");
        }

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
        // Calculate changesets before modifying hashed state used to reconstruct historical tries.
        let overlay_context =
            self.overlay_context.as_ref().ok_or(ProviderError::UnsupportedProvider)?;
        let trie_revert = overlay_context.trie_changesets_for_unwind(self, from)?;

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
}

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
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

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Deletes the transaction, receipt, sender and changeset static files and restarts each
    /// segment after `pivot`, so the next block appended is `pivot + 1`. Headers are kept.
    ///
    /// Each segment gets an empty file anchored at `pivot`. Blocks below it read as expired history
    /// although the pruner never ran, and `pivot` itself reads as missing since nothing is stored
    /// for it.
    ///
    /// CAUTION: destructive. The files are deleted immediately, while the anchor is written on
    /// commit. The caller moves the stage and prune checkpoints to `pivot` in the same commit and
    /// must be able to resume if the process stops between the static file and database commits.
    /// Errors unless storage v2 is enabled.
    pub fn anchor_pruned_static_files(&self, pivot: BlockNumber) -> ProviderResult<()> {
        if !self.cached_storage_settings().storage_v2 {
            return Err(ProviderError::other(StaticFileWriterError::new(
                "pruned anchor requires storage v2",
            )))
        }
        let static_files = self.static_file_provider();
        for segment in StaticFileSegment::iter().filter(|segment| !segment.is_headers()) {
            static_files.delete_segment(segment)?;
            static_files.get_writer(pivot, segment)?.initialize_pruned_anchor(pivot)?;
        }
        // The pivot's own body is never stored, so reads of `pivot` find nothing while blocks
        // below it are reported as expired.
        static_files.set_earliest_history_height(pivot);
        Ok(())
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
        let execution_state = self.take_state_above(block, *range.end())?;

        let blocks = self.recovered_block_range(range)?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove_blocks_above(block)?;

        // Update pipeline progress
        self.update_pipeline_stages_after_unwind(block)?;

        Ok(Chain::new(blocks, execution_state, BTreeMap::new()))
    }

    fn remove_block_and_execution_above(
        &self,
        block: BlockNumber,
    ) -> ProviderResult<PersistenceFrontiers> {
        self.unwind_trie_state_from(block + 1)?;

        // remove execution res
        self.remove_state_above(block)?;

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        self.remove_blocks_above(block)?;

        // Update pipeline progress
        self.update_pipeline_stages_after_unwind(block)
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
            SortedTrieData::default(),
        );

        self.save_blocks_inner(
            std::slice::from_ref(&executed_block),
            &[],
            &[],
            None,
            SaveBlocksMode::BlocksOnly,
        )?;

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
        let unwind_tx_from = self.next_tx_num_after_block(block)?;

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
        let unwind_tx_from = self.next_tx_num_after_block(block)?;

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
                        let key = B256::from(*storage_key);
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

    fn clear_transaction_lookup(&self) -> ProviderResult<()> {
        if self.cached_storage_settings().storage_v2 {
            self.rocksdb_provider.clear::<tables::TransactionHashNumbers>()
        } else {
            self.tx.clear::<tables::TransactionHashNumbers>().map_err(Into::into)
        }
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

impl<TX: DbTx + 'static, N: NodeTypes> DbTxProvider for DatabaseProvider<TX, N> {
    type Tx = TX;

    fn tx(&self) -> &Self::Tx {
        &self.tx
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> DBProvider for DatabaseProvider<TX, N> {
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
        if self.static_file_provider.has_unwind_queued() || self.commit_order.is_unwind() {
            self.commit_unwind()?;
        } else {
            // Normal path: finalize() will call sync_all() if not already synced
            let mut timings = metrics::CommitTimings::default();

            let start = Instant::now();
            self.static_file_provider.finalize()?;
            timings.sf = start.elapsed();

            let start = Instant::now();
            let batches = std::mem::take(&mut *self.pending_rocksdb_batches.lock());
            for batch in batches {
                self.rocksdb_provider.commit_batch(batch)?;
            }
            timings.rocksdb = start.elapsed();

            let start = Instant::now();
            self.tx.commit()?;
            timings.mdbx = start.elapsed();

            self.metrics.record_commit(&timings);
        }

        Ok(())
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
        test_utils::{
            blocks::BlockchainTestData, create_test_provider_factory,
            create_test_provider_factory_with_chain_spec_and_db_args,
            create_test_provider_factory_with_in_memory_state_writer,
        },
        BundleStateInit, ChainStateBlockReader, ChainStateBlockWriter, ReceiptProvider,
        RevertsInit,
    };
    use alloy_consensus::Header;
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{
        keccak256,
        map::{AddressMap, B256Map, HashMap},
        U256,
    };
    use evm2::evm::AccountInfo;
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_chainspec::MAINNET;
    use reth_db::mdbx::{DatabaseArguments, SyncMode};
    use reth_db_api::{
        cursor::DbDupCursorRO,
        models::{AccountBeforeTx, BlockNumberAddress, SnapAttempt, StorageSettings},
    };
    use reth_ethereum_primitives::Receipt;
    use reth_execution_types::{AccountRevertInit, BlockExecutionOutput, BlockExecutionResult};
    use reth_primitives_traits::{Account, SealedBlock, SealedHeader, StorageEntry};
    use reth_storage_api::{DatabaseProviderFactory, DurableBlockReader, MetadataWriter};
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use reth_trie::{
        bundle_state::BundleState, HashedPostState, KeccakKeyHasher, Nibbles, SortedTrieData,
        StoredNibbles, StoredNibblesSubKey,
    };
    use std::{sync::mpsc, time::Duration};

    /// Seeds block zero through the writer core because [`SaveBlocksInput`] only describes
    /// advancing an existing persistence frontier.
    fn save_genesis<TX, N>(
        provider: &DatabaseProvider<TX, N>,
        genesis: &ExecutedBlock<N::Primitives>,
    ) -> ProviderResult<()>
    where
        TX: DbTx + DbTxMut + 'static,
        N: NodeTypesForProvider,
    {
        assert_eq!(genesis.recovered_block().number(), 0);
        provider.save_blocks_inner(
            std::slice::from_ref(genesis),
            std::slice::from_ref(genesis),
            &[],
            None,
            SaveBlocksMode::Full,
        )
    }

    #[test]
    fn snap_attempt_guards_finish_checkpoint_writers() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();
        provider.update_pipeline_stages(5, false).unwrap();
        let mut attempt = SnapAttempt::start(
            None,
            BlockNumHash::new(10, B256::repeat_byte(1)),
            B256::repeat_byte(2),
        );
        provider.write_snap_attempt(&attempt).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_rw().unwrap();
        let refused = provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(10));
        assert!(matches!(refused, Err(ProviderError::UnverifiedSnapState { attempt: 0 })));
        let refused = provider.update_pipeline_stages(10, false);
        assert!(matches!(refused, Err(ProviderError::UnverifiedSnapState { attempt: 0 })));
        for stage in [StageId::Finish, StageId::Headers] {
            assert_eq!(provider.get_stage_checkpoint(stage).unwrap().unwrap().block_number, 5);
        }

        // Rewinding claims nothing about the downloaded state, through either writer.
        provider.update_pipeline_stages(4, true).unwrap();
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(3)).unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap().block_number,
            3
        );

        // Abandoned leftovers are still not complete state.
        attempt.abandon();
        provider.write_snap_attempt(&attempt).unwrap();
        let refused = provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(10));
        assert!(matches!(refused, Err(ProviderError::UnverifiedSnapState { attempt: 0 })));

        // Header progress does not claim the downloaded state is complete.
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(10)).unwrap();
        attempt.verify();
        provider.write_snap_attempt(&attempt).unwrap();
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(10)).unwrap();
        provider.update_pipeline_stages(11, false).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap().block_number,
            11
        );
    }

    #[test]
    fn last_durable_block_num_hash_follows_finish_checkpoint() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();
        let genesis_hash = data.genesis.hash();
        let provider = factory.provider_rw().unwrap();
        provider.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        for (block, _) in &data.blocks {
            provider.insert_block(block).unwrap();
        }
        provider.commit().unwrap();
        let hash = |number: u64| {
            data.blocks.iter().find(|(block, _)| block.number() == number).unwrap().0.hash()
        };

        let provider = factory.provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0)).unwrap();
        assert_eq!(
            provider.last_durable_block_num_hash().unwrap(),
            BlockNumHash::new(0, genesis_hash)
        );

        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(2)).unwrap();
        assert_eq!(provider.last_durable_block_num_hash().unwrap(), BlockNumHash::new(2, hash(2)));

        provider
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(3)
                    .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(1) }),
            )
            .unwrap();
        assert_eq!(provider.last_durable_block_num_hash().unwrap(), BlockNumHash::new(1, hash(1)));
        provider.commit().unwrap();

        assert_eq!(factory.last_durable_block_num_hash().unwrap(), BlockNumHash::new(1, hash(1)));
    }

    #[test]
    fn snap_attempt_guards_partial_state_trie_advances() {
        for abandoned in [false, true] {
            let factory = create_test_provider_factory();
            let provider = factory.database_provider_rw().unwrap();
            provider.update_pipeline_stages(100, false).unwrap();
            let checkpoint = StageCheckpoint::new(100)
                .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(50) });
            provider.save_stage_checkpoint(StageId::Finish, checkpoint).unwrap();
            let mut attempt = SnapAttempt::start(None, BlockNumHash::default(), B256::ZERO);
            if abandoned {
                attempt.abandon();
            }
            provider.write_snap_attempt(&attempt).unwrap();

            for block in [100, 90] {
                let result =
                    provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(block));
                assert!(matches!(result, Err(ProviderError::UnverifiedSnapState { .. })));
                let result = provider.update_pipeline_stages(block, true);
                assert!(matches!(result, Err(ProviderError::UnverifiedSnapState { .. })));
                for stage in [StageId::Finish, StageId::Headers] {
                    assert_eq!(
                        provider.get_stage_checkpoint(stage).unwrap().unwrap().block_number,
                        100
                    );
                }
                assert_eq!(
                    provider.get_stage_checkpoint(StageId::Finish).unwrap(),
                    Some(checkpoint)
                );
            }

            let advanced = StageCheckpoint::new(100)
                .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(60) });
            assert!(matches!(
                provider.save_stage_checkpoint(StageId::Finish, advanced),
                Err(ProviderError::UnverifiedSnapState { .. })
            ));

            // Retaining the partial frontier does not claim additional state progress.
            provider.update_pipeline_stages(90, false).unwrap();
            let rewind = StageCheckpoint::new(80)
                .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(40) });
            provider.save_stage_checkpoint(StageId::Finish, rewind).unwrap();
            provider.update_pipeline_stages(30, true).unwrap();

            attempt.verify();
            provider.write_snap_attempt(&attempt).unwrap();
            provider.save_stage_checkpoint(StageId::Finish, advanced).unwrap();
            provider.update_pipeline_stages(100, true).unwrap();
        }
    }

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
    fn unwind_commit_waits_for_pre_commit_readers() {
        let factory = create_test_provider_factory();

        let reader = factory.provider().unwrap();
        let provider_rw = factory.unwind_provider_rw().unwrap();
        provider_rw.write_metadata("unwind-wait-test", vec![1]).unwrap();
        let (done_tx, done_rx) = mpsc::channel();

        let handle = std::thread::spawn(move || {
            let result = provider_rw.commit();
            done_tx.send(result).unwrap();
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "unwind commit should wait while an older read transaction is still open"
        );

        drop(reader);

        done_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn unwind_commit_syncs_non_durable_db() {
        let factory = create_test_provider_factory_with_chain_spec_and_db_args(
            MAINNET.clone(),
            DatabaseArguments::test().with_sync_mode(Some(SyncMode::SafeNoSync)),
        );
        let info = || factory.db_ref().db().info().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.write_metadata("unwind-sync-test", vec![1]).unwrap();
        provider_rw.commit().unwrap();
        assert!(
            info().last_steady_txnid() < Some(info().last_txnid() as u64),
            "safe-no-sync commits must not be steady"
        );

        let provider_rw = factory.unwind_provider_rw().unwrap();
        provider_rw.write_metadata("unwind-sync-test", vec![2]).unwrap();
        provider_rw.commit().unwrap();
        assert_eq!(
            info().last_steady_txnid(),
            Some(info().last_txnid() as u64),
            "the unwind commit must be steady"
        );
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
        for (block, outcome) in data.blocks.iter().take(3) {
            provider_rw.insert_block(block).unwrap();
            provider_rw
                .write_state(outcome, crate::OriginalValuesKnown::No, StateWriteConfig::default())
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
        for (block, outcome) in data.blocks.iter().take(3) {
            provider_rw.insert_block(block).unwrap();
            provider_rw
                .write_state(outcome, crate::OriginalValuesKnown::No, StateWriteConfig::default())
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
        for (block, outcome) in data.blocks.iter().take(3) {
            provider_rw.insert_block(block).unwrap();
            provider_rw
                .write_state(outcome, crate::OriginalValuesKnown::No, StateWriteConfig::default())
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
        for (block, outcome) in data.blocks.iter().take(3) {
            provider_rw.insert_block(block).unwrap();
            provider_rw
                .write_state(outcome, crate::OriginalValuesKnown::No, StateWriteConfig::default())
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
    fn test_receipts_by_block_returns_none_for_missing_unpruned_receipts() {
        let factory = create_test_provider_factory();
        let data = BlockchainTestData::default();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(&data.genesis.try_recover().unwrap()).unwrap();
        provider_rw.insert_block(&data.blocks[0].0).unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        assert!(provider.receipts_by_block(1.into()).unwrap().is_none());
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
        let storage_address1 = B256::repeat_byte(1u8);
        let storage_address2 = B256::repeat_byte(2u8);
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

            // Add storage nodes for address2.
            storage_cursor
                .upsert(
                    storage_address2,
                    &StorageTrieEntry {
                        nibbles: StoredNibblesSubKey(Nibbles::from_nibbles([0xa, 0xb])),
                        node: BranchNodeCompact::new(
                            0b1100_1100_1100_1100,
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
                            0b0011_1100_0011_1100,
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
            storage_nodes: vec![
                (Nibbles::from_nibbles([0xa, 0xb]), None),
                (Nibbles::from_nibbles([0xc, 0xd]), None),
            ],
        };

        let mut storage_tries = B256Map::default();
        storage_tries.insert(storage_address1, storage_trie1);
        storage_tries.insert(storage_address2, storage_trie2);

        let trie_updates = TrieUpdatesSorted::new(account_nodes, storage_tries);

        // Write the sorted trie updates
        let num_entries = provider_rw.write_trie_updates_sorted(&trie_updates).unwrap();

        // We should have 2 account insertions + 1 account deletion + 1 storage insertion + 3
        // storage deletions = 7
        assert_eq!(num_entries, 7);

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

        // Check storage for address2 was removed
        let storage_entries2: Vec<_> = storage_cursor
            .walk_dup(Some(storage_address2), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(storage_entries2.len(), 0, "Storage address2 should be empty after removal");

        provider_rw.commit().unwrap();
    }

    #[test]
    fn test_save_blocks_only_masks_trie_with_deferred_blocks() {
        use reth_trie::{
            updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
            BranchNodeCompact, HashedPostStateSorted, HashedStorageSorted,
        };

        fn branch(mask: u16) -> BranchNodeCompact {
            BranchNodeCompact::new(mask, 0, 0, vec![], None)
        }

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v1());

        let mut test_block_builder = TestBlockBuilder::eth().with_state();
        let genesis = test_block_builder.get_executed_blocks(0..1).next().unwrap();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..3).collect();

        let provider_rw = factory.provider_rw().unwrap();
        save_genesis(&provider_rw, &genesis).unwrap();
        provider_rw.commit().unwrap();

        let kept_account = B256::with_last_byte(0x11);
        let masked_account = B256::with_last_byte(0x12);
        let kept_storage = B256::with_last_byte(0x21);
        let masked_storage = B256::with_last_byte(0x22);
        let kept_slot = B256::with_last_byte(0x31);
        let masked_slot = B256::with_last_byte(0x32);
        let kept_account_node = Nibbles::from_nibbles([0x1, 0x2]);
        let masked_account_node = Nibbles::from_nibbles([0x1, 0x3]);
        let kept_storage_node = Nibbles::from_nibbles([0x2, 0x1]);
        let masked_storage_node = Nibbles::from_nibbles([0x2, 0x2]);
        let full_persist_base = &blocks[0];
        let deferred_trie_base = &blocks[1];

        let full_persist_hashed_state = HashedPostStateSorted::new(
            vec![
                (kept_account, Some(Account::default())),
                (masked_account, Some(Account { nonce: 1, ..Default::default() })),
            ],
            B256Map::from_iter([
                (kept_storage, HashedStorageSorted { storage_slots: vec![(kept_slot, U256::ONE)] }),
                (
                    masked_storage,
                    HashedStorageSorted { storage_slots: vec![(masked_slot, U256::from(2))] },
                ),
            ]),
        );
        let full_persist_trie_updates = TrieUpdatesSorted::new(
            vec![
                (kept_account_node, Some(branch(0b0000_1111_0000_1111))),
                (masked_account_node, Some(branch(0b1111_0000_1111_0000))),
            ],
            B256Map::from_iter([
                (
                    kept_storage,
                    StorageTrieUpdatesSorted {
                        storage_nodes: vec![(kept_storage_node, Some(branch(0b1010)))],
                    },
                ),
                (
                    masked_storage,
                    StorageTrieUpdatesSorted {
                        storage_nodes: vec![(masked_storage_node, Some(branch(0b0101)))],
                    },
                ),
            ]),
        );

        let full_persist_block = ExecutedBlock::new(
            Arc::clone(&full_persist_base.recovered_block),
            Arc::clone(&full_persist_base.execution_output),
            SortedTrieData::new(
                Arc::new(full_persist_hashed_state),
                Arc::new(full_persist_trie_updates),
            ),
        );

        let deferred_trie_hashed_state = HashedPostStateSorted::new(
            vec![(masked_account, Some(Account { nonce: 3, ..Default::default() }))],
            B256Map::from_iter([(
                masked_storage,
                HashedStorageSorted { storage_slots: vec![(masked_slot, U256::from(4))] },
            )]),
        );
        let deferred_trie_updates = TrieUpdatesSorted::new(
            vec![(masked_account_node, Some(branch(0b0011_0011)))],
            B256Map::from_iter([(
                masked_storage,
                StorageTrieUpdatesSorted {
                    storage_nodes: vec![(masked_storage_node, Some(branch(0b1100)))],
                },
            )]),
        );
        let deferred_trie_block = ExecutedBlock::new(
            Arc::clone(&deferred_trie_base.recovered_block),
            Arc::clone(&deferred_trie_base.execution_output),
            SortedTrieData::new(
                Arc::new(deferred_trie_hashed_state),
                Arc::new(deferred_trie_updates),
            ),
        );

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(
            vec![full_persist_block.clone(), deferred_trie_block.clone()],
            0,
            0,
            2,
            0,
        );
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        let input =
            SaveBlocksInput::new(vec![full_persist_block, deferred_trie_block.clone()], 2, 0, 2, 1);
        assert!(input.persist_rest_blocks().is_empty());
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let tx = provider.tx_ref();
        let finish_checkpoint = provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap();
        assert_eq!(finish_checkpoint.block_number, 2);
        assert_eq!(
            finish_checkpoint.finish_stage_checkpoint().unwrap().partial_state_trie,
            Some(1)
        );
        assert!(provider.block_hash(2).unwrap().is_some());

        let mut hashed_accounts = tx.cursor_read::<tables::HashedAccounts>().unwrap();
        assert!(hashed_accounts.seek_exact(kept_account).unwrap().is_some());
        assert!(hashed_accounts.seek_exact(masked_account).unwrap().is_none());

        let mut hashed_storages = tx.cursor_dup_read::<tables::HashedStorages>().unwrap();
        assert!(hashed_storages.seek_by_key_subkey(kept_storage, kept_slot).unwrap().is_some());
        assert!(hashed_storages
            .walk_dup(Some(masked_storage), None)
            .unwrap()
            .next()
            .transpose()
            .unwrap()
            .is_none());

        let mut account_trie = tx.cursor_read::<tables::AccountsTrie>().unwrap();
        assert!(account_trie.seek_exact(StoredNibbles(kept_account_node)).unwrap().is_some());
        assert!(account_trie.seek_exact(StoredNibbles(masked_account_node)).unwrap().is_none());

        let mut storage_trie = tx.cursor_dup_read::<tables::StoragesTrie>().unwrap();
        let kept_entries: Vec<_> = storage_trie
            .walk_dup(Some(kept_storage), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(kept_entries.len(), 1);
        assert_eq!(kept_entries[0].1.nibbles.0, kept_storage_node);

        let masked_entries: Vec<_> = storage_trie
            .walk_dup(Some(masked_storage), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(masked_entries.is_empty());

        drop(storage_trie);
        drop(account_trie);
        drop(hashed_storages);
        drop(hashed_accounts);
        drop(provider);

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(vec![deferred_trie_block], 2, 1, 2, 2);
        assert!(input.persist_rest_blocks().is_empty());
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let finish_checkpoint = provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap();
        assert_eq!(finish_checkpoint.block_number, 2);
        assert!(finish_checkpoint.finish_stage_checkpoint().is_none());

        let mut hashed_accounts =
            provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        let (_, account) = hashed_accounts.seek_exact(masked_account).unwrap().unwrap();
        assert_eq!(account.nonce, 3);

        let mut hashed_storages =
            provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        let storage =
            hashed_storages.seek_by_key_subkey(masked_storage, masked_slot).unwrap().unwrap();
        assert_eq!(storage.value, U256::from(4));

        let mut account_trie = provider.tx_ref().cursor_read::<tables::AccountsTrie>().unwrap();
        assert!(account_trie.seek_exact(StoredNibbles(masked_account_node)).unwrap().is_some());

        let mut storage_trie = provider.tx_ref().cursor_dup_read::<tables::StoragesTrie>().unwrap();
        let masked_entries: Vec<_> = storage_trie
            .walk_dup(Some(masked_storage), None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(masked_entries.len(), 1);
        assert_eq!(masked_entries[0].1.nibbles.0, masked_storage_node);
    }

    #[test]
    fn test_save_blocks_partial_cycles_do_not_duplicate_static_file_writes() {
        let factory = create_test_provider_factory();
        let mut test_block_builder = TestBlockBuilder::eth().with_state();

        let genesis = test_block_builder.get_executed_blocks(0..1).next().unwrap();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..5).collect();

        let provider_rw = factory.provider_rw().unwrap();
        save_genesis(&provider_rw, &genesis).unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(blocks[..2].to_vec(), 0, 0, 2, 2);
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(blocks[2..].to_vec(), 2, 2, 4, 2);
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        let stale_input = SaveBlocksInput::new(vec![blocks[0].clone()], 0, 0, 1, 1);
        let err = provider_rw.save_blocks(&stale_input).unwrap_err();
        assert!(err.to_string().contains("persistence frontiers do not match Finish checkpoint"));
        drop(provider_rw);

        let provider = factory.provider().unwrap();
        let finish_checkpoint = provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap();
        assert_eq!(finish_checkpoint.block_number, 4);
        assert_eq!(
            finish_checkpoint.finish_stage_checkpoint().unwrap().partial_state_trie,
            Some(2)
        );

        let static_files = factory.static_file_provider();
        assert_eq!(static_files.get_highest_static_file_block(StaticFileSegment::Headers), Some(4));
        assert_eq!(
            static_files.get_highest_static_file_block(StaticFileSegment::Transactions),
            Some(4)
        );
        assert_eq!(
            static_files.get_highest_static_file_block(StaticFileSegment::Receipts),
            Some(4)
        );
    }

    #[test]
    fn unwind_restores_trie_nodes_masked_above_the_frontier() {
        let (factory, mut in_memory_state_writer) =
            create_test_provider_factory_with_in_memory_state_writer(MAINNET.clone());
        let blocks = TestBlockBuilder::eth().get_executed_blocks(0..11).collect::<Vec<_>>();
        let provider = factory.provider_rw().unwrap();
        for block in &blocks {
            provider.insert_block(block.recovered_block()).unwrap();
        }
        for block in &blocks[6..] {
            in_memory_state_writer.insert_executed(block.clone());
        }
        provider
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(10)
                    .with_finish_stage_checkpoint(FinishCheckpoint { partial_state_trie: Some(5) }),
            )
            .unwrap();
        let address = Address::with_last_byte(1);
        // Block 9 deletes storage that existed at block 4. Its masking leaves the trie tables
        // empty despite the persisted frontier being block 5; the changeset restores that storage.
        provider
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                9,
                AccountBeforeTx { address, info: Some(Account { nonce: 1, ..Default::default() }) },
            )
            .unwrap();
        for slot in 0..257 {
            provider
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((9, address)),
                    StorageEntry { key: B256::from(U256::from(slot)), value: U256::from(1) },
                )
                .unwrap();
        }
        provider.commit().unwrap();
        let provider = factory.provider_rw().unwrap();
        assert_eq!(provider.tx_ref().entries::<tables::StoragesTrie>().unwrap(), 0);
        provider.unwind_trie_state_from(9).unwrap();
        assert!(provider.tx_ref().entries::<tables::StoragesTrie>().unwrap() > 0);
    }

    #[test]
    fn remove_block_and_execution_above_returns_persistence_frontiers() {
        let (factory, mut in_memory_state_writer) =
            create_test_provider_factory_with_in_memory_state_writer(MAINNET.clone());
        let mut test_block_builder = TestBlockBuilder::eth().with_state();

        let genesis = test_block_builder.get_executed_blocks(0..1).next().unwrap();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..5).collect();

        let provider_rw = factory.provider_rw().unwrap();
        save_genesis(&provider_rw, &genesis).unwrap();
        provider_rw.commit().unwrap();

        for block in &blocks[2..] {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(blocks, 0, 0, 4, 2);
        provider_rw.save_blocks(&input).unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        let frontiers = provider_rw.remove_block_and_execution_above(3).unwrap();
        assert_eq!(frontiers, PersistenceFrontiers { db_tip: 3, partial_state_trie: 2 });
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let checkpoint = provider.get_stage_checkpoint(StageId::Finish).unwrap().unwrap();
        assert_eq!(
            checkpoint.finish_stage_checkpoint().and_then(|finish| finish.partial_state_trie()),
            Some(2)
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

            assert!(provider.receipts_by_block(0.into()).unwrap().is_none());
            assert!(provider
                .receipts_by_block((tip_block - 1).into())
                .unwrap()
                .is_some_and(|r| r.len() == 1));
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
            for (num, has_receipt) in [(0, false), (1, false), (2, true), (3, true)] {
                let receipts = provider.receipts_by_block(num.into()).unwrap();
                if has_receipt {
                    assert!(receipts.is_some_and(|r| r.len() == 1));
                } else {
                    assert!(receipts.is_none());
                }

                let receipt = provider.receipt(num).unwrap();
                if has_receipt {
                    assert!(receipt.is_some_and(|r| r.cumulative_gas_used == num));
                } else {
                    assert!(receipt.is_none());
                }
            }
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
                .upsert(address, &Account::default())
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
                Some(Account::default()),
                Some(Account { nonce: 1, ..Default::default() }),
                storage_map,
            ),
        );

        let mut reverts_init: RevertsInit = HashMap::default();
        let mut block_reverts: AddressMap<AccountRevertInit> = AddressMap::default();
        block_reverts.insert(
            address,
            (
                Some(Some(Account::default())),
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
    fn test_write_state_hashed() {
        use reth_trie::{HashedPostState, KeccakKeyHasher};

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
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StorageMode {
        V1,
        V2,
    }

    fn run_save_blocks_and_verify(mode: StorageMode) {
        use alloy_primitives::map::{FbBuildHasher, HashMap};

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
                Header { number: 0, difficulty: U256::ONE, ..Default::default() },
                B256::ZERO,
            ),
            Default::default(),
        );

        let genesis_executed: ExecutedBlock = ExecutedBlock::new(
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
            SortedTrieData::default(),
        );
        let provider_rw = factory.provider_rw().unwrap();
        save_genesis(&provider_rw, &genesis_executed).unwrap();
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

                let storage: HashMap<U256, (U256, U256), FbBuildHasher<32>> = (1..=
                    slots_per_account as u64)
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
                difficulty: U256::ONE,
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
                SortedTrieData::new(Arc::new(hashed_state), Default::default()),
            );
            blocks.push(executed);
        }

        let provider_rw = factory.provider_rw().unwrap();
        let input = SaveBlocksInput::new(blocks, 0, 0, num_blocks, num_blocks);
        provider_rw.save_blocks(&input).unwrap();
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

            let rocksdb = factory.rocksdb_provider();
            for block_num in 1..=num_blocks {
                for acct_idx in 0..accounts_per_block {
                    let address = Address::with_last_byte((block_num * 10 + acct_idx as u64) as u8);
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
                .upsert(hashed_address, &Account::default())
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
    fn test_unwind_storage_history_indices_v2() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());

        let address = Address::with_last_byte(1);
        let slot_key = B256::with_last_byte(42);

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

    #[test]
    fn test_chain_state_block_numbers_are_read_by_exact_key() {
        for (finalized, safe) in
            [(None, None), (None, Some(100)), (Some(90), None), (Some(90), Some(100))]
        {
            let factory = create_test_provider_factory();
            let provider = factory.provider_rw().unwrap();
            if let Some(block_number) = finalized {
                provider.save_finalized_block_number(block_number).unwrap();
            }
            if let Some(block_number) = safe {
                provider.save_safe_block_number(block_number).unwrap();
            }

            assert_eq!(provider.last_finalized_block_number().unwrap(), finalized);
            assert_eq!(provider.last_safe_block_number().unwrap(), safe);
            provider.commit().unwrap();

            let provider = factory.provider().unwrap();
            assert_eq!(provider.last_finalized_block_number().unwrap(), finalized);
            assert_eq!(provider.last_safe_block_number().unwrap(), safe);
        }
    }

    #[test]
    fn take_state_outcome_stops_at_execution_tip() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v1());
        let provider = factory.database_provider_rw().unwrap();
        let tx = provider.tx_ref();
        for block in 0..=3 {
            tx.put::<tables::BlockBodyIndices>(
                block,
                StoredBlockBodyIndices { first_tx_num: 0, tx_count: 0 },
            )
            .unwrap();
        }
        // State above the execution tip must still be reverted.
        tx.put::<tables::AccountChangeSets>(
            3,
            AccountBeforeTx { address: Address::ZERO, info: None },
        )
        .unwrap();
        tx.put::<tables::PlainAccountState>(Address::ZERO, Account::default()).unwrap();
        let sf = factory.static_file_provider();
        let mut headers = sf.latest_writer(StaticFileSegment::Headers).unwrap();
        for number in 0..=3 {
            let header = Header { number, ..Default::default() };
            headers.append_header(&header, &header.hash_slow()).unwrap();
        }
        headers.commit().unwrap();
        drop(headers);
        assert_eq!(provider.last_block_number().unwrap(), 3);
        let outcome = provider.take_state_above(1, 2).unwrap();
        assert_eq!(outcome.first_block(), 2);
        assert_eq!(outcome.len(), 1);
        assert_eq!(tx.get::<tables::PlainAccountState>(Address::ZERO).unwrap(), None);
    }
}
