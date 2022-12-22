//! Transaction pool metrics.

use metrics::{describe_counter, register_counter, Counter};

pub(crate) struct TxPoolMetrics {
    /// Number of transactions inserted in the pool
    pub(crate) inserted_transactions: Counter,
    /// Number of invalid transactions
    pub(crate) invalid_transactions: Counter,
    /// Number of removed transactions from the pool 
    pub(crate) removed_transactions: Counter,
}

impl Default for TxPoolMetrics {
    /// Initialize txpool metrics struct and register them
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
    describe_counter!(
        "transaction_pool.inserted_transactions",
        "Number of transactions inserted in the pool"
    );
    describe_counter!("transaction_pool.invalid_transactions", "Number of invalid transactions");
    describe_counter!(
        "transaction_pool.removed_transactions",
        "Number of removed transactions from the pool"
    );
}
