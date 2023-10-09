use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Segment of the data that can be pruned.
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PruneSegment {
    /// Prune segment responsible for the `TxSenders` table.
    SenderRecovery,
    /// Prune segment responsible for the `TxHashNumber` table.
    TransactionLookup,
    /// Prune segment responsible for all rows in `Receipts` table.
    Receipts,
    /// Prune segment responsible for some rows in `Receipts` table filtered by logs.
    ContractLogs,
    /// Prune segment responsible for the `AccountChangeSet` and `AccountHistory` tables.
    AccountHistory,
    /// Prune segment responsible for the `StorageChangeSet` and `StorageHistory` tables.
    StorageHistory,
    /// Prune segment responsible for the `CanonicalHeaders`, `Headers` and `HeaderTD` tables.
    Headers,
    /// Prune segment responsible for the `Transactions` table.
    Transactions,
}

/// PruneSegment error type.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PruneSegmentError {
    /// Invalid configuration of a prune segment.
    #[error("The configuration provided for {0} is invalid.")]
    Configuration(PruneSegment),
    /// Receipts have been pruned
    #[error("Receipts have been pruned")]
    ReceiptsPruned,
}

#[cfg(test)]
impl Default for PruneSegment {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
