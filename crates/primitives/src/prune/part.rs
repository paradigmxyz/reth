use reth_codecs::{main_codec, Compact};

/// Part of the data that can be pruned.
#[main_codec]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(test, derive(Default))]
pub enum PrunePart {
    /// Default prune part available only during the tests.
    #[cfg_attr(test, default)]
    #[cfg(test)]
    Default,
    /// Prune part responsible for the `TxSenders` table.
    SenderRecovery,
    /// Prune part responsible for the `TxHashNumber` table.
    TransactionLookup,
    /// Prune part responsible for the `Receipts` table.
    Receipts,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    AccountHistory,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    StorageHistory,
}
