use reth_codecs::{main_codec, Compact};

/// Part of the data that can be pruned.
#[main_codec]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
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

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
