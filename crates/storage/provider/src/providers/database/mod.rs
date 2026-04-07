use crate::{
    providers::{
        state::latest::LatestStateProvider, NodeTypesForProvider, RocksDBProvider,
        StaticFileProvider, StaticFileProviderRWRefMut,
    },
    to_range,
    traits::{BlockSource, ReceiptProvider},
    BlockHashReader, BlockNumReader, BlockReader, ChainSpecProvider, DatabaseProviderFactory,
    EitherWriterDestination, HashedPostStateProvider, HeaderProvider, HeaderSyncGapProvider,
    MetadataProvider, ProviderError, PruneCheckpointReader, RocksDBProviderFactory,
    StageCheckpointReader, StateProviderBox, StaticFileProviderFactory, StaticFileWriter,
    TransactionVariant, TransactionsProvider,
};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, BlockHash, BlockNumber, TxHash, TxNumber, B256};
use core::fmt;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::{database::Database, models::StoredBlockBodyIndices};
use reth_errors::{RethError, RethResult};
use reth_node_types::{
    BlockTy, HeaderTy, NodeTypesWithDB, NodeTypesWithDBAdapter, ReceiptTy, TxTy,
};
use reth_primitives_traits::{RecoveredBlock, SealedHeader};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment, MINIMUM_UNWIND_SAFE_DISTANCE};
use reth_stages_types::{PipelineTarget, StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, ChainStateBlockReader, ChainStateBlockWriter, DBProvider,
    NodePrimitivesProvider, StorageSettings, StorageSettingsCache, TryIntoHistoricalStateProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::HashedPostState;
use reth_trie_db::ChangesetCache;
use revm_database::BundleState;
use std::{
    ops::{RangeBounds, RangeInclusive},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tracing::{info, instrument, trace, warn};

mod provider;
pub use provider::{
    CommitOrder, DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW, SaveBlocksMode,
};

use super::ProviderNodeTypes;
use reth_trie::KeccakKeyHasher;

mod builder;
pub use builder::{ProviderFactoryBuilder, ReadOnlyConfig};

mod metrics;

mod chain;
pub use chain::*;

/// Sync state for read-only [`ProviderFactory`] instances.
struct ReadOnlySyncState {
    /// Last MDBX txn ID we synced `RocksDB` secondary / static file indexes to.
    last_synced_txnid: AtomicU64,
    /// Serializes the slow-path catch-up (`RocksDB` + static file re-init).
    sync_lock: Mutex<()>,
}

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
pub struct ProviderFactory<N: NodeTypesWithDB> {
    /// Database instance
    db: N::DB,
    /// Chain spec
    chain_spec: Arc<N::ChainSpec>,
    /// Static File Provider
    static_file_provider: StaticFileProvider<N::Primitives>,
    /// Optional pruning configuration
    prune_modes: PruneModes,
    /// The node storage handler.
    storage: Arc<N::Storage>,
    /// Storage configuration settings for this node
    storage_settings: Arc<RwLock<StorageSettings>>,
    /// `RocksDB` provider
    rocksdb_provider: RocksDBProvider,
    /// Changeset cache for trie unwinding
    changeset_cache: ChangesetCache,
    /// Task runtime for spawning parallel I/O work.
    runtime: reth_tasks::Runtime,
    /// Minimum distance from tip required before pruning can occur.
    minimum_pruning_distance: u64,
    /// State for on-demand syncing of `RocksDB` secondary and static file indexes.
    ///
    /// Only set for read-only factories. Can be disabled if there is no concurrent read-write
    /// factory writing to the database (e.g as part of a running reth node).
    read_only_sync: Option<Arc<ReadOnlySyncState>>,
}

impl<N: NodeTypesForProvider> ProviderFactory<NodeTypesWithDBAdapter<N, DatabaseEnv>> {
    /// Instantiates the builder for this type
    pub fn builder() -> ProviderFactoryBuilder<N> {
        ProviderFactoryBuilder::default()
    }
}

impl<N: ProviderNodeTypes> ProviderFactory<N> {
    /// Create new database provider factory.
    ///
    /// The storage backends used by the produced factory MAY be inconsistent.
    /// It is recommended to call [`Self::check_consistency`] after
    /// creation to ensure consistency between the database and static files.
    /// If the function returns unwind targets, the caller MUST unwind the
    /// inner database to the minimum of the two targets to ensure consistency.
    pub fn new(
        db: N::DB,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
        runtime: reth_tasks::Runtime,
    ) -> ProviderResult<Self> {
        // Load storage settings from database at init time. Creates a temporary provider
        // to read persisted settings, falling back to legacy defaults if none exist.
        //
        // Both factory and all providers it creates should share these cached settings.
        let legacy_settings = StorageSettings::v1();
        let storage_settings = DatabaseProvider::<_, N>::new(
            db.tx()?,
            chain_spec.clone(),
            static_file_provider.clone(),
            Default::default(),
            Default::default(),
            Arc::new(RwLock::new(legacy_settings)),
            rocksdb_provider.clone(),
            ChangesetCache::new(),
            runtime.clone(),
            db.path(),
        )
        .storage_settings()?
        .unwrap_or(legacy_settings);

        Ok(Self {
            db,
            chain_spec,
            static_file_provider,
            prune_modes: PruneModes::default(),
            storage: Default::default(),
            storage_settings: Arc::new(RwLock::new(storage_settings)),
            rocksdb_provider,
            changeset_cache: ChangesetCache::new(),
            runtime,
            minimum_pruning_distance: MINIMUM_UNWIND_SAFE_DISTANCE,
            read_only_sync: None,
        })
    }

    /// Create new database provider factory and perform consistency checks.
    ///
    /// This will call [`Self::check_consistency`] internally and return
    /// [`ProviderError::MustUnwind`] if inconsistencies are found. It may also
    /// return any [`ProviderError`] that [`Self::new`] may return, or that are
    /// encountered during consistency checks.
    pub fn new_checked(
        db: N::DB,
        chain_spec: Arc<N::ChainSpec>,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
        runtime: reth_tasks::Runtime,
    ) -> ProviderResult<Self> {
        Self::new(db, chain_spec, static_file_provider, rocksdb_provider, runtime)
            .and_then(Self::assert_consistent)
    }
}

impl<N: NodeTypesWithDB> ProviderFactory<N> {
    /// Sets the pruning configuration for an existing [`ProviderFactory`].
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }

    /// Sets the changeset cache for an existing [`ProviderFactory`].
    pub fn with_changeset_cache(mut self, changeset_cache: ChangesetCache) -> Self {
        self.changeset_cache = changeset_cache;
        self
    }

    /// Sets the minimum pruning distance for an existing [`ProviderFactory`].
    ///
    /// This controls the minimum distance from tip required before pruning can occur.
    /// The default is [`MINIMUM_UNWIND_SAFE_DISTANCE`].
    pub const fn with_minimum_pruning_distance(mut self, distance: u64) -> Self {
        self.minimum_pruning_distance = distance;
        self
    }

    /// Enables on-demand syncing of `RocksDB` secondary and static file indexes for read-only
    /// factories. Initializes the tracker to the current MDBX txn ID.
    ///
    /// Should be used for read-only factories that are running concurrently to a reth node writing
    /// new data to the database. Would effectively be a no-op if database directory is unchanged.
    pub fn with_read_only_sync(mut self, watch: bool) -> Self
    where
        N::DB: Database,
    {
        // Initialize to 0 so the first `sync_providers_if_needed` call always
        // triggers a RocksDB/static-file catch-up, regardless of what MDBX txnid
        // the database was at when we opened it.
        let state = Arc::new(ReadOnlySyncState {
            last_synced_txnid: AtomicU64::new(0),
            sync_lock: Mutex::new(()),
        });
        self.read_only_sync = Some(state);

        if watch {
            self.watch_db_directory();
        }
        self
    }

    /// Watches the MDBX data directory for changes and eagerly syncs `RocksDB` secondary and
    /// static file indexes when modifications are detected.
    fn watch_db_directory(&self)
    where
        N::DB: Database,
    {
        let factory = self.clone();
        let db_path = self.db.path();
        reth_tasks::spawn_os_thread("ro-sync", move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = RecommendedWatcher::new(
                move |res| {
                    let _ = tx.send(res);
                },
                notify::Config::default(),
            )
            .expect("failed to create watcher");

            watcher
                .watch(&db_path, RecursiveMode::NonRecursive)
                .expect("failed to watch MDBX path");

            while let Ok(res) = rx.recv() {
                match res {
                    Ok(event) => {
                        if !matches!(
                            event.kind,
                            notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                        ) {
                            continue;
                        }

                        if let Err(err) = factory.sync_providers_if_needed() {
                            warn!(target: "reth::provider", %err, "background ro-sync failed");
                        }
                    }
                    Err(err) => {
                        warn!(target: "reth::provider", ?err, "MDBX directory watcher error");
                    }
                }
            }
        });
    }

    /// For read-only factories, checks whether the MDBX committed txn ID has advanced since the
    /// last sync and, if so, catches up the `RocksDB` secondary instance and re-initializes the
    /// static file index.
    ///
    /// No-op for read-write factories.
    pub fn sync_providers_if_needed(&self) -> ProviderResult<()> {
        let Some(sync_state) = &self.read_only_sync else { return Ok(()) };
        let current_txnid = self.db.last_txnid().unwrap_or(0);

        // Fast path: no contention when nothing changed.
        if current_txnid == sync_state.last_synced_txnid.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Slow path: serialize the actual catch-up I/O.
        let _guard = sync_state.sync_lock.lock().unwrap_or_else(|e| e.into_inner());

        // Double-check after acquiring the lock — another thread may have already synced.
        if current_txnid == sync_state.last_synced_txnid.load(Ordering::Relaxed) {
            return Ok(());
        }

        self.rocksdb_provider.try_catch_up_with_primary()?;
        self.static_file_provider.initialize_index()?;
        sync_state.last_synced_txnid.store(current_txnid, Ordering::Relaxed);
        Ok(())
    }

    /// Returns reference to the underlying database.
    pub const fn db_ref(&self) -> &N::DB {
        &self.db
    }

    #[cfg(any(test, feature = "test-utils"))]
    /// Consumes Self and returns DB
    pub fn into_db(self) -> N::DB {
        self.db
    }
}

