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

    /// Number of transactions in the blob sub-pool
    pub(crate) blob_pool_transactions: Gauge,
    /// Total amount of memory used by the transactions in the blob sub-pool in bytes
    pub(crate) blob_pool_size_bytes: Gauge,

    /// Number of all transactions of all sub-pools: pending + basefee + queued + blob
    pub(crate) total_transactions: Gauge,
    /// Number of all legacy transactions in the pool
    pub(crate) total_legacy_transactions: Gauge,
    /// Number of all EIP-2930 transactions in the pool
    pub(crate) total_eip2930_transactions: Gauge,
    /// Number of all EIP-1559 transactions in the pool
    pub(crate) total_eip1559_transactions: Gauge,
    /// Number of all EIP-4844 transactions in the pool
    pub(crate) total_eip4844_transactions: Gauge,
    /// Number of all EIP-7702 transactions in the pool
    pub(crate) total_eip7702_transactions: Gauge,

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
    /// Gauge indicating the number of addresses with pending updates in the pool,
    /// requiring their account information to be fetched.
    pub(crate) dirty_accounts: Gauge,
    /// Counter for the number of times the pool state diverged from the canonical blockchain
    /// state.
    pub(crate) drift_count: Counter,
    /// Counter for the number of transactions reinserted into the pool following a blockchain
    /// reorganization (reorg).
    pub(crate) reinserted_transactions: Counter,
    /// Counter for the number of finalized blob transactions that have been removed from tracking.
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

/// All Transactions metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
pub struct AllTransactionsMetrics {
    /// Number of all transactions by hash in the pool
    pub(crate) all_transactions_by_hash: Gauge,
    /// Number of all transactions by id in the pool
    pub(crate) all_transactions_by_id: Gauge,
    /// Number of all transactions by all senders in the pool
    pub(crate) all_transactions_by_all_senders: Gauge,
    /// Number of blob transactions nonce gaps.
    pub(crate) blob_transactions_nonce_gaps: Counter,
    /// The current blob base fee
    pub(crate) blob_base_fee: Gauge,
    /// The current base fee
    pub(crate) base_fee: Gauge,
}
