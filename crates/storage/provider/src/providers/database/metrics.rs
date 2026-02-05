use metrics::{Gauge, Histogram};
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct DurationsRecorder<'a> {
    start: Instant,
    current_metrics: &'a DatabaseProviderMetrics,
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Option<Duration>,
}

impl<'a> DurationsRecorder<'a> {
    /// Creates a new durations recorder with the given metrics instance.
    pub(crate) fn new(metrics: &'a DatabaseProviderMetrics) -> Self {
        Self { start: Instant::now(), actions: Vec::new(), latest: None, current_metrics: metrics }
    }
}

impl<'a> DurationsRecorder<'a> {
    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: Action) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        self.current_metrics.record_duration(action, duration);
        self.latest = Some(elapsed);
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum Action {
    InsertBlock,
    InsertState,
    InsertHashes,
    InsertHistoryIndices,
    UpdatePipelineStages,
    InsertHeaderNumbers,
    InsertBlockBodyIndices,
    InsertTransactionBlocks,
    InsertTransactionSenders,
    InsertTransactionHashNumbers,
    #[cfg(feature = "edge")]
    EdgeWritePlainAccounts,
    #[cfg(feature = "edge")]
    EdgeWriteBytecodes,
    #[cfg(feature = "edge")]
    EdgeWritePlainStorage,
    #[cfg(feature = "edge")]
    EdgeWriteHashedAccounts,
    #[cfg(feature = "edge")]
    EdgeWriteHashedStorages,
    #[cfg(feature = "edge")]
    EdgeWriteAccountTrie,
    #[cfg(feature = "edge")]
    EdgeWriteStorageTrie,
}

