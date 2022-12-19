/// Guarantees max transactions for one sender, compatible with geth/erigon
pub(crate) const MAX_ACCOUNT_SLOTS_PER_SENDER: usize = 16;

///! Configuration options for the Transaction pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Max number of transaction in the pending sub-pool
    pub pending_limit: SubPoolLimit,
    /// Max number of transaction in the basefee sub-pool
    pub basefee_limit: SubPoolLimit,
    /// Max number of transaction in the queued sub-pool
    pub queued_limit: SubPoolLimit,
    /// Max number of executable transaction slots guaranteed per account
    pub max_account_slots: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pending_limit: Default::default(),
            basefee_limit: Default::default(),
            queued_limit: Default::default(),
            max_account_slots: MAX_ACCOUNT_SLOTS_PER_SENDER,
        }
    }
}

/// Size limits for a sub-pool.
#[derive(Debug, Clone)]
pub struct SubPoolLimit {
    /// Maximum amount of transaction in the pool.
    pub max_txs: usize,
    /// Maximum combined size (in bytes) of transactions in the pool.
    pub max_size: usize,
}

impl SubPoolLimit {
    /// Returns whether the size or amount constraint is violated.
    #[inline]
    pub fn is_exceeded(&self, txs: usize, size: usize) -> bool {
        self.max_txs < txs || self.max_size < size
    }
}

impl Default for SubPoolLimit {
    fn default() -> Self {
        // either 10k transactions or 20MB
        Self { max_txs: 10_000, max_size: 20 * 1024 * 1024 }
    }
}
