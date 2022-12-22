//! Transaction pool metrics.

use metrics::{register_counter, describe_counter, Counter};

pub(crate) struct TxPoolMetrics {
    pub(crate) inserted_transactions: Counter,
    pub(crate) invalid_transactions: Counter,
    pub(crate) removed_transactions: Counter,
}

impl Default for TxPoolMetrics {
    /// Initialize TxPoolMetrics struct and register them
    fn default() -> Self {
        Self {
            inserted_transactions: register_counter!("transaction_pool.inserted_transactions"),
            invalid_transactions: register_counter!("transaction_pool.invalid_transactions"),
            removed_transactions: register_counter!("transaction_pool.removed_transactions"),
        }
    }
}

/// Describe transaction pool metrics
pub fn describe() {
    describe_counter!("transaction_pool.inserted_transactions", "Number of transactions inserted in the pool");
    describe_counter!(
        "transaction_pool.invalid_transactions",
        "Number of invalid transactions"
    );
    describe_counter!(
        "transaction_pool.removed_transactions",
        "Number of removed transactions from the pool"
    );
}