/// Database provider metrics
#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
pub(crate) struct DatabaseProviderMetrics {
    /// Duration of insert block
    insert_block: Histogram,
    /// Duration of insert state
    insert_state: Histogram,
    /// Duration of insert hashes
    insert_hashes: Histogram,
    /// Duration of insert history indices
    insert_history_indices: Histogram,
    /// Duration of update pipeline stages
    update_pipeline_stages: Histogram,
    /// Duration of insert header numbers
    insert_header_numbers: Histogram,
    /// Duration of insert block body indices
    insert_block_body_indices: Histogram,
    /// Duration of insert transaction blocks
    insert_tx_blocks: Histogram,
    /// Duration of insert transaction senders
    insert_transaction_senders: Histogram,
    /// Duration of insert transaction hash numbers
    insert_transaction_hash_numbers: Histogram,
    /// Duration of `save_blocks`
    save_blocks_total: Histogram,
    /// Duration of MDBX work in `save_blocks`
    save_blocks_mdbx: Histogram,
    /// Duration of static file work in `save_blocks`
    save_blocks_sf: Histogram,
    /// Duration of `RocksDB` work in `save_blocks`
    save_blocks_rocksdb: Histogram,
    /// Duration of `insert_block` in `save_blocks`
    save_blocks_insert_block: Histogram,
    /// Duration of `write_state` in `save_blocks`
    save_blocks_write_state: Histogram,
    /// Duration of `write_hashed_state` in `save_blocks`
    save_blocks_write_hashed_state: Histogram,
    /// Duration of `write_trie_updates` in `save_blocks`
    save_blocks_write_trie_updates: Histogram,
    /// Duration of `update_history_indices` in `save_blocks`
    save_blocks_update_history_indices: Histogram,
    /// Duration of `update_pipeline_stages` in `save_blocks`
    save_blocks_update_pipeline_stages: Histogram,
    /// Number of blocks per `save_blocks` call
    save_blocks_block_count: Histogram,
    /// Duration of MDBX commit in `save_blocks`
    save_blocks_commit_mdbx: Histogram,
    /// Duration of static file commit in `save_blocks`
    save_blocks_commit_sf: Histogram,
    /// Duration of `RocksDB` commit in `save_blocks`
    save_blocks_commit_rocksdb: Histogram,
    /// Last duration of `save_blocks`
    save_blocks_total_last: Gauge,
    /// Last duration of MDBX work in `save_blocks`
    save_blocks_mdbx_last: Gauge,
    /// Last duration of static file work in `save_blocks`
    save_blocks_sf_last: Gauge,
    /// Last duration of `RocksDB` work in `save_blocks`
    save_blocks_rocksdb_last: Gauge,
    /// Last duration of `insert_block` in `save_blocks`
    save_blocks_insert_block_last: Gauge,
    /// Last duration of `write_state` in `save_blocks`
    save_blocks_write_state_last: Gauge,
    /// Last duration of `write_hashed_state` in `save_blocks`
    save_blocks_write_hashed_state_last: Gauge,
    /// Last duration of `write_trie_updates` in `save_blocks`
    save_blocks_write_trie_updates_last: Gauge,
    /// Last duration of `update_history_indices` in `save_blocks`
    save_blocks_update_history_indices_last: Gauge,
    /// Last duration of `update_pipeline_stages` in `save_blocks`
    save_blocks_update_pipeline_stages_last: Gauge,
    /// Last number of blocks per `save_blocks` call
    save_blocks_block_count_last: Gauge,
    /// Last duration of MDBX commit in `save_blocks`
    save_blocks_commit_mdbx_last: Gauge,
    /// Last duration of static file commit in `save_blocks`
    save_blocks_commit_sf_last: Gauge,
    /// Last duration of `RocksDB` commit in `save_blocks`
    save_blocks_commit_rocksdb_last: Gauge,
    // Edge mode parallel write metrics
    /// Duration of PlainAccountState writes
    edge_write_plain_accounts: Histogram,
    /// Last duration of PlainAccountState writes
    edge_write_plain_accounts_last: Gauge,
    /// Duration of Bytecodes writes
    edge_write_bytecodes: Histogram,
    /// Last duration of Bytecodes writes
    edge_write_bytecodes_last: Gauge,
    /// Duration of PlainStorageState writes
    edge_write_plain_storage: Histogram,
    /// Last duration of PlainStorageState writes
    edge_write_plain_storage_last: Gauge,
    /// Duration of HashedAccounts writes
    edge_write_hashed_accounts: Histogram,
    /// Last duration of HashedAccounts writes
    edge_write_hashed_accounts_last: Gauge,
    /// Duration of HashedStorages writes
    edge_write_hashed_storages: Histogram,
    /// Last duration of HashedStorages writes
    edge_write_hashed_storages_last: Gauge,
    /// Duration of AccountsTrie writes
    edge_write_account_trie: Histogram,
    /// Last duration of AccountsTrie writes
    edge_write_account_trie_last: Gauge,
    /// Duration of StoragesTrie writes
    edge_write_storage_trie: Histogram,
    /// Last duration of StoragesTrie writes
    edge_write_storage_trie_last: Gauge,
    /// Duration of preprocessing (merging, sorting, converting)
    edge_preprocessing: Histogram,
    /// Last duration of preprocessing
    edge_preprocessing_last: Gauge,
    /// Wall-clock time for parallel writes only (excludes preprocessing)
    edge_parallel_wall: Histogram,
    /// Last wall-clock time for parallel writes
    edge_parallel_wall_last: Gauge,
    /// Total edge mode time including preprocessing
    edge_parallel_writes_total: Histogram,
    /// Last total edge mode time
    edge_parallel_writes_total_last: Gauge,
    /// Number of parallel subtxns used
    edge_parallel_subtxn_count: Histogram,
    /// Last number of parallel subtxns used
    edge_parallel_subtxn_count_last: Gauge,
}

/// Per-table arena hint metrics for tracking estimation quality.
#[derive(Debug)]
#[cfg(feature = "edge")]
pub(crate) struct ArenaHintMetrics {
    handles: std::collections::HashMap<&'static str, ArenaHintTableMetrics>,
}

