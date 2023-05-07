//! Transaction pool metrics.

use metrics::{Counter, Gauge};
use reth_metrics_derive::Metrics;

/// Transaction pool metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
pub struct TxPoolMetrics {
    /// Number of transactions inserted in the pool
    pub(crate) inserted_transactions: Counter,
    /// Number of invalid transactions
    pub(crate) invalid_transactions: Counter,
    /// Number of removed transactions from the pool
    pub(crate) removed_transactions: Counter,
    /// Total number of transactions in the pool
    pub(crate) total_number_transactions: Gauge,
    /// Total amount of memory used by the transactions in the pool in bytes
    pub(crate) total_size_bytes: Gauge,

    /// Number of transactions in the pending sub-pool
    pub(crate) pending_pool_length: Gauge,
    /// Total amount of memory used by the transactions in the pending sub-pool in bytes
    pub(crate) pending_sub_pool_size_bytes: Gauge,

    /// Number of transactions in the basefee sub-pool
    pub(crate) basefee_pool_length: Gauge,
    /// Total amount of memory used by the transactions in the basefee sub-pool in bytes
    pub(crate) basefee_sub_pool_size_bytes: Gauge,

    /// Number of transactions in the queued sub-pool
    pub(crate) queued_pool_length: Gauge,
    /// Total amount of memory used by the transactions in the queued sub-pool in bytes
    pub(crate) queued_sub_pool_size_bytes: Gauge,
}
