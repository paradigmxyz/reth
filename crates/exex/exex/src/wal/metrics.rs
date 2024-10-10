use metrics::Gauge;
use reth_metrics::Metrics;

/// Metrics for the [WAL](`super::Wal`)
#[derive(Metrics)]
#[metrics(scope = "exex.wal")]
pub(super) struct Metrics {
    /// Size of all notifications in WAL in bytes
    pub size_bytes: Gauge,
    /// Number of notifications in WAL
    pub notifications_count: Gauge,
    /// Number of committed blocks in WAL
    pub committed_blocks_count: Gauge,
    /// Lowest committed block height in WAL
    pub lowest_committed_block_height: Gauge,
    /// Highest committed block height in WAL
    pub highest_committed_block_height: Gauge,
}
