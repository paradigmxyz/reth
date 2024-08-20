use reth_metrics::{
    metrics::{Gauge, Histogram},
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
    /// How long it took for blocks to be pruned
    pub(crate) prune_before_duration_seconds: Histogram,
}