#[cfg(feature = "edge")]
impl Default for ArenaHintMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "edge")]
impl ArenaHintMetrics {
    pub(crate) fn new() -> Self {
        use reth_db::tables;
        use reth_db_api::table::Table;
        let tables = [
            tables::PlainAccountState::NAME,
            tables::PlainStorageState::NAME,
            tables::Bytecodes::NAME,
            tables::HashedAccounts::NAME,
            tables::HashedStorages::NAME,
            tables::AccountsTrie::NAME,
            tables::StoragesTrie::NAME,
        ];

        let handles =
            tables.into_iter().map(|name| (name, ArenaHintTableMetrics::new(name))).collect();

        Self { handles }
    }

    pub(crate) fn record(&self, table: &'static str, detail: &super::ArenaHintDetail) {
        if let Some(metrics) = self.handles.get(&table) {
            ArenaHintTableMetrics::record(metrics, detail);
        }
    }
}

#[cfg(feature = "edge")]
#[derive(Debug)]
struct ArenaHintTableMetrics {
    estimated: Gauge,
    used: Gauge,
    source: Gauge,
}

#[cfg(feature = "edge")]
impl ArenaHintTableMetrics {
    fn new(table: &'static str) -> Self {
        Self {
            estimated: metrics::gauge!("database_edge_arena_hint_estimated", "table" => table),
            used: metrics::gauge!("database_edge_arena_hint_used", "table" => table),
            source: metrics::gauge!("database_edge_arena_hint_source", "table" => table),
        }
    }

    fn record(&self, detail: &super::ArenaHintDetail) {
        self.estimated.set(detail.estimated as f64);
        self.used.set(detail.used as f64);
        self.source.set(detail.source.as_f64());
    }
}

/// Raw input counts used for arena hint estimation.
/// These metrics enable correlation between inputs and actual page demand.
#[derive(Debug, Default, Clone, Copy)]
#[cfg(feature = "edge")]
pub(crate) struct ArenaHintInputs {
    /// Number of account changes in batch
    pub num_accounts: usize,
    /// Total number of storage slot changes across all addresses
    pub num_storage: usize,
    /// Number of new contracts
    pub num_contracts: usize,
    /// Number of account trie node updates
    pub num_account_trie_nodes: usize,
    /// Number of storage trie node updates (summed across all addresses)
    pub num_storage_trie_nodes: usize,
    /// Number of unique addresses with storage trie updates
    pub num_storage_trie_addresses: usize,
}

/// Metrics for recording arena hint estimation inputs.
#[derive(Debug)]
#[cfg(feature = "edge")]
pub(crate) struct ArenaHintInputMetrics {
    num_accounts: Gauge,
    num_storage: Gauge,
    num_contracts: Gauge,
    num_account_trie_nodes: Gauge,
    num_storage_trie_nodes: Gauge,
    num_storage_trie_addresses: Gauge,
}

#[cfg(feature = "edge")]
impl Default for ArenaHintInputMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "edge")]
impl ArenaHintInputMetrics {
    pub(crate) fn new() -> Self {
        Self {
            num_accounts: metrics::gauge!("database_edge_input_num_accounts"),
            num_storage: metrics::gauge!("database_edge_input_num_storage"),
            num_contracts: metrics::gauge!("database_edge_input_num_contracts"),
            num_account_trie_nodes: metrics::gauge!("database_edge_input_num_account_trie_nodes"),
            num_storage_trie_nodes: metrics::gauge!("database_edge_input_num_storage_trie_nodes"),
            num_storage_trie_addresses: metrics::gauge!(
                "database_edge_input_num_storage_trie_addresses"
            ),
        }
    }

