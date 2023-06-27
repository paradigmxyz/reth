use serde::{Deserialize, Serialize};

/// All data parts that can be pruned.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Ord, PartialOrd, Serialize, Deserialize)]
pub enum PrunePart {
    /// `TxSenders` table modified by `SenderRecovery` stage.
    SenderRecovery,
    /// `TxHashNumber` table modified by `TransactionLookup` stage.
    TransactionLookup,
    /// `Receipts` table modified by `Execution` stage.
    Receipts,
    /// `AccountHistory` and `AccountChangeSet` tables
    /// modified by `IndexAccountHistory` and `Execution` stages accordingly.
    AccountHistory,
    /// `StorageHistory` and `StorageChangeSet` tables
    /// modified by `IndexStorageHistory` and `Execution` stages accordingly.
    StorageHistory,
}

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