impl<N: NodeTypesWithDB> StorageSettingsCache for ProviderFactory<N> {
    fn cached_storage_settings(&self) -> StorageSettings {
        *self.storage_settings.read()
    }

    fn set_storage_settings_cache(&self, settings: StorageSettings) {
        *self.storage_settings.write() = settings;
    }
}

impl<N: NodeTypesWithDB> RocksDBProviderFactory for ProviderFactory<N> {
    fn rocksdb_provider(&self) -> RocksDBProvider {
        self.rocksdb_provider.clone()
    }

    fn set_pending_rocksdb_batch(&self, _batch: rocksdb::WriteBatchWithTransaction<true>) {
        unimplemented!("ProviderFactory is a factory, not a provider - use DatabaseProvider::set_pending_rocksdb_batch instead")
    }

    fn commit_pending_rocksdb_batches(&self) -> ProviderResult<()> {
        unimplemented!("ProviderFactory is a factory, not a provider - use DatabaseProvider::commit_pending_rocksdb_batches instead")
    }
}

impl<N: ProviderNodeTypes<DB = DatabaseEnv>> ProviderFactory<N> {
    /// Create new database provider by passing a path. [`ProviderFactory`] will own the database
    /// instance.
    pub fn new_with_database_path<P: AsRef<Path>>(
        path: P,
        chain_spec: Arc<N::ChainSpec>,
        args: DatabaseArguments,
        static_file_provider: StaticFileProvider<N::Primitives>,
        rocksdb_provider: RocksDBProvider,
        runtime: reth_tasks::Runtime,
    ) -> RethResult<Self> {
        Self::new(
            init_db(path, args).map_err(RethError::msg)?,
            chain_spec,
            static_file_provider,
            rocksdb_provider,
            runtime,
        )
        .map_err(RethError::Provider)
    }
}

