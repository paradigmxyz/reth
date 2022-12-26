//! Transaction pool metrics.

use metrics::Counter;
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
}

// impl TxPoolMetrics {
//     /// Describe transaction pool metrics
//     pub fn describe() {
//         describe_counter!(
//             "transaction_pool.inserted_transactions",
//             "Number of transactions inserted in the pool"
//         );
//         describe_counter!(
//             "transaction_pool.invalid_transactions",
//             "Number of invalid transactions"
//         );
//         describe_counter!(
//             "transaction_pool.removed_transactions",
//             "Number of removed transactions from the pool"
//         );
//     }
// }
