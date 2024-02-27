use metrics::Gauge;
use reth_metrics::Metrics;

#[derive(Metrics)]
#[metrics(scope = "storage.libmdbxrs.txn_manager")]
/// `TxnManager` metrics.
pub struct TxnManagerMetrics {
    /// The number of aborted transactions that are currently tracked.
    pub(super) aborted: Gauge,
}