impl<N: ProviderNodeTypes> ProviderFactory<N> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`]. This may fail if the inner read database transaction fails to open.
    ///
    /// This sets the [`PruneModes`] to [`None`], because they should only be relevant for writing
    /// data.
    #[track_caller]
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<N::DB, N>> {
        let db_tx = self.db.tx()?;

        // Sync providers after opening the database transaction to make
        // sure that no data is pruned from rocksdb or static files.
        //
        // Reorg logic ensures that no data is pruned from rocksdb or static files while there is an
        // mdbx transaction open that might rely on this data.
        self.sync_providers_if_needed()?;

        Ok(DatabaseProvider::new(
            db_tx,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
            self.storage.clone(),
            self.storage_settings.clone(),
            self.rocksdb_provider.clone(),
            self.changeset_cache.clone(),
            self.runtime.clone(),
            self.db.path(),
        )
        .with_minimum_pruning_distance(self.minimum_pruning_distance))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashReader`].  This may fail if the inner read/write database transaction fails to
    /// open.
    #[track_caller]
    pub fn provider_rw(&self) -> ProviderResult<DatabaseProviderRW<N::DB, N>> {
        Ok(DatabaseProviderRW(
            DatabaseProvider::new_rw(
                self.db.tx_mut()?,
                self.chain_spec.clone(),
                self.static_file_provider.clone(),
                self.prune_modes.clone(),
                self.storage.clone(),
                self.storage_settings.clone(),
                self.rocksdb_provider.clone(),
                self.changeset_cache.clone(),
                self.runtime.clone(),
                self.db.path(),
            )
            .with_reader_txn_tracker(self.db.clone())
            .with_minimum_pruning_distance(self.minimum_pruning_distance),
        ))
    }

    /// Returns a provider with a created `DbTxMut` inside, configured for unwind operations.
    /// Uses unwind commit order (MDBX first, then `RocksDB`, then static files) to allow
    /// recovery by truncating static files on restart if interrupted.
    ///
    /// Unwind commits may wait for pre-existing readers to drain before finishing later
    /// cross-store steps. Drop any long-lived read providers before committing this provider.
    #[track_caller]
    pub fn unwind_provider_rw(
        &self,
    ) -> ProviderResult<DatabaseProvider<<N::DB as Database>::TXMut, N>> {
        Ok(DatabaseProvider::new_unwind_rw(
            self.db.tx_mut()?,
            self.chain_spec.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
            self.storage.clone(),
            self.storage_settings.clone(),
            self.rocksdb_provider.clone(),
            self.changeset_cache.clone(),
            self.runtime.clone(),
            self.db.path(),
        )
        .with_reader_txn_tracker(self.db.clone())
        .with_minimum_pruning_distance(self.minimum_pruning_distance))
    }

    /// State provider for latest block
    #[track_caller]
    pub fn latest(&self) -> ProviderResult<StateProviderBox> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.database_provider_ro()?)))
    }

    /// Storage provider for state at that given block
    pub fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox> {
        let state_provider = self.provider()?.try_into_history_at_block(block_number)?;
        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(state_provider)
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> ProviderResult<StateProviderBox> {
        let provider = self.provider()?;

        let block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        let state_provider = provider.try_into_history_at_block(block_number)?;
        trace!(target: "providers::db", ?block_number, %block_hash, "Returning historical state provider for block hash");
        Ok(state_provider)
    }

    /// Asserts that the static files and database are consistent. If not,
    /// returns [`ProviderError::MustUnwind`] with the appropriate unwind
    /// target. May also return any [`ProviderError`] that
    /// [`Self::check_consistency`] may return.
    pub fn assert_consistent(self) -> ProviderResult<Self> {
        let (rocksdb_unwind, static_file_unwind) = self.check_consistency()?;

        let source = match (rocksdb_unwind, static_file_unwind) {
            (None, None) => return Ok(self),
            (Some(_), Some(_)) => "RocksDB and Static Files",
            (Some(_), None) => "RocksDB",
            (None, Some(_)) => "Static Files",
        };

        Err(ProviderError::MustUnwind {
            data_source: source,
            unwind_to: rocksdb_unwind
                .into_iter()
                .chain(static_file_unwind)
                .min()
                .expect("at least one unwind target must be Some"),
        })
    }

    /// Checks the consistency between the static files and the database. This
    /// may result in static files being pruned or otherwise healed to ensure
    /// consistency. I.e. this MAY result in writes to the static files.
    #[instrument(err, skip(self))]
    pub fn check_consistency(&self) -> ProviderResult<(Option<u64>, Option<u64>)> {
        let provider_ro = self
            .database_provider_ro()?
            // Healing can run long-lived read transactions (e.g., iterating changesets
            // over millions of blocks). Disable the default timeout so MDBX doesn't
            // kill the transaction mid-heal, which causes a crash loop on startup.
            .disable_long_read_transaction_safety();

        // Step 1: heal file-level inconsistencies (no pruning)
        self.static_file_provider().check_file_consistency(&provider_ro)?;

        // Step 2: RocksDB consistency check (needs static files tx data)
        let rocksdb_unwind = self.rocksdb_provider().check_consistency(&provider_ro)?;

        // Step 3: Static file checkpoint consistency (may prune)
        let static_file_unwind = self.static_file_provider().check_consistency(&provider_ro)?.map(
            |target| match target {
                PipelineTarget::Unwind(block) => block,
                PipelineTarget::Sync(_) => unreachable!("check_consistency returns Unwind"),
            },
        );

        // Step 4: Heal finalized/safe block numbers that may be ahead of the
        // highest header on nodes coming from <=1.10.2.
        //
        // Unwinds already set it to the target block.
        if rocksdb_unwind.is_none() && static_file_unwind.is_none() {
            self.heal_chain_state_block_numbers(&provider_ro)?;
        }

        Ok((rocksdb_unwind, static_file_unwind))
    }

    /// If the stored finalized or safe block number is ahead of the highest
    /// header, resets it to the highest header.
    fn heal_chain_state_block_numbers(
        &self,
        provider_ro: &DatabaseProvider<<N::DB as Database>::TX, N>,
    ) -> ProviderResult<()> {
        let highest_header = self.last_block_number()?;

        let finalized = provider_ro.last_finalized_block_number()?;
        let safe = provider_ro.last_safe_block_number()?;

        if finalized.is_none_or(|f| f <= highest_header) && safe.is_none_or(|s| s <= highest_header)
        {
            return Ok(());
        }

        let provider_rw = self.provider_rw()?;

        if let Some(finalized) = finalized.filter(|&f| f > highest_header) {
            info!(
                target: "providers::db",
                finalized,
                highest_header,
                "Healing finalized block number",
            );
            provider_rw.save_finalized_block_number(highest_header)?;
        }

        if let Some(safe) = safe.filter(|&s| s > highest_header) {
            info!(
                target: "providers::db",
                safe,
                highest_header,
                "Healing safe block number",
            );
            provider_rw.save_safe_block_number(highest_header)?;
        }

        provider_rw.commit()?;

        Ok(())
    }

    /// Returns a static file provider. For read-only instances, this will also invoke
    /// [`Self::sync_providers_if_needed`] to make sure that the static file provider is up to date.
    pub fn caught_up_static_file_provider(
        &self,
    ) -> ProviderResult<StaticFileProvider<N::Primitives>> {
        self.sync_providers_if_needed()?;
        Ok(self.static_file_provider.clone())
    }
}

