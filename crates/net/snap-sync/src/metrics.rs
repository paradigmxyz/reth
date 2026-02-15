//! Snap sync metrics.

use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

#[derive(Metrics)]
#[metrics(scope = "snap_sync")]
pub(crate) struct SnapSyncMetrics {
    /// Number of accounts downloaded.
    pub(crate) accounts_downloaded: Gauge,
    /// Number of storage slots downloaded.
    pub(crate) storage_slots_downloaded: Gauge,
    /// Number of bytecodes downloaded.
    pub(crate) bytecodes_downloaded: Gauge,
    /// Current phase (0=idle, 1=accounts, 2=storages, 3=bytecodes, 4=verify, 5=done).
    pub(crate) phase: Gauge,
    /// Total peer request failures.
    pub(crate) request_failures: Counter,
}
