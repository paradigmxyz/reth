use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Part of the data that can be pruned.
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PrunePart {
    /// Prune part responsible for the `TxSenders` table.
    SenderRecovery,
    /// Prune part responsible for the `TxHashNumber` table.
    TransactionLookup,
    /// Prune part responsible for the `Receipts` table.
    Receipts,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    AccountHistory,
    /// Prune part responsible for the `StorageChangeSet` and `StorageHistory` tables.
    StorageHistory,
}

/// PrunePart error type.
#[derive(Debug, Error)]
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