impl<N: NodeTypesWithDB> NodePrimitivesProvider for ProviderFactory<N> {
    type Primitives = N::Primitives;
}

impl<N: ProviderNodeTypes> DatabaseProviderFactory for ProviderFactory<N> {
    type DB = N::DB;
    type Provider = DatabaseProvider<<N::DB as Database>::TX, N>;
    type ProviderRW = DatabaseProvider<<N::DB as Database>::TXMut, N>;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        self.provider()
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        self.provider_rw().map(|provider| provider.0)
    }
}

impl<N: NodeTypesWithDB> StaticFileProviderFactory for ProviderFactory<N> {
    /// Returns static file provider
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        self.static_file_provider.clone()
    }

    fn get_static_file_writer(
        &self,
        block: BlockNumber,
        segment: StaticFileSegment,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, Self::Primitives>> {
        self.static_file_provider.get_writer(block, segment)
    }
}

impl<N: ProviderNodeTypes> HeaderSyncGapProvider for ProviderFactory<N> {
    type Header = HeaderTy<N>;
    fn local_tip_header(
        &self,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<SealedHeader<Self::Header>> {
        self.provider()?.local_tip_header(highest_uninterrupted_block)
    }
}

impl<N: ProviderNodeTypes> HeaderProvider for ProviderFactory<N> {
    type Header = HeaderTy<N>;

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Self::Header>> {
        self.caught_up_static_file_provider()?.header_by_number(num)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        self.caught_up_static_file_provider()?.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        self.caught_up_static_file_provider()?.sealed_header(number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.caught_up_static_file_provider()?.sealed_headers_range(range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        self.caught_up_static_file_provider()?.sealed_headers_while(range, predicate)
    }
}

impl<N: ProviderNodeTypes> BlockHashReader for ProviderFactory<N> {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.caught_up_static_file_provider()?.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.caught_up_static_file_provider()?.canonical_hashes_range(start, end)
    }
}

impl<N: ProviderNodeTypes> BlockNumReader for ProviderFactory<N> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        self.provider()?.chain_info()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        self.provider()?.best_block_number()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.caught_up_static_file_provider()?.last_block_number()
    }

