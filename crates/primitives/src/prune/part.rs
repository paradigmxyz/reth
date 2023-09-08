use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Part of the data that can be pruned.
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PrunePart {
    /// Prune part responsible for the `TransactionSenders` table.
    SenderRecovery,
    /// Prune part responsible for the `TransactionHashNumbers` table.
    TransactionLookup,
    /// Prune part responsible for all `Receipts`.
    Receipts,
    /// Prune part responsible for some `Receipts` filtered by logs.
    ContractLogs,
    /// Prune part responsible for the `AccountChangeSets` and `AccountHistories` tables.
    AccountHistories,
    /// Prune part responsible for the `StorageChangeSet` and `StorageHistories` tables.
    StorageHistories,
}

/// PrunePart error type.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrunePartError {
    /// Invalid configuration of a prune part.
    #[error("The configuration provided for {0} is invalid.")]
    Configuration(PrunePart),
}

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
