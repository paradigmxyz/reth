/// Batch sizes for configuring the pruner.
/// The batch size for each prune part should be both large enough to prune the data that generated
/// with each new block, and small enough to not generate excessive load on the database due to
/// deleting too many rows at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneBatchSizes {
    /// Maximum number of receipts to prune in one run.
    pub receipts: usize,
    /// Maximum number of transaction lookup entries to prune in one run.
    pub transaction_lookup: usize,
    /// Maximum number of transaction senders to prune in one run.
    pub transaction_senders: usize,
    /// Maximum number of account history entries to prune in one run.
    /// Measured in the number of `AccountChangeSet` table rows.
    pub account_history: usize,
    /// Maximum number of storage history entries to prune in one run.
    /// Measured in the number of `StorageChangeSet` table rows.
    pub storage_history: usize,
}

impl PruneBatchSizes {
    /// Default prune batch sizes for Ethereum mainnet.
    /// These settings are sufficient to prune more data than generated with each new block.
    pub const fn mainnet() -> Self {
        Self {
            receipts: 500,
            transaction_lookup: 500,
            transaction_senders: 500,
            account_history: 1000,
            storage_history: 1000,
        }
    }

    /// Default prune batch sizes for Ethereum testnets.
    /// These settings are sufficient to prune more data than generated with each new block.
    pub const fn testnet() -> Self {
        Self {
            receipts: 100,
            transaction_lookup: 100,
            transaction_senders: 100,
            account_history: 500,
            storage_history: 500,
        }
    }
}

impl Default for PruneBatchSizes {
    fn default() -> Self {
        Self::mainnet()
    }
}