    fn earliest_block_number(&self) -> ProviderResult<BlockNumber> {
        // earliest history height tracks the lowest block number that has __not__ been expired, in
        // other words, the first/earliest available block.
        Ok(self.caught_up_static_file_provider()?.earliest_history_height())
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<N: ProviderNodeTypes> BlockReader for ProviderFactory<N> {
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.pending_block()
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        self.provider()?.pending_block_and_receipts()
    }

    fn recovered_block(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.recovered_block(id, transaction_kind)
    }

    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        self.provider()?.sealed_block_with_senders(id, transaction_kind)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        self.provider()?.block_range(range)
    }

    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.provider()?.block_with_senders_range(range)
    }

    fn recovered_block_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        self.provider()?.recovered_block_range(range)
    }

    fn block_by_transaction_id(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        self.provider()?.block_by_transaction_id(id)
    }
}

impl<N: ProviderNodeTypes> TransactionsProvider for ProviderFactory<N> {
    type Transaction = TxTy<N>;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        self.caught_up_static_file_provider()?.transaction_by_id(id)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        self.caught_up_static_file_provider()?.transaction_by_id_unhashed(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        self.provider()?.transactions_by_block_range(range)
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        self.caught_up_static_file_provider()?.transactions_by_tx_range(range)
    }

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.caught_up_static_file_provider()?.senders_by_tx_range(range)
        } else {
            self.provider()?.senders_by_tx_range(range)
        }
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        if EitherWriterDestination::senders(self).is_static_file() {
            self.caught_up_static_file_provider()?.transaction_sender(id)
        } else {
            self.provider()?.transaction_sender(id)
        }
    }
}

