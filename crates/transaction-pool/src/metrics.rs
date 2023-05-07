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
    /// Total number of pending transactions in the pool
    pub(crate) total_num_pending_transactions: Gauge,
    /// Total number of queued transactions in the pool
    pub(crate) total_num_queued_transactions: Gauge,
    /// Total number of base fee pooled transactions in the pool
    pub(crate) total_base_fee_pooled_transactions: Gauge,
    /// Total number of transactions in the pool
    pub(crate) total_num_transactions: Gauge,
    /// Total Pool Size (pending_pool_size + base_fee_pool_size + queued_pool_size)
    pub(crate) total_pool_size: Gauge,
}
