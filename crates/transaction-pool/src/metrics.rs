//! Transaction pool metrics.

use metrics::{describe_counter, register_counter, Counter};
use std::fmt;

/// Transaction pool metrics
pub struct TxPoolMetrics {
    /// Number of transactions inserted in the pool
    pub(crate) inserted_transactions: Counter,
    /// Number of invalid transactions
    pub(crate) invalid_transactions: Counter,
    /// Number of removed transactions from the pool
    pub(crate) removed_transactions: Counter,
}

impl TxPoolMetrics {
    /// Describe transaction pool metrics
    pub fn describe() {
        describe_counter!(
            "transaction_pool.inserted_transactions",
            "Number of transactions inserted in the pool"
        );
        describe_counter!(
            "transaction_pool.invalid_transactions",
            "Number of invalid transactions"
        );
        describe_counter!(
            "transaction_pool.removed_transactions",
            "Number of removed transactions from the pool"
        );
    }
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

impl fmt::Debug for TxPoolMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxPoolMetrics").finish()
    }
}