impl<N: ProviderNodeTypes> ReceiptProvider for ProviderFactory<N> {
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        self.caught_up_static_file_provider()?.get_with_static_file_or_database(
            StaticFileSegment::Receipts,
            id,
            |static_file| static_file.receipt(id),
            || self.provider()?.receipt(id),
        )
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }

    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        self.caught_up_static_file_provider()?.get_range_with_static_file_or_database(
            StaticFileSegment::Receipts,
            to_range(range),
            |static_file, range, _| static_file.receipts_by_tx_range(range),
            |range, _| self.provider()?.receipts_by_tx_range(range),
            |_| true,
        )
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        self.provider()?.receipts_by_block_range(block_range)
    }
}

impl<N: ProviderNodeTypes> BlockBodyIndicesProvider for ProviderFactory<N> {
    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(number)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices_range(range)
    }
}

impl<N: ProviderNodeTypes> StageCheckpointReader for ProviderFactory<N> {
    fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(&self, id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_stage_checkpoint_progress(id)
    }
    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.provider()?.get_all_checkpoints()
    }
}

impl<N: NodeTypesWithDB> ChainSpecProvider for ProviderFactory<N> {
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<N::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<N: ProviderNodeTypes> PruneCheckpointReader for ProviderFactory<N> {
    fn get_prune_checkpoint(
        &self,
        segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        self.provider()?.get_prune_checkpoint(segment)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        self.provider()?.get_prune_checkpoints()
    }
}

impl<N: ProviderNodeTypes> HashedPostStateProvider for ProviderFactory<N> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<N: ProviderNodeTypes> MetadataProvider for ProviderFactory<N> {
    fn get_metadata(&self, key: &str) -> ProviderResult<Option<Vec<u8>>> {
        self.provider()?.get_metadata(key)
    }
}

impl<N> fmt::Debug for ProviderFactory<N>
where
    N: NodeTypesWithDB<DB: fmt::Debug, ChainSpec: fmt::Debug, Storage: fmt::Debug>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            db,
            chain_spec,
            static_file_provider,
            prune_modes,
            storage,
            storage_settings,
            rocksdb_provider,
            changeset_cache,
            runtime,
            minimum_pruning_distance,
            read_only_sync,
        } = self;
        f.debug_struct("ProviderFactory")
            .field("db", &db)
            .field("chain_spec", &chain_spec)
            .field("static_file_provider", &static_file_provider)
            .field("prune_modes", &prune_modes)
            .field("storage", &storage)
            .field("storage_settings", &*storage_settings.read())
            .field("rocksdb_provider", &rocksdb_provider)
            .field("changeset_cache", &changeset_cache)
            .field("runtime", &runtime)
            .field("minimum_pruning_distance", &minimum_pruning_distance)
            .field(
                "read_only_sync",
                &read_only_sync.as_ref().map(|s| s.last_synced_txnid.load(Ordering::Relaxed)),
            )
            .finish()
    }
}

