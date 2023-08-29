use paste::paste;
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

macro_rules! impl_prune_batch_size_methods {
    ($(($human_name:expr, $name:ident)),+) => {
        paste! {
            impl PruneBatchSizes {
                $(
                    #[doc = concat!("Maximum number of ", $human_name, " to prune, accounting for the block interval.")]
                    pub fn $name(&self, block_interval: usize) -> usize {
                        self.$name * block_interval
                    }

                    #[doc = concat!("Set the maximum number of ", $human_name, " to prune per block.")]
                    pub fn [<with_ $name>](mut self, batch_size: usize) -> Self {
                        self.$name = batch_size;
                        self
                    }
                )+
            }
        }
    };
}

impl_prune_batch_size_methods!(
    ("receipts", receipts),
    ("transaction lookup entries", transaction_lookup),
    ("transaction senders", transaction_senders),
    ("account history entries", account_history),
    ("storage history entries", storage_history)
);

impl PruneBatchSizes {
    /// Default prune batch sizes for Ethereum mainnet.
    /// These settings are sufficient to prune more data than generated with each new block.
    pub const fn mainnet() -> Self {
        Self {
            receipts: 250,
            transaction_lookup: 250,
            transaction_senders: 1000,
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
            transaction_senders: 500,
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
