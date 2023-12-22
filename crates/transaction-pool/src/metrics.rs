//! Transaction pool metrics.

use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

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

    /// Number of transactions in the pending sub-pool
    pub(crate) pending_pool_transactions: Gauge,
    /// Total amount of memory used by the transactions in the pending sub-pool in bytes
    pub(crate) pending_pool_size_bytes: Gauge,

    /// Number of transactions in the basefee sub-pool
    pub(crate) basefee_pool_transactions: Gauge,
    /// Total amount of memory used by the transactions in the basefee sub-pool in bytes
    pub(crate) basefee_pool_size_bytes: Gauge,

    /// Number of transactions in the queued sub-pool
    pub(crate) queued_pool_transactions: Gauge,
    /// Total amount of memory used by the transactions in the queued sub-pool in bytes
    pub(crate) queued_pool_size_bytes: Gauge,

    /// Number of all transactions of all sub-pools: pending + basefee + queued
    pub(crate) total_transactions: Gauge,

    /// How often the pool was updated after the canonical state changed
    pub(crate) performed_state_updates: Counter,
}

/// Transaction pool blobstore metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
pub struct BlobStoreMetrics {
    /// Number of failed inserts into the blobstore
    pub(crate) blobstore_failed_inserts: Counter,
    /// Number of failed deletes into the blobstore
    pub(crate) blobstore_failed_deletes: Counter,
    /// The number of bytes the blobs in the blobstore take up
    pub(crate) blobstore_byte_size: Gauge,
    /// How many blobs are currently in the blobstore
    pub(crate) blobstore_entries: Gauge,
}

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
pub struct MaintainPoolMetrics {
    /// Number of currently dirty addresses that need to be updated in the pool by fetching account
    /// info
    pub(crate) dirty_accounts: Gauge,
    /// How often the pool drifted from the canonical state.
    pub(crate) drift_count: Counter,
    /// Number of transaction reinserted into the pool after reorg.
    pub(crate) reinserted_transactions: Counter,
    /// Number of transactions finalized blob transactions we were tracking.
    pub(crate) deleted_tracked_finalized_blobs: Counter,
}

impl MaintainPoolMetrics {
    #[inline]
    pub(crate) fn set_dirty_accounts_len(&self, count: usize) {
        self.dirty_accounts.set(count as f64);
    }

    #[inline]
    pub(crate) fn inc_reinserted_transactions(&self, count: usize) {
        self.reinserted_transactions.increment(count as u64);
    }

    #[inline]
    pub(crate) fn inc_deleted_tracked_blobs(&self, count: usize) {
        self.deleted_tracked_finalized_blobs.increment(count as u64);
    }

    #[inline]
    pub(crate) fn inc_drift(&self) {
        self.drift_count.increment(1);
    }
}