    pub(crate) fn record(&self, inputs: &ArenaHintInputs) {
        self.num_accounts.set(inputs.num_accounts as f64);
        self.num_storage.set(inputs.num_storage as f64);
        self.num_contracts.set(inputs.num_contracts as f64);
        self.num_account_trie_nodes.set(inputs.num_account_trie_nodes as f64);
        self.num_storage_trie_nodes.set(inputs.num_storage_trie_nodes as f64);
        self.num_storage_trie_addresses.set(inputs.num_storage_trie_addresses as f64);
    }
}

/// Timings collected during a `save_blocks` call.
#[derive(Debug, Default)]
pub(crate) struct SaveBlocksTimings {
    pub total: Duration,
    pub mdbx: Duration,
    pub sf: Duration,
    pub rocksdb: Duration,
    pub insert_block: Duration,
    pub write_state: Duration,
    pub write_hashed_state: Duration,
    pub write_trie_updates: Duration,
    pub update_history_indices: Duration,
    pub update_pipeline_stages: Duration,
    pub block_count: u64,
}

/// Timings collected during a `commit` call.
#[derive(Debug, Default)]
pub(crate) struct CommitTimings {
    pub mdbx: Duration,
    pub sf: Duration,
    pub rocksdb: Duration,
}

/// Timings collected during edge mode parallel writes.
#[cfg(feature = "edge")]
#[derive(Debug, Default)]
pub(crate) struct EdgeWriteTimings {
    /// Duration of preprocessing (merging states, sorting, converting)
    pub preprocessing: Duration,
    pub plain_accounts: Duration,
    pub bytecodes: Duration,
    pub plain_storage: Duration,
    pub hashed_accounts: Duration,
    pub hashed_storages: Duration,
    pub account_trie: Duration,
    pub storage_trie: Duration,
    /// Wall-clock time for parallel writes only (excludes preprocessing)
    pub parallel_wall: Duration,
    /// Total time including preprocessing
    pub total: Duration,
    pub subtxn_count: u64,
}

impl DatabaseProviderMetrics {
    /// Records the duration for the given action.
    pub(crate) fn record_duration(&self, action: Action, duration: Duration) {
        match action {
            Action::InsertBlock => self.insert_block.record(duration),
            Action::InsertState => self.insert_state.record(duration),
            Action::InsertHashes => self.insert_hashes.record(duration),
            Action::InsertHistoryIndices => self.insert_history_indices.record(duration),
            Action::UpdatePipelineStages => self.update_pipeline_stages.record(duration),
            Action::InsertHeaderNumbers => self.insert_header_numbers.record(duration),
            Action::InsertBlockBodyIndices => self.insert_block_body_indices.record(duration),
            Action::InsertTransactionBlocks => self.insert_tx_blocks.record(duration),
            Action::InsertTransactionSenders => self.insert_transaction_senders.record(duration),
            Action::InsertTransactionHashNumbers => {
                self.insert_transaction_hash_numbers.record(duration)
            }
            #[cfg(feature = "edge")]
            Action::EdgeWritePlainAccounts => self.edge_write_plain_accounts.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWriteBytecodes => self.edge_write_bytecodes.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWritePlainStorage => self.edge_write_plain_storage.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWriteHashedAccounts => self.edge_write_hashed_accounts.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWriteHashedStorages => self.edge_write_hashed_storages.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWriteAccountTrie => self.edge_write_account_trie.record(duration),
            #[cfg(feature = "edge")]
            Action::EdgeWriteStorageTrie => self.edge_write_storage_trie.record(duration),
        }
    }

