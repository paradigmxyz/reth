/// Guarantees max transactions for one sender, compatible with geth/erigon
pub(crate) const MAX_ACCOUNT_SLOTS_PER_SENDER: usize = 16;

///! Configuration options for the Transaction pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Max number of transaction in the pending sub-pool
    pub pending_limit: usize,
    /// Max number of transaction in the basefee sub-pool
    pub basefee_limit: usize,
    /// Max number of transaction in the queued sub-pool
    pub queued_limit: usize,
    /// Max number of executable transaction slots guaranteed per account
    pub max_account_slots: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pending_limit: 10_000,
            basefee_limit: 10_000,
            queued_limit: 10_000,
            max_account_slots: MAX_ACCOUNT_SLOTS_PER_SENDER,
        }
    }
}
