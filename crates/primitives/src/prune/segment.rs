use crate::MINIMUM_PRUNING_DISTANCE;
use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Segment of the data that can be pruned.
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PruneSegment {
    /// Prune segment responsible for the `TransactionSenders` table.
    SenderRecovery,
    /// Prune segment responsible for the `TransactionHashNumbers` table.
    TransactionLookup,
    /// Prune segment responsible for all rows in `Receipts` table.
    Receipts,
    /// Prune segment responsible for some rows in `Receipts` table filtered by logs.
    ContractLogs,
    /// Prune segment responsible for the `AccountChangeSets` and `AccountsHistory` tables.
    AccountHistory,
    /// Prune segment responsible for the `StorageChangeSets` and `StoragesHistory` tables.
    StorageHistory,
    /// Prune segment responsible for the `CanonicalHeaders`, `Headers` and
    /// `HeaderTerminalDifficulties` tables.
    Headers,
    /// Prune segment responsible for the `Transactions` table.
    Transactions,
}

impl PruneSegment {
    /// Returns minimum number of blocks to left in the database for this segment.
    pub fn min_blocks(&self, purpose: PrunePurpose) -> u64 {
        match self {
            Self::SenderRecovery | Self::TransactionLookup | Self::Headers | Self::Transactions => {
                0
            }
            Self::Receipts if purpose.is_static_file() => 0,
            Self::ContractLogs | Self::AccountHistory | Self::StorageHistory => {
                MINIMUM_PRUNING_DISTANCE
            }
            Self::Receipts => MINIMUM_PRUNING_DISTANCE,
        }
    }
}

/// Prune purpose.
#[derive(Debug, Clone, Copy)]
pub enum PrunePurpose {
    /// Prune data according to user configuration.
    User,
    /// Prune data according to highest static_files to delete the data from database.
    StaticFile,
}

impl PrunePurpose {
    /// Returns true if the purpose is [`PrunePurpose::User`].
    pub fn is_user(self) -> bool {
        matches!(self, Self::User)
    }

    /// Returns true if the purpose is [`PrunePurpose::StaticFile`].
    pub fn is_static_file(self) -> bool {
        matches!(self, Self::StaticFile)
    }
}

/// PruneSegment error type.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PruneSegmentError {
    /// Invalid configuration of a prune segment.
    #[error("the configuration provided for {0} is invalid")]
    Configuration(PruneSegment),
    /// Receipts have been pruned
    #[error("receipts have been pruned")]
    ReceiptsPruned,
}

#[cfg(test)]
impl Default for PruneSegment {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