impl<N: NodeTypesWithDB> Clone for ProviderFactory<N> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            chain_spec: self.chain_spec.clone(),
            static_file_provider: self.static_file_provider.clone(),
            prune_modes: self.prune_modes.clone(),
            storage: self.storage.clone(),
            storage_settings: self.storage_settings.clone(),
            rocksdb_provider: self.rocksdb_provider.clone(),
            changeset_cache: self.changeset_cache.clone(),
            runtime: self.runtime.clone(),
            minimum_pruning_distance: self.minimum_pruning_distance,
            read_only_sync: self.read_only_sync.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{StaticFileProvider, StaticFileWriter},
        test_utils::{blocks::TEST_BLOCK, create_test_provider_factory, MockNodeTypesWithDB},
        BlockHashReader, BlockNumReader, BlockWriter, DBProvider, HeaderSyncGapProvider,
        TransactionsProvider,
    };
    use alloy_primitives::{TxNumber, B256};
    use assert_matches::assert_matches;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db::{
        mdbx::DatabaseArguments,
        test_utils::{create_test_rocksdb_dir, create_test_static_files_dir, ERROR_TEMPDIR},
    };
    use reth_db_api::tables;
    use reth_primitives_traits::SignerRecoverable;
    use reth_prune_types::{PruneMode, PruneModes};
    use reth_storage_errors::provider::ProviderError;
    use reth_testing_utils::generators::{self, random_block, random_header, BlockParams};
    use std::{ops::RangeInclusive, sync::Arc};

    #[test]
    fn common_history_provider() {
        let factory = create_test_provider_factory();
        let _ = factory.latest();
    }

    #[test]
    fn default_chain_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, B256::ZERO);
    }

    #[test]
    fn provider_flow() {
        let factory = create_test_provider_factory();
        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn provider_factory_with_database_path() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let (_rocksdb_dir, rocksdb_path) = create_test_rocksdb_dir();
        let _db_tempdir = tempfile::TempDir::new().expect(ERROR_TEMPDIR);
        let factory = ProviderFactory::<MockNodeTypesWithDB<DatabaseEnv>>::new_with_database_path(
            _db_tempdir.path(),
            Arc::new(chain_spec),
            DatabaseArguments::new(Default::default()),
            StaticFileProvider::read_write(static_dir_path).unwrap(),
            RocksDBProvider::builder(&rocksdb_path).build().unwrap(),
            reth_tasks::Runtime::test(),
        )
        .unwrap();
        let provider = factory.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }

    #[test]
    fn insert_block_with_prune_modes() {
        let block = TEST_BLOCK.clone();

        {
            let factory = create_test_provider_factory();
            let provider = factory.provider_rw().unwrap();
            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));
            assert_matches!(
                provider.transaction_sender(0), Ok(Some(sender))
                if sender == block.body().transactions[0].recover_signer().unwrap()
            );
            assert_matches!(
                provider.transaction_id(*block.body().transactions[0].tx_hash()),
                Ok(Some(0))
            );
        }

        {
            let prune_modes = PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: Some(PruneMode::Full),
                ..PruneModes::default()
            };
            // Keep factory alive until provider is dropped to prevent TempDatabase cleanup
            let factory = create_test_provider_factory().with_prune_modes(prune_modes);
            let provider = factory.provider_rw().unwrap();
            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));
            assert_matches!(provider.transaction_sender(0), Ok(None));
            assert_matches!(
                provider.transaction_id(*block.body().transactions[0].tx_hash()),
                Ok(None)
            );
        }
    }

    #[test]
    fn take_block_transaction_range_recover_senders() {
        let mut rng = generators::rng();
        let block =
            random_block(&mut rng, 0, BlockParams { tx_count: Some(3), ..Default::default() });

        let tx_ranges: Vec<RangeInclusive<TxNumber>> = vec![0..=0, 1..=1, 2..=2, 0..=1, 1..=2];
        for range in tx_ranges {
            let factory = create_test_provider_factory();
            let provider = factory.provider_rw().unwrap();

            assert_matches!(provider.insert_block(&block.clone().try_recover().unwrap()), Ok(_));

            let senders = provider.take::<tables::TransactionSenders>(range.clone()).unwrap();
            assert_eq!(
                senders,
                range
                    .clone()
                    .map(|tx_number| (
                        tx_number,
                        block.body().transactions[tx_number as usize].recover_signer().unwrap()
                    ))
                    .collect::<Vec<_>>()
            );

            let db_senders = provider.senders_by_tx_range(range);
            assert!(matches!(db_senders, Ok(ref v) if v.is_empty()));
        }
    }

    #[test]
    fn header_sync_gap_lookup() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let mut rng = generators::rng();

        // Genesis
        let checkpoint = 0;
        let head = random_header(&mut rng, 0, None);

        // Empty database
        assert_matches!(
            provider.local_tip_header(checkpoint),
            Err(ProviderError::HeaderNotFound(block_number))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer =
            static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        static_file_writer.append_header(head.header(), &head.hash()).unwrap();
        static_file_writer.commit().unwrap();
        drop(static_file_writer);

        let local_head = provider.local_tip_header(checkpoint).unwrap();

        assert_eq!(local_head, head);
    }
}
