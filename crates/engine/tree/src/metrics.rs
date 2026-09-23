use reth_metrics::{
    metrics::{Counter, Gauge, Histogram},
    Metrics,
};

/// Metrics for the `BasicBlockDownloader`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct BlockDownloaderMetrics {
    /// How many blocks are currently being downloaded.
    pub(crate) active_block_downloads: Gauge,
}

/// Metrics for the `PersistenceService`
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.persistence")]
pub(crate) struct PersistenceMetrics {
    /// How long it took for blocks to be removed
    pub(crate) remove_blocks_above_duration_seconds: Histogram,
    /// How long it took for blocks to be saved
    pub(crate) save_blocks_duration_seconds: Histogram,
    /// How many blocks we persist at once.
    pub(crate) save_blocks_batch_size: Histogram,
    /// Blocks durably persisted after a successful commit.
    pub(crate) persisted_blocks_total: Counter,
    /// Transactions in the block data durably persisted.
    pub(crate) persisted_transactions_total: Counter,
    /// Blocks whose state/trie updates were durably persisted.
    pub(crate) persisted_state_trie_blocks_total: Counter,
    /// Provider commit wall time per successful batch.
    pub(crate) commit_duration_seconds: Histogram,
    /// How long it took for blocks to be pruned
    pub(crate) prune_before_duration_seconds: Histogram,
}