    /// Records all `save_blocks` timings.
    pub(crate) fn record_save_blocks(&self, timings: &SaveBlocksTimings) {
        self.save_blocks_total.record(timings.total);
        self.save_blocks_mdbx.record(timings.mdbx);
        self.save_blocks_sf.record(timings.sf);
        self.save_blocks_rocksdb.record(timings.rocksdb);
        self.save_blocks_insert_block.record(timings.insert_block);
        self.save_blocks_write_state.record(timings.write_state);
        self.save_blocks_write_hashed_state.record(timings.write_hashed_state);
        self.save_blocks_write_trie_updates.record(timings.write_trie_updates);
        self.save_blocks_update_history_indices.record(timings.update_history_indices);
        self.save_blocks_update_pipeline_stages.record(timings.update_pipeline_stages);
        self.save_blocks_block_count.record(timings.block_count as f64);

        self.save_blocks_total_last.set(timings.total.as_secs_f64());
        self.save_blocks_mdbx_last.set(timings.mdbx.as_secs_f64());
        self.save_blocks_sf_last.set(timings.sf.as_secs_f64());
        self.save_blocks_rocksdb_last.set(timings.rocksdb.as_secs_f64());
        self.save_blocks_insert_block_last.set(timings.insert_block.as_secs_f64());
        self.save_blocks_write_state_last.set(timings.write_state.as_secs_f64());
        self.save_blocks_write_hashed_state_last.set(timings.write_hashed_state.as_secs_f64());
        self.save_blocks_write_trie_updates_last.set(timings.write_trie_updates.as_secs_f64());
        self.save_blocks_update_history_indices_last
            .set(timings.update_history_indices.as_secs_f64());
        self.save_blocks_update_pipeline_stages_last
            .set(timings.update_pipeline_stages.as_secs_f64());
        self.save_blocks_block_count_last.set(timings.block_count as f64);
    }

    /// Records all commit timings.
    pub(crate) fn record_commit(&self, timings: &CommitTimings) {
        self.save_blocks_commit_mdbx.record(timings.mdbx);
        self.save_blocks_commit_sf.record(timings.sf);
        self.save_blocks_commit_rocksdb.record(timings.rocksdb);

        self.save_blocks_commit_mdbx_last.set(timings.mdbx.as_secs_f64());
        self.save_blocks_commit_sf_last.set(timings.sf.as_secs_f64());
        self.save_blocks_commit_rocksdb_last.set(timings.rocksdb.as_secs_f64());
    }

    /// Records all edge mode parallel write timings.
    #[cfg(feature = "edge")]
    pub(crate) fn record_edge_writes(&self, timings: &EdgeWriteTimings) {
        self.edge_write_plain_accounts.record(timings.plain_accounts);
        self.edge_write_plain_accounts_last.set(timings.plain_accounts.as_secs_f64());

        self.edge_write_bytecodes.record(timings.bytecodes);
        self.edge_write_bytecodes_last.set(timings.bytecodes.as_secs_f64());

        self.edge_write_plain_storage.record(timings.plain_storage);
        self.edge_write_plain_storage_last.set(timings.plain_storage.as_secs_f64());

        self.edge_write_hashed_accounts.record(timings.hashed_accounts);
        self.edge_write_hashed_accounts_last.set(timings.hashed_accounts.as_secs_f64());

        self.edge_write_hashed_storages.record(timings.hashed_storages);
        self.edge_write_hashed_storages_last.set(timings.hashed_storages.as_secs_f64());

        self.edge_write_account_trie.record(timings.account_trie);
        self.edge_write_account_trie_last.set(timings.account_trie.as_secs_f64());

        self.edge_write_storage_trie.record(timings.storage_trie);
        self.edge_write_storage_trie_last.set(timings.storage_trie.as_secs_f64());

        self.edge_preprocessing.record(timings.preprocessing);
        self.edge_preprocessing_last.set(timings.preprocessing.as_secs_f64());

        self.edge_parallel_wall.record(timings.parallel_wall);
        self.edge_parallel_wall_last.set(timings.parallel_wall.as_secs_f64());

        self.edge_parallel_writes_total.record(timings.total);
        self.edge_parallel_writes_total_last.set(timings.total.as_secs_f64());

        self.edge_parallel_subtxn_count.record(timings.subtxn_count as f64);
        self.edge_parallel_subtxn_count_last.set(timings.subtxn_count as f64);
    }
}
