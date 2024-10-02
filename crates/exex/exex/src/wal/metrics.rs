use metrics::Gauge;
use reth_metrics::Metrics;

/// Metrics for the [WAL](`super::Wal`)
#[derive(Metrics)]
#[metrics(scope = "exex.wal")]
pub(super) struct Metrics {
    /// Size of all notifications in WAL in bytes
    pub size_bytes: Gauge,
    /// Total number of notifications in WAL
    pub notifications_total: Gauge,
    /// Total number of committed blocks in WAL
    pub committed_blocks_total: Gauge,
    /// Lowest committed block height in WAL
    pub lowest_committed_block_height: Gauge,
    /// Highest committed block height in WAL
    pub highest_committed_block_height: Gauge,
}
