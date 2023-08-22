use serde::{Deserialize, Serialize};

/// Batch sizes for configuring the pruner.
/// The batch size for each prune part should be both large enough to prune the data which was
/// generated with each new block, and small enough to not generate an excessive load on the
/// database due to deletion of too many rows at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneBatchSizes {
    /// Maximum number of receipts to prune, per block.
    receipts: usize,
    /// Maximum number of transaction lookup entries to prune, per block.
    transaction_lookup: usize,
    /// Maximum number of transaction senders to prune, per block.
    transaction_senders: usize,
    /// Maximum number of account history entries to prune, per block.
    /// Measured in the number of `AccountChangeSet` table rows.
    account_history: usize,
    /// Maximum number of storage history entries to prune, per block.
    /// Measured in the number of `StorageChangeSet` table rows.
    storage_history: usize,
}

impl PruneBatchSizes {
    /// Maximum number of receipts to prune, accounting for the block interval.
    pub fn receipts(&self, block_interval: usize) -> usize {
        self.receipts * block_interval
    }

    /// Maximum number of transaction lookup entries to prune, accounting for the block interval.
    pub fn transaction_lookup(&self, block_interval: usize) -> usize {
        self.transaction_lookup * block_interval
    }

    /// Maximum number of transaction senders to prune, accounting for the block interval.
    pub fn transaction_senders(&self, block_interval: usize) -> usize {
        self.transaction_senders * block_interval
    }

    /// Maximum number of account history entries to prune, accounting for the block interval.
    /// Measured in the number of `AccountChangeSet` table rows.
    pub fn account_history(&self, block_interval: usize) -> usize {
        self.account_history * block_interval
    }

    /// Maximum number of storage history entries to prune, accounting for the block interval.
    /// Measured in the number of `StorageChangeSet` table rows.
    pub fn storage_history(&self, block_interval: usize) -> usize {
        self.storage_history * block_interval
    }

    /// Default prune batch sizes for Ethereum mainnet.
    /// These settings are sufficient to prune more data than generated with each new block.
    pub const fn mainnet() -> Self {
        Self {
            receipts: 250,
            transaction_lookup: 250,
            transaction_senders: 250,
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
